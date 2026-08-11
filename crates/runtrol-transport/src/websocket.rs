//! Bounded WebSocket messages carrying canonical Noise records.
//!
//! HTTP Host and Origin admission happens before this module receives an upgrade. The WebSocket itself authenticates
//! no device. Noise IK message one reveals an authenticated static key, and the resulting pending link cannot send
//! message two or expose an application channel until the daemon pins that key to one stored paired device.

use bytes::Bytes;
use fastwebsockets::upgrade::UpgradeFut;
use fastwebsockets::{Frame, OpCode, WebSocket};
use hyper::upgrade::Upgraded;
use hyper_util::rt::TokioIo;

use crate::crypto::{
    Channel, CryptoError, EncryptedRecord, MAX_ENCRYPTED_RECORD_WIRE, PendingSession, PublicKey,
    SessionResponder,
};

type UpgradedSocket = WebSocket<TokioIo<Upgraded>>;

/// An admitted HTTP upgrade waiting for RFC 6455 and Noise message one.
pub struct NoiseUpgrade {
    upgraded: UpgradeFut,
    responder: SessionResponder,
}

impl NoiseUpgrade {
    pub(crate) const fn new(upgraded: UpgradeFut, responder: SessionResponder) -> Self {
        Self {
            upgraded,
            responder,
        }
    }

    /// Finish HTTP upgrade and authenticate enough of Noise IK to recover the initiator static key.
    ///
    /// No handshake reply is sent yet. The returned value must first be matched to one durable paired device.
    ///
    /// # Errors
    ///
    /// [`WebSocketLinkError::WebSocket`] when RFC 6455 fails, [`WebSocketLinkError::BinaryRequired`] for a text
    /// message, or [`WebSocketLinkError::Crypto`] when Noise message one is malformed.
    pub async fn receive(self) -> Result<PendingNoiseWebSocket, WebSocketLinkError> {
        let mut socket = self.upgraded.await.map_err(WebSocketLinkError::from)?;
        socket.set_max_message_size(MAX_ENCRYPTED_RECORD_WIRE);
        let Some(message) = read_binary_message(&mut socket).await? else {
            return Err(WebSocketLinkError::ClosedDuringHandshake);
        };
        let first = exact_record(&message)?;
        let session = self.responder.receive(&first)?;
        Ok(PendingNoiseWebSocket { socket, session })
    }
}

/// A Noise-authenticated WebSocket whose static key has not been authorized as a paired device yet.
pub struct PendingNoiseWebSocket {
    socket: UpgradedSocket,
    session: PendingSession,
}

impl PendingNoiseWebSocket {
    /// The authenticated initiator key used to select one restored paired device.
    #[must_use]
    pub const fn remote_public_key(&self) -> PublicKey {
        self.session.remote_public_key()
    }

    /// Pin the Noise identity, require an empty handshake payload, send message two, and expose the channel.
    ///
    /// # Errors
    ///
    /// [`WebSocketLinkError::Crypto`] when the identity differs or message two fails,
    /// [`WebSocketLinkError::HandshakePayload`] when an application frame was smuggled before authorization, or
    /// [`WebSocketLinkError::WebSocket`] when the reply cannot be sent.
    pub async fn approve(
        mut self,
        expected: PublicKey,
    ) -> Result<NoiseWebSocket, WebSocketLinkError> {
        let (channel, reply, payload) = self.session.approve(expected, &[])?;
        if !payload.is_empty() {
            return Err(WebSocketLinkError::HandshakePayload);
        }
        write_record(&mut self.socket, &reply).await?;
        Ok(NoiseWebSocket {
            socket: self.socket,
            channel,
        })
    }
}

/// A mutually authenticated application stream over one browser-compatible WebSocket.
pub struct NoiseWebSocket {
    socket: UpgradedSocket,
    channel: Channel,
}

