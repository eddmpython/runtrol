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
//! compares the peer UID with the socket owner. Windows owner-only endpoints install the current-user DACL and
//! compare the peer process's user SID with the daemon's. Logon-only endpoints instead restrict the DACL to the
//! current logon and verify the first read under the pipe client's impersonated token before returning a frame.
//! Both expose the exact kernel peer process identity and reject remote clients. The Windows
//! system calls are confined to audited unsafe blocks in this module.

use bytes::{Bytes, BytesMut};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

use crate::frame::{FrameError, HEADER, MAX_FRAME, encode, frame_size};

/// How much the reader holds between frames.
///
/// Constant, and deliberately not a function of [`MAX_FRAME`]. A frame larger than this grows the buffer for as long as
/// that frame is being assembled and no longer, so one big message is a spike and not a new resting size.
pub const READ_BUFFER: usize = 64 * 1024;

/// Kernel identity of the process at the client end of an owner-only connection.
///
/// A PID alone can be reused. The start value is captured while the authenticated connection is accepted and uses
/// the platform's native unit: Windows file time ticks, Linux boot ticks, or macOS epoch microseconds. Callers compare
/// the complete pair and never interpret the value or persist it across a reboot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PeerProcess {
    identity: runtrol_provider::ProcessIdentity,
}

impl PeerProcess {
    fn new(pid: u32, started: u64) -> Option<Self> {
        runtrol_provider::ProcessIdentity::new(pid, started).map(|identity| Self { identity })
    }

    /// Operating-system process identifier observed on the connected transport.
    #[must_use]
    pub const fn pid(self) -> u32 {
        self.identity.pid()
    }

    /// Kernel start value that closes PID reuse for the lifetime of this machine boot.
    #[must_use]
    pub const fn started(self) -> u64 {
        self.identity.started()
    }

    /// Complete structural process identity for ancestry and PID-reuse checks.
    #[must_use]
    pub const fn identity(self) -> runtrol_provider::ProcessIdentity {
        self.identity
    }
}

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
    /// Exact client process identity, present only on the accepting end of an owner-only endpoint.
    peer_process: Option<PeerProcess>,
    /// A logon-only pipe must prove its first received bytes before returning any frame.
    #[cfg(windows)]
    logon_pending: bool,
}

