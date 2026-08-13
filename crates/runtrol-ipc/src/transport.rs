//! Getting frames between two of runtrol's own processes on one machine.
//!
//! # Two platforms, one signature
//!
//! A named pipe on Windows, a socket file on Unix. Both are addressed by text, which the kernel's layout hands over
//! already correct, so nothing here computes an address and nothing has two spellings of one.
//!
//! # Why not a socket file on Windows
//!
//! Measured against the design's own survey: that platform's socket support has no peer credential API and no way to
//! ask who is on the other end, and the meaning of file permissions on one is unspecified. A named pipe answers both
//! questions with documented calls. Choosing the familiar one would mean a local surface that cannot tell its own
//! operator from anybody else logged in.
//!
//! # What protects the endpoint
//!
//! **Remote clients are refused.** A named pipe is asked to reject them explicitly rather than relying on a default,
//! because a default is something a later release may change.
//!
//! **The endpoint admits the operator only.** Unix creates its parent at mode 0700, narrows the socket to 0600, and
//! compares the peer UID with the socket owner. Windows installs an explicit current-user DACL at pipe creation,
//! rejects remote clients, observes the peer process, impersonates the connected pipe client, and compares its token
//! user SID with the daemon's. The Windows system calls are confined to audited unsafe blocks in this module.

use bytes::{Bytes, BytesMut};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

use crate::frame::{Decoded, FrameError, MAX_FRAME, encode};

/// How much the reader holds between frames.
///
/// Constant, and deliberately not a function of [`MAX_FRAME`]. A frame larger than this grows the buffer for as long as
/// that frame is being assembled and no longer, so one big message is a spike and not a new resting size.
pub const READ_BUFFER: usize = 64 * 1024;

/// A connection could not be made, kept, or read.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum TransportError {
    /// The endpoint could not be created.
    ///
    /// Names the address, because the operator's next question is where runtrol tried to listen.
    #[error("cannot listen at {address}: {detail}")]
    Bind {
        /// Where it tried.
        address: String,
        /// What the platform said.
        detail: String,
    },

    /// Nothing is listening there.
    ///
    /// The ordinary case on a machine where the daemon is not up yet, and the caller starts one rather than treating it
    /// as a failure. Told apart from every other reason a connection fails, so that distinction is possible.
    #[error("nothing is listening at {address}")]
    NotListening {
        /// Where it looked.
        address: String,
    },

    /// The connection failed for a reason other than nothing listening.
    #[error("cannot reach {address}: {detail}")]
    Connect {
        /// Where it looked.
        address: String,
        /// What the platform said.
        detail: String,
    },

    /// Reading or writing failed.
    #[error("the connection failed while {doing}: {detail}")]
    Io {
        /// What runtrol was doing.
        doing: &'static str,
        /// What the platform said.
        detail: String,
    },

    /// A frame on the connection was not one this build can carry.
    #[error(transparent)]
    Frame(#[from] FrameError),
}

impl TransportError {
    /// Whether this means the daemon is simply not running yet.
    ///
    /// The one question a command surface asks before deciding to start one. Anything else is a real failure and must
    /// not be answered by launching a second daemon on top of a broken endpoint.
    #[must_use]
    pub const fn means_no_daemon(&self) -> bool {
        matches!(self, Self::NotListening { .. })
    }
}

/// One end of a connection, framed.
///
/// Owns its read buffer, so a partial frame is held until the rest arrives rather than being reported as a failure.
pub struct Connection {
    /// The byte stream.
    stream: platform::Stream,
    /// What has arrived and not yet been read as a frame.
    pending: BytesMut,
    /// Set once the other end has gone, so nothing keeps reading a finished connection.
    closed: bool,
}

impl Connection {
    /// Send one frame.
    ///
    /// # Errors
    ///
    /// [`TransportError::Frame`] when the payload is past [`MAX_FRAME`], [`TransportError::Io`] when the write fails.
    pub async fn send(&mut self, payload: &[u8]) -> Result<(), TransportError> {
        let mut framed = Vec::with_capacity(payload.len() + 4);
        encode(payload, &mut framed)?;
        self.stream
            .write_all(&framed)
            .await
            .map_err(|error| TransportError::Io {
                doing: "sending a frame",
                detail: error.to_string(),
            })?;
        self.stream
            .flush()
            .await
            .map_err(|error| TransportError::Io {
                doing: "flushing a frame",
                detail: error.to_string(),
            })
    }