impl NoiseWebSocket {
    /// Receive and decrypt one complete bounded runtrol frame, or `None` after a close handshake.
    ///
    /// # Errors
    ///
    /// [`WebSocketLinkError::WebSocket`] when RFC 6455 fails, [`WebSocketLinkError::BinaryRequired`] for text, or
    /// [`WebSocketLinkError::Crypto`] for an invalid, reordered, or oversized Noise record.
    pub async fn recv(&mut self) -> Result<Option<Bytes>, WebSocketLinkError> {
        loop {
            let Some(message) = read_binary_message(&mut self.socket).await? else {
                return Ok(None);
            };
            let record = exact_record(&message)?;
            if let Some(frame) = self.channel.open_record(&record)? {
                return Ok(Some(Bytes::from(frame)));
            }
        }
    }

    /// Encrypt and send one complete bounded runtrol frame.
    ///
    /// # Errors
    ///
    /// [`WebSocketLinkError::Crypto`] above the transport frame bound or when Noise fails, and
    /// [`WebSocketLinkError::WebSocket`] when RFC 6455 cannot write a message.
    pub async fn send(&mut self, payload: &[u8]) -> Result<(), WebSocketLinkError> {
        for record in self.channel.seal_frame(payload)? {
            write_record(&mut self.socket, &record).await?;
        }
        Ok(())
    }

    /// Send one frame assembled from encoded slices.
    ///
    /// # Errors
    ///
    /// The same failures as [`Self::send`].
    pub async fn send_parts(&mut self, parts: &[&[u8]]) -> Result<(), WebSocketLinkError> {
        let length = parts.iter().try_fold(0_usize, |total, part| {
            total
                .checked_add(part.len())
                .ok_or(CryptoError::FrameTooLarge {
                    length: usize::MAX,
                    max: crate::MAX_TRANSPORT_FRAME,
                })
        })?;
        if length > crate::MAX_TRANSPORT_FRAME {
            return Err(CryptoError::FrameTooLarge {
                length,
                max: crate::MAX_TRANSPORT_FRAME,
            }
            .into());
        }
        let mut payload = Vec::with_capacity(length);
        for part in parts {
            payload.extend_from_slice(part);
        }
        self.send(&payload).await
    }
}

async fn read_binary_message(
    socket: &mut UpgradedSocket,
) -> Result<Option<Vec<u8>>, WebSocketLinkError> {
    let mut partial: Option<Vec<u8>> = None;
    loop {
        let frame = socket
            .read_frame()
            .await
            .map_err(WebSocketLinkError::from)?;
        match frame.opcode {
            OpCode::Binary if partial.is_none() && frame.fin => {
                return Ok(Some(frame.payload.to_vec()));
            }
            OpCode::Binary if partial.is_none() => {
                partial = Some(frame.payload.to_vec());
            }
            OpCode::Continuation => {
                let Some(buffer) = partial.as_mut() else {
                    return Err(WebSocketLinkError::FragmentOrder);
                };
                let next = buffer
                    .len()
                    .checked_add(frame.payload.len())
                    .ok_or(WebSocketLinkError::RecordEnvelope)?;
                if next > MAX_ENCRYPTED_RECORD_WIRE {
                    return Err(WebSocketLinkError::RecordEnvelope);
                }
                buffer.extend_from_slice(&frame.payload);
                if frame.fin {
                    return Ok(partial.take());
                }
            }
            OpCode::Binary => return Err(WebSocketLinkError::FragmentOrder),
            OpCode::Text => return Err(WebSocketLinkError::BinaryRequired),
            OpCode::Close => return Ok(None),
            OpCode::Ping | OpCode::Pong => {}
        }
    }
}

fn exact_record(message: &[u8]) -> Result<EncryptedRecord, WebSocketLinkError> {
    let (record, consumed) = EncryptedRecord::decode_wire(message)?;
    if consumed != message.len() {
        return Err(WebSocketLinkError::RecordEnvelope);
    }
    Ok(record)
}

async fn write_record(
    socket: &mut UpgradedSocket,
    record: &EncryptedRecord,
) -> Result<(), WebSocketLinkError> {
    let mut encoded = Vec::with_capacity(record.as_ciphertext().len() + 3);
    record.append_wire(&mut encoded)?;
    socket
        .write_frame(Frame::binary(encoded.into()))
        .await
        .map_err(WebSocketLinkError::from)
}

