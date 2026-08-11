//! HTTP admission for a browser-reachable listener.
//!
//! # One wrapper, before every route
//!
//! [`PhoneHttp::serve_connection`] owns the HTTP connection and accepts the route handler as a value. The handler is
//! called only after admission. That shape matters: a newly added 404, preflight, or upgrade route cannot be placed
//! outside a middleware stack by accident because there is no route stack here to order incorrectly.
//!
//! # What each browser signal does
//!
//! `Sec-Fetch-Site` stops ordinary CSRF, but it does not stop DNS rebinding: after a successful rebind the browser
//! itself reports `same-origin`. Exact Host validation is what stops rebinding. Exact Origin validation and a
//! mandatory non-simple header are independent chokepoints. No cookie reaches a handler and no response can set one.

use std::convert::Infallible;
use std::future::Future;
use std::sync::Arc;

use bytes::Bytes;
use http_body_util::{BodyExt as _, Empty, Full, combinators::BoxBody};
use hyper::body::Incoming;
use hyper::header::{
    ACCESS_CONTROL_ALLOW_CREDENTIALS, ACCESS_CONTROL_ALLOW_HEADERS, ACCESS_CONTROL_ALLOW_METHODS,
    ACCESS_CONTROL_ALLOW_ORIGIN, ACCESS_CONTROL_REQUEST_HEADERS, ACCESS_CONTROL_REQUEST_METHOD,
    AUTHORIZATION, COOKIE, HOST, ORIGIN, SET_COOKIE, VARY, WWW_AUTHENTICATE,
};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{HeaderMap, Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use runtrol_security::{Caller, DeviceId};
use sha2::{Digest as _, Sha256};
use tokio::net::TcpStream;
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::{NoiseUpgrade, SessionBinding, SessionResponder, StaticKeypair, WebSocketLinkError};

const PROTOCOL_HEADER: &str = "x-runtrol-proto";
const PROTOCOL_VERSION: &str = "1";
const TOKEN_BYTES: usize = 32;
const TOKEN_HEX: usize = TOKEN_BYTES * 2;
const TOKEN_FINGERPRINT_DOMAIN: &[u8] = b"runtrol/http-credential/1";
/// Exact HTTP path used by the browser-compatible Noise link.
pub const NOISE_LINK_PATH: &str = "/v1/link";

/// Exact RFC 6455 subprotocol selected for the browser-compatible Noise link.
pub const NOISE_LINK_PROTOCOL: &str = "runtrol.noise.v1";

/// A response body on the phone-facing HTTP plane.
///
/// Boxed so a later streaming route can share the same guarded connection without changing the admission API.
pub type PhoneBody = BoxBody<Bytes, Infallible>;

/// A device bearer credential.
///
/// It deliberately implements neither `Debug` nor `Display`. A diagnostic can name the device but has no reason to
/// print the secret that proves it. The canonical representation is 32 bytes encoded as 64 lowercase hexadecimal
/// characters.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct AccessToken([u8; TOKEN_BYTES]);

impl AccessToken {
    /// Parse a canonical 256-bit token.
    ///
    /// # Errors
    ///
    /// [`PhoneHttpError::InvalidToken`] when the text is not exactly 64 lowercase hexadecimal characters.
    pub fn parse(encoded: &str) -> Result<Self, PhoneHttpError> {
        decode_token(encoded.as_bytes()).map(Self)
    }

    /// Derive the non-secret value stored for this credential.
    #[must_use]
    pub fn fingerprint(&self) -> CredentialFingerprint {
        let mut hasher = Sha256::new();
        hasher.update(TOKEN_FINGERPRINT_DOMAIN);
        hasher.update(self.0);
        CredentialFingerprint(hasher.finalize().into())
    }
}

/// The one-way value stored for a device bearer credential.
///
/// A database disclosure does not reveal a bearer token. Tokens carry 256 random bits, and the domain-separated
/// SHA-256 image is compared in constant time after an incoming canonical token is decoded.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct CredentialFingerprint([u8; TOKEN_BYTES]);

impl CredentialFingerprint {
    /// Rebuild a fingerprint from its stored bytes.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; TOKEN_BYTES]) -> Self {
        Self(bytes)
    }

    /// Return the bytes safe to persist in the device authorization row.
    #[must_use]
    pub const fn to_bytes(self) -> [u8; TOKEN_BYTES] {
        self.0
    }

    fn matches_encoded(&self, encoded: &[u8]) -> bool {
        let candidate = match decode_token(encoded) {
            Ok(bytes) => AccessToken(bytes).fingerprint(),
            Err(_) => return false,
        };

        let mut mismatch = 0_u8;
        for (stored, offered) in self.0.iter().zip(candidate.0) {
            mismatch |= *stored ^ offered;
        }
        mismatch == 0
    }
}

