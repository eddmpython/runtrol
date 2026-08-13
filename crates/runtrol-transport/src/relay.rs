//! Default-deny WSS relay client carrying only bounded Noise records.
//!
//! DNS discovery happens once per connection attempt. Every resolved address must be public, and the result is
//! converted into exact [`crate::EgressPolicy`] capabilities before any socket is opened. TLS then authenticates the
//! configured DNS name on that already-approved socket. Redirects, proxies, cookies, query credentials, plaintext
//! `WebSockets`, and response bodies above the small protocol limits do not exist in this client.

use std::collections::BTreeSet;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use base64ct::{Base64, Base64UrlUnpadded, Encoding as _};
use bytes::Bytes;
use fastwebsockets::{Frame, OpCode, Role, WebSocket};
use http_body_util::{BodyExt as _, Full};
use hyper::body::Incoming;
use hyper::header::{
    AUTHORIZATION, CONNECTION, CONTENT_TYPE, HOST, SEC_WEBSOCKET_ACCEPT, SEC_WEBSOCKET_KEY,
    SEC_WEBSOCKET_PROTOCOL, SEC_WEBSOCKET_VERSION, UPGRADE,
};
use hyper::upgrade::Upgraded;
use hyper::{HeaderMap, Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use serde::Deserialize;
use sha1::{Digest as _, Sha1};
use tokio::net::TcpStream;
use tokio::task::JoinHandle;
use tokio::time::timeout;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;
use tokio_rustls::rustls::pki_types::ServerName;
use tokio_rustls::rustls::{ClientConfig, RootCertStore};
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

use crate::{ApprovedDestination, EgressPolicy, EncryptedRecord, MAX_ENCRYPTED_RECORD_WIRE};

const TOKEN_BYTES: usize = 32;
const TOKEN_TEXT_BYTES: usize = 43;
const PEER_ID_BYTES: usize = 32;
const RELAY_PORT: u16 = 443;
const RELAY_PROTOCOL: &str = "runtrol.relay.v1";
const TICKET_PROTOCOL_PREFIX: &str = "runtrol.ticket.";
const HTTP_BODY_LIMIT: usize = 512;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const WEBSOCKET_GUID: &[u8] = b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

type RelayWebSocket = WebSocket<TokioIo<Upgraded>>;

/// One canonical HTTPS origin for a relay deployment.
///
/// The production relay intentionally accepts a lowercase DNS name on port 443 only. A fixed origin keeps browser
/// storage and the Noise prologue stable, while excluding userinfo, paths, IP literals, local names, and alternate
/// ports from the egress surface.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RelayOrigin {
    encoded: Box<str>,
    host: Box<str>,
}

impl RelayOrigin {
    /// Parse a canonical relay origin such as `https://relay.example.com`.
    ///
    /// # Errors
    ///
    /// [`RelayError::InvalidOrigin`] when the value is not an exact lowercase HTTPS DNS origin on port 443.
    pub fn parse(origin: &str) -> Result<Self, RelayError> {
        let Some(host) = origin.strip_prefix("https://") else {
            return Err(RelayError::InvalidOrigin);
        };
        if !valid_dns_name(host) {
            return Err(RelayError::InvalidOrigin);
        }
        Ok(Self {
            encoded: origin.into(),
            host: host.into(),
        })
    }

    /// Borrow the canonical origin bound into every Noise session.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.encoded
    }
}

/// A public, non-secret 256-bit relay route identifier.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RelayRoute(Box<str>);

impl RelayRoute {
    /// Encode a fresh route identifier.
    #[must_use]
    pub fn from_bytes(bytes: [u8; TOKEN_BYTES]) -> Self {
        Self(Base64UrlUnpadded::encode_string(&bytes).into())
    }

    /// Parse a canonical unpadded base64url route identifier.
    ///
    /// # Errors
    ///
    /// [`RelayError::InvalidRoute`] when the value is not exactly 256 canonical bits.
    pub fn parse(route: &str) -> Result<Self, RelayError> {
        if canonical_token(route) {
            Ok(Self(route.into()))
        } else {
            Err(RelayError::InvalidRoute)
        }
    }