impl Connection {
    /// Exact process at the other end when this is the accepting side of an owner-only endpoint.
    ///
    /// Ordinary private control endpoints and the connecting side carry no process authority.
    #[must_use]
    pub const fn peer_process(&self) -> Option<PeerProcess> {
        self.peer_process
    }

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
        self.recv_bounded(MAX_FRAME).await
    }

    /// Receive one frame within the caller's payload ceiling, checked before reading or allocating its body.
    ///
    /// Cancellation preserves partial input in this connection. No bytes from the next frame are read early.
    ///
    /// # Errors
    ///
    /// [`TransportError::Frame`] when the prefix exceeds `limit` or the wire ceiling, [`TransportError::Io`]
    /// when the read fails. A refused connection must be closed by its caller.
    pub async fn recv_bounded(&mut self, limit: usize) -> Result<Option<Bytes>, TransportError> {
        loop {
            let total = frame_size(&self.pending, limit)?;
            if let Some(total) = total
                && self.pending.len() >= total
            {
                let frame = self.pending.split_to(total).freeze();
                return Ok(Some(frame.slice(HEADER..)));
            }
            if self.closed {
                return Ok(None);
            }
            let remaining = total.unwrap_or(HEADER) - self.pending.len();
            self.pending.reserve(remaining);
            // Read the prefix alone, then only its admitted payload. Even bytes already waiting in the pipe
            // cannot make a narrow endpoint allocate the broader transport ceiling.
            let read = (&mut self.stream)
                .take(remaining as u64)
                .read_buf(&mut self.pending)
                .await
                .map_err(|error| TransportError::Io {
                    doing: "receiving a frame",
                    detail: error.to_string(),
                })?;
            #[cfg(windows)]
            if read != 0 && self.logon_pending {
                if let Err(error) = self.stream.verify_logon() {
                    self.closed = true;
                    self.pending.clear();
                    return Err(TransportError::Io {
                        doing: "verifying the pipe client's logon",
                        detail: error.to_string(),
                    });
                }
                self.logon_pending = false;
            }
            if read == 0 {
                self.closed = true;
                if self.pending.is_empty() {
                    return Ok(None);
                }
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AccessPolicy {
    Private,
    Owner,
    Logon,
}

/// Where the daemon waits for the command surface.
pub struct Listener {
    /// The platform's own mechanism.
    inner: platform::Listener,
    /// The address, kept for the error messages that have to name it.
    address: String,
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
            inner: platform::Listener::bind(address, AccessPolicy::Private)?,
            address: address.to_owned(),
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
            inner: platform::Listener::bind(address, AccessPolicy::Owner)?,
            address: address.to_owned(),
        })
    }

    /// Start an endpoint restricted to the current Windows logon, or to the owning user on Unix.
    ///
    /// Windows installs a logon SID DACL and verifies the client's impersonated logon on the first read, before
    /// returning a frame. A silent client therefore never makes accepting the next connection wait for input.
    ///
    /// # Errors
    ///
    /// [`TransportError::Bind`] when the platform cannot install this policy. There is no weaker fallback.
    #[expect(
        clippy::unused_async,
        reason = "the endpoint has to be registered with the active Tokio I/O runtime"
    )]
    pub async fn bind_logon_only(address: &str) -> Result<Self, TransportError> {
        Ok(Self {
            inner: platform::Listener::bind(address, AccessPolicy::Logon)?,
            address: address.to_owned(),
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
        let (stream, peer_process) = self.inner.accept(&self.address).await?;
        Ok(Connection {
            stream,
            pending: BytesMut::with_capacity(READ_BUFFER),
            closed: false,
            peer_process,
            #[cfg(windows)]
            logon_pending: self.inner.logon_only(),
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
        peer_process: None,
        #[cfg(windows)]
        logon_pending: false,
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
        CloseHandle, FILETIME, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE, LocalFree,
    };
    use windows_sys::Win32::Security::Authorization::{
        ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
        SDDL_REVISION_1,
    };
    use windows_sys::Win32::Security::{
        EqualSid, GetTokenInformation, PSECURITY_DESCRIPTOR, PSID, RevertToSelf,
        SECURITY_ATTRIBUTES, TOKEN_GROUPS, TOKEN_QUERY, TOKEN_USER, TokenLogonSid, TokenUser,
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
        GetCurrentProcess, GetCurrentThread, GetProcessTimes, OpenProcess, OpenProcessToken,
        OpenThreadToken, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    use super::{AccessPolicy, PeerProcess, TransportError};

    /// The byte stream, whichever end this is.
    pub(super) enum Stream {
        /// The daemon's end.
        Server(NamedPipeServer),
        /// The command surface's end.
        Client(NamedPipeClient),
    }

    impl Stream {
        pub(super) fn verify_logon(&self) -> std::io::Result<()> {
            let Self::Server(pipe) = self else {
                return Err(std::io::Error::other(
                    "only a server pipe may verify its client's logon",
                ));
            };
            if pipe_client_is_current_logon(pipe)? {
                Ok(())
            } else {
                Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "the pipe client does not belong to the Runtime's logon",
                ))
            }
        }
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
    /// A named pipe has no accept loop of its own: one instance serves one client. Two unclaimed instances remain
    /// alive so a burst caller always finds the name, and cancelling an `accept` future never drops the instance it
    /// was waiting on.
    pub(super) struct Listener {
        /// The instance whose connection is accepted next.
        waiting: Option<NamedPipeServer>,
        /// One additional unclaimed instance for a caller arriving before the owner loop accepts again.
        spare: Option<NamedPipeServer>,
        /// Every instance and accepted peer use this same authority.
        policy: AccessPolicy,
    }

    impl Listener {
        pub(super) fn bind(address: &str, policy: AccessPolicy) -> Result<Self, TransportError> {
            let first = instance(address, true, policy)?;
            let spare = instance(address, false, policy)?;
            Ok(Self {
                waiting: Some(first),
                spare: Some(spare),
                policy,
            })
        }

        pub(super) fn logon_only(&self) -> bool {
            self.policy == AccessPolicy::Logon
        }

        #[expect(
            unsafe_code,
            reason = "Windows exposes the connected named-pipe client process only through a handle system call; the live handle and output pointer are bounded beside the call"
        )]
        pub(super) async fn accept(
            &mut self,
            address: &str,
        ) -> Result<(Stream, Option<PeerProcess>), TransportError> {
            loop {
                // Both borrowed instances stay in `self` across the await. A Windows caller may claim either
                // available instance, so accepting only one of them can strand a successful single caller until a
                // second caller happens to arrive. Dropping this future leaves both instances alive for the next call.
                let waiting = self.waiting.as_ref().ok_or_else(|| TransportError::Bind {
                    address: address.to_owned(),
                    detail: "the Windows listener lost its waiting pipe".to_owned(),
                })?;
                let spare = self.spare.as_ref().ok_or_else(|| TransportError::Bind {
                    address: address.to_owned(),
                    detail: "the Windows listener lost its spare pipe".to_owned(),
                })?;
                let accepted_waiting = tokio::select! {
                    connected = waiting.connect() => connected
                        .map(|()| true)
                        .map_err(|error| TransportError::Bind {
                            address: address.to_owned(),
                            detail: error.to_string(),
                        }),
                    connected = spare.connect() => connected
                        .map(|()| false)
                        .map_err(|error| TransportError::Bind {
                            address: address.to_owned(),
                            detail: error.to_string(),
                        }),
                }?;
                let replacement = instance(address, false, self.policy)?;
                let waiting = if accepted_waiting {
                    self.waiting.replace(replacement)
                } else {
                    self.spare.replace(replacement)
                }
                .ok_or_else(|| TransportError::Bind {
                    address: address.to_owned(),
                    detail: "the Windows listener lost its connected pipe".to_owned(),
                })?;
                let peer_process = if self.policy == AccessPolicy::Private {
                    None
                } else {
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
                    // A sandbox may use another primary process user while connecting under this logon. The
                    // logon-only policy proves the effective pipe token on first read, not the primary user.
                    if self.policy == AccessPolicy::Owner {
                        match pipe_client_is_current_user(peer) {
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
                    let started =
                        process_start_for_pid(peer).map_err(|error| TransportError::Bind {
                            address: address.to_owned(),
                            detail: error.to_string(),
                        })?;
                    Some(
                        PeerProcess::new(peer, started).ok_or_else(|| TransportError::Bind {
                            address: address.to_owned(),
                            detail: "the Runtime pipe peer exposed an unusable process identity"
                                .to_owned(),
                        })?,
                    )
                };
                return Ok((Stream::Server(waiting), peer_process));
            }
        }
    }

    /// One pipe instance.
    fn instance(
        address: &str,
        first: bool,
        policy: AccessPolicy,
    ) -> Result<NamedPipeServer, TransportError> {
        if policy != AccessPolicy::Private {
            return restricted_instance(address, first, policy);
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

    /// One restricted pipe instance. Tokio's safe builder has no security-attributes input, so the exact DACL is
    /// installed at creation and the resulting overlapped handle is transferred to Tokio once.
    #[expect(
        unsafe_code,
        reason = "Tokio has no safe SECURITY_ATTRIBUTES input; one owned overlapped handle is created with an explicit DACL and transferred exactly once"
    )]
    fn restricted_instance(
        address: &str,
        first: bool,
        policy: AccessPolicy,
    ) -> Result<NamedPipeServer, TransportError> {
        let descriptor = if policy == AccessPolicy::Logon {
            SecurityDescriptor::current_logon()
        } else {
            SecurityDescriptor::current_owner()
        };
        let mut security = descriptor.map_err(|error| TransportError::Bind {
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
        fn current_owner() -> std::io::Result<Self> {
            let owner = process_user()?.sid_string()?;
            Self::for_principal(&owner, &owner)
        }

        fn current_logon() -> std::io::Result<Self> {
            let owner = process_user()?.sid_string()?;
            let logon = process_logon()?.sid_string()?;
            // A user SID spans logons. Only the token's logon SID is allowed to open this endpoint.
            // https://learn.microsoft.com/en-us/windows/win32/ipc/named-pipe-security-and-access-rights
            Self::for_principal(&owner, &logon)
        }

        #[expect(
            unsafe_code,
            reason = "Windows allocates an SDDL security descriptor through an FFI output pointer that is checked and then owned by this RAII value"
        )]
        fn for_principal(owner: &str, principal: &str) -> std::io::Result<Self> {
            // The exact process token user owns the object; only the requested principal receives full control.
            // Token default ownership can be an administrator group for service accounts, so both ownership and the
            // protected DACL must be explicit. No owner-rights alias, Everyone, anonymous, network, administrator, or
            // SYSTEM ACE is present.
            let sddl: Vec<u16> = format!("O:{owner}D:P(A;;GA;;;{principal})")
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

    #[derive(Clone, Copy)]
    enum TokenSid {
        User,
        Logon,
    }

    /// Aligned storage returned by `GetTokenInformation`, owning the SID that points into it.
    struct OwnedTokenSid {
        storage: Vec<usize>,
        kind: TokenSid,
    }

    impl OwnedTokenSid {
        #[expect(
            unsafe_code,
            reason = "GetTokenInformation writes a bounded TOKEN_USER or TOKEN_GROUPS into aligned owned storage and both calls validate their output lengths"
        )]
        fn read(token: HANDLE, kind: TokenSid) -> std::io::Result<Self> {
            let (class, minimum) = match kind {
                TokenSid::User => (TokenUser, core::mem::size_of::<TOKEN_USER>()),
                TokenSid::Logon => (TokenLogonSid, core::mem::size_of::<TOKEN_GROUPS>()),
            };
            let mut needed = 0_u32;
            // SAFETY: the first call intentionally supplies no output storage and only requests the required size.
            unsafe { GetTokenInformation(token, class, core::ptr::null_mut(), 0, &raw mut needed) };
            if needed == 0 {
                return Err(std::io::Error::last_os_error());
            }
            let needed_usize = usize::try_from(needed).map_err(|_| {
                std::io::Error::other("token information length does not fit usize")
            })?;
            if needed_usize < minimum {
                return Err(std::io::Error::other(
                    "the token did not expose a complete SID record",
                ));
            }
            let words = needed_usize.div_ceil(core::mem::size_of::<usize>());
            let mut storage = vec![0_usize; words];
            // SAFETY: storage is aligned for both token layouts, has `needed` writable bytes, and remains alive.
            let read = unsafe {
                GetTokenInformation(
                    token,
                    class,
                    storage.as_mut_ptr().cast(),
                    needed,
                    &raw mut needed,
                )
            };
            if read == 0 {
                return Err(std::io::Error::last_os_error());
            }
            if !usize::try_from(needed)
                .is_ok_and(|written| written >= minimum && written <= needed_usize)
            {
                return Err(std::io::Error::other(
                    "the token returned an incomplete SID record",
                ));
            }
            if matches!(kind, TokenSid::Logon) {
                // TokenLogonSid returns TOKEN_GROUPS with the token's one logon SID. No absent or ambiguous SID
                // may become a weaker user-only check.
                // https://learn.microsoft.com/en-us/windows/win32/api/winnt/ne-winnt-token_information_class
                // SAFETY: the successful call initialized at least one complete aligned TOKEN_GROUPS value.
                let count = unsafe { (*storage.as_ptr().cast::<TOKEN_GROUPS>()).GroupCount };
                if count != 1 {
                    return Err(std::io::Error::other(
                        "the token does not expose exactly one logon SID",
                    ));
                }
            }
            Ok(Self { storage, kind })
        }

        #[expect(
            unsafe_code,
            reason = "the storage was populated and length-checked for the selected token layout and owns its SID"
        )]
        fn sid(&self) -> PSID {
            match self.kind {
                TokenSid::User => {
                    // SAFETY: `read` initialized a complete aligned TOKEN_USER and the allocation is still owned.
                    unsafe { (*self.storage.as_ptr().cast::<TOKEN_USER>()).User.Sid }
                }
                TokenSid::Logon => {
                    // SAFETY: `read` proved a complete TOKEN_GROUPS with exactly one group and keeps it alive.
                    unsafe { (*self.storage.as_ptr().cast::<TOKEN_GROUPS>()).Groups }
                        .first()
                        .map_or(core::ptr::null_mut(), |group| group.Sid)
                }
            }
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
    fn process_user() -> std::io::Result<OwnedTokenSid> {
        // SAFETY: GetCurrentProcess returns a valid pseudo-handle borrowed only for this call.
        let token = process_token(unsafe { GetCurrentProcess() })?;
        OwnedTokenSid::read(token.0, TokenSid::User)
    }

    #[expect(
        unsafe_code,
        reason = "the current process pseudo-handle is borrowed to query its primary token's logon SID"
    )]
    fn process_logon() -> std::io::Result<OwnedTokenSid> {
        // SAFETY: GetCurrentProcess returns a valid borrowed pseudo-handle.
        let token = process_token(unsafe { GetCurrentProcess() })?;
        OwnedTokenSid::read(token.0, TokenSid::Logon)
    }

    #[expect(
        unsafe_code,
        reason = "the connected pipe reports one client PID whose process token is opened read-only and owned for the bounded SID comparison"
    )]
    fn process_user_for_pid(process_id: u32) -> std::io::Result<OwnedTokenSid> {
        // SAFETY: the process identifier came from the live connected pipe. The returned query-only handle is checked
        // and transferred to `OwnedHandle` exactly once.
        let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, process_id) };
        if process.is_null() {
            return Err(std::io::Error::last_os_error());
        }
        let process = OwnedHandle(process);
        let token = process_token(process.0)?;
        OwnedTokenSid::read(token.0, TokenSid::User)
    }

    #[expect(
        unsafe_code,
        reason = "the connected pipe reports one client PID whose query handle yields an exact process creation time"
    )]
    fn process_start_for_pid(process_id: u32) -> std::io::Result<u64> {
        // SAFETY: the process identifier came from the live connected pipe. The returned query-only handle is checked
        // and transferred to `OwnedHandle` exactly once.
        let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, process_id) };
        if process.is_null() {
            return Err(std::io::Error::last_os_error());
        }
        let process = OwnedHandle(process);
        let mut creation = FILETIME::default();
        let mut exit = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        // SAFETY: the checked process handle has query rights and every output pointer names one writable FILETIME.
        if unsafe {
            GetProcessTimes(
                process.0,
                &raw mut creation,
                &raw mut exit,
                &raw mut kernel,
                &raw mut user,
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error());
        }
        Ok((u64::from(creation.dwHighDateTime) << 32) | u64::from(creation.dwLowDateTime))
    }

    #[expect(
        unsafe_code,
        reason = "a borrowed live process handle is used only to open one owned query token"
    )]
    fn process_token(process: HANDLE) -> std::io::Result<OwnedHandle> {
        let mut token: HANDLE = core::ptr::null_mut();
        // SAFETY: `process` is a borrowed live process handle and `token` is a writable output pointer.
        if unsafe { OpenProcessToken(process, TOKEN_QUERY, &raw mut token) } == 0 || token.is_null()
        {
            return Err(std::io::Error::last_os_error());
        }
        Ok(OwnedHandle(token))
    }

    /// An impersonation never crosses an await or returns to Tokio. A failed revert cannot leave a worker running
    /// as a client: Windows requires the process to stop in that case.
    /// <https://learn.microsoft.com/en-us/windows/win32/api/securitybaseapi/nf-securitybaseapi-reverttoself>
    struct RevertGuard;

    impl Drop for RevertGuard {
        #[expect(
            unsafe_code,
            reason = "this synchronous scope owns the calling thread's impersonation and must revert before return"
        )]
        fn drop(&mut self) {
            // SAFETY: this guard exists only after a successful impersonation on this thread and never crosses await.
            if unsafe { RevertToSelf() } == 0 {
                std::process::abort();
            }
        }
    }

    #[expect(
        unsafe_code,
        reason = "the live server pipe and thread token are queried synchronously under a guaranteed revert guard"
    )]
    fn pipe_client_is_current_logon(pipe: &NamedPipeServer) -> std::io::Result<bool> {
        let expected = process_logon()?;
        // SAFETY: the server owns this connected handle and has read bytes from it. The guard below owns reverting
        // this exact thread before any return, panic unwind, or future poll.
        if unsafe { ImpersonateNamedPipeClient(pipe.as_raw_handle()) } == 0 {
            return Err(std::io::Error::last_os_error());
        }
        let _revert = RevertGuard;
        let mut token = core::ptr::null_mut();
        // SAFETY: the current-thread pseudo-handle and output pointer remain valid for the call. OpenAsSelf queries
        // with the server process identity, including when the client's token grants identification only.
        if unsafe { OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, 1, &raw mut token) } == 0
            || token.is_null()
        {
            return Err(std::io::Error::last_os_error());
        }
        let token = OwnedHandle(token);
        let actual = OwnedTokenSid::read(token.0, TokenSid::Logon)?;
        // SAFETY: both pointers belong to validated live token buffers held until this comparison returns.
        Ok(unsafe { EqualSid(expected.sid(), actual.sid()) } != 0)
    }

    #[expect(
        unsafe_code,
        reason = "Windows compares two validated TOKEN_USER SID pointers whose owned buffers stay live for the call"
    )]
    fn pipe_client_is_current_user(process_id: u32) -> std::io::Result<bool> {
        let current = process_user()?;
        let peer = process_user_for_pid(process_id)?;
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
    #[cfg(test)]
    mod tests;
}