impl core::fmt::Debug for CredentialFingerprint {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("CredentialFingerprint(..)")
    }
}

fn decode_token(encoded: &[u8]) -> Result<[u8; TOKEN_BYTES], PhoneHttpError> {
    if encoded.len() != TOKEN_HEX {
        return Err(PhoneHttpError::InvalidToken);
    }
    let mut bytes = [0_u8; TOKEN_BYTES];
    for (slot, pair) in bytes.iter_mut().zip(encoded.chunks_exact(2)) {
        let &[high_byte, low_byte] = pair else {
            return Err(PhoneHttpError::InvalidToken);
        };
        let Some(high) = hex_nibble(high_byte) else {
            return Err(PhoneHttpError::InvalidToken);
        };
        let Some(low) = hex_nibble(low_byte) else {
            return Err(PhoneHttpError::InvalidToken);
        };
        *slot = (high << 4) | low;
    }
    Ok(bytes)
}

/// One credential and the paired device identity it establishes.
///
/// The request never names its caller. The credential registry establishes it and the connection wrapper inserts a
/// [`Caller`] extension before the handler can see the request.
pub struct DeviceCredential {
    device: DeviceId,
    fingerprint: CredentialFingerprint,
}

impl DeviceCredential {
    /// Bind `token` to the device created by pairing.
    #[must_use]
    pub fn new(device: DeviceId, token: &AccessToken) -> Self {
        Self {
            device,
            fingerprint: token.fingerprint(),
        }
    }

    /// Rebuild a credential binding from its non-secret stored fingerprint.
    #[must_use]
    pub const fn from_fingerprint(device: DeviceId, fingerprint: CredentialFingerprint) -> Self {
        Self {
            device,
            fingerprint,
        }
    }
}

/// A phone-facing HTTP connection server with immutable admission policy.
#[derive(Clone)]
pub struct PhoneHttp {
    policy: Arc<Policy>,
}

/// One exact-origin WebSocket upgrade admitted only to begin Noise device authentication.
///
/// The request carries no [`Caller`]. A caller exists only after [`NoiseUpgrade::receive`] authenticates its static
/// key and the daemon maps that key to one durable paired device.
pub struct AdmittedNoiseUpgrade {
    request: Request<Incoming>,
}

impl AdmittedNoiseUpgrade {
    /// Produce HTTP 101 and a pending Noise responder bound to the selected physical link.
    ///
    /// The response must be returned from the HTTP handler before awaiting the [`NoiseUpgrade`].
    ///
    /// # Errors
    ///
    /// [`WebSocketLinkError::Crypto`] when the Noise responder cannot be initialized, or
    /// [`WebSocketLinkError::WebSocket`] when the RFC 6455 upgrade fields are invalid.
    pub fn begin(
        mut self,
        local: &StaticKeypair,
        binding: &SessionBinding,
    ) -> Result<(Response<PhoneBody>, NoiseUpgrade), WebSocketLinkError> {
        let responder = SessionResponder::new(local, binding)?;
        let (mut response, upgraded) = fastwebsockets::upgrade::upgrade(&mut self.request)
            .map_err(WebSocketLinkError::from)?;
        response.headers_mut().insert(
            "Sec-WebSocket-Protocol",
            hyper::header::HeaderValue::from_static(NOISE_LINK_PROTOCOL),
        );
        let (parts, _) = response.into_parts();
        Ok((
            Response::from_parts(parts, Empty::<Bytes>::new().boxed()),
            NoiseUpgrade::new(upgraded, responder),
        ))
    }
}

struct Policy {
    hosts: Vec<Box<str>>,
    origins: Vec<Box<str>>,
    credentials: Vec<DeviceCredential>,
}