    /// Borrow the identifier for route construction and pairing metadata.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A 256-bit route credential held only by the PC and paired phones.
///
/// It deliberately implements neither `Debug`, `Display`, nor `Clone`.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct RelayCredential(String);

impl RelayCredential {
    /// Encode credential bytes without exposing a formatting implementation.
    #[must_use]
    pub fn from_bytes(bytes: [u8; TOKEN_BYTES]) -> Self {
        Self(Base64UrlUnpadded::encode_string(&bytes))
    }

    /// Parse a canonical unpadded base64url credential.
    ///
    /// # Errors
    ///
    /// [`RelayError::InvalidCredential`] when the value is not exactly 256 canonical bits.
    pub fn parse(credential: &str) -> Result<Self, RelayError> {
        if canonical_token(credential) {
            Ok(Self(credential.to_owned()))
        } else {
            Err(RelayError::InvalidCredential)
        }
    }

    fn authorization(&self) -> Result<hyper::header::HeaderValue, RelayError> {
        let mut value = Zeroizing::new(String::with_capacity(7 + self.0.len()));
        value.push_str("Bearer ");
        value.push_str(&self.0);
        hyper::header::HeaderValue::from_str(&value).map_err(|_| RelayError::InvalidCredential)
    }
}

/// A configured relay before DNS discovery mints exact socket capabilities.
pub struct RelayEndpoint {
    origin: RelayOrigin,
    route: RelayRoute,
    credential: RelayCredential,
}

impl RelayEndpoint {
    /// Bind one origin, route, and credential into a connection configuration.
    #[must_use]
    pub const fn new(origin: RelayOrigin, route: RelayRoute, credential: RelayCredential) -> Self {
        Self {
            origin,
            route,
            credential,
        }
    }

    /// Resolve the configured DNS name once and approve only its exact public addresses.
    ///
    /// # Errors
    ///
    /// [`RelayError::Resolve`] when DNS fails or returns no addresses, [`RelayError::PrivateAddress`] when any answer
    /// is not globally routable, or [`RelayError::Egress`] when an exact capability cannot be minted.
    pub async fn resolve(self) -> Result<ResolvedRelay, RelayError> {
        let resolved = timeout(
            CONNECT_TIMEOUT,
            tokio::net::lookup_host((&*self.origin.host, RELAY_PORT)),
        )
        .await
        .map_err(|_| RelayError::Timeout)?
        .map_err(|_| RelayError::Resolve)?;
        let addresses: BTreeSet<SocketAddr> = resolved.collect();
        if addresses.is_empty() {
            return Err(RelayError::Resolve);
        }
        if addresses
            .iter()
            .any(|address| !public_relay_address(address.ip()))
        {
            return Err(RelayError::PrivateAddress);
        }
        let policy = EgressPolicy::new(addresses.iter().copied());
        let destinations = addresses
            .into_iter()
            .map(|address| policy.approve(address).map_err(RelayError::from))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(ResolvedRelay {
            endpoint: self,
            policy,
            destinations,
        })
    }
}

/// A relay whose DNS answers have been converted into exact egress capabilities.
pub struct ResolvedRelay {
    endpoint: RelayEndpoint,
    policy: EgressPolicy,
    destinations: Vec<ApprovedDestination>,
}

impl ResolvedRelay {
    /// Borrow the canonical origin used by the WebSocket and Noise prologue.
    #[must_use]
    pub fn origin(&self) -> &RelayOrigin {
        &self.endpoint.origin
    }

    /// Register this route idempotently. The service stores only the credential digest.
    ///
    /// # Errors
    ///
    /// [`RelayError`] when exact egress, TLS, HTTP, or the relay contract fails.
    pub async fn register(&self) -> Result<(), RelayError> {
        let path = format!("/v1/routes/{}", self.endpoint.route.as_str());
        let response = self.request(Method::PUT, &path, Bytes::new(), None).await?;
        if response.status == StatusCode::NO_CONTENT {
            Ok(())
        } else {
            Err(RelayError::Refused {
                operation: "registering a relay route",
                status: response.status,
            })
        }
    }