    /// Send one frame assembled from already encoded slices without copying them into another full-size buffer.
    ///
    /// # Errors
    ///
    /// [`TransportError::Frame`] when the combined payload is past [`MAX_FRAME`], [`TransportError::Io`] when the
    /// write fails.
    pub async fn send_parts(&mut self, parts: &[&[u8]]) -> Result<(), TransportError> {
        let length = parts.iter().try_fold(0_usize, |total, part| {
            total.checked_add(part.len()).ok_or(FrameError::TooLarge {
                bytes: usize::MAX,
                max: MAX_FRAME,
            })
        })?;
        if length > MAX_FRAME {
            return Err(FrameError::TooLarge {
                bytes: length,
                max: MAX_FRAME,
            }
            .into());
        }
        #[expect(
            clippy::cast_possible_truncation,
            reason = "length <= MAX_FRAME, which is well under u32::MAX"
        )]
        let header = (length as u32).to_be_bytes();
        self.stream
            .write_all(&header)
            .await
            .map_err(|error| TransportError::Io {
                doing: "sending a frame header",
                detail: error.to_string(),
            })?;
        for part in parts {
            self.stream
                .write_all(part)
                .await
                .map_err(|error| TransportError::Io {
                    doing: "sending a frame payload",
                    detail: error.to_string(),
                })?;
        }
        self.stream
            .flush()
            .await
            .map_err(|error| TransportError::Io {
                doing: "flushing a frame",
                detail: error.to_string(),
            })
    }

    /// Receive one frame, or `None` once the other end is gone.
    ///
    /// # Errors
    ///
    /// [`TransportError::Frame`] when a length prefix claims more than this build carries, [`TransportError::Io`] when
    /// the read fails.
    pub async fn recv(&mut self) -> Result<Option<Bytes>, TransportError> {
        loop {
            // Whatever has already arrived is tried first, because a read may have delivered several frames at once and
            // waiting for more bytes before handing over the ones in hand would stall on the last of them.
            let held = Bytes::copy_from_slice(&self.pending);
            match crate::frame::decode(&held)? {
                Decoded::Frame { payload, consumed } => {
                    drop(self.pending.split_to(consumed));
                    return Ok(Some(payload));
                }
                Decoded::NeedMore { at_least } => {
                    if self.closed {
                        // The other end went away mid-frame. Reported as the end rather than as a failure: a command
                        // surface that stopped is not an error the daemon has to act on.
                        return Ok(None);
                    }
                    self.pending.reserve(at_least.min(MAX_FRAME));
                }
            }

            let read = self
                .stream
                .read_buf(&mut self.pending)
                .await
                .map_err(|error| TransportError::Io {
                    doing: "receiving a frame",
                    detail: error.to_string(),
                })?;
            if read == 0 {
                self.closed = true;
                if self.pending.is_empty() {
                    return Ok(None);
                }
            }
        }
    }
}

/// Where the daemon waits for the command surface.
pub struct Listener {
    /// The platform's own mechanism.
    inner: platform::Listener,
    /// The address, kept for the error messages that have to name it.
    address: String,
    /// Whether every accepted peer must be the endpoint owner.
    owner_only: bool,
}

impl Listener {
    /// Start listening at `address`.
    ///
    /// # Why this is asynchronous when it never waits
    ///
    /// Because an endpoint has to be registered with the runtime's own reader the moment it exists, on both
    /// platforms, and a program that creates one anywhere else stops with a message about a reactor rather than
    /// anything to do with runtrol. Measured: a daemon whose endpoint was created just outside its runtime started,
    /// stopped instantly, and the command that started it waited the full ten seconds before saying so.
    ///
    /// Being asynchronous is how that requirement is stated in a form the compiler holds, rather than a sentence
    /// somebody has to have read. An earlier version of this said it was synchronous "because neither platform waits
    /// to create an endpoint", which was true about waiting and silent about the thing that actually matters.
    ///
    /// # Errors
    ///
    /// [`TransportError::Bind`] when the platform refuses. Not worked around: a daemon that cannot be reached is a
    /// daemon nothing can use, and starting anyway would leave the operator with a process that does nothing.
    #[expect(
        clippy::unused_async,
        reason = "an endpoint has to be created on the runtime, and this is how that is required rather than remembered"
    )]
    pub async fn bind(address: &str) -> Result<Self, TransportError> {
        Ok(Self {
            inner: platform::Listener::bind(address, false)?,
            address: address.to_owned(),
            owner_only: false,
        })
    }

    /// Start an endpoint whose OS object admits only the owning user and whose accepted peer is verified where the
    /// platform exposes peer identity.
    ///
    /// # Errors
    ///
    /// [`TransportError::Bind`] when owner-only admission cannot be installed. It never falls back to the ordinary
    /// listener because this constructor is an authorization boundary.
    #[expect(
        clippy::unused_async,
        reason = "the endpoint has to be registered with the active Tokio I/O runtime"
    )]
    pub async fn bind_owner_only(address: &str) -> Result<Self, TransportError> {
        Ok(Self {
            inner: platform::Listener::bind(address, true)?,
            address: address.to_owned(),
            owner_only: true,
        })
    }

    /// The address being listened on.
    #[must_use]
    pub fn address(&self) -> &str {
        &self.address
    }

    /// Wait for the next connection.
    ///
    /// # Errors
    ///
    /// [`TransportError::Bind`] when the platform refuses to keep listening.
    pub async fn accept(&mut self) -> Result<Connection, TransportError> {
        let stream = self.inner.accept(&self.address, self.owner_only).await?;
        Ok(Connection {
            stream,
            pending: BytesMut::with_capacity(READ_BUFFER),
            closed: false,
        })
    }
}

/// Connect to a daemon.
///
/// # Errors
///
/// [`TransportError::NotListening`] when no daemon is there, which the caller answers by starting one.
/// [`TransportError::Connect`] for anything else, which it must not.
pub async fn connect(address: &str) -> Result<Connection, TransportError> {
    Ok(Connection {
        stream: platform::connect(address).await?,
        pending: BytesMut::with_capacity(READ_BUFFER),
        closed: false,
    })
}

/// Create and durably write one new regular file with an explicit owner-only OS access policy.
///
/// The path must not exist. Callers publish the completed sibling with an atomic rename, so incomplete bytes never
/// appear under the durable name.
///
/// # Errors
///
/// Returns the operating-system failure without falling back to inherited or process-umask permissions.
pub fn create_owner_only_file(path: &std::path::Path, contents: &[u8]) -> std::io::Result<()> {
    platform::create_owner_only_file(path, contents)
}