impl PhoneHttp {
    /// Build a policy for a listener bound to a loopback port.
    ///
    /// Accepted Host values are exactly `127.0.0.1`, `[::1]`, and `localhost` with this port. Origins must be
    /// explicit HTTPS origins. An empty origin or credential set remains valid but denies every request, which is the
    /// remote plane's safe first-run state.
    ///
    /// # Errors
    ///
    /// [`PhoneHttpError::InvalidPort`] for port zero, or [`PhoneHttpError::InvalidOrigin`] for a wildcard,
    /// non-HTTPS, or path-bearing origin.
    pub fn loopback(
        port: u16,
        origins: impl IntoIterator<Item = impl AsRef<str>>,
        credentials: impl IntoIterator<Item = DeviceCredential>,
    ) -> Result<Self, PhoneHttpError> {
        if port == 0 {
            return Err(PhoneHttpError::InvalidPort);
        }

        let mut checked_origins: Vec<Box<str>> = Vec::new();
        for origin in origins {
            let origin = origin.as_ref();
            if !valid_origin(origin) {
                return Err(PhoneHttpError::InvalidOrigin {
                    origin: origin.into(),
                });
            }
            if !checked_origins.iter().any(|known| &**known == origin) {
                checked_origins.push(Box::<str>::from(origin));
            }
        }

        Ok(Self {
            policy: Arc::new(Policy {
                hosts: vec![
                    format!("127.0.0.1:{port}").into(),
                    format!("[::1]:{port}").into(),
                    format!("localhost:{port}").into(),
                ],
                origins: checked_origins,
                credentials: credentials.into_iter().collect(),
            }),
        })
    }

    /// Serve one accepted TCP connection behind the complete browser boundary.
    ///
    /// The handler receives only admitted requests and each one carries a [`Caller::Device`] extension established
    /// from the bearer credential. CORS preflight is answered here and never reaches it. Cookie headers are removed,
    /// and cookie-setting or ambient-credential CORS headers are removed from every handler response.
    ///
    /// # Errors
    ///
    /// A Hyper connection error when the peer violates HTTP framing or the socket fails.
    pub async fn serve_connection<F, Fut>(
        &self,
        stream: TcpStream,
        handler: F,
    ) -> Result<(), hyper::Error>
    where
        F: Fn(Request<Incoming>) -> Fut + Clone + Send + Sync + 'static,
        Fut: Future<Output = Response<PhoneBody>> + Send + 'static,
    {
        let server = self.clone();
        let service = service_fn(move |mut request| {
            let server = server.clone();
            let handler = handler.clone();
            async move {
                let decision = server.policy.admit(&request);
                let answer = match decision {
                    Decision::Forward { caller, origin } => {
                        request.headers_mut().remove(COOKIE);
                        request.extensions_mut().insert(caller);
                        let response = handler(request).await;
                        secure_response(response, Some(&origin))
                    }
                    Decision::Preflight { origin } => preflight_response(&origin),
                    Decision::Refuse {
                        status,
                        allowed_origin,
                    } => refusal(status, allowed_origin.as_deref()),
                    Decision::NoiseUpgrade { origin } => {
                        refusal(StatusCode::FORBIDDEN, Some(&origin))
                    }
                };
                Ok::<_, Infallible>(answer)
            }
        });

        http1::Builder::new()
            .serve_connection(TokioIo::new(stream), service)
            .with_upgrades()
            .await
    }

    /// Serve one TCP connection that admits only the browser Noise WebSocket endpoint.
    ///
    /// Host, Origin, browser metadata, path, method, RFC 6455 upgrade fields, and the exact subprotocol are checked
    /// before `handler` receives [`AdmittedNoiseUpgrade`]. No bearer token crosses the unencrypted LAN hop. The
    /// upgraded stream itself remains unusable until Noise authenticates and the daemon authorizes a stored key.
    ///
    /// # Errors
    ///
    /// A Hyper connection error when the peer violates HTTP framing or the socket fails.
    pub async fn serve_noise_connection<F, Fut>(
        &self,
        stream: TcpStream,
        handler: F,
    ) -> Result<(), hyper::Error>
    where
        F: Fn(AdmittedNoiseUpgrade) -> Fut + Clone + Send + Sync + 'static,
        Fut: Future<Output = Response<PhoneBody>> + Send + 'static,
    {
        let server = self.clone();
        let service = service_fn(move |request| {
            let server = server.clone();
            let handler = handler.clone();
            async move {
                let answer = match server.policy.admit_noise(&request) {
                    Decision::NoiseUpgrade { origin } => {
                        let response = handler(AdmittedNoiseUpgrade { request }).await;
                        secure_response(response, Some(&origin))
                    }
                    Decision::Refuse {
                        status,
                        allowed_origin,
                    } => refusal(status, allowed_origin.as_deref()),
                    Decision::Forward { .. } | Decision::Preflight { .. } => {
                        refusal(StatusCode::FORBIDDEN, None)
                    }
                };
                Ok::<_, Infallible>(answer)
            }
        });

        http1::Builder::new()
            .serve_connection(TokioIo::new(stream), service)
            .with_upgrades()
            .await
    }
}