    /// Mint one short-lived PC ticket and establish the multiplexed WSS connection.
    ///
    /// # Errors
    ///
    /// [`RelayError`] when ticket exchange, exact egress, TLS, WebSocket upgrade, or subprotocol selection fails.
    pub async fn connect_pc(&self) -> Result<RelaySocket, RelayError> {
        let path = format!("/v1/routes/{}/tickets", self.endpoint.route.as_str());
        let response = self
            .request(
                Method::POST,
                &path,
                Bytes::from_static(br#"{"role":"pc"}"#),
                Some("application/json"),
            )
            .await?;
        if response.status != StatusCode::CREATED
            || !header_is(&response.headers, CONTENT_TYPE, "application/json")
        {
            return Err(RelayError::Refused {
                operation: "minting a relay ticket",
                status: response.status,
            });
        }
        let parsed: TicketResponse =
            serde_json::from_slice(&response.body).map_err(|_| RelayError::InvalidResponse)?;
        let ticket = RelayTicket::parse(parsed.ticket)?;
        self.websocket(ticket).await
    }

    async fn request(
        &self,
        method: Method,
        path: &str,
        body: Bytes,
        content_type: Option<&'static str>,
    ) -> Result<HttpAnswer, RelayError> {
        let tls = self.connect_tls().await?;
        let (mut sender, connection) = timeout(
            REQUEST_TIMEOUT,
            hyper::client::conn::http1::handshake(TokioIo::new(tls)),
        )
        .await
        .map_err(|_| RelayError::Timeout)?
        .map_err(|_| RelayError::Http)?;
        let driver = ConnectionDriver::new(tokio::spawn(connection));
        let mut builder = Request::builder()
            .method(method)
            .uri(path)
            .header(HOST, &*self.endpoint.origin.host)
            .header(AUTHORIZATION, self.endpoint.credential.authorization()?);
        if let Some(content_type) = content_type {
            builder = builder.header(CONTENT_TYPE, content_type);
        }
        let request = builder
            .body(Full::new(body))
            .map_err(|_| RelayError::Http)?;
        let response = timeout(REQUEST_TIMEOUT, sender.send_request(request))
            .await
            .map_err(|_| RelayError::Timeout)?
            .map_err(|_| RelayError::Http)?;
        drop(sender);
        let answer = read_answer(response).await?;
        drop(driver);
        Ok(answer)
    }

    async fn websocket(&self, ticket: RelayTicket) -> Result<RelaySocket, RelayError> {
        let tls = self.connect_tls().await?;
        let (mut sender, connection) = timeout(
            REQUEST_TIMEOUT,
            hyper::client::conn::http1::handshake(TokioIo::new(tls)),
        )
        .await
        .map_err(|_| RelayError::Timeout)?
        .map_err(|_| RelayError::Http)?;
        let driver = ConnectionDriver::new(tokio::spawn(connection.with_upgrades()));
        let key = fastwebsockets::handshake::generate_key();
        let protocol = ticket.protocol()?;
        let path = format!("/v1/routes/{}/connect", self.endpoint.route.as_str());
        let request = Request::builder()
            .method(Method::GET)
            .uri(path)
            .header(HOST, &*self.endpoint.origin.host)
            .header(UPGRADE, "websocket")
            .header(CONNECTION, "upgrade")
            .header(SEC_WEBSOCKET_KEY, &key)
            .header(SEC_WEBSOCKET_VERSION, "13")
            .header(SEC_WEBSOCKET_PROTOCOL, protocol)
            .body(Full::new(Bytes::new()))
            .map_err(|_| RelayError::WebSocket)?;
        let mut response = timeout(REQUEST_TIMEOUT, sender.send_request(request))
            .await
            .map_err(|_| RelayError::Timeout)?
            .map_err(|_| RelayError::WebSocket)?;
        verify_upgrade(&response, &key)?;
        let upgraded = timeout(REQUEST_TIMEOUT, hyper::upgrade::on(&mut response))
            .await
            .map_err(|_| RelayError::Timeout)?
            .map_err(|_| RelayError::WebSocket)?;
        drop(sender);
        drop(driver);
        let mut socket = WebSocket::after_handshake(TokioIo::new(upgraded), Role::Client);
        socket.set_max_message_size(PEER_ID_BYTES + MAX_ENCRYPTED_RECORD_WIRE);
        Ok(RelaySocket { socket })
    }

    async fn connect_tls(&self) -> Result<TlsStream<TcpStream>, RelayError> {
        let connector = TlsConnector::from(tls_config());
        for destination in &self.destinations {
            let connected = timeout(CONNECT_TIMEOUT, self.policy.connect(*destination)).await;
            let Ok(Ok(stream)) = connected else {
                continue;
            };
            let name = ServerName::try_from(self.endpoint.origin.host.to_string())
                .map_err(|_| RelayError::InvalidOrigin)?;
            let secured = timeout(CONNECT_TIMEOUT, connector.connect(name, stream)).await;
            if let Ok(Ok(stream)) = secured {
                return Ok(stream);
            }
        }
        Err(RelayError::Connect)
    }
}

/// One incoming relay envelope. An absent record means that the named peer disconnected.
pub struct RelayEnvelope {
    peer_id: [u8; PEER_ID_BYTES],
    record: Option<EncryptedRecord>,
}

impl RelayEnvelope {
    /// The random peer identifier that must be included in the Noise session binding.
    #[must_use]
    pub const fn peer_id(&self) -> [u8; PEER_ID_BYTES] {
        self.peer_id
    }

