//! Bounded public framing over a local named pipe or Unix socket.

use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

use crate::ClientError;
use runtrol_runtime_protocol::MAX_FRAME_BYTES;

/// One local public Runtime byte stream.
pub(crate) struct Connection {
    stream: platform::Stream,
}

impl Connection {
    /// Connect to the validated locator endpoint.
    pub(crate) async fn connect(endpoint: &str) -> Result<Self, ClientError> {
        Ok(Self {
            stream: platform::connect(endpoint).await?,
        })
    }

    /// Send one bounded JSON payload.
    pub(crate) async fn send(&mut self, payload: &[u8]) -> Result<(), ClientError> {
        if payload.len() > MAX_FRAME_BYTES {
            return Err(ClientError::Protocol(format!(
                "a frame of {} bytes is past the limit of {MAX_FRAME_BYTES}",
                payload.len()
            )));
        }
        let length = u32::try_from(payload.len()).map_err(|_| {
            ClientError::Protocol("the frame length does not fit its header".to_owned())
        })?;
        self.stream
            .write_all(&length.to_be_bytes())
            .await
            .map_err(|error| transport("sending a frame header", &error))?;
        self.stream
            .write_all(payload)
            .await
            .map_err(|error| transport("sending a frame payload", &error))?;
        self.stream
            .flush()
            .await
            .map_err(|error| transport("flushing a frame", &error))
    }

    /// Receive one bounded JSON payload. The untrusted length is checked before allocation.
    pub(crate) async fn receive(&mut self) -> Result<Vec<u8>, ClientError> {
        let mut header = [0_u8; 4];
        self.stream
            .read_exact(&mut header)
            .await
            .map_err(|error| transport("receiving a frame header", &error))?;
        let length = usize::try_from(u32::from_be_bytes(header)).map_err(|_| {
            ClientError::Protocol("the frame length cannot be represented".to_owned())
        })?;
        if length > MAX_FRAME_BYTES {
            return Err(ClientError::Protocol(format!(
                "the Runtime announced {length} frame bytes and the limit is {MAX_FRAME_BYTES}"
            )));
        }
        let mut payload = vec![0_u8; length];
        self.stream
            .read_exact(&mut payload)
            .await
            .map_err(|error| transport("receiving a frame payload", &error))?;
        Ok(payload)
    }
}

fn transport(doing: &'static str, error: &std::io::Error) -> ClientError {
    ClientError::Transport {
        doing,
        detail: error.to_string(),
    }
}

#[cfg(windows)]
mod platform {
    use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeClient};

    use crate::ClientError;

    pub(super) type Stream = NamedPipeClient;

    const ALL_INSTANCES_TAKEN: i32 = 231;
    const WHILE_BUSY: core::time::Duration = core::time::Duration::from_secs(2);
    const BETWEEN_TRIES: core::time::Duration = core::time::Duration::from_millis(20);

    pub(super) async fn connect(endpoint: &str) -> Result<Stream, ClientError> {
        let give_up_at = tokio::time::Instant::now() + WHILE_BUSY;
        loop {
            match ClientOptions::new().open(endpoint) {
                Ok(client) => return Ok(client),
                Err(error) if error.raw_os_error() == Some(ALL_INSTANCES_TAKEN) => {
                    if tokio::time::Instant::now() >= give_up_at {
                        return Err(ClientError::Transport {
                            doing: "connecting to the Runtime pipe",
                            detail: "every local pipe instance stayed occupied for two seconds"
                                .to_owned(),
                        });
                    }
                    tokio::time::sleep(BETWEEN_TRIES).await;
                }
                Err(error) => {
                    return Err(ClientError::Transport {
                        doing: "connecting to the Runtime pipe",
                        detail: error.to_string(),
                    });
                }
            }
        }
    }
}

#[cfg(unix)]
mod platform {
    use tokio::net::UnixStream;

    use crate::ClientError;

    pub(super) type Stream = UnixStream;

    pub(super) async fn connect(endpoint: &str) -> Result<Stream, ClientError> {
        UnixStream::connect(endpoint)
            .await
            .map_err(|error| ClientError::Transport {
                doing: "connecting to the Runtime socket",
                detail: error.to_string(),
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[tokio::test]
    async fn hostile_length_is_rejected_before_payload_allocation() {
        use tokio::net::windows::named_pipe::{ClientOptions, ServerOptions};

        let endpoint = format!(
            r"\\.\pipe\runtrol-runtime-client-frame-{}",
            std::process::id()
        );
        let mut server = ServerOptions::new()
            .first_pipe_instance(true)
            .reject_remote_clients(true)
            .create(&endpoint)
            .expect("create test pipe");
        let connecting = tokio::spawn({
            let endpoint = endpoint.clone();
            async move {
                ClientOptions::new()
                    .open(endpoint)
                    .expect("connect test pipe")
            }
        });
        server.connect().await.expect("accept test client");
        let client = connecting.await.expect("join test client");
        server
            .write_all(&u32::MAX.to_be_bytes())
            .await
            .expect("write hostile header");
        let mut connection = Connection { stream: client };
        assert!(matches!(
            connection.receive().await,
            Err(ClientError::Protocol(_))
        ));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn hostile_length_is_rejected_before_payload_allocation() {
        let (mut hostile, stream) = tokio::net::UnixStream::pair().expect("test stream pair");
        hostile
            .write_all(&u32::MAX.to_be_bytes())
            .await
            .expect("write hostile header");
        let mut connection = Connection { stream };
        assert!(matches!(
            connection.receive().await,
            Err(ClientError::Protocol(_))
        ));
    }
}