impl Policy {
    fn admit<B>(&self, request: &Request<B>) -> Decision {
        if !self.host_allowed(request.headers()) {
            return Decision::Refuse {
                status: StatusCode::MISDIRECTED_REQUEST,
                allowed_origin: None,
            };
        }

        let Some(origin) = single_header(request.headers(), ORIGIN) else {
            return Decision::Refuse {
                status: StatusCode::FORBIDDEN,
                allowed_origin: None,
            };
        };
        let Some(origin) = self.origins.iter().find(|known| &***known == origin) else {
            return Decision::Refuse {
                status: StatusCode::FORBIDDEN,
                allowed_origin: None,
            };
        };
        let origin = origin.clone();

        if request.method() == Method::OPTIONS {
            return if valid_preflight(request.headers()) {
                Decision::Preflight { origin }
            } else {
                Decision::Refuse {
                    status: StatusCode::FORBIDDEN,
                    allowed_origin: Some(origin),
                }
            };
        }

        if request.method() != Method::GET && request.method() != Method::POST {
            return Decision::Refuse {
                status: StatusCode::METHOD_NOT_ALLOWED,
                allowed_origin: Some(origin),
            };
        }

        if !matches!(
            single_header(request.headers(), "sec-fetch-site"),
            Some("same-origin" | "none")
        ) || single_header(request.headers(), PROTOCOL_HEADER) != Some(PROTOCOL_VERSION)
        {
            return Decision::Refuse {
                status: StatusCode::FORBIDDEN,
                allowed_origin: Some(origin),
            };
        }

        let Some(encoded) = bearer(request.headers()) else {
            return Decision::Refuse {
                status: StatusCode::UNAUTHORIZED,
                allowed_origin: Some(origin),
            };
        };
        let Some(credential) = self
            .credentials
            .iter()
            .find(|credential| credential.fingerprint.matches_encoded(encoded))
        else {
            return Decision::Refuse {
                status: StatusCode::UNAUTHORIZED,
                allowed_origin: Some(origin),
            };
        };

        Decision::Forward {
            caller: Caller::Device {
                device: credential.device,
            },
            origin,
        }
    }

    fn host_allowed(&self, headers: &HeaderMap) -> bool {
        let Some(offered) = single_header(headers, HOST) else {
            return false;
        };
        self.hosts
            .iter()
            .any(|allowed| allowed.eq_ignore_ascii_case(offered))
    }

    fn admit_noise<B>(&self, request: &Request<B>) -> Decision {
        if !self.host_allowed(request.headers()) {
            return Decision::Refuse {
                status: StatusCode::MISDIRECTED_REQUEST,
                allowed_origin: None,
            };
        }
        let Some(origin) = single_header(request.headers(), ORIGIN) else {
            return Decision::Refuse {
                status: StatusCode::FORBIDDEN,
                allowed_origin: None,
            };
        };
        let Some(origin) = self.origins.iter().find(|known| &***known == origin) else {
            return Decision::Refuse {
                status: StatusCode::FORBIDDEN,
                allowed_origin: None,
            };
        };
        let origin = origin.clone();
        if request.method() != Method::GET {
            return Decision::Refuse {
                status: StatusCode::METHOD_NOT_ALLOWED,
                allowed_origin: Some(origin),
            };
        }
        if request.uri().path() != NOISE_LINK_PATH || request.uri().query().is_some() {
            return Decision::Refuse {
                status: StatusCode::NOT_FOUND,
                allowed_origin: Some(origin),
            };
        }
        if !matches!(
            single_header(request.headers(), "sec-fetch-site"),
            Some("same-origin" | "none")
        ) || single_header(request.headers(), "sec-websocket-protocol")
            != Some(NOISE_LINK_PROTOCOL)
            || !fastwebsockets::upgrade::is_upgrade_request(request)
        {
            return Decision::Refuse {
                status: StatusCode::FORBIDDEN,
                allowed_origin: Some(origin),
            };
        }
        Decision::NoiseUpgrade { origin }
    }
}