    /// Consume the envelope and take its Noise record, or `None` for a disconnect signal.
    #[must_use]
    pub fn into_record(self) -> Option<EncryptedRecord> {
        self.record
    }
}

/// One multiplexed PC WebSocket to an untrusted relay.
pub struct RelaySocket {
    socket: RelayWebSocket,
}

impl RelaySocket {
    /// Receive one peer-qualified Noise record or disconnect signal.
    ///
    /// # Errors
    ///
    /// [`RelayError::WebSocket`] for framing failure and [`RelayError::InvalidEnvelope`] for text, fragmentation,
    /// invalid peer ids, trailing bytes, or records outside the Noise bound.
    pub async fn recv(&mut self) -> Result<Option<RelayEnvelope>, RelayError> {
        loop {
            let frame = self
                .socket
                .read_frame()
                .await
                .map_err(|_| RelayError::WebSocket)?;
            match frame.opcode {
                OpCode::Binary if frame.fin => return decode_envelope(&frame.payload).map(Some),
                OpCode::Close => return Ok(None),
                OpCode::Ping | OpCode::Pong => {}
                OpCode::Binary | OpCode::Continuation | OpCode::Text => {
                    return Err(RelayError::InvalidEnvelope);
                }
            }
        }
    }

    /// Send one canonical Noise record to one exact relay peer.
    ///
    /// # Errors
    ///
    /// [`RelayError::InvalidEnvelope`] if record encoding fails, or [`RelayError::WebSocket`] when the relay link
    /// cannot write the binary message.
    pub async fn send(
        &mut self,
        peer_id: [u8; PEER_ID_BYTES],
        record: &EncryptedRecord,
    ) -> Result<(), RelayError> {
        let mut encoded = Vec::with_capacity(PEER_ID_BYTES + MAX_ENCRYPTED_RECORD_WIRE);
        encoded.extend_from_slice(&peer_id);
        record
            .append_wire(&mut encoded)
            .map_err(|_| RelayError::InvalidEnvelope)?;
        self.socket
            .write_frame(Frame::binary(encoded.into()))
            .await
            .map_err(|_| RelayError::WebSocket)
    }
}

struct RelayTicket(String);

impl RelayTicket {
    fn parse(mut ticket: String) -> Result<Self, RelayError> {
        if canonical_token(&ticket) {
            Ok(Self(ticket))
        } else {
            ticket.zeroize();
            Err(RelayError::InvalidResponse)
        }
    }