/// A fail-closed browser link error that never retains payload or cipher diagnostics.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum WebSocketLinkError {
    /// RFC 6455 upgrade, framing, masking, or I/O failed.
    #[error("WebSocket link failed")]
    WebSocket,

    /// Noise authentication, ordering, size, or state failed.
    #[error(transparent)]
    Crypto(#[from] CryptoError),

    /// The peer closed before completing Noise IK.
    #[error("WebSocket closed during Noise handshake")]
    ClosedDuringHandshake,

    /// Application and Noise messages must be opaque binary data.
    #[error("WebSocket link accepts binary messages only")]
    BinaryRequired,

    /// Fragment ordering was not one initial binary frame followed by continuations.
    #[error("WebSocket binary fragments arrived out of order")]
    FragmentOrder,

    /// One WebSocket message was not exactly one canonical bounded Noise record.
    #[error("WebSocket message is not one canonical Noise record")]
    RecordEnvelope,

    /// No application data may precede durable device authorization.
    #[error("Noise handshake carried application data before device authorization")]
    HandshakePayload,
}

impl From<fastwebsockets::WebSocketError> for WebSocketLinkError {
    fn from(_: fastwebsockets::WebSocketError) -> Self {
        Self::WebSocket
    }
}

#[cfg(test)]
mod tests {
    use core::future::Future;
    use std::net::SocketAddr;
    use std::sync::Arc;

    use fastwebsockets::handshake;
    use http_body_util::Empty;
    use hyper::Request;
    use tokio::net::{TcpListener, TcpStream};
    use tokio::sync::mpsc;
    use tokio::task::JoinHandle;

    use super::*;
    use crate::{
        InitiatorHandshake, LinkKind, NOISE_LINK_PATH, NOISE_LINK_PROTOCOL, PhoneHttp,
        SessionBinding, StaticKeypair, StatusCode, response as http_response,
    };

    const ORIGIN: &str = "https://phone.runtrol.test";

    struct TokioExecutor;