enum Decision {
    Forward {
        caller: Caller,
        origin: Box<str>,
    },
    Preflight {
        origin: Box<str>,
    },
    NoiseUpgrade {
        origin: Box<str>,
    },
    Refuse {
        status: StatusCode,
        allowed_origin: Option<Box<str>>,
    },
}

/// Build a response body suitable for a guarded phone route.
#[must_use]
pub fn response(status: StatusCode, body: impl Into<Bytes>) -> Response<PhoneBody> {
    let mut response = Response::new(Full::new(body.into()).boxed());
    *response.status_mut() = status;
    response
}

fn refusal(status: StatusCode, allowed_origin: Option<&str>) -> Response<PhoneBody> {
    let mut response = response(status, status.canonical_reason().unwrap_or("refused"));
    if status == StatusCode::UNAUTHORIZED {
        response.headers_mut().insert(
            WWW_AUTHENTICATE,
            hyper::header::HeaderValue::from_static("Bearer"),
        );
    }
    secure_response(response, allowed_origin)
}

fn preflight_response(origin: &str) -> Response<PhoneBody> {
    let mut response = response(StatusCode::NO_CONTENT, Bytes::new());
    response.headers_mut().insert(
        ACCESS_CONTROL_ALLOW_METHODS,
        hyper::header::HeaderValue::from_static("GET, POST"),
    );
    response.headers_mut().insert(
        ACCESS_CONTROL_ALLOW_HEADERS,
        hyper::header::HeaderValue::from_static("Authorization, X-Runtrol-Proto"),
    );
    secure_response(response, Some(origin))
}

fn secure_response(
    mut response: Response<PhoneBody>,
    allowed_origin: Option<&str>,
) -> Response<PhoneBody> {
    let headers = response.headers_mut();
    headers.remove(SET_COOKIE);
    headers.remove(ACCESS_CONTROL_ALLOW_CREDENTIALS);
    headers.remove(ACCESS_CONTROL_ALLOW_ORIGIN);
    headers.append(VARY, hyper::header::HeaderValue::from_static("Origin"));
    if let Some(origin) = allowed_origin
        && let Ok(value) = hyper::header::HeaderValue::from_str(origin)
    {
        headers.insert(ACCESS_CONTROL_ALLOW_ORIGIN, value);
    }
    response
}

fn single_header(headers: &HeaderMap, name: impl hyper::header::AsHeaderName) -> Option<&str> {
    let mut values = headers.get_all(name).iter();
    let Ok(value) = values.next()?.to_str() else {
        return None;
    };
    if values.next().is_some() {
        return None;
    }
    Some(value)
}

fn bearer(headers: &HeaderMap) -> Option<&[u8]> {
    let value = single_header(headers, AUTHORIZATION)?;
    let (scheme, encoded) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") || encoded.contains(' ') {
        return None;
    }
    Some(encoded.as_bytes())
}

fn valid_preflight(headers: &HeaderMap) -> bool {
    if !matches!(
        single_header(headers, ACCESS_CONTROL_REQUEST_METHOD),
        Some("GET" | "POST")
    ) {
        return false;
    }
    let Some(offered) = single_header(headers, ACCESS_CONTROL_REQUEST_HEADERS) else {
        return false;
    };
    let mut authorization = false;
    let mut protocol = false;
    for name in offered.split(',').map(str::trim) {
        if name.eq_ignore_ascii_case("authorization") {
            authorization = true;
        } else if name.eq_ignore_ascii_case(PROTOCOL_HEADER) {
            protocol = true;
        } else {
            return false;
        }
    }
    authorization && protocol
}

fn valid_origin(origin: &str) -> bool {
    let Some(authority) = origin.strip_prefix("https://") else {
        return false;
    };
    !authority.is_empty()
        && authority.is_ascii()
        && !authority.contains(['/', '?', '#', '@', '*'])
        && authority.parse::<hyper::http::uri::Authority>().is_ok()
}

const fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