    fn protocol(&self) -> Result<hyper::header::HeaderValue, RelayError> {
        let mut protocol = Zeroizing::new(String::with_capacity(
            TICKET_PROTOCOL_PREFIX.len() + self.0.len() + RELAY_PROTOCOL.len() + 2,
        ));
        protocol.push_str(RELAY_PROTOCOL);
        protocol.push_str(", ");
        protocol.push_str(TICKET_PROTOCOL_PREFIX);
        protocol.push_str(&self.0);
        hyper::header::HeaderValue::from_str(&protocol).map_err(|_| RelayError::InvalidResponse)
    }
}

impl Drop for RelayTicket {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

#[derive(Deserialize)]
struct TicketResponse {
    ticket: String,
    #[serde(rename = "expiresAt")]
    _expires_at: u64,
}

struct HttpAnswer {
    status: StatusCode,
    headers: HeaderMap,
    body: Zeroizing<Vec<u8>>,
}

struct ConnectionDriver {
    task: JoinHandle<Result<(), hyper::Error>>,
}

impl ConnectionDriver {
    const fn new(task: JoinHandle<Result<(), hyper::Error>>) -> Self {
        Self { task }
    }
}

impl Drop for ConnectionDriver {
    fn drop(&mut self) {
        // Every request uses one connection. Once its response or upgrade is complete, no future work needs the HTTP
        // driver. Aborting here also closes it on every early error path instead of detaching a background task.
        self.task.abort();
    }
}

async fn read_answer(response: Response<Incoming>) -> Result<HttpAnswer, RelayError> {
    let (parts, mut body) = response.into_parts();
    let mut bytes = Zeroizing::new(Vec::new());
    loop {
        let frame = timeout(REQUEST_TIMEOUT, body.frame())
            .await
            .map_err(|_| RelayError::Timeout)?;
        let Some(frame) = frame else {
            break;
        };
        let frame = frame.map_err(|_| RelayError::Http)?;
        if let Some(data) = frame.data_ref() {
            let next = bytes
                .len()
                .checked_add(data.len())
                .ok_or(RelayError::InvalidResponse)?;
            if next > HTTP_BODY_LIMIT {
                return Err(RelayError::InvalidResponse);
            }
            bytes.extend_from_slice(data);
        }
    }
    Ok(HttpAnswer {
        status: parts.status,
        headers: parts.headers,
        body: bytes,
    })
}

fn verify_upgrade(response: &Response<Incoming>, key: &str) -> Result<(), RelayError> {
    if response.status() != StatusCode::SWITCHING_PROTOCOLS
        || !header_is(response.headers(), UPGRADE, "websocket")
        || !header_contains(response.headers(), CONNECTION, "upgrade")
        || !header_is(response.headers(), SEC_WEBSOCKET_PROTOCOL, RELAY_PROTOCOL)
    {
        return Err(RelayError::WebSocket);
    }
    let mut accept = Sha1::new();
    accept.update(key.as_bytes());
    accept.update(WEBSOCKET_GUID);
    let expected = Base64::encode_string(&accept.finalize());
    if !header_is(response.headers(), SEC_WEBSOCKET_ACCEPT, expected.as_str()) {
        return Err(RelayError::WebSocket);
    }
    Ok(())
}

fn header_is(headers: &HeaderMap, name: hyper::header::HeaderName, expected: &str) -> bool {
    let Some(value) = headers.get(name) else {
        return false;
    };
    let Ok(value) = value.to_str() else {
        return false;
    };
    value.eq_ignore_ascii_case(expected)
}

fn header_contains(headers: &HeaderMap, name: hyper::header::HeaderName, expected: &str) -> bool {
    for value in headers.get_all(name) {
        let Ok(value) = value.to_str() else {
            return false;
        };
        if value
            .split(',')
            .any(|value| value.trim().eq_ignore_ascii_case(expected))
        {
            return true;
        }
    }
    false
}

fn decode_envelope(message: &[u8]) -> Result<RelayEnvelope, RelayError> {
    let peer = message
        .get(..PEER_ID_BYTES)
        .ok_or(RelayError::InvalidEnvelope)?;
    let mut peer_id = [0_u8; PEER_ID_BYTES];
    peer_id.copy_from_slice(peer);
    let record = if message.len() == PEER_ID_BYTES {
        None
    } else {
        let encoded = message
            .get(PEER_ID_BYTES..)
            .ok_or(RelayError::InvalidEnvelope)?;
        let (record, consumed) =
            EncryptedRecord::decode_wire(encoded).map_err(|_| RelayError::InvalidEnvelope)?;
        if consumed != encoded.len() {
            return Err(RelayError::InvalidEnvelope);
        }
        Some(record)
    };
    Ok(RelayEnvelope { peer_id, record })
}

fn canonical_token(value: &str) -> bool {
    if value.len() != TOKEN_TEXT_BYTES {
        return false;
    }
    let mut decoded = [0_u8; TOKEN_BYTES];
    let Ok(bytes) = Base64UrlUnpadded::decode(value, &mut decoded) else {
        return false;
    };
    bytes.len() == TOKEN_BYTES && Base64UrlUnpadded::encode_string(&decoded) == value
}

fn valid_dns_name(host: &str) -> bool {
    host.len() <= 253
        && host.contains('.')
        && host.parse::<IpAddr>().is_err()
        && host.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'.' || byte == b'-'
        })
        && host.split('.').all(|label| {
            let bytes = label.as_bytes();
            !label.is_empty()
                && label.len() <= 63
                && bytes.first().is_some_and(u8::is_ascii_alphanumeric)
                && bytes.last().is_some_and(u8::is_ascii_alphanumeric)
        })
}

fn public_relay_address(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => {
            !address.is_private()
                && !address.is_loopback()
                && !address.is_link_local()
                && !address.is_broadcast()
                && !address.is_documentation()
                && !address.is_unspecified()
        }
        IpAddr::V6(address) => {
            !address.is_loopback()
                && !address.is_unspecified()
                && !address.is_unique_local()
                && !address.is_unicast_link_local()
        }
    }
}

