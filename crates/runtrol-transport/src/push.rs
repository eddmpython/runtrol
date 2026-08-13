//! Bodyless Web Push doorbells with stable VAPID identity and encrypted capability storage.
//!
//! A push subscription URL is a bearer capability. It is accepted only from the authenticated Noise channel,
//! validated against the reviewed push-service host set, and encrypted with device-bound AAD before persistence.
//! Delivery resolves the service afresh, rejects every DNS set containing a non-public address, converts the exact
//! answers into egress capabilities, authenticates the DNS name with `WebPKI`, and sends an empty HTTP/2 POST. No
//! session, prompt, output, approval subject, provider, workspace, or identifier enters the request body.

use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::str::FromStr as _;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aes_gcm::aead::{Aead as _, KeyInit as _, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use base64ct::{Base64UrlUnpadded, Encoding as _};
use bytes::Bytes;
use http_body_util::{BodyExt as _, Full};
use hyper::body::Incoming;
use hyper::header::AUTHORIZATION;
use hyper::{Method, Request, StatusCode, Uri};
use hyper_util::rt::{TokioExecutor, TokioIo};
use p256::ecdsa::signature::Signer as _;
use p256::ecdsa::{Signature, SigningKey};
use tokio::net::TcpStream;
use tokio::task::JoinHandle;
use tokio::time::timeout;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;
use tokio_rustls::rustls::pki_types::ServerName;
use tokio_rustls::rustls::{ClientConfig, RootCertStore};
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

use crate::{ApprovedDestination, EgressPolicy};

const KEY_BYTES: usize = 32;
const NONCE_BYTES: usize = 12;
const TAG_BYTES: usize = 16;
const PUBLIC_KEY_BYTES: usize = 65;
const MAX_ENDPOINT_BYTES: usize = 2_048;
const PUSH_PORT: u16 = 443;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const RESPONSE_BODY_LIMIT: usize = 512;
const JWT_LIFETIME_SECONDS: u64 = 12 * 60 * 60;
const STORAGE_SALT: &[u8] = b"runtrol/push-storage/1";
const VAPID_SALT: &[u8] = b"runtrol/vapid-signing/1";
const STORAGE_INFO: &[u8] = b"push endpoint encryption";
const VAPID_INFO: &[u8] = b"VAPID ES256 signing key";
const ENDPOINT_AAD: &[u8] = b"runtrol/push-endpoint/1";
const VAPID_SUBJECT: &str = "https://github.com/eddmpython/runtrol";

/// Stable push identity derived from the operating-system-protected machine secret.
///
/// It implements neither `Debug`, `Display`, nor `Clone`. The signing and storage keys are domain-separated from
/// the Noise identity and relay route material, and only the VAPID public key is releasable.
#[derive(ZeroizeOnDrop)]
pub struct PushIdentity {
    #[zeroize(skip)]
    signing: SigningKey,
    storage_key: [u8; KEY_BYTES],
}

impl PushIdentity {
    /// Derive stable VAPID signing and endpoint-storage keys from one protected machine secret.
    ///
    /// # Errors
    ///
    /// [`PushError::KeyDerivation`] if fixed-length HKDF expansion fails or no valid P-256 scalar is produced.
    pub fn derive(machine_secret: &[u8; KEY_BYTES]) -> Result<Self, PushError> {
        let storage = hkdf::Hkdf::<sha2::Sha256>::new(Some(STORAGE_SALT), machine_secret);
        let mut storage_key = [0_u8; KEY_BYTES];
        storage
            .expand(STORAGE_INFO, &mut storage_key)
            .map_err(|_| PushError::KeyDerivation)?;

        let vapid = hkdf::Hkdf::<sha2::Sha256>::new(Some(VAPID_SALT), machine_secret);
        for counter in 0_u8..=u8::MAX {
            let mut candidate = Zeroizing::new([0_u8; KEY_BYTES]);
            let mut info = Zeroizing::new(Vec::with_capacity(VAPID_INFO.len() + 1));
            info.extend_from_slice(VAPID_INFO);
            info.push(counter);
            vapid
                .expand(&info, &mut *candidate)
                .map_err(|_| PushError::KeyDerivation)?;
            if let Ok(signing) = SigningKey::from_slice(candidate.as_slice()) {
                return Ok(Self {
                    signing,
                    storage_key,
                });
            }
        }
        storage_key.zeroize();
        Err(PushError::KeyDerivation)
    }

    /// Return the canonical uncompressed P-256 application-server key for `PushManager.subscribe`.
    #[must_use]
    pub fn application_server_key(&self) -> String {
        Base64UrlUnpadded::encode_string(self.public_key_bytes().as_slice())
    }

    /// Validate and encrypt one device's push capability URL for durable metadata storage.
    ///
    /// # Errors
    ///
    /// [`PushError::InvalidEndpoint`] for an unsupported endpoint, randomness failure, or encryption failure.
    pub fn seal_endpoint(&self, device: [u8; 16], endpoint: &str) -> Result<Vec<u8>, PushError> {
        let endpoint = PushEndpoint::parse(endpoint)?;
        let mut nonce = [0_u8; NONCE_BYTES];
        getrandom::fill(&mut nonce).map_err(|_| PushError::RandomUnavailable)?;
        let cipher =
            Aes256Gcm::new_from_slice(&self.storage_key).map_err(|_| PushError::KeyDerivation)?;
        let aad = endpoint_aad(device);
        let encrypted = cipher
            .encrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: endpoint.encoded.as_bytes(),
                    aad: &aad,
                },
            )
            .map_err(|_| PushError::Encryption)?;
        let mut stored = Vec::with_capacity(NONCE_BYTES + encrypted.len());
        stored.extend_from_slice(&nonce);
        stored.extend_from_slice(&encrypted);
        Ok(stored)
    }

    /// Open, authenticate, and revalidate one encrypted device subscription.
    ///
    /// # Errors
    ///
    /// [`PushError::InvalidStoredEndpoint`] when the blob is truncated, moved between devices, modified, or no
    /// longer matches the endpoint policy.
    fn open_endpoint(&self, device: [u8; 16], stored: &[u8]) -> Result<PushEndpoint, PushError> {
        if stored.len() < NONCE_BYTES + TAG_BYTES + 1
            || stored.len() > NONCE_BYTES + TAG_BYTES + MAX_ENDPOINT_BYTES
        {
            return Err(PushError::InvalidStoredEndpoint);
        }
        let (nonce, ciphertext) = stored.split_at(NONCE_BYTES);
        let cipher =
            Aes256Gcm::new_from_slice(&self.storage_key).map_err(|_| PushError::KeyDerivation)?;
        let aad = endpoint_aad(device);
        let mut plaintext = Zeroizing::new(
            cipher
                .decrypt(
                    Nonce::from_slice(nonce),
                    Payload {
                        msg: ciphertext,
                        aad: &aad,
                    },
                )
                .map_err(|_| PushError::InvalidStoredEndpoint)?,
        );
        let text =
            core::str::from_utf8(&plaintext).map_err(|_| PushError::InvalidStoredEndpoint)?;
        let parsed = PushEndpoint::parse(text).map_err(|_| PushError::InvalidStoredEndpoint);
        plaintext.zeroize();
        parsed
    }

    /// Send one content-free wake signal to an encrypted subscription.
    ///
    /// # Errors
    ///
    /// [`PushError`] for stored capability, DNS, egress, TLS, HTTP, time, or push-service refusal failures.
    pub async fn wake(&self, device: [u8; 16], stored: &[u8]) -> Result<PushDelivery, PushError> {
        let endpoint = self.open_endpoint(device, stored)?;
        endpoint.resolve().await?.send(self).await
    }

    /// Authenticate and revalidate an encrypted subscription without releasing its plaintext.
    ///
    /// # Errors
    ///
    /// The same stored capability errors as [`Self::wake`], before any network operation begins.
    pub fn validate_stored_endpoint(
        &self,
        device: [u8; 16],
        stored: &[u8],
    ) -> Result<(), PushError> {
        drop(self.open_endpoint(device, stored)?);
        Ok(())
    }

    fn public_key_bytes(&self) -> [u8; PUBLIC_KEY_BYTES] {
        let encoded = self.signing.verifying_key().to_encoded_point(false);
        let mut bytes = [0_u8; PUBLIC_KEY_BYTES];
        bytes.copy_from_slice(encoded.as_bytes());
        bytes
    }

    fn authorization(&self, audience: &str) -> Result<hyper::header::HeaderValue, PushError> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| PushError::Clock)?
            .as_secs();
        self.authorization_at(audience, now)
    }

    fn authorization_at(
        &self,
        audience: &str,
        now: u64,
    ) -> Result<hyper::header::HeaderValue, PushError> {
        let expiry = now
            .checked_add(JWT_LIFETIME_SECONDS)
            .ok_or(PushError::Clock)?;
        let header = Base64UrlUnpadded::encode_string(br#"{"typ":"JWT","alg":"ES256"}"#);
        let claims = format!(r#"{{"aud":"{audience}","exp":{expiry},"sub":"{VAPID_SUBJECT}"}}"#);
        let claims = Base64UrlUnpadded::encode_string(claims.as_bytes());
        let signed = Zeroizing::new(format!("{header}.{claims}"));
        let signature: Signature = self.signing.sign(signed.as_bytes());
        let signature = Base64UrlUnpadded::encode_string(signature.to_bytes().as_slice());
        let key = self.application_server_key();
        let value = Zeroizing::new(format!("vapid t={}.{signature}, k={key}", signed.as_str()));
        hyper::header::HeaderValue::from_str(&value).map_err(|_| PushError::Authorization)
    }
}

fn endpoint_aad(device: [u8; 16]) -> [u8; ENDPOINT_AAD.len() + 16] {
    let mut aad = [0_u8; ENDPOINT_AAD.len() + 16];
    let (domain, identity) = aad.split_at_mut(ENDPOINT_AAD.len());
    domain.copy_from_slice(ENDPOINT_AAD);
    identity.copy_from_slice(&device);
    aad
}

struct PushEndpoint {
    encoded: Box<str>,
    host: Box<str>,
    audience: Box<str>,
}

impl PushEndpoint {
    fn parse(endpoint: &str) -> Result<Self, PushError> {
        if endpoint.is_empty() || endpoint.len() > MAX_ENDPOINT_BYTES || !endpoint.is_ascii() {
            return Err(PushError::InvalidEndpoint);
        }
        let uri = Uri::from_str(endpoint).map_err(|_| PushError::InvalidEndpoint)?;
        if uri.scheme_str() != Some("https") {
            return Err(PushError::InvalidEndpoint);
        }
        let authority = uri.authority().ok_or(PushError::InvalidEndpoint)?;
        if authority.port_u16().is_some() || authority.as_str().contains('@') {
            return Err(PushError::InvalidEndpoint);
        }
        let host = authority.host();
        if !allowed_push_host(host) {
            return Err(PushError::InvalidEndpoint);
        }
        let target = uri
            .path_and_query()
            .map(hyper::http::uri::PathAndQuery::as_str)
            .ok_or(PushError::InvalidEndpoint)?;
        if target.len() < 2 || !target.starts_with('/') {
            return Err(PushError::InvalidEndpoint);
        }
        let canonical = format!("https://{host}{target}");
        if canonical != endpoint {
            return Err(PushError::InvalidEndpoint);
        }
        Ok(Self {
            encoded: endpoint.into(),
            host: host.into(),
            audience: format!("https://{host}").into(),
        })
    }

    async fn resolve(self) -> Result<ResolvedPush, PushError> {
        let resolved = timeout(
            CONNECT_TIMEOUT,
            tokio::net::lookup_host((&*self.host, PUSH_PORT)),
        )
        .await
        .map_err(|_| PushError::Timeout)?
        .map_err(|_| PushError::Resolve)?;
        let addresses: BTreeSet<SocketAddr> = resolved.collect();
        if addresses.is_empty() {
            return Err(PushError::Resolve);
        }
        if addresses
            .iter()
            .any(|address| !crate::egress::public_internet_address(address.ip()))
        {
            return Err(PushError::PrivateAddress);
        }
        let policy = EgressPolicy::new(addresses.iter().copied());
        let destinations = addresses
            .into_iter()
            .map(|address| policy.approve(address).map_err(PushError::from))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(ResolvedPush {
            endpoint: self,
            policy,
            destinations,
        })
    }

    fn wake_request(&self, identity: &PushIdentity) -> Result<Request<Full<Bytes>>, PushError> {
        Request::builder()
            .method(Method::POST)
            .uri(&*self.encoded)
            .header(AUTHORIZATION, identity.authorization(&self.audience)?)
            .header("ttl", "60")
            .header("urgency", "high")
            .header("topic", "runtrol-attention")
            .body(Full::new(Bytes::new()))
            .map_err(|_| PushError::Http)
    }
}

fn allowed_push_host(host: &str) -> bool {
    host == "fcm.googleapis.com"
        || host == "web.push.apple.com"
        || host
            .strip_suffix(".push.apple.com")
            .is_some_and(|prefix| !prefix.is_empty() && !prefix.contains('.'))
}

struct ResolvedPush {
    endpoint: PushEndpoint,
    policy: EgressPolicy,
    destinations: Vec<ApprovedDestination>,
}

impl ResolvedPush {
    async fn send(self, identity: &PushIdentity) -> Result<PushDelivery, PushError> {
        let tls = self.connect_tls().await?;
        let (mut sender, connection) = timeout(
            REQUEST_TIMEOUT,
            hyper::client::conn::http2::handshake(TokioExecutor::new(), TokioIo::new(tls)),
        )
        .await
        .map_err(|_| PushError::Timeout)?
        .map_err(|_| PushError::Http)?;
        let driver = ConnectionDriver::new(tokio::spawn(connection));
        let request = self.endpoint.wake_request(identity)?;
        let response = timeout(REQUEST_TIMEOUT, sender.send_request(request))
            .await
            .map_err(|_| PushError::Timeout)?
            .map_err(|_| PushError::Http)?;
        drop(sender);
        let status = response.status();
        drain_bounded(response).await?;
        drop(driver);
        match status {
            StatusCode::CREATED | StatusCode::ACCEPTED | StatusCode::NO_CONTENT => {
                Ok(PushDelivery::Accepted)
            }
            StatusCode::NOT_FOUND | StatusCode::GONE => Ok(PushDelivery::SubscriptionExpired),
            _ => Err(PushError::Refused { status }),
        }
    }

    async fn connect_tls(&self) -> Result<TlsStream<TcpStream>, PushError> {
        let connector = TlsConnector::from(push_tls_config());
        for destination in &self.destinations {
            let connected = timeout(CONNECT_TIMEOUT, self.policy.connect(*destination)).await;
            let Ok(Ok(stream)) = connected else {
                continue;
            };
            let name = ServerName::try_from(self.endpoint.host.to_string())
                .map_err(|_| PushError::InvalidEndpoint)?;
            let secured = timeout(CONNECT_TIMEOUT, connector.connect(name, stream)).await;
            if let Ok(Ok(stream)) = secured {
                return Ok(stream);
            }
        }
        Err(PushError::Connect)
    }
}

async fn drain_bounded(response: hyper::Response<Incoming>) -> Result<(), PushError> {
    let mut body = response.into_body();
    let mut read = 0_usize;
    loop {
        let frame = timeout(REQUEST_TIMEOUT, body.frame())
            .await
            .map_err(|_| PushError::Timeout)?;
        let Some(frame) = frame else {
            return Ok(());
        };
        let frame = frame.map_err(|_| PushError::Http)?;
        if let Some(data) = frame.data_ref() {
            read = read
                .checked_add(data.len())
                .ok_or(PushError::ResponseTooLarge)?;
            if read > RESPONSE_BODY_LIMIT {
                return Err(PushError::ResponseTooLarge);
            }
        }
    }
}

fn push_tls_config() -> Arc<ClientConfig> {
    static CONFIG: OnceLock<Arc<ClientConfig>> = OnceLock::new();
    Arc::clone(CONFIG.get_or_init(|| {
        let roots = webpki_roots::TLS_SERVER_ROOTS
            .iter()
            .cloned()
            .collect::<RootCertStore>();
        let mut config = ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        config.alpn_protocols = vec![b"h2".to_vec()];
        Arc::new(config)
    }))
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
        self.task.abort();
    }
}