/// Invalid immutable policy configuration.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum PhoneHttpError {
    /// Port zero cannot appear in an exact Host allowlist.
    #[error("phone HTTP policy needs the listener's assigned nonzero port")]
    InvalidPort,

    /// A bearer token was not the canonical 256-bit representation.
    #[error("a device bearer token must be 64 lowercase hexadecimal characters")]
    InvalidToken,

    /// An origin was not an explicit HTTPS origin.
    #[error("{origin} is not an explicit HTTPS origin without a path")]
    InvalidOrigin {
        /// The refused configuration value.
        origin: Box<str>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    const PHONE_ORIGIN: &str = "https://phone.runtrol.test";

    #[test]
    fn tokens_are_fixed_strength_and_canonical() {
        let canonical = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let token = AccessToken::parse(canonical).expect("canonical");
        let fingerprint = token.fingerprint();
        assert!(fingerprint.matches_encoded(canonical.as_bytes()));
        assert!(!fingerprint.matches_encoded(
            "1123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".as_bytes()
        ));
        assert!(AccessToken::parse("short").is_err());
        assert!(AccessToken::parse(&canonical.to_ascii_uppercase()).is_err());
    }

    #[test]
    fn persisted_fingerprints_restore_credentials_without_storing_tokens() {
        let encoded = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let token = AccessToken::parse(encoded).expect("canonical");
        let stored = token.fingerprint().to_bytes();
        let restored = CredentialFingerprint::from_bytes(stored);
        let credential = DeviceCredential::from_fingerprint(DeviceId::now(), restored);

        assert!(credential.fingerprint.matches_encoded(encoded.as_bytes()));
        assert_eq!(
            format!("{:?}", credential.fingerprint),
            "CredentialFingerprint(..)"
        );
        assert!(!format!("{:?}", credential.fingerprint).contains("012345"));
    }

    #[test]
    fn wildcard_and_path_origins_cannot_enter_the_policy() {
        for origin in [
            "*",
            "http://phone.runtrol.test",
            "https://*.runtrol.test",
            "https://phone.runtrol.test/path",
            "https://user@phone.runtrol.test",
        ] {
            let result = PhoneHttp::loopback(49152, [origin], []);
            assert!(result.is_err(), "{origin}");
        }
    }

    #[test]
    fn no_origin_and_no_device_is_a_valid_default_deny_policy() {
        let policy = PhoneHttp::loopback(49152, std::iter::empty::<&str>(), []);
        assert!(policy.is_ok());
    }

    #[test]
    fn noise_upgrade_requires_the_exact_browser_boundary() {
        let server = PhoneHttp::loopback(49152, [PHONE_ORIGIN], []).expect("policy");
        assert!(matches!(
            server.policy.admit_noise(&noise_request()),
            Decision::NoiseUpgrade { .. }
        ));

        for (name, value, status) in [
            (HOST, "attacker.test:49152", StatusCode::MISDIRECTED_REQUEST),
            (ORIGIN, "https://attacker.test", StatusCode::FORBIDDEN),
            (
                hyper::header::HeaderName::from_static("sec-fetch-site"),
                "cross-site",
                StatusCode::FORBIDDEN,
            ),
            (
                hyper::header::HeaderName::from_static("sec-websocket-protocol"),
                "runtrol.other.v1",
                StatusCode::FORBIDDEN,
            ),
        ] {
            let mut request = noise_request();
            request.headers_mut().insert(
                name,
                hyper::header::HeaderValue::from_str(value).expect("header value"),
            );
            assert!(matches!(
                server.policy.admit_noise(&request),
                Decision::Refuse { status: found, .. } if found == status
            ));
        }
    }

    #[test]
    fn noise_upgrade_rejects_a_query_or_non_link_path() {
        let server = PhoneHttp::loopback(49152, [PHONE_ORIGIN], []).expect("policy");
        for uri in ["/v1/link?token=plaintext", "/v1/other"] {
            let mut request = noise_request();
            *request.uri_mut() = uri.parse().expect("request URI");
            assert!(matches!(
                server.policy.admit_noise(&request),
                Decision::Refuse {
                    status: StatusCode::NOT_FOUND,
                    ..
                }
            ));
        }
    }

    fn noise_request() -> Request<()> {
        Request::builder()
            .method(Method::GET)
            .uri(NOISE_LINK_PATH)
            .header(HOST, "127.0.0.1:49152")
            .header(ORIGIN, PHONE_ORIGIN)
            .header("Sec-Fetch-Site", "same-origin")
            .header("Connection", "Upgrade")
            .header("Upgrade", "websocket")
            .header("Sec-WebSocket-Protocol", NOISE_LINK_PROTOCOL)
            .body(())
            .expect("noise request")
    }
}