    impl<F> hyper::rt::Executor<F> for TokioExecutor
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        fn execute(&self, future: F) {
            tokio::spawn(future);
        }
    }

    struct TestServer {
        address: SocketAddr,
        completed: mpsc::Receiver<Result<Vec<u8>, WebSocketLinkError>>,
        task: JoinHandle<()>,
    }

    async fn start_server(
        pc: Arc<StaticKeypair>,
        phone_public: PublicKey,
        binding: Arc<SessionBinding>,
    ) -> TestServer {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("listener");
        let address = listener.local_addr().expect("listener address");
        let http = PhoneHttp::loopback(address.port(), [ORIGIN], []).expect("HTTP policy");
        let (accepted, completed) = mpsc::channel(1);
        let task = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accepted TCP");
            http.serve_noise_connection(stream, move |admitted| {
                let pc = Arc::clone(&pc);
                let binding = Arc::clone(&binding);
                let accepted = accepted.clone();
                async move {
                    let Ok((switching, upgrade)) = admitted.begin(&pc, &binding) else {
                        return http_response(StatusCode::BAD_REQUEST, "invalid upgrade");
                    };
                    tokio::spawn(async move {
                        let result = exchange_with_phone(upgrade, phone_public).await;
                        accepted.send(result).await.expect("test result receiver");
                    });
                    switching
                }
            })
            .await
            .expect("served HTTP connection");
        });
        TestServer {
            address,
            completed,
            task,
        }
    }

    async fn exchange_with_phone(
        upgrade: NoiseUpgrade,
        phone_public: PublicKey,
    ) -> Result<Vec<u8>, WebSocketLinkError> {
        let pending = upgrade.receive().await?;
        if pending.remote_public_key() != phone_public {
            return Err(WebSocketLinkError::HandshakePayload);
        }
        let mut link = pending.approve(phone_public).await?;
        let request = link
            .recv()
            .await?
            .ok_or(WebSocketLinkError::ClosedDuringHandshake)?;
        link.send_parts(&[b"same ", b"core"]).await?;
        Ok(request.to_vec())
    }

    async fn connect_browser(address: SocketAddr) -> UpgradedSocket {
        let stream = TcpStream::connect(address).await.expect("connected TCP");
        let request = Request::builder()
            .method("GET")
            .uri(format!("ws://{address}{NOISE_LINK_PATH}"))
            .header("Host", address.to_string())
            .header("Origin", ORIGIN)
            .header("Sec-Fetch-Site", "same-origin")
            .header("Upgrade", "websocket")
            .header("Connection", "Upgrade")
            .header("Sec-WebSocket-Key", handshake::generate_key())
            .header("Sec-WebSocket-Version", "13")
            .header("Sec-WebSocket-Protocol", NOISE_LINK_PROTOCOL)
            .body(Empty::<Bytes>::new())
            .expect("client request");
        let (socket, switched) = handshake::client(&TokioExecutor, request, stream)
            .await
            .expect("WebSocket handshake");
        assert_eq!(switched.status(), StatusCode::SWITCHING_PROTOCOLS);
        assert_eq!(
            switched
                .headers()
                .get("Sec-WebSocket-Protocol")
                .map(|value| value.to_str().expect("ASCII subprotocol")),
            Some(NOISE_LINK_PROTOCOL)
        );
        socket
    }

    async fn send_fragmented_record(socket: &mut UpgradedSocket, record: &EncryptedRecord) {
        let mut encoded = Vec::new();
        record
            .append_wire(&mut encoded)
            .expect("encoded Noise record");
        let split = encoded.len() / 2;
        let first_half = encoded.get(..split).expect("midpoint is in bounds");
        let second_half = encoded.get(split..).expect("midpoint is in bounds");
        socket
            .write_frame(Frame::new(
                false,
                OpCode::Binary,
                None,
                first_half.to_vec().into(),
            ))
            .await
            .expect("first fragment");
        socket
            .write_frame(Frame::new(
                true,
                OpCode::Continuation,
                None,
                second_half.to_vec().into(),
            ))
            .await
            .expect("final fragment");
    }

    async fn finish_handshake(
        socket: &mut UpgradedSocket,
        phone: &StaticKeypair,
        pc_public: PublicKey,
        binding: &SessionBinding,
    ) -> Channel {
        let mut initiator =
            InitiatorHandshake::session(phone, pc_public, binding).expect("Noise initiator");
        let first = initiator.write_first(&[]).expect("Noise message one");
        send_fragmented_record(socket, &first).await;
        let reply = read_binary_message(socket)
            .await
            .expect("reply message")
            .expect("reply before close");
        let reply = exact_record(&reply).expect("canonical reply");
        let (channel, payload) = initiator.finish(&reply).expect("Noise message two");
        assert!(payload.is_empty());
        channel
    }

    async fn receive_frame(socket: &mut UpgradedSocket, channel: &mut Channel) -> Vec<u8> {
        loop {
            let message = read_binary_message(socket)
                .await
                .expect("answer message")
                .expect("answer before close");
            let record = exact_record(&message).expect("canonical answer");
            if let Some(frame) = channel.open_record(&record).expect("opened answer") {
                return frame;
            }
        }
    }

    #[tokio::test]
    async fn a_browser_shaped_socket_carries_one_pinned_noise_channel() {
        let pc = Arc::new(StaticKeypair::generate().expect("pc key"));
        let phone = StaticKeypair::generate().expect("phone key");
        let phone_public = phone.public_key();
        let binding = Arc::new(
            SessionBinding::direct(LinkKind::Loopback, pc.public_key().to_bytes())
                .expect("loopback binding"),
        );
        let mut server = start_server(Arc::clone(&pc), phone_public, Arc::clone(&binding)).await;
        let mut socket = connect_browser(server.address).await;
        let mut phone_channel =
            finish_handshake(&mut socket, &phone, pc.public_key(), &binding).await;

        for record in phone_channel
            .seal_frame(b"from phone")
            .expect("phone frame")
        {
            write_record(&mut socket, &record)
                .await
                .expect("phone record");
        }
        let answer = receive_frame(&mut socket, &mut phone_channel).await;
        assert_eq!(answer, b"same core");
        assert_eq!(
            server
                .completed
                .recv()
                .await
                .expect("server result")
                .expect("server link"),
            b"from phone"
        );
        drop(socket);
        server.task.await.expect("server task");
    }
}