#[cfg(windows)]
mod platform {
    //! A named pipe. Remote clients are refused explicitly rather than by default.

    use std::io::Write as _;
    use std::os::windows::io::{AsRawHandle as _, FromRawHandle as _};

    use tokio::net::windows::named_pipe::{
        ClientOptions, NamedPipeClient, NamedPipeServer, ServerOptions,
    };
    use windows_sys::Win32::Foundation::{
        CloseHandle, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE, LocalFree,
    };
    use windows_sys::Win32::Security::Authorization::{
        ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
        SDDL_REVISION_1,
    };
    use windows_sys::Win32::Security::{
        EqualSid, GetTokenInformation, PSECURITY_DESCRIPTOR, PSID, RevertToSelf,
        SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER, TokenUser,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CREATE_NEW, CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_FLAG_FIRST_PIPE_INSTANCE,
        FILE_FLAG_OVERLAPPED, FILE_SHARE_READ, PIPE_ACCESS_DUPLEX,
    };
    use windows_sys::Win32::System::Pipes::{
        CreateNamedPipeW, GetNamedPipeClientProcessId, ImpersonateNamedPipeClient,
        PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES,
        PIPE_WAIT,
    };
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, GetCurrentThread, OpenProcessToken, OpenThreadToken,
    };

    use super::TransportError;

    /// The byte stream, whichever end this is.
    pub(super) enum Stream {
        /// The daemon's end.
        Server(NamedPipeServer),
        /// The command surface's end.
        Client(NamedPipeClient),
    }

    impl tokio::io::AsyncRead for Stream {
        fn poll_read(
            self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
            buf: &mut tokio::io::ReadBuf<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            match self.get_mut() {
                Self::Server(one) => std::pin::Pin::new(one).poll_read(cx, buf),
                Self::Client(one) => std::pin::Pin::new(one).poll_read(cx, buf),
            }
        }
    }

    impl tokio::io::AsyncWrite for Stream {
        fn poll_write(
            self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
            buf: &[u8],
        ) -> std::task::Poll<std::io::Result<usize>> {
            match self.get_mut() {
                Self::Server(one) => std::pin::Pin::new(one).poll_write(cx, buf),
                Self::Client(one) => std::pin::Pin::new(one).poll_write(cx, buf),
            }
        }

        fn poll_flush(
            self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            match self.get_mut() {
                Self::Server(one) => std::pin::Pin::new(one).poll_flush(cx),
                Self::Client(one) => std::pin::Pin::new(one).poll_flush(cx),
            }
        }

        fn poll_shutdown(
            self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            match self.get_mut() {
                Self::Server(one) => std::pin::Pin::new(one).poll_shutdown(cx),
                Self::Client(one) => std::pin::Pin::new(one).poll_shutdown(cx),
            }
        }
    }

    /// The pipe the daemon waits on.
    ///
    /// A named pipe has no accept loop of its own: one instance serves one client, so the next instance is created
    /// while the current one is being handed over. Creating it first is what keeps a client that connects in that
    /// moment from finding nothing there.
    pub(super) struct Listener {
        /// The instance waiting for the next client.
        next: Option<NamedPipeServer>,
        /// Every instance receives the current-owner DACL when set.
        owner_only: bool,
    }

    impl Listener {
        pub(super) fn bind(address: &str, owner_only: bool) -> Result<Self, TransportError> {
            let first = instance(address, true, owner_only)?;
            Ok(Self {
                next: Some(first),
                owner_only,
            })
        }

        #[expect(
            unsafe_code,
            reason = "Windows exposes the connected named-pipe client process only through a handle system call; the live handle and output pointer are bounded beside the call"
        )]
        pub(super) async fn accept(
            &mut self,
            address: &str,
            owner_only: bool,
        ) -> Result<Stream, TransportError> {
            if owner_only != self.owner_only {
                return Err(TransportError::Bind {
                    address: address.to_owned(),
                    detail: "the listener's owner-only policy changed while accepting".to_owned(),
                });
            }
            loop {
                let waiting = match self.next.take() {
                    Some(one) => one,
                    // Only reachable if a previous accept failed after taking the instance. Creating one rather than
                    // refusing keeps a listener that had one bad moment usable.
                    None => instance(address, false, owner_only)?,
                };
                waiting
                    .connect()
                    .await
                    .map_err(|error| TransportError::Bind {
                        address: address.to_owned(),
                        detail: error.to_string(),
                    })?;
                // Created before the current one is handed over, so a client connecting right now finds an instance.
                self.next = Some(instance(address, false, owner_only)?);
                if owner_only {
                    let mut peer = 0_u32;
                    // SAFETY: `waiting` owns a live connected pipe handle, and `peer` is a valid writable u32 for the
                    // duration of the call. The call borrows the handle and does not close or retain it.
                    let observed = unsafe {
                        GetNamedPipeClientProcessId(waiting.as_raw_handle(), &raw mut peer)
                    };
                    if observed == 0 || peer == 0 {
                        return Err(TransportError::Bind {
                            address: address.to_owned(),
                            detail: std::io::Error::last_os_error().to_string(),
                        });
                    }
                    match pipe_client_is_current_user(waiting.as_raw_handle()) {
                        Ok(true) => {}
                        Ok(false) => continue,
                        Err(error) => {
                            return Err(TransportError::Bind {
                                address: address.to_owned(),
                                detail: error.to_string(),
                            });
                        }
                    }
                }
                return Ok(Stream::Server(waiting));
            }
        }
    }

    /// One pipe instance.
    fn instance(
        address: &str,
        first: bool,
        owner_only: bool,
    ) -> Result<NamedPipeServer, TransportError> {
        if owner_only {
            return owner_only_instance(address, first);
        }
        ServerOptions::new()
            .first_pipe_instance(first)
            // Explicit rather than relying on the default. A default is something a later release may change, and this
            // one decides whether a machine on the network can talk to the operator's agents.
            .reject_remote_clients(true)
            .create(address)
            .map_err(|error| TransportError::Bind {
                address: address.to_owned(),
                detail: error.to_string(),
            })
    }

    /// One current-owner pipe instance. Tokio's safe builder has no security-attributes input, so the exact DACL is
    /// installed at creation and the resulting overlapped handle is transferred to Tokio once.
    #[expect(
        unsafe_code,
        reason = "Tokio has no safe SECURITY_ATTRIBUTES input; one owned overlapped handle is created with an explicit DACL and transferred exactly once"
    )]
    fn owner_only_instance(address: &str, first: bool) -> Result<NamedPipeServer, TransportError> {
        let mut security =
            SecurityDescriptor::current_owner().map_err(|error| TransportError::Bind {
                address: address.to_owned(),
                detail: error.to_string(),
            })?;
        let mut attributes = SECURITY_ATTRIBUTES {
            nLength: u32::try_from(core::mem::size_of::<SECURITY_ATTRIBUTES>()).map_err(|_| {
                TransportError::Bind {
                    address: address.to_owned(),
                    detail: "the Windows security attributes size does not fit u32".to_owned(),
                }
            })?,
            lpSecurityDescriptor: security.as_mut_ptr(),
            bInheritHandle: 0,
        };
        let wide: Vec<u16> = address.encode_utf16().chain(core::iter::once(0)).collect();
        let mut open_mode = PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED;
        if first {
            open_mode |= FILE_FLAG_FIRST_PIPE_INSTANCE;
        }
        // SAFETY: `wide` is NUL terminated, `attributes` and its descriptor remain alive for the call, buffer sizes
        // are bounded constants, and the returned handle is checked before ownership is transferred exactly once.
        let handle = unsafe {
            CreateNamedPipeW(
                wide.as_ptr(),
                open_mode,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                PIPE_UNLIMITED_INSTANCES,
                64 * 1024,
                64 * 1024,
                0,
                &raw mut attributes,
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(TransportError::Bind {
                address: address.to_owned(),
                detail: std::io::Error::last_os_error().to_string(),
            });
        }
        // SAFETY: `CreateNamedPipeW` returned one owned, overlapped server handle. No other owner is constructed, and
        // Tokio becomes solely responsible for closing it even if I/O registration fails.
        unsafe { NamedPipeServer::from_raw_handle(handle) }.map_err(|error| TransportError::Bind {
            address: address.to_owned(),
            detail: error.to_string(),
        })
    }

    pub(super) fn create_owner_only_file(
        path: &std::path::Path,
        contents: &[u8],
    ) -> std::io::Result<()> {
        create_owner_only_file_inner(path, contents)
    }

    #[expect(
        unsafe_code,
        reason = "Windows has no safe file-creation API that accepts the explicit owner-only SECURITY_ATTRIBUTES required before the path becomes visible"
    )]
    fn create_owner_only_file_inner(
        path: &std::path::Path,
        contents: &[u8],
    ) -> std::io::Result<()> {
        use std::os::windows::ffi::OsStrExt as _;

        let mut security = SecurityDescriptor::current_owner()?;
        let mut attributes = SECURITY_ATTRIBUTES {
            nLength: u32::try_from(core::mem::size_of::<SECURITY_ATTRIBUTES>())
                .map_err(|_| std::io::Error::other("security attributes size does not fit u32"))?,
            lpSecurityDescriptor: security.as_mut_ptr(),
            bInheritHandle: 0,
        };
        let wide: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(core::iter::once(0))
            .collect();
        // SAFETY: the path is NUL terminated, the descriptor remains alive for the call, and CREATE_NEW prevents
        // following or replacing an existing path. The checked handle is transferred to File exactly once.
        let handle = unsafe {
            CreateFileW(
                wide.as_ptr(),
                GENERIC_WRITE,
                FILE_SHARE_READ,
                &raw mut attributes,
                CREATE_NEW,
                FILE_ATTRIBUTE_NORMAL,
                core::ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: CreateFileW returned one owned synchronous file handle and no other owner is constructed.
        let mut file = unsafe { std::fs::File::from_raw_handle(handle) };
        file.write_all(contents)?;
        file.sync_all()
    }

    /// A Windows self-relative security descriptor released through `LocalFree`.
    struct SecurityDescriptor(PSECURITY_DESCRIPTOR);

    impl SecurityDescriptor {
        #[expect(
            unsafe_code,
            reason = "Windows allocates an SDDL security descriptor through an FFI output pointer that is checked and then owned by this RAII value"
        )]
        fn current_owner() -> std::io::Result<Self> {
            let owner = process_user()?.sid_string()?;
            // Only the exact process token user receives full control. No owner-rights alias, Everyone, anonymous,
            // network, administrator, or SYSTEM ACE is present.
            let sddl: Vec<u16> = format!("D:P(A;;GA;;;{owner})")
                .encode_utf16()
                .chain(core::iter::once(0))
                .collect();
            let mut descriptor: PSECURITY_DESCRIPTOR = core::ptr::null_mut();
            // SAFETY: `sddl` is a valid NUL-terminated UTF-16 string and `descriptor` is a writable output pointer.
            // Windows allocates the returned self-relative descriptor for `LocalFree`.
            let converted = unsafe {
                ConvertStringSecurityDescriptorToSecurityDescriptorW(
                    sddl.as_ptr(),
                    SDDL_REVISION_1,
                    &raw mut descriptor,
                    core::ptr::null_mut(),
                )
            };
            if converted == 0 || descriptor.is_null() {
                return Err(std::io::Error::last_os_error());
            }
            Ok(Self(descriptor))
        }

        fn as_mut_ptr(&mut self) -> PSECURITY_DESCRIPTOR {
            self.0
        }
    }

    /// One handle returned by a Windows open call.
    struct OwnedHandle(HANDLE);

    impl Drop for OwnedHandle {
        #[expect(
            unsafe_code,
            reason = "the non-null handle is owned by this value and CloseHandle is its release API"
        )]
        fn drop(&mut self) {
            // SAFETY: successful token open calls return one owned non-null handle and this value is its only owner.
            let closed = unsafe { CloseHandle(self.0) };
            debug_assert_ne!(closed, 0, "CloseHandle did not release a token handle");
        }
    }

    /// Aligned storage returned by `GetTokenInformation(TokenUser)`.
    struct OwnedTokenUser {
        storage: Vec<usize>,
    }

    impl OwnedTokenUser {
        #[expect(
            unsafe_code,
            reason = "GetTokenInformation writes a bounded TOKEN_USER into aligned owned storage and both calls validate their output lengths"
        )]
        fn read(token: HANDLE) -> std::io::Result<Self> {
            let mut needed = 0_u32;
            // SAFETY: the first call intentionally supplies no output storage and only requests the required size.
            unsafe {
                GetTokenInformation(token, TokenUser, core::ptr::null_mut(), 0, &raw mut needed)
            };
            if needed == 0 {
                return Err(std::io::Error::last_os_error());
            }
            let needed_usize = usize::try_from(needed)
                .map_err(|_| std::io::Error::other("token user length does not fit usize"))?;
            let words = needed_usize.div_ceil(core::mem::size_of::<usize>());
            let mut storage = vec![0_usize; words];
            // SAFETY: storage is aligned for TOKEN_USER, has at least `needed` writable bytes, and remains alive.
            let read = unsafe {
                GetTokenInformation(
                    token,
                    TokenUser,
                    storage.as_mut_ptr().cast(),
                    needed,
                    &raw mut needed,
                )
            };
            if read == 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(Self { storage })
        }

        #[expect(
            unsafe_code,
            reason = "the storage was populated as TOKEN_USER and owns the SID allocation it points into"
        )]
        fn sid(&self) -> PSID {
            // SAFETY: `read` initialized the aligned buffer as TOKEN_USER and the buffer has not moved or been freed.
            unsafe { (*self.storage.as_ptr().cast::<TOKEN_USER>()).User.Sid }
        }

        #[expect(
            unsafe_code,
            reason = "Windows converts the validated token SID into one LocalFree-owned UTF-16 allocation"
        )]
        fn sid_string(&self) -> std::io::Result<String> {
            let mut text = core::ptr::null_mut();
            // SAFETY: the SID belongs to this live token buffer and `text` is a writable output pointer.
            if unsafe { ConvertSidToStringSidW(self.sid(), &raw mut text) } == 0 || text.is_null() {
                return Err(std::io::Error::last_os_error());
            }
            let text = LocalWideString(text);
            let mut length = 0_usize;
            // SAFETY: ConvertSidToStringSidW returns a NUL-terminated UTF-16 allocation.
            while unsafe { *text.0.add(length) } != 0 {
                length = length
                    .checked_add(1)
                    .ok_or_else(|| std::io::Error::other("SID string length overflow"))?;
            }
            // SAFETY: the scan above found the terminator inside the Windows-owned SID string.
            String::from_utf16(unsafe { core::slice::from_raw_parts(text.0, length) })
                .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
        }
    }

    struct LocalWideString(*mut u16);

    impl Drop for LocalWideString {
        #[expect(
            unsafe_code,
            reason = "the pointer was returned by ConvertSidToStringSidW and LocalFree is its release API"
        )]
        fn drop(&mut self) {
            // SAFETY: this value owns the allocation and no Windows call retains it.
            let leftover = unsafe { LocalFree(self.0.cast()) };
            debug_assert!(leftover.is_null(), "LocalFree did not release a SID string");
        }
    }

    #[expect(
        unsafe_code,
        reason = "the current process pseudo-handle is borrowed only long enough to open one owned query token"
    )]
    fn process_user() -> std::io::Result<OwnedTokenUser> {
        let mut token: HANDLE = core::ptr::null_mut();
        // SAFETY: GetCurrentProcess returns a valid pseudo-handle and `token` is a writable output pointer.
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &raw mut token) } == 0
            || token.is_null()
        {
            return Err(std::io::Error::last_os_error());
        }
        OwnedTokenUser::read(OwnedHandle(token).0)
    }

    /// Impersonation is scoped to one synchronous check and always reverted before this function returns.
    struct RevertGuard(bool);

    impl RevertGuard {
        #[expect(
            unsafe_code,
            reason = "RevertToSelf ends the pipe-client impersonation started in the same function"
        )]
        fn revert(mut self) -> std::io::Result<()> {
            // SAFETY: the active flag is set only after successful ImpersonateNamedPipeClient on this thread.
            if self.0 && unsafe { RevertToSelf() } == 0 {
                return Err(std::io::Error::last_os_error());
            }
            self.0 = false;
            Ok(())
        }
    }

    impl Drop for RevertGuard {
        #[expect(
            unsafe_code,
            reason = "panic cleanup must end a pipe-client impersonation before the worker thread is reused"
        )]
        fn drop(&mut self) {
            if self.0 {
                // SAFETY: the active flag is set only after successful impersonation on this thread.
                let reverted = unsafe { RevertToSelf() };
                debug_assert_ne!(reverted, 0, "RevertToSelf failed during cleanup");
            }
        }
    }

    #[expect(
        unsafe_code,
        reason = "Windows exposes the connected pipe client token only through thread impersonation and token handle calls; every handle and impersonation lifetime is bounded here"
    )]
    fn pipe_client_is_current_user(pipe: HANDLE) -> std::io::Result<bool> {
        let current = process_user()?;
        // SAFETY: `pipe` is a live connected named-pipe server handle borrowed for this synchronous call.
        if unsafe { ImpersonateNamedPipeClient(pipe) } == 0 {
            return Err(std::io::Error::last_os_error());
        }
        let reverting = RevertGuard(true);
        let mut token: HANDLE = core::ptr::null_mut();
        // SAFETY: the current thread is impersonating the connected client and `token` is writable.
        let opened = unsafe { OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, 1, &raw mut token) };
        let peer_token = if opened == 0 || token.is_null() {
            let failure = std::io::Error::last_os_error();
            reverting.revert()?;
            return Err(failure);
        } else {
            OwnedHandle(token)
        };
        reverting.revert()?;
        let peer = OwnedTokenUser::read(peer_token.0)?;
        // SAFETY: both SID pointers belong to live TOKEN_USER buffers for the duration of the comparison.
        Ok(unsafe { EqualSid(current.sid(), peer.sid()) } != 0)
    }

    impl Drop for SecurityDescriptor {
        #[expect(
            unsafe_code,
            reason = "the pointer was returned by the SDDL converter, is owned by this value, and LocalFree is its only release API"
        )]
        fn drop(&mut self) {
            // SAFETY: this pointer came from the successful conversion call above, has not been freed, and no pipe
            // creation call retains it after returning.
            let leftover = unsafe { LocalFree(self.0) };
            debug_assert!(
                leftover.is_null(),
                "LocalFree did not release the security descriptor"
            );
        }
    }

    /// What the platform says when every instance of a pipe already has a client.
    ///
    /// `ERROR_PIPE_BUSY`. Named here as a number because it is the whole of what this file needs from the platform's
    /// error definitions, and one integer is not worth a dependency on the definitions of thousands.
    const ALL_INSTANCES_TAKEN: i32 = 231;

    /// How long to keep trying while every instance is taken.
    ///
    /// The daemon creates the next instance immediately after handing over the current one, so the window is the gap
    /// between those two moments and is measured in microseconds when nothing else is happening. This is generous
    /// against a loaded machine and still short enough that a daemon which has genuinely stopped accepting is
    /// reported rather than waited on forever.
    const WHILE_BUSY: core::time::Duration = core::time::Duration::from_secs(2);

    /// How long to wait before asking again.
    const BETWEEN_TRIES: core::time::Duration = core::time::Duration::from_millis(20);

    /// Connect to the daemon's pipe.
    ///
    /// # Why this waits rather than reporting that the daemon is busy
    ///
    /// A pipe instance serves one client. The daemon creates the next one as soon as it has handed over the current
    /// one, but there is a moment in between, and a second caller arriving in that moment is told every instance is
    /// taken. That is not a failure of anything: nothing is wrong, the daemon is up, and asking again works.
    ///
    /// Reporting it would put the retry in every caller, which means an operator running one command while another
    /// is starting would see an error that means nothing to them and goes away by itself. Waiting is the documented
    /// answer to this condition and it belongs in the one place that knows what the condition is.
    pub(super) async fn connect(address: &str) -> Result<Stream, TransportError> {
        let give_up_at = tokio::time::Instant::now() + WHILE_BUSY;
        loop {
            match ClientOptions::new().open(address) {
                Ok(client) => return Ok(Stream::Client(client)),

                // Nothing is listening. The one answer a caller acts on, by starting a daemon.
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    return Err(TransportError::NotListening {
                        address: address.to_owned(),
                    });
                }

                // Every instance has a client and the next one has not appeared yet. Said out loud only if it goes
                // on long enough to mean something other than a moment between two clients.
                Err(error) if error.raw_os_error() == Some(ALL_INSTANCES_TAKEN) => {
                    if tokio::time::Instant::now() >= give_up_at {
                        return Err(TransportError::Connect {
                            address: address.to_owned(),
                            detail: format!(
                                "every instance has been taken for {} seconds, so the daemon is up and not accepting",
                                WHILE_BUSY.as_secs()
                            ),
                        });
                    }
                    tokio::time::sleep(BETWEEN_TRIES).await;
                }

                Err(error) => {
                    return Err(TransportError::Connect {
                        address: address.to_owned(),
                        detail: error.to_string(),
                    });
                }
            }
        }
    }
}