/// Result of a push-service request without retaining response content.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PushDelivery {
    /// The service accepted the wake signal.
    Accepted,
    /// The browser-side subscription no longer exists and should be replaced.
    SubscriptionExpired,
}

/// VAPID, protected storage, endpoint, egress, or push delivery failure.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PushError {
    /// Domain-separated fixed-length key derivation failed.
    #[error("push key derivation failed")]
    KeyDerivation,
    /// The operating system would not provide a unique storage nonce.
    #[error("the operating system could not generate push storage material")]
    RandomUnavailable,
    /// Endpoint encryption failed without retaining plaintext.
    #[error("push endpoint encryption failed")]
    Encryption,
    /// The supplied endpoint was not a canonical reviewed push-service capability.
    #[error("push endpoint is not a canonical supported HTTPS capability")]
    InvalidEndpoint,
    /// Stored endpoint authentication, device binding, or current validation failed.
    #[error("stored push endpoint cannot be authenticated")]
    InvalidStoredEndpoint,
    /// System time cannot produce a bounded VAPID expiry.
    #[error("system time cannot produce a VAPID expiry")]
    Clock,
    /// The canonical VAPID authorization value could not be formed.
    #[error("VAPID authorization could not be formed")]
    Authorization,
    /// DNS resolution failed or returned no address.
    #[error("push-service DNS discovery failed")]
    Resolve,
    /// At least one DNS answer entered a local or reserved range.
    #[error("push-service DNS discovery returned a non-public address")]
    PrivateAddress,
    /// Exact socket admission refused the resolved address.
    #[error(transparent)]
    Egress(#[from] crate::EgressError),
    /// No approved address completed `WebPKI` authentication.
    #[error("no approved push-service destination completed TLS")]
    Connect,
    /// A bounded network operation exceeded its deadline.
    #[error("push-service network operation timed out")]
    Timeout,
    /// HTTP/2 framing failed.
    #[error("push-service HTTP exchange failed")]
    Http,
    /// The push service returned more diagnostic content than the fixed bound.
    #[error("push-service response exceeded its bound")]
    ResponseTooLarge,
    /// The service refused the content-free delivery request.
    #[error("push service refused the wake signal with HTTP {status}")]
    Refused {
        /// Non-secret HTTP status.
        status: StatusCode,
    },
}