#[cfg(unix)]
mod platform {
    //! A socket file, protected by the directory it sits in.

    use tokio::net::{UnixListener, UnixStream};

    use super::{AccessPolicy, PeerProcess, TransportError};

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
        /// Unix logon-only endpoints have the same owner boundary as owner-only endpoints.
        policy: AccessPolicy,
    }

    impl Listener {
        pub(super) fn bind(address: &str, policy: AccessPolicy) -> Result<Self, TransportError> {
            use std::os::unix::fs::PermissionsExt as _;

            if policy != AccessPolicy::Private {
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
            // A socket file left by a daemon that is gone refuses the bind, but the file alone cannot say
            // whether its daemon is gone. Removing it blindly let a duplicate daemon unlink a LIVE listener
            // and take its address (measured 2026-08-27 on the CI hosts: one transiently refused connect made
            // a command start a second same-generation daemon, which stole the socket, could not take the
            // exclusive store, and sat unaccepting for the whole handover deadline while every new client
            // connected into its backlog and was never greeted). The bind is tried as-is first; only an
            // address that is in use AND answers nobody is a leftover, and only that is removed.
            let inner = match UnixListener::bind(address) {
                Ok(inner) => inner,
                Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => {
                    match std::os::unix::net::UnixStream::connect(address) {
                        Ok(_live) => {
                            return Err(TransportError::Bind {
                                address: address.to_owned(),
                                detail: "another daemon is already serving this address".to_owned(),
                            });
                        }
                        Err(_refused) => {
                            drop(std::fs::remove_file(address));
                            UnixListener::bind(address).map_err(|error| TransportError::Bind {
                                address: address.to_owned(),
                                detail: error.to_string(),
                            })?
                        }
                    }
                }
                Err(error) => {
                    return Err(TransportError::Bind {
                        address: address.to_owned(),
                        detail: error.to_string(),
                    });
                }
            };
            if policy != AccessPolicy::Private {
                std::fs::set_permissions(address, std::fs::Permissions::from_mode(0o600)).map_err(
                    |error| TransportError::Bind {
                        address: address.to_owned(),
                        detail: error.to_string(),
                    },
                )?;
            }
            Ok(Self { inner, policy })
        }

        pub(super) async fn accept(
            &mut self,
            address: &str,
        ) -> Result<(Stream, Option<PeerProcess>), TransportError> {
            use std::os::unix::fs::MetadataExt as _;

            let (stream, _peer) =
                self.inner
                    .accept()
                    .await
                    .map_err(|error| TransportError::Bind {
                        address: address.to_owned(),
                        detail: error.to_string(),
                    })?;
            let peer_process = if self.policy == AccessPolicy::Private {
                None
            } else {
                let expected = std::fs::metadata(address)
                    .map_err(|error| TransportError::Bind {
                        address: address.to_owned(),
                        detail: error.to_string(),
                    })?
                    .uid();
                let credentials = stream.peer_cred().map_err(|error| TransportError::Bind {
                    address: address.to_owned(),
                    detail: error.to_string(),
                })?;
                let actual = credentials.uid();
                if actual != expected {
                    return Err(TransportError::Bind {
                        address: address.to_owned(),
                        detail: "the Runtime socket peer does not match its owner".to_owned(),
                    });
                }
                let raw_pid = credentials.pid().ok_or_else(|| TransportError::Bind {
                    address: address.to_owned(),
                    detail: "the Runtime socket did not expose its peer process".to_owned(),
                })?;
                let pid = u32::try_from(raw_pid).map_err(|error| TransportError::Bind {
                    address: address.to_owned(),
                    detail: error.to_string(),
                })?;
                let started = process_start_for_pid(pid).map_err(|error| TransportError::Bind {
                    address: address.to_owned(),
                    detail: error.to_string(),
                })?;
                Some(
                    PeerProcess::new(pid, started).ok_or_else(|| TransportError::Bind {
                        address: address.to_owned(),
                        detail: "the Runtime socket peer exposed an unusable process identity"
                            .to_owned(),
                    })?,
                )
            };
            Ok((stream, peer_process))
        }
    }

    #[cfg(target_os = "linux")]
    fn process_start_for_pid(pid: u32) -> std::io::Result<u64> {
        let text = std::fs::read_to_string(format!("/proc/{pid}/stat"))?;
        let tail = text
            .rsplit_once(") ")
            .map(|(_, tail)| tail)
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "the process stat record has no command boundary",
                )
            })?;
        tail.split_whitespace()
            // Field 22 overall. The tail begins at field 3, so the start tick is index 19.
            .nth(19)
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "the process stat record has no start tick",
                )
            })?
            .parse::<u64>()
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
    }

    #[cfg(target_os = "macos")]
    #[expect(
        unsafe_code,
        reason = "macOS exposes the connected peer process start instant only through proc_pidinfo"
    )]
    fn process_start_for_pid(pid: u32) -> std::io::Result<u64> {
        let pid = i32::try_from(pid)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error))?;
        let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::uninit();
        let size = i32::try_from(std::mem::size_of::<libc::proc_bsdinfo>())
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error))?;
        // SAFETY: `info` is writable for the exact size passed to the kernel and is read only after a complete-size
        // result states that every byte was initialized.
        let read = unsafe {
            libc::proc_pidinfo(
                pid,
                libc::PROC_PIDTBSDINFO,
                0,
                info.as_mut_ptr().cast(),
                size,
            )
        };
        if read != size {
            return Err(if read == 0 {
                std::io::Error::last_os_error()
            } else {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "the kernel returned an incomplete process identity",
                )
            });
        }
        // SAFETY: the complete-size result above initialized every byte of `info`.
        let info = unsafe { info.assume_init() };
        info.pbi_start_tvsec
            .checked_mul(1_000_000)
            .and_then(|seconds| seconds.checked_add(info.pbi_start_tvusec))
            .ok_or_else(|| std::io::Error::other("the process start identity overflowed"))
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
            let peer = connection
                .peer_process()
                .expect("an accepted owner-only connection carries its kernel peer");
            let frame = connection
                .recv()
                .await
                .expect("owner connection is readable")
                .expect("owner frame arrives");
            (peer, frame)
        });

        let mut client = connect(&address).await.expect("the owner connects");
        assert_eq!(client.peer_process(), None);
        client.send(b"owner").await.expect("owner frame writes");
        let (peer, frame) = serving.await.expect("server task finishes");
        assert_eq!(peer.pid(), std::process::id());
        assert_ne!(peer.started(), 0);
        assert_eq!(
            peer.identity(),
            runtrol_childproc::process_identity(std::process::id())
                .expect("the process-tree authority reads the same current client")
        );
        assert_eq!(&*frame, b"owner");
    }

    #[tokio::test]
    async fn a_logon_only_endpoint_returns_the_verified_clients_frame() {
        let address = an_address("logon-roundtrip");
        let mut listener = Listener::bind_logon_only(&address)
            .await
            .expect("the logon-only endpoint binds");
        let serving = tokio::spawn(async move {
            let mut connection = listener.accept().await.expect("the client arrives");
            let frame = connection
                .recv_bounded(16)
                .await
                .expect("logon verified")
                .expect("frame");
            assert_eq!(
                connection.peer_process().expect("kernel peer").pid(),
                std::process::id()
            );
            connection.send(&frame).await.expect("reply writes");
        });
        let mut client = connect(&address).await.expect("the same logon connects");
        client.send(b"logon").await.expect("frame writes");
        assert_eq!(
            client.recv().await.expect("reply reads").expect("reply"),
            &b"logon"[..]
        );
        serving.await.expect("the server finished");
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

    #[cfg(windows)]
    #[tokio::test]
    async fn raw_windows_clients_always_find_a_waiting_pipe_instance() {
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };
        use std::time::Duration;

        use tokio::net::windows::named_pipe::ClientOptions;

        let address = an_address("two-raw-windows-clients");
        let mut listener = Listener::bind(&address).await.expect("the endpoint binds");
        let first_accepted = Arc::new(AtomicBool::new(false));
        let server_observed = Arc::clone(&first_accepted);
        let serving = tokio::spawn(async move {
            let first = listener
                .accept()
                .await
                .expect("the first raw caller arrives");
            server_observed.store(true, Ordering::Release);
            let second = listener
                .accept()
                .await
                .expect("the second raw caller arrives");
            (first, second)
        });
        tokio::task::yield_now().await;

        let first = ClientOptions::new()
            .open(&address)
            .expect("first raw pipe opens");
        tokio::time::timeout(Duration::from_secs(1), async {
            while !first_accepted.load(Ordering::Acquire) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("one raw caller is accepted without needing a second");
        let second = ClientOptions::new()
            .open(&address)
            .expect("second raw pipe opens");
        let served = serving
            .await
            .expect("the daemon side accepted both raw clients");
        drop((first, second, served));
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn cancelling_accept_keeps_two_waiting_pipe_instances() {
        use std::time::Duration;

        use tokio::net::windows::named_pipe::ClientOptions;

        let address = an_address("cancelled-windows-accept");
        let mut listener = Listener::bind(&address).await.expect("the endpoint binds");
        let timed_out = tokio::time::timeout(Duration::from_millis(1), listener.accept()).await;
        assert!(timed_out.is_err(), "accept stays pending without a caller");

        let first = ClientOptions::new()
            .open(&address)
            .expect("first raw pipe opens after cancellation");
        let second = ClientOptions::new()
            .open(&address)
            .expect("second raw pipe opens after cancellation");
        let accepted_first = listener.accept().await.expect("first caller is accepted");
        let accepted_second = listener.accept().await.expect("second caller is accepted");
        drop((first, second, accepted_first, accepted_second));
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
    async fn a_narrow_receiver_refuses_the_prefix_without_reading_or_reserving_its_body() {
        let address = an_address("narrow-prefix");
        let mut listener = Listener::bind(&address).await.expect("binds");
        let serving = tokio::spawn(async move {
            let mut connection = listener.accept().await.expect("a client arrives");
            let capacity = connection.pending.capacity();
            let error = connection
                .recv_bounded(32)
                .await
                .expect_err("prefix is too large");
            assert!(matches!(
                error,
                TransportError::Frame(FrameError::TooLarge {
                    bytes: 65537,
                    max: 32
                })
            ));
            assert_eq!(connection.pending.len(), HEADER);
            assert_eq!(connection.pending.capacity(), capacity);
        });
        let mut client = connect(&address).await.expect("connects");
        client
            .stream
            .write_all(&65_537_u32.to_be_bytes())
            .await
            .expect("prefix writes");
        tokio::time::timeout(std::time::Duration::from_secs(2), serving)
            .await
            .expect("refusal does not wait for a body")
            .expect("the receiver finished");
        drop(client);
    }

    #[tokio::test]
    async fn cancelling_a_partial_bounded_read_keeps_the_frame_and_its_successor_exact() {
        let address = an_address("narrow-cancel");
        let mut listener = Listener::bind(&address).await.expect("binds");
        let (ready, resume) = tokio::sync::oneshot::channel();
        let serving = tokio::spawn(async move {
            let mut connection = listener.accept().await.expect("a client arrives");
            assert!(
                tokio::time::timeout(
                    std::time::Duration::from_millis(100),
                    connection.recv_bounded(3)
                )
                .await
                .is_err()
            );
            ready.send(()).expect("the writer waits");
            assert_eq!(
                connection
                    .recv_bounded(3)
                    .await
                    .expect("complete")
                    .expect("frame"),
                &b"one"[..]
            );
            assert_eq!(
                connection
                    .recv_bounded(3)
                    .await
                    .expect("next")
                    .expect("frame"),
                &b"two"[..]
            );
        });
        let mut client = connect(&address).await.expect("connects");
        client
            .stream
            .write_all(&[0, 0, 0, 3, b'o'])
            .await
            .expect("partial frame writes");
        resume.await.expect("the read was cancelled");
        client
            .stream
            .write_all(b"ne")
            .await
            .expect("remainder writes");
        client.send(b"two").await.expect("next frame writes");
        serving.await.expect("the receiver finished");
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
