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
//! # What protects the endpoint today, and what does not yet
//!
//! **Remote clients are refused.** A named pipe is asked to reject them explicitly rather than relying on a default,
//! because a default is something a later release may change.
//!
//! **The endpoint lives inside a directory only the operator can enter.** That is what makes the socket file
//! unreachable on Unix rather than its own mode, which matters: setting a mode after binding leaves a window in which
//! the file exists with whatever the process umask said, and closing that window needs a process-global setting no
//! library should reach for.
//!
//! **Not yet: the security descriptor that narrows the pipe to the operator's own account, and asking the kernel who
//! the peer is.** Both need the platform's security APIs, which have no safe wrapper, and this crate forbids `unsafe`.
//! That is a boundary and not an oversight: the one crate allowed to write `unsafe` is the one that owns child
//! processes, and these calls belong with it rather than here. Until then the endpoint is protected by the directory it
//! sits in and by the refusal of remote clients, and this paragraph is the record of what that leaves open.

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
            inner: platform::Listener::bind(address)?,
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
        let stream = self.inner.accept(&self.address).await?;
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

#[cfg(windows)]
mod platform {
    //! A named pipe. Remote clients are refused explicitly rather than by default.

    use tokio::net::windows::named_pipe::{
        ClientOptions, NamedPipeClient, NamedPipeServer, ServerOptions,
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
    }

    impl Listener {
        pub(super) fn bind(address: &str) -> Result<Self, TransportError> {
            let first = instance(address, true)?;
            Ok(Self { next: Some(first) })
        }

        pub(super) async fn accept(&mut self, address: &str) -> Result<Stream, TransportError> {
            let waiting = match self.next.take() {
                Some(one) => one,
                // Only reachable if a previous accept failed after taking the instance. Creating one rather than
                // refusing keeps a listener that had one bad moment usable.
                None => instance(address, false)?,
            };
            waiting
                .connect()
                .await
                .map_err(|error| TransportError::Bind {
                    address: address.to_owned(),
                    detail: error.to_string(),
                })?;
            // Created before the current one is handed over, so a client connecting right now finds an instance.
            self.next = Some(instance(address, false)?);
            Ok(Stream::Server(waiting))
        }
    }

    /// One pipe instance.
    fn instance(address: &str, first: bool) -> Result<NamedPipeServer, TransportError> {
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

    /// The byte stream. One type for both ends here, unlike the other platform.
    pub(super) type Stream = UnixStream;

    /// The socket the daemon waits on.
    pub(super) struct Listener {
        /// The platform's listener.
        inner: UnixListener,
    }

    impl Listener {
        pub(super) fn bind(address: &str) -> Result<Self, TransportError> {
            // A socket file left by a daemon that is gone would refuse the bind. Removed first, and a failure to remove
            // is not reported here: the bind below is what decides, and it names the address either way.
            if std::path::Path::new(address).exists() {
                drop(std::fs::remove_file(address));
            }
            let inner = UnixListener::bind(address).map_err(|error| TransportError::Bind {
                address: address.to_owned(),
                detail: error.to_string(),
            })?;
            Ok(Self { inner })
        }

        pub(super) async fn accept(&mut self, address: &str) -> Result<Stream, TransportError> {
            let (stream, _peer) =
                self.inner
                    .accept()
                    .await
                    .map_err(|error| TransportError::Bind {
                        address: address.to_owned(),
                        detail: error.to_string(),
                    })?;
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