#[cfg(test)]
mod tests {
    use http_body_util::BodyExt as _;
    use p256::ecdsa::signature::Verifier as _;

    use super::*;

    fn identity() -> PushIdentity {
        PushIdentity::derive(&[0x42; KEY_BYTES]).expect("derived identity")
    }

    #[test]
    fn the_vapid_identity_is_stable_and_uses_a_verifiable_es256_jwt() {
        let first = identity();
        let second = identity();
        assert_eq!(
            first.application_server_key(),
            second.application_server_key()
        );
        let authorization = first
            .authorization_at("https://fcm.googleapis.com", 1_800_000_000)
            .expect("authorization");
        let authorization = authorization.to_str().expect("ASCII authorization");
        let parameters = authorization
            .strip_prefix("vapid t=")
            .expect("vapid scheme");
        let (token, key) = parameters.split_once(", k=").expect("two parameters");
        assert_eq!(key, first.application_server_key());
        let mut segments = token.split('.');
        let header = segments.next().expect("header");
        let claims = segments.next().expect("claims");
        let signature = segments.next().expect("signature");
        assert!(segments.next().is_none());
        let mut signature_bytes = [0_u8; 64];
        Base64UrlUnpadded::decode(signature, &mut signature_bytes).expect("signature base64");
        let signature = Signature::from_slice(&signature_bytes).expect("signature shape");
        first
            .signing
            .verifying_key()
            .verify(format!("{header}.{claims}").as_bytes(), &signature)
            .expect("valid ES256 signature");
        let mut claims_bytes = [0_u8; 256];
        let decoded = Base64UrlUnpadded::decode(claims, &mut claims_bytes).expect("claims base64");
        let claims = core::str::from_utf8(decoded).expect("UTF-8 claims");
        assert!(claims.contains(r#""aud":"https://fcm.googleapis.com""#));
        assert!(claims.contains(r#""exp":1800043200"#));
        assert!(claims.contains(VAPID_SUBJECT));
    }

    #[test]
    fn endpoint_storage_is_device_bound_and_tamper_evident() {
        let identity = identity();
        let endpoint = "https://fcm.googleapis.com/fcm/send/capability";
        let stored = identity.seal_endpoint([1; 16], endpoint).expect("sealed");
        assert!(
            !stored
                .windows(endpoint.len())
                .any(|part| part == endpoint.as_bytes())
        );
        assert_eq!(
            identity
                .open_endpoint([1; 16], &stored)
                .expect("opened")
                .encoded
                .as_ref(),
            endpoint
        );
        assert!(identity.open_endpoint([2; 16], &stored).is_err());
        let mut tampered = stored;
        let last = tampered.len() - 1;
        *tampered.get_mut(last).expect("nonempty sealed endpoint") ^= 1;
        assert!(identity.open_endpoint([1; 16], &tampered).is_err());
    }

    #[test]
    fn only_canonical_reviewed_push_capabilities_are_accepted() {
        for endpoint in [
            "https://fcm.googleapis.com/fcm/send/value",
            "https://web.push.apple.com/Q/value",
        ] {
            PushEndpoint::parse(endpoint).expect("reviewed endpoint");
        }
        for endpoint in [
            "http://fcm.googleapis.com/fcm/send/value",
            "https://fcm.googleapis.com:443/fcm/send/value",
            "https://FCM.googleapis.com/fcm/send/value",
            "https://evil.example/fcm/send/value",
            "https://fcm.googleapis.com/",
            "https://fcm.googleapis.com/fcm/send/value#fragment",
        ] {
            assert!(PushEndpoint::parse(endpoint).is_err(), "{endpoint}");
        }
    }

    #[tokio::test]
    async fn a_wake_request_has_no_payload_or_content_metadata() {
        let endpoint = PushEndpoint::parse("https://fcm.googleapis.com/fcm/send/value")
            .expect("reviewed endpoint");
        let request = endpoint.wake_request(&identity()).expect("wake request");
        assert_eq!(request.method(), Method::POST);
        assert_eq!(request.uri(), "https://fcm.googleapis.com/fcm/send/value");
        assert!(request.headers().get("content-type").is_none());
        assert!(request.headers().get("content-encoding").is_none());
        assert!(
            request
                .into_body()
                .collect()
                .await
                .expect("in-memory body")
                .to_bytes()
                .is_empty()
        );
    }
}