#[cfg(unix)]
mod platform {
    //! A socket file, protected by the directory it sits in.

    use tokio::net::{UnixListener, UnixStream};

    use super::TransportError;

    pub(super) fn create_owner_only_file(
        path: &std::path::Path,
        contents: &[u8],
    ) -> std::io::Result<()> {
        use std::io::Write as _;
        use std::os::unix::fs::OpenOptionsExt as _;

        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)?;
        file.write_all(contents)?;
        file.sync_all()
    }

    /// The byte stream. One type for both ends here, unlike the other platform.
    pub(super) type Stream = UnixStream;

    /// The socket the daemon waits on.
    pub(super) struct Listener {
        /// The platform's listener.
        inner: UnixListener,
    }

    impl Listener {
        pub(super) fn bind(address: &str, owner_only: bool) -> Result<Self, TransportError> {
            use std::os::unix::fs::PermissionsExt as _;

            if owner_only {
                let Some(parent) = std::path::Path::new(address).parent() else {
                    return Err(TransportError::Bind {
                        address: address.to_owned(),
                        detail: "the owner-only socket has no parent directory".to_owned(),
                    });
                };
                std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700)).map_err(
                    |error| TransportError::Bind {
                        address: address.to_owned(),
                        detail: error.to_string(),
                    },
                )?;
            }
            // A socket file left by a daemon that is gone would refuse the bind. Removed first, and a failure to remove
            // is not reported here: the bind below is what decides, and it names the address either way.
            if std::path::Path::new(address).exists() {
                drop(std::fs::remove_file(address));
            }
            let inner = UnixListener::bind(address).map_err(|error| TransportError::Bind {
                address: address.to_owned(),
                detail: error.to_string(),
            })?;
            if owner_only {
                std::fs::set_permissions(address, std::fs::Permissions::from_mode(0o600)).map_err(
                    |error| TransportError::Bind {
                        address: address.to_owned(),
                        detail: error.to_string(),
                    },
                )?;
            }
            Ok(Self { inner })
        }

        pub(super) async fn accept(
            &mut self,
            address: &str,
            owner_only: bool,
        ) -> Result<Stream, TransportError> {
            use std::os::unix::fs::MetadataExt as _;

            let (stream, _peer) =
                self.inner
                    .accept()
                    .await
                    .map_err(|error| TransportError::Bind {
                        address: address.to_owned(),
                        detail: error.to_string(),
                    })?;
            if owner_only {
                let expected = std::fs::metadata(address)
                    .map_err(|error| TransportError::Bind {
                        address: address.to_owned(),
                        detail: error.to_string(),
                    })?
                    .uid();
                let actual = stream
                    .peer_cred()
                    .map_err(|error| TransportError::Bind {
                        address: address.to_owned(),
                        detail: error.to_string(),
                    })?
                    .uid();
                if actual != expected {
                    return Err(TransportError::Bind {
                        address: address.to_owned(),
                        detail: "the Runtime socket peer does not match its owner".to_owned(),
                    });
                }
            }
            Ok(stream)
        }
    }

    /// Connect to the daemon's socket.
    pub(super) async fn connect(address: &str) -> Result<Stream, TransportError> {
        match UnixStream::connect(address).await {
            Ok(stream) => Ok(stream),
            // Both mean the same thing to a caller deciding whether to start a daemon: nothing is there. A socket file
            // that exists with nobody listening answers with a refused connection rather than a missing file.
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
                ) =>
            {
                Err(TransportError::NotListening {
                    address: address.to_owned(),
                })
            }
            Err(error) => Err(TransportError::Connect {
                address: address.to_owned(),
                detail: error.to_string(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An address of this test's own, inside a directory it made.
    fn an_address(name: &str) -> String {
        if cfg!(windows) {
            format!(r"\\.\pipe\runtrol-test-{name}-{}", std::process::id())
        } else {
            let dir =
                std::env::temp_dir().join(format!("runtrol-ipc-{name}-{}", std::process::id()));
            std::fs::create_dir_all(&dir).expect("create the scratch directory");
            dir.join("runtrol.sock").to_string_lossy().into_owned()
        }
    }

    #[tokio::test]
    async fn a_frame_sent_at_one_end_arrives_whole_at_the_other() {
        let address = an_address("roundtrip");
        let mut listener = Listener::bind(&address).await.expect("the endpoint binds");
        assert_eq!(listener.address(), address);

        let serving = tokio::spawn(async move {
            let mut connection = listener.accept().await.expect("a client arrives");
            let asked = connection.recv().await.expect("readable").expect("a frame");
            connection
                .send(&asked)
                .await
                .expect("the answer is writable");
        });

        let mut client = connect(&address).await.expect("the daemon is there");
        client.send(b"{\"ask\":\"list\"}").await.expect("writable");
        let answer = client.recv().await.expect("readable").expect("a frame");
        assert_eq!(&*answer, b"{\"ask\":\"list\"}");

        serving.await.expect("the server finished");
    }

    #[tokio::test]
    async fn an_owner_only_endpoint_accepts_its_current_user() {
        let address = an_address("owner-only");
        let mut listener = Listener::bind_owner_only(&address)
            .await
            .expect("the owner-only endpoint binds");
        let serving = tokio::spawn(async move {
            let mut connection = listener.accept().await.expect("the owner arrives");
            connection
                .recv()
                .await
                .expect("owner connection is readable")
                .expect("owner frame arrives")
        });

        let mut client = connect(&address).await.expect("the owner connects");
        client.send(b"owner").await.expect("owner frame writes");
        assert_eq!(&*serving.await.expect("server task finishes"), b"owner");
    }

    #[tokio::test]
    async fn frame_parts_cross_without_a_full_size_staging_buffer() {
        let address = an_address("parts");
        let mut listener = Listener::bind(&address).await.expect("the endpoint binds");
        let serving = tokio::spawn(async move {
            let mut connection = listener.accept().await.expect("a client arrives");
            connection.recv().await.expect("readable").expect("a frame")
        });

        let mut client = connect(&address).await.expect("the daemon is there");
        client
            .send_parts(&[b"first", b"-", b"second"])
            .await
            .expect("parts are writable");
        let received = serving.await.expect("the server finished");
        assert_eq!(&*received, b"first-second");
    }

    #[tokio::test]
    async fn a_second_caller_arriving_before_the_first_is_accepted_still_gets_in() {
        // An operator running one command while another is starting. On one platform a pipe instance serves exactly
        // one client, so the second caller arrives at a moment when every instance is taken and the next has not been
        // made yet. Nothing is wrong when that happens, and reporting it would show an error that means nothing and
        // goes away by itself.
        let address = an_address("two-at-once");
        let mut listener = Listener::bind(&address).await.expect("the endpoint binds");

        // The daemon's side, waiting. It cannot run until this test waits for something, which is what puts the
        // second caller in the moment being reproduced.
        let serving = tokio::spawn(async move {
            let first = listener.accept().await.expect("the first caller arrives");
            let second = listener.accept().await.expect("the second caller arrives");
            (first, second)
        });

        // Neither of these waits for anything when it succeeds, so nothing has been accepted by the time the second
        // one asks. That is the moment: one endpoint, one caller already on it, and the next not made yet.
        let first = connect(&address).await.expect("the first caller gets in");
        let second = connect(&address)
            .await
            .expect("a caller arriving before the daemon has accepted still gets in");

        let served = serving.await.expect("the daemon's side finished");
        drop((first, second, served));
    }

    #[tokio::test]
    async fn several_frames_written_together_are_read_one_at_a_time() {
        // A read delivers whatever has arrived, which may be three frames. Waiting for more bytes before handing over
        // the ones in hand would stall on the last of them.
        let address = an_address("several");
        let mut listener = Listener::bind(&address).await.expect("binds");

        let serving = tokio::spawn(async move {
            let mut connection = listener.accept().await.expect("a client arrives");
            let mut read = Vec::new();
            while let Some(frame) = connection.recv().await.expect("readable") {
                read.push(String::from_utf8_lossy(&frame).into_owned());
                if read.len() == 3 {
                    break;
                }
            }
            read
        });

        let mut client = connect(&address).await.expect("the daemon is there");
        for text in ["one", "two", "three"] {
            client.send(text.as_bytes()).await.expect("writable");
        }
        let read = serving.await.expect("the server finished");
        assert_eq!(read, vec!["one", "two", "three"]);
    }

    #[tokio::test]
    async fn nothing_listening_is_told_apart_from_a_real_failure() {
        // The one question a command surface asks before starting a daemon. Anything else must not be answered by
        // launching a second one on top of a broken endpoint.
        let address = an_address("absent");
        match connect(&address).await {
            Err(error) => {
                assert!(
                    error.means_no_daemon(),
                    "an absent daemon has to be recognisable: {error}"
                );
                assert!(error.to_string().contains(&address), "{error}");
            }
            Ok(_) => panic!("nothing should be listening at a fresh address"),
        }
    }

    #[tokio::test]
    async fn the_other_end_going_away_is_the_end_and_not_a_failure() {
        // A command surface that stopped is not an error the daemon has to act on.
        let address = an_address("hangup");
        let mut listener = Listener::bind(&address).await.expect("binds");

        let serving = tokio::spawn(async move {
            let mut connection = listener.accept().await.expect("a client arrives");
            connection.recv().await
        });

        let client = connect(&address).await.expect("the daemon is there");
        drop(client);

        let answer = serving.await.expect("the server finished");
        assert!(
            matches!(answer, Ok(None)),
            "a hangup is the end, not a failure: {answer:?}"
        );
    }

    #[tokio::test]
    async fn a_frame_larger_than_this_build_carries_is_refused_at_the_sender() {
        let address = an_address("toolarge");
        let mut listener = Listener::bind(&address).await.expect("binds");
        let serving = tokio::spawn(async move { listener.accept().await.map(|_| ()) });

        let mut client = connect(&address).await.expect("the daemon is there");
        let error = client
            .send(&vec![0; MAX_FRAME + 1])
            .await
            .expect_err("a payload past the limit must be refused");
        assert!(matches!(error, TransportError::Frame(_)), "{error}");
        assert!(!error.means_no_daemon());

        drop(serving.await);
    }

    #[test]
    fn only_an_absent_daemon_reads_as_an_absent_daemon() {
        // Every other reason a connection fails is a real failure, and answering one of those by starting a second
        // daemon would put two of them on one endpoint.
        let address = "somewhere".to_owned();
        assert!(
            TransportError::NotListening {
                address: address.clone()
            }
            .means_no_daemon()
        );
        for other in [
            TransportError::Bind {
                address: address.clone(),
                detail: "in use".to_owned(),
            },
            TransportError::Connect {
                address,
                detail: "refused".to_owned(),
            },
            TransportError::Io {
                doing: "reading",
                detail: "broken".to_owned(),
            },
        ] {
            assert!(!other.means_no_daemon(), "{other}");
        }
    }
}