fn tls_config() -> Arc<ClientConfig> {
    static CONFIG: OnceLock<Arc<ClientConfig>> = OnceLock::new();
    Arc::clone(CONFIG.get_or_init(|| {
        let roots = webpki_roots::TLS_SERVER_ROOTS
            .iter()
            .cloned()
            .collect::<RootCertStore>();
        Arc::new(
            ClientConfig::builder()
                .with_root_certificates(roots)
                .with_no_client_auth(),
        )
    }))
}

/// Relay discovery, authentication, TLS, HTTP, WebSocket, or envelope failure.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum RelayError {
    /// The relay origin was not the exact supported HTTPS DNS shape.
    #[error("relay origin must be a canonical lowercase HTTPS DNS origin on port 443")]
    InvalidOrigin,

    /// The route id was not 256 canonical base64url bits.
    #[error("relay route identifier is invalid")]
    InvalidRoute,

    /// The route credential was not 256 canonical base64url bits.
    #[error("relay route credential is invalid")]
    InvalidCredential,

    /// DNS resolution failed or returned no destination.
    #[error("relay DNS discovery failed")]
    Resolve,

    /// At least one DNS answer entered a local or reserved address range.
    #[error("relay DNS discovery returned a non-public address")]
    PrivateAddress,

    /// Exact socket admission refused a destination.
    #[error(transparent)]
    Egress(#[from] crate::EgressError),

    /// No approved destination completed TCP and `WebPKI` authentication.
    #[error("no approved relay destination completed TLS")]
    Connect,

    /// A bounded network operation exceeded its deadline.
    #[error("relay network operation timed out")]
    Timeout,

    /// HTTP framing failed without retaining a response body.
    #[error("relay HTTP exchange failed")]
    Http,

    /// The relay refused a named control operation.
    #[error("relay refused while {operation}: HTTP {status}")]
    Refused {
        /// The fixed operation name.
        operation: &'static str,
        /// The non-secret status code.
        status: StatusCode,
    },

    /// A response exceeded its bound or did not match the exact protocol schema.
    #[error("relay returned an invalid bounded response")]
    InvalidResponse,

    /// WebSocket upgrade, authentication, or framing failed.
    #[error("relay WebSocket exchange failed")]
    WebSocket,

    /// A relay message was not exactly one peer id and optional canonical Noise record.
    #[error("relay returned an invalid ciphertext envelope")]
    InvalidEnvelope,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origins_are_canonical_public_dns_names_without_paths_or_ports() {
        let origin = RelayOrigin::parse("https://relay.example.com").expect("valid origin");
        assert_eq!(origin.as_str(), "https://relay.example.com");
        for refused in [
            "http://relay.example.com",
            "https://Relay.example.com",
            "https://relay.example.com:443",
            "https://relay.example.com/path",
            "https://127.0.0.1",
            "https://localhost",
            "https://-relay.example.com",
        ] {
            assert!(RelayOrigin::parse(refused).is_err(), "accepted {refused}");
        }
    }

    #[test]
    fn route_and_credential_tokens_are_canonical_and_secret_safe() {
        let route = RelayRoute::from_bytes([7; TOKEN_BYTES]);
        assert_eq!(
            RelayRoute::parse(route.as_str()).expect("parsed route"),
            route
        );
        assert!(RelayRoute::parse("short").is_err());
        assert!(RelayCredential::parse("short").is_err());
        let credential = RelayCredential::from_bytes([9; TOKEN_BYTES]);
        let endpoint = RelayEndpoint::new(
            RelayOrigin::parse("https://relay.example.com").expect("valid origin"),
            RelayRoute::from_bytes([8; TOKEN_BYTES]),
            credential,
        );
        assert!(!core::any::type_name_of_val(&endpoint).contains("CQkJ"));
    }

    #[test]
    fn envelopes_are_exactly_peer_id_and_optional_noise_record() {
        let peer = [3_u8; PEER_ID_BYTES];
        let closed = decode_envelope(&peer).expect("disconnect envelope");
        assert_eq!(closed.peer_id(), peer);
        assert!(closed.into_record().is_none());

        let record = EncryptedRecord::from_ciphertext(vec![4_u8; 16]).expect("minimum record");
        let mut message = peer.to_vec();
        record.append_wire(&mut message).expect("record encoding");
        let decoded = decode_envelope(&message).expect("record envelope");
        assert_eq!(decoded.peer_id(), peer);
        assert_eq!(
            decoded
                .into_record()
                .expect("record present")
                .as_ciphertext(),
            record.as_ciphertext()
        );

        assert!(decode_envelope(&[]).is_err());
        message.push(0);
        assert!(decode_envelope(&message).is_err());
    }

    #[test]
    fn local_and_reserved_dns_answers_are_refused() {
        for address in [
            "127.0.0.1".parse().expect("IPv4 loopback"),
            "10.0.0.1".parse().expect("IPv4 private"),
            "192.0.2.1".parse().expect("IPv4 documentation"),
            "::1".parse().expect("IPv6 loopback"),
            "fc00::1".parse().expect("IPv6 private"),
            "fe80::1".parse().expect("IPv6 link local"),
        ] {
            assert!(!public_relay_address(address));
        }
        assert!(public_relay_address(
            "1.1.1.1".parse().expect("public IPv4")
        ));
        assert!(public_relay_address(
            "2606:4700:4700::1111".parse().expect("public IPv6")
        ));
    }
}
