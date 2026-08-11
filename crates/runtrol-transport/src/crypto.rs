//! Mutually authenticated, end-to-end encrypted phone channels.
//!
//! The relay receives only [`EncryptedRecord`] values. Every link performs a fresh
//! `Noise_IK_25519_AESGCM_SHA256` handshake with pinned static keys. Pairing uses the exact
//! `Noise_IKpsk1_25519_AESGCM_SHA256` pattern and expands its 128-bit QR value with domain-separated HKDF-SHA256.

use std::fmt;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use hkdf::Hkdf;
use runtrol_security::{DeviceId, GrantRequest, PairingIdentity, PcPresence};
use sha2::Sha256;
use snow::params::{DHChoice, NoiseParams};
use snow::resolvers::{CryptoResolver as _, DefaultResolver};
use snow::{Builder, HandshakeState, TransportState};
use zeroize::{Zeroize, ZeroizeOnDrop};

const SESSION_PATTERN: &str = "Noise_IK_25519_AESGCM_SHA256";
const PAIRING_PATTERN: &str = "Noise_IKpsk1_25519_AESGCM_SHA256";
const PAIRING_PROLOGUE: &[u8] = b"runtrol/pair/1";
const QR_PSK_SALT: &[u8] = b"runtrol/pairing-psk/1";
const QR_PSK_INFO: &[u8] = b"Noise_IKpsk1_25519_AESGCM_SHA256";
const NOISE_MESSAGE_MAX: usize = 65_535;
const NOISE_TAG_LEN: usize = 16;
const RECORD_FRAME: u8 = 0x01;
const RECORD_REKEY: u8 = 0x08;
const CHUNK_HEADER_LEN: usize = 10;
const REKEY_AFTER_MESSAGES: u64 = 1 << 24;
const REKEY_AFTER_TIME: Duration = Duration::from_mins(15);
const PAIRING_VALID_FOR: Duration = Duration::from_mins(2);
const PAIRING_MAX_ATTEMPTS: u8 = 5;

/// Maximum plaintext accepted by one Noise transport message.
pub const MAX_NOISE_PLAINTEXT: usize = NOISE_MESSAGE_MAX - NOISE_TAG_LEN;

/// Maximum canonical wire bytes occupied by one encrypted Noise record.
pub const MAX_ENCRYPTED_RECORD_WIRE: usize = NOISE_MESSAGE_MAX + 3;

/// Maximum in-memory frame carried by this boundary.
///
/// This deliberately matches the local IPC envelope budget without depending on the IPC crate. Transport remains a
/// layer-one component and cannot learn local command types.
pub const MAX_TRANSPORT_FRAME: usize = 16 * 1024 * 1024 + 64 * 1024;

/// An X25519 public key.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct PublicKey([u8; 32]);

impl PublicKey {
    /// Rebuild a public key from its stored bytes.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Return the canonical X25519 public bytes.
    #[must_use]
    pub const fn to_bytes(self) -> [u8; 32] {
        self.0
    }
}

impl fmt::Debug for PublicKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PublicKey(..)")
    }
}

/// A long-lived X25519 identity keypair.
///
/// The private key is zeroized on drop. The type implements neither `Debug` nor `Display` so diagnostics cannot
/// accidentally print it.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct StaticKeypair {
    private: [u8; 32],
    #[zeroize(skip)]
    public: PublicKey,
}

impl StaticKeypair {
    /// Generate an identity from the operating-system random source used by the Noise resolver.
    ///
    /// # Errors
    ///
    /// [`CryptoError::Noise`] if the platform random source or X25519 implementation is unavailable.
    pub fn generate() -> Result<Self, CryptoError> {
        let params = noise_params(SESSION_PATTERN)?;
        let mut generated = Builder::new(params)
            .generate_keypair()
            .map_err(CryptoError::from)?;
        if generated.private.len() != 32 || generated.public.len() != 32 {
            generated.private.zeroize();
            return Err(CryptoError::InvalidKeyLength);
        }
        let mut private = [0_u8; 32];
        private.copy_from_slice(&generated.private);
        generated.private.zeroize();
        let public = generated
            .public
            .try_into()
            .map_err(|_| CryptoError::InvalidKeyLength)?;
        Ok(Self {
            private,
            public: PublicKey(public),
        })
    }

    /// Rebuild a long-lived identity from operating-system-protected private bytes.
    ///
    /// The input is borrowed so the caller can keep it in zeroizing storage. The returned keypair immediately owns
    /// its own zeroizing copy and derives the public half through the same Curve25519 implementation Noise uses.
    ///
    /// # Errors
    ///
    /// [`CryptoError::Noise`] if the configured resolver has no Curve25519 implementation, or
    /// [`CryptoError::InvalidKeyLength`] if that implementation returns a public key of another size.
    pub fn from_private(private: &[u8; 32]) -> Result<Self, CryptoError> {
        let mut dh = DefaultResolver
            .resolve_dh(&DHChoice::Curve25519)
            .ok_or(CryptoError::Noise)?;
        dh.set(private);
        let public = dh
            .pubkey()
            .try_into()
            .map_err(|_| CryptoError::InvalidKeyLength)?;
        Ok(Self {
            private: *private,
            public: PublicKey(public),
        })
    }

    /// Return the non-secret public half.
    #[must_use]
    pub const fn public_key(&self) -> PublicKey {
        self.public
    }
}

/// A QR-carried 128-bit one-time pairing secret expanded for Noise.
///
/// The expanded value is zeroized on drop and has no diagnostic representation.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct PairingSecret([u8; 32]);

impl PairingSecret {
    /// Domain-separate a 128-bit QR value into Noise's required 32-byte PSK.
    ///
    /// # Errors
    ///
    /// [`CryptoError::KeyDerivation`] if the HKDF output length is rejected.
    pub fn from_qr(mut qr_value: [u8; 16]) -> Result<Self, CryptoError> {
        let mut expanded = [0_u8; 32];
        let derivation = Hkdf::<Sha256>::new(Some(QR_PSK_SALT), &qr_value);
        let result = derivation
            .expand(QR_PSK_INFO, &mut expanded)
            .map_err(|_| CryptoError::KeyDerivation);
        qr_value.zeroize();
        result?;
        Ok(Self(expanded))
    }
}

/// QR material shown only on the PC during a short-lived pairing offer.
///
/// The value clears on drop and has no `Debug` or `Display` implementation. [`Self::qr_value`] exists solely for
/// the local UI to encode it in a fragment or QR image; it must never enter a request URL, log, or persistent row.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct PairingInvitation {
    qr_value: [u8; 16],
    expires_at_unix_ms: u64,
}

impl PairingInvitation {
    /// Raw 128-bit value the local UI encodes into the pairing QR fragment.
    #[must_use]
    pub const fn qr_value(&self) -> &[u8; 16] {
        &self.qr_value
    }

    /// Expand the QR value for a Rust initiator fixture or native client.
    ///
    /// Browser clients perform the same documented HKDF operation with `WebCrypto`.
    ///
    /// # Errors
    ///
    /// [`CryptoError::KeyDerivation`] if HKDF rejects the fixed output size.
    pub fn noise_secret(&self) -> Result<PairingSecret, CryptoError> {
        PairingSecret::from_qr(self.qr_value)
    }

    /// Wall-clock expiry carried in the QR so a client can refuse stale material before dialing.
    #[must_use]
    pub const fn expires_at_unix_ms(&self) -> u64 {
        self.expires_at_unix_ms
    }
}

enum PairingOfferState {
    Active(PairingSecret),
    Spent,
    Locked,
    Expired,
}

/// A 120-second, single-use, attempt-limited pairing offer owned by the PC.
///
/// A valid first Noise message consumes the PSK immediately, before local approval. This means denying a proposed
/// device never leaves the photographed QR usable for another attempt.
pub struct PairingOffer {
    state: PairingOfferState,
    attempt_id: [u8; 16],
    failed_attempts: u8,
    expires_at: Instant,
}

impl PairingOffer {
    /// Generate a fresh QR invitation from the operating-system random source.
    ///
    /// # Errors
    ///
    /// [`CryptoError::RandomUnavailable`] if the OS random source fails, [`CryptoError::ClockUnavailable`] if the
    /// QR wall-clock expiry cannot be represented, or [`CryptoError::KeyDerivation`] if HKDF fails.
    pub fn generate() -> Result<(Self, PairingInvitation), CryptoError> {
        let mut random = [0_u8; 32];
        getrandom::fill(&mut random).map_err(|_| CryptoError::RandomUnavailable)?;
        let mut qr_value = [0_u8; 16];
        let mut attempt_id = [0_u8; 16];
        let (qr_source, attempt_source) = random.split_at(16);
        qr_value.copy_from_slice(qr_source);
        attempt_id.copy_from_slice(attempt_source);
        random.zeroize();

        let secret = PairingSecret::from_qr(qr_value)?;
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| CryptoError::ClockUnavailable)?
            .as_millis();
        let valid_ms = PAIRING_VALID_FOR.as_millis();
        let expires_at_unix_ms = u64::try_from(
            now_ms
                .checked_add(valid_ms)
                .ok_or(CryptoError::ClockUnavailable)?,
        )
        .map_err(|_| CryptoError::ClockUnavailable)?;

        Ok((
            Self {
                state: PairingOfferState::Active(secret),
                attempt_id,
                failed_attempts: 0,
                expires_at: Instant::now() + PAIRING_VALID_FOR,
            },
            PairingInvitation {
                qr_value,
                expires_at_unix_ms,
            },
        ))
    }

    /// Protocol lifetime for every generated offer.
    #[must_use]
    pub const fn valid_for() -> Duration {
        PAIRING_VALID_FOR
    }

    /// Attempts left before the PSK is destroyed and the offer locks.
    #[must_use]
    pub const fn remaining_attempts(&self) -> u8 {
        match self.state {
            PairingOfferState::Active(_) => {
                PAIRING_MAX_ATTEMPTS.saturating_sub(self.failed_attempts)
            }
            PairingOfferState::Spent | PairingOfferState::Locked | PairingOfferState::Expired => 0,
        }
    }

    /// Whether a valid message one already consumed this offer.
    #[must_use]
    pub const fn is_spent(&self) -> bool {
        matches!(self.state, PairingOfferState::Spent)
    }

    /// Authenticate one proposed device and hold message two until exact PC presence is supplied.
    ///
    /// Every failed Noise message consumes one of five attempts. A successful message consumes the QR secret at
    /// once and returns a [`PendingPairing`] that cannot produce a channel without a matching [`PcPresence`].
    ///
    /// # Errors
    ///
    /// [`CryptoError::PairingExpired`], [`CryptoError::PairingLocked`], or [`CryptoError::PairingSpent`] for a
    /// closed offer. Other cryptographic errors count toward the five-attempt limit.
    pub fn receive(
        &mut self,
        local: &StaticKeypair,
        first: &EncryptedRecord,
    ) -> Result<PendingPairing, CryptoError> {
        if Instant::now() >= self.expires_at {
            self.state = PairingOfferState::Expired;
            return Err(CryptoError::PairingExpired);
        }
        match self.state {
            PairingOfferState::Spent => return Err(CryptoError::PairingSpent),
            PairingOfferState::Locked => return Err(CryptoError::PairingLocked),
            PairingOfferState::Expired => return Err(CryptoError::PairingExpired),
            PairingOfferState::Active(_) => {}
        }

        let attempted = (|| {
            let PairingOfferState::Active(secret) = &self.state else {
                return Err(CryptoError::PairingSpent);
            };
            let mut state = Builder::new(noise_params(PAIRING_PATTERN)?)
                .local_private_key(&local.private)?
                .psk(1, &secret.0)?
                .prologue(PAIRING_PROLOGUE)?
                .build_responder()?;
            let initiator_payload = read_handshake(&mut state, first)?;
            let remote_public = remote_public(&state)?;
            Ok((state, remote_public, initiator_payload))
        })();

        match attempted {
            Ok((state, remote_public, initiator_payload)) => {
                self.state = PairingOfferState::Spent;
                Ok(PendingPairing {
                    state,
                    attempt_id: self.attempt_id,
                    remote_public,
                    initiator_payload,
                })
            }
            Err(error) => {
                self.failed_attempts = self.failed_attempts.saturating_add(1);
                if self.failed_attempts >= PAIRING_MAX_ATTEMPTS {
                    self.state = PairingOfferState::Locked;
                }
                Err(error)
            }
        }
    }

    #[cfg(test)]
    fn expire_for_test(&mut self) {
        self.expires_at = Instant::now();
    }
}

/// A PSK-authenticated proposal waiting for the operator at the PC.
///
/// It implements no diagnostic formatting because the first handshake payload contains device-controlled pairing
/// metadata. Dropping it denies the pairing and emits no Noise response.
pub struct PendingPairing {
    state: HandshakeState,
    attempt_id: [u8; 16],
    remote_public: PublicKey,
    initiator_payload: Vec<u8>,
}

impl PendingPairing {
    /// Static X25519 identity authenticated by the QR PSK.
    #[must_use]
    pub const fn remote_public_key(&self) -> PublicKey {
        self.remote_public
    }

    /// Validate display labels and bind them to this exact attempt and static key.
    ///
    /// # Errors
    ///
    /// [`CryptoError::InvalidPairingIdentity`] when a label cannot safely enter the presence prompt.
    pub fn identity(
        &self,
        name: impl AsRef<str>,
        platform: impl AsRef<str>,
    ) -> Result<PairingIdentity, CryptoError> {
        PairingIdentity::new(
            self.attempt_id,
            self.remote_public.to_bytes(),
            name,
            platform,
        )
        .map_err(|_| CryptoError::InvalidPairingIdentity)
    }

    /// Build the exact local presence request this pending handshake can consume.
    ///
    /// # Errors
    ///
    /// [`CryptoError::PairingIdentityMismatch`] if the identity belongs to another offer or static key.
    pub fn approval_request(
        &self,
        identity: &PairingIdentity,
    ) -> Result<GrantRequest, CryptoError> {
        if identity.attempt_id() != self.attempt_id
            || identity.static_key() != self.remote_public.to_bytes()
        {
            return Err(CryptoError::PairingIdentityMismatch);
        }
        Ok(GrantRequest::PairDevice {
            identity: identity.clone(),
        })
    }

    /// Borrow the PSK-authenticated proposal payload for the local pairing UI.
    #[must_use]
    pub fn initiator_payload(&self) -> &[u8] {
        &self.initiator_payload
    }

    /// Spend an exact fresh PC presence witness and emit Noise message two.
    ///
    /// The paired [`DeviceId`] is minted only after the witness matches. No remote input can mint an authorized
    /// device or obtain a transport channel before this call succeeds.
    ///
    /// # Errors
    ///
    /// [`CryptoError::PairingIdentityMismatch`] for another proposal, [`CryptoError::PairingNeedsPcApproval`] for
    /// a stale or mismatched witness, or [`CryptoError::Noise`] if message two cannot be written.
    pub fn approve(
        mut self,
        identity: &PairingIdentity,
        witness: &PcPresence,
        response_payload: &[u8],
    ) -> Result<ApprovedPairing, CryptoError> {
        let request = self.approval_request(identity)?;
        witness
            .check(&request)
            .map_err(|_| CryptoError::PairingNeedsPcApproval)?;
        let reply = write_handshake(&mut self.state, response_payload)?;
        let transport = self.state.into_transport_mode()?;
        Ok(ApprovedPairing {
            channel: Channel::new(transport),
            reply,
            device_id: DeviceId::now(),
            remote_public: self.remote_public,
            initiator_payload: self.initiator_payload,
        })
    }
}

/// A pairing that passed exact local presence and may now be persisted by the assembly layer.
pub struct ApprovedPairing {
    channel: Channel,
    reply: EncryptedRecord,
    device_id: DeviceId,
    remote_public: PublicKey,
    initiator_payload: Vec<u8>,
}

impl ApprovedPairing {
    /// Locally minted identity to use for the device record and scope ledger.
    #[must_use]
    pub const fn device_id(&self) -> DeviceId {
        self.device_id
    }

    /// Pinned remote static key to persist with the device record.
    #[must_use]
    pub const fn remote_public_key(&self) -> PublicKey {
        self.remote_public
    }

    /// Noise message two to return to the approved initiator.
    #[must_use]
    pub const fn reply(&self) -> &EncryptedRecord {
        &self.reply
    }

    /// Authenticated proposal metadata received in message one.
    #[must_use]
    pub fn initiator_payload(&self) -> &[u8] {
        &self.initiator_payload
    }

    /// Consume the approval result and take the established transport channel.
    #[must_use]
    pub fn into_channel(self) -> Channel {
        self.channel
    }
}

/// Physical path used by a freshly negotiated session link.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum LinkKind {
    /// Browser to a listener on the same PC.
    Loopback = 1,
    /// Direct local-area network link.
    Lan = 2,
    /// Direct WebRTC peer link.
    PeerToPeer = 3,
    /// TLS relay carrying only Noise ciphertext.
    Relay = 4,
}

/// Immutable bytes bound into a session handshake.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionBinding {
    link_kind: LinkKind,
    relay_origin: Box<str>,
    peer_id: [u8; 32],
}

impl SessionBinding {
    /// Bind a non-relay link kind and peer identity.
    ///
    /// # Errors
    ///
    /// [`CryptoError::InvalidBinding`] when `link_kind` is [`LinkKind::Relay`].
    pub fn direct(link_kind: LinkKind, peer_id: [u8; 32]) -> Result<Self, CryptoError> {
        if link_kind == LinkKind::Relay {
            return Err(CryptoError::InvalidBinding);
        }
        Ok(Self {
            link_kind,
            relay_origin: "".into(),
            peer_id,
        })
    }

    /// Bind a relay link to one explicit HTTPS origin and peer identity.
    ///
    /// # Errors
    ///
    /// [`CryptoError::InvalidBinding`] for a wildcard, non-HTTPS, or path-bearing origin.
    pub fn relay(origin: &str, peer_id: [u8; 32]) -> Result<Self, CryptoError> {
        if !valid_relay_origin(origin) {
            return Err(CryptoError::InvalidBinding);
        }
        Ok(Self {
            link_kind: LinkKind::Relay,
            relay_origin: origin.into(),
            peer_id,
        })
    }

    fn prologue(&self) -> Vec<u8> {
        let mut prologue = Vec::with_capacity(10 + self.relay_origin.len() + self.peer_id.len());
        prologue.extend_from_slice(b"runtrol/1");
        prologue.push(self.link_kind as u8);
        prologue.extend_from_slice(self.relay_origin.as_bytes());
        prologue.extend_from_slice(&self.peer_id);
        prologue
    }
}

/// Initiator half of either the session or pairing handshake.
pub struct InitiatorHandshake {
    state: HandshakeState,
    expected_remote: PublicKey,
}

impl InitiatorHandshake {
    /// Start a fresh mutually authenticated session handshake.
    ///
    /// # Errors
    ///
    /// [`CryptoError::Noise`] when the exact suite cannot be initialized.
    pub fn session(
        local: &StaticKeypair,
        remote: PublicKey,
        binding: &SessionBinding,
    ) -> Result<Self, CryptoError> {
        let prologue = binding.prologue();
        let state = Builder::new(noise_params(SESSION_PATTERN)?)
            .local_private_key(&local.private)?
            .remote_public_key(&remote.0)?
            .prologue(&prologue)?
            .build_initiator()?;
        Ok(Self {
            state,
            expected_remote: remote,
        })
    }

    /// Start a one-time PSK-authenticated pairing handshake.
    ///
    /// # Errors
    ///
    /// [`CryptoError::Noise`] when the exact suite cannot be initialized.
    pub fn pairing(
        local: &StaticKeypair,
        remote: PublicKey,
        secret: &PairingSecret,
    ) -> Result<Self, CryptoError> {
        let state = Builder::new(noise_params(PAIRING_PATTERN)?)
            .local_private_key(&local.private)?
            .remote_public_key(&remote.0)?
            .psk(1, &secret.0)?
            .prologue(PAIRING_PROLOGUE)?
            .build_initiator()?;
        Ok(Self {
            state,
            expected_remote: remote,
        })
    }

    /// Write Noise message one, including the optional 0.5-RTT payload.
    ///
    /// # Errors
    ///
    /// [`CryptoError::Noise`] when the payload does not fit or this state already sent message one.
    pub fn write_first(&mut self, payload: &[u8]) -> Result<EncryptedRecord, CryptoError> {
        write_handshake(&mut self.state, payload)
    }

    /// Authenticate Noise message two and enter transport mode.
    ///
    /// # Errors
    ///
    /// [`CryptoError::Noise`] for an invalid message, or [`CryptoError::RemoteIdentity`] if it does not authenticate
    /// as the pinned responder.
    pub fn finish(mut self, reply: &EncryptedRecord) -> Result<(Channel, Vec<u8>), CryptoError> {
        let payload = read_handshake(&mut self.state, reply)?;
        verify_remote(&self.state, self.expected_remote)?;
        let transport = self.state.into_transport_mode()?;
        Ok((Channel::new(transport), payload))
    }
}

/// Responder half of either the session or pairing handshake.
pub struct ResponderHandshake {
    state: HandshakeState,
    expected_remote: PublicKey,
}

impl ResponderHandshake {
    /// Prepare a fresh session responder pinned to one paired initiator.
    ///
    /// # Errors
    ///
    /// [`CryptoError::Noise`] when the exact suite cannot be initialized.
    pub fn session(
        local: &StaticKeypair,
        expected_remote: PublicKey,
        binding: &SessionBinding,
    ) -> Result<Self, CryptoError> {
        let state = session_responder_state(local, binding)?;
        Ok(Self {
            state,
            expected_remote,
        })
    }

    /// Prepare a pairing responder pinned to the proposed device key and QR secret.
    ///
    /// # Errors
    ///
    /// [`CryptoError::Noise`] when the exact suite cannot be initialized.
    pub fn pairing(
        local: &StaticKeypair,
        expected_remote: PublicKey,
        secret: &PairingSecret,
    ) -> Result<Self, CryptoError> {
        let state = Builder::new(noise_params(PAIRING_PATTERN)?)
            .local_private_key(&local.private)?
            .psk(1, &secret.0)?
            .prologue(PAIRING_PROLOGUE)?
            .build_responder()?;
        Ok(Self {
            state,
            expected_remote,
        })
    }

    /// Authenticate message one, write message two, and enter transport mode.
    ///
    /// # Errors
    ///
    /// [`CryptoError::Noise`] for an invalid message, or [`CryptoError::RemoteIdentity`] if the initiator does not
    /// match the pinned device key.
    pub fn answer(
        mut self,
        first: &EncryptedRecord,
        payload: &[u8],
    ) -> Result<(Channel, EncryptedRecord, Vec<u8>), CryptoError> {
        let first_payload = read_handshake(&mut self.state, first)?;
        verify_remote(&self.state, self.expected_remote)?;
        let reply = write_handshake(&mut self.state, payload)?;
        let transport = self.state.into_transport_mode()?;
        Ok((Channel::new(transport), reply, first_payload))
    }
}

/// A session responder before the authenticated static key is mapped to a paired device.
///
/// Noise IK hides the initiator static key inside message one. This state may decrypt that message and expose only
/// the authenticated public key. It cannot produce message two or a transport channel until [`PendingSession::approve`]
/// pins that key to one previously paired identity.
pub struct SessionResponder {
    state: HandshakeState,
}

impl SessionResponder {
    /// Prepare a responder for one transport-bound session without guessing which paired device connected.
    ///
    /// # Errors
    ///
    /// [`CryptoError::Noise`] when the exact suite cannot be initialized.
    pub fn new(local: &StaticKeypair, binding: &SessionBinding) -> Result<Self, CryptoError> {
        Ok(Self {
            state: session_responder_state(local, binding)?,
        })
    }

    /// Authenticate message one enough to recover its static key, but do not establish a usable channel yet.
    ///
    /// # Errors
    ///
    /// [`CryptoError::Noise`] for an invalid message or [`CryptoError::RemoteIdentity`] when Noise did not reveal an
    /// authenticated initiator static key.
    pub fn receive(mut self, first: &EncryptedRecord) -> Result<PendingSession, CryptoError> {
        let initiator_payload = read_handshake(&mut self.state, first)?;
        let remote_public = remote_public(&self.state)?;
        Ok(PendingSession {
            state: self.state,
            remote_public,
            initiator_payload,
        })
    }
}

/// An authenticated Noise initiator that is not authorized as a stored paired device yet.
pub struct PendingSession {
    state: HandshakeState,
    remote_public: PublicKey,
    initiator_payload: Vec<u8>,
}

impl PendingSession {
    /// The authenticated static key used to find one restored paired device.
    #[must_use]
    pub const fn remote_public_key(&self) -> PublicKey {
        self.remote_public
    }

    /// Pin the authenticated key to the selected paired identity and enter transport mode.
    ///
    /// # Errors
    ///
    /// [`CryptoError::RemoteIdentity`] if `expected` is not the key authenticated by message one, or
    /// [`CryptoError::Noise`] if message two cannot be written.
    pub fn approve(
        mut self,
        expected: PublicKey,
        payload: &[u8],
    ) -> Result<(Channel, EncryptedRecord, Vec<u8>), CryptoError> {
        verify_remote(&self.state, expected)?;
        let reply = write_handshake(&mut self.state, payload)?;
        let transport = self.state.into_transport_mode()?;
        Ok((Channel::new(transport), reply, self.initiator_payload))
    }
}

/// An authenticated bidirectional Noise transport with bounded frame reassembly.
pub struct Channel {
    transport: TransportState,
    partial: Vec<u8>,
    expected_total: Option<usize>,
    outgoing_messages: u64,
    rekey_deadline: Instant,
}

impl Channel {
    fn new(transport: TransportState) -> Self {
        Self {
            transport,
            partial: Vec::new(),
            expected_total: None,
            outgoing_messages: 0,
            rekey_deadline: Instant::now() + REKEY_AFTER_TIME,
        }
    }

    /// Encrypt one bounded application frame, chunking it across Noise messages when necessary.
    ///
    /// A rekey control record is inserted first after 2^24 messages or 15 minutes, whichever comes first.
    ///
    /// # Errors
    ///
    /// [`CryptoError::FrameTooLarge`] above [`MAX_TRANSPORT_FRAME`], or [`CryptoError::Noise`] if the cipher state
    /// refuses a record.
    pub fn seal_frame(&mut self, frame: &[u8]) -> Result<Vec<EncryptedRecord>, CryptoError> {
        if frame.len() > MAX_TRANSPORT_FRAME {
            return Err(CryptoError::FrameTooLarge {
                length: frame.len(),
                max: MAX_TRANSPORT_FRAME,
            });
        }

        let mut records = Vec::new();
        if self.outgoing_messages >= REKEY_AFTER_MESSAGES || Instant::now() >= self.rekey_deadline {
            records.push(self.request_rekey()?);
        }

        let total = u32::try_from(frame.len()).map_err(|_| CryptoError::FrameTooLarge {
            length: frame.len(),
            max: MAX_TRANSPORT_FRAME,
        })?;
        let chunk_capacity = MAX_NOISE_PLAINTEXT - CHUNK_HEADER_LEN;
        let mut offset = 0_usize;
        loop {
            let end = offset.saturating_add(chunk_capacity).min(frame.len());
            let chunk = frame.get(offset..end).ok_or(CryptoError::InvalidRecord)?;
            let final_chunk = end == frame.len();
            let offset_u32 = u32::try_from(offset).map_err(|_| CryptoError::InvalidRecord)?;
            let mut plaintext = Vec::with_capacity(CHUNK_HEADER_LEN + chunk.len());
            plaintext.push(RECORD_FRAME);
            plaintext.extend_from_slice(&total.to_le_bytes());
            plaintext.extend_from_slice(&offset_u32.to_le_bytes());
            plaintext.push(u8::from(final_chunk));
            plaintext.extend_from_slice(chunk);
            records.push(self.seal_record(&plaintext)?);
            if final_chunk {
                break;
            }
            offset = end;
        }
        Ok(records)
    }

    /// Decrypt one ordered Noise record and return a complete frame when its final chunk arrives.
    ///
    /// # Errors
    ///
    /// [`CryptoError::Noise`] on failed authentication, or [`CryptoError::InvalidRecord`] for malformed,
    /// out-of-order, inconsistent, or oversized chunk metadata.
    pub fn open_record(
        &mut self,
        record: &EncryptedRecord,
    ) -> Result<Option<Vec<u8>>, CryptoError> {
        let mut plaintext = vec![0_u8; MAX_NOISE_PLAINTEXT];
        let written = self
            .transport
            .read_message(record.as_ciphertext(), &mut plaintext)?;
        plaintext.truncate(written);
        let Some((&kind, body)) = plaintext.split_first() else {
            return Err(CryptoError::InvalidRecord);
        };
        if kind == RECORD_REKEY {
            if !body.is_empty() {
                return Err(CryptoError::InvalidRecord);
            }
            self.transport.rekey_incoming();
            return Ok(None);
        }
        if kind != RECORD_FRAME || body.len() < CHUNK_HEADER_LEN - 1 {
            return Err(CryptoError::InvalidRecord);
        }

        let (total_bytes, rest) = body.split_at(4);
        let (offset_bytes, rest) = rest.split_at(4);
        let Some((&final_byte, chunk)) = rest.split_first() else {
            return Err(CryptoError::InvalidRecord);
        };
        let total = usize::try_from(u32::from_le_bytes(
            total_bytes
                .try_into()
                .map_err(|_| CryptoError::InvalidRecord)?,
        ))
        .map_err(|_| CryptoError::InvalidRecord)?;
        let offset = usize::try_from(u32::from_le_bytes(
            offset_bytes
                .try_into()
                .map_err(|_| CryptoError::InvalidRecord)?,
        ))
        .map_err(|_| CryptoError::InvalidRecord)?;
        if total > MAX_TRANSPORT_FRAME || final_byte > 1 || offset != self.partial.len() {
            return Err(CryptoError::InvalidRecord);
        }
        match self.expected_total {
            None if offset == 0 => {
                self.expected_total = Some(total);
                self.partial
                    .try_reserve(total)
                    .map_err(|_| CryptoError::InvalidRecord)?;
            }
            Some(expected) if expected == total => {}
            _ => return Err(CryptoError::InvalidRecord),
        }
        let next_length = self
            .partial
            .len()
            .checked_add(chunk.len())
            .ok_or(CryptoError::InvalidRecord)?;
        if next_length > total || (final_byte == 1) != (next_length == total) {
            return Err(CryptoError::InvalidRecord);
        }
        self.partial.extend_from_slice(chunk);
        if final_byte == 0 {
            return Ok(None);
        }
        self.expected_total = None;
        Ok(Some(std::mem::take(&mut self.partial)))
    }

    /// Emit an authenticated control record and rotate the outgoing Noise key.
    ///
    /// The peer rotates its matching incoming key only after authenticating this record under the old key.
    ///
    /// # Errors
    ///
    /// [`CryptoError::Noise`] if the current cipher state refuses the control record.
    pub fn request_rekey(&mut self) -> Result<EncryptedRecord, CryptoError> {
        let record = self.seal_record(&[RECORD_REKEY])?;
        self.transport.rekey_outgoing();
        self.outgoing_messages = 0;
        self.rekey_deadline = Instant::now() + REKEY_AFTER_TIME;
        Ok(record)
    }

    fn seal_record(&mut self, plaintext: &[u8]) -> Result<EncryptedRecord, CryptoError> {
        let mut ciphertext = vec![0_u8; NOISE_MESSAGE_MAX];
        let written = self.transport.write_message(plaintext, &mut ciphertext)?;
        ciphertext.truncate(written);
        self.outgoing_messages = self.outgoing_messages.saturating_add(1);
        EncryptedRecord::from_ciphertext(ciphertext)
    }
}

/// Ciphertext safe to expose to an untrusted relay.
///
/// Its diagnostic form contains only its public length.
#[derive(Clone, PartialEq, Eq)]
pub struct EncryptedRecord(Vec<u8>);

impl EncryptedRecord {
    /// Validate a Noise ciphertext record received from the wire.
    ///
    /// # Errors
    ///
    /// [`CryptoError::InvalidRecord`] when it cannot contain an authentication tag or exceeds Noise's limit.
    pub fn from_ciphertext(ciphertext: Vec<u8>) -> Result<Self, CryptoError> {
        if !(NOISE_TAG_LEN..=NOISE_MESSAGE_MAX).contains(&ciphertext.len()) {
            return Err(CryptoError::InvalidRecord);
        }
        Ok(Self(ciphertext))
    }

    /// Borrow the opaque ciphertext for length-prefix framing.
    #[must_use]
    pub fn as_ciphertext(&self) -> &[u8] {
        &self.0
    }

    /// Append the protocol's `varint(length) || ciphertext` wire representation.
    ///
    /// # Errors
    ///
    /// [`CryptoError::InvalidRecord`] only if this platform cannot represent the seven-bit groups used by the
    /// fixed Noise length limit.
    pub fn append_wire(&self, output: &mut Vec<u8>) -> Result<(), CryptoError> {
        let mut remaining = self.0.len();
        loop {
            let low = u8::try_from(remaining & 0x7f).map_err(|_| CryptoError::InvalidRecord)?;
            remaining >>= 7;
            output.push(if remaining == 0 { low } else { low | 0x80 });
            if remaining == 0 {
                break;
            }
        }
        output.extend_from_slice(&self.0);
        Ok(())
    }

    /// Decode one canonical `varint(length) || ciphertext` record.
    ///
    /// The returned byte count lets a stream decoder leave any following record untouched.
    ///
    /// # Errors
    ///
    /// [`CryptoError::InvalidRecord`] for truncated, overlong, non-canonical, or out-of-range input.
    pub fn decode_wire(input: &[u8]) -> Result<(Self, usize), CryptoError> {
        let mut length = 0_usize;
        let mut prefix = 0_usize;
        loop {
            let byte = *input.get(prefix).ok_or(CryptoError::InvalidRecord)?;
            if prefix >= 3 {
                return Err(CryptoError::InvalidRecord);
            }
            let shift = prefix.checked_mul(7).ok_or(CryptoError::InvalidRecord)?;
            length |= usize::from(byte & 0x7f)
                .checked_shl(u32::try_from(shift).map_err(|_| CryptoError::InvalidRecord)?)
                .ok_or(CryptoError::InvalidRecord)?;
            prefix += 1;
            if byte & 0x80 == 0 {
                if prefix > 1 && byte == 0 {
                    return Err(CryptoError::InvalidRecord);
                }
                break;
            }
        }
        if length > NOISE_MESSAGE_MAX {
            return Err(CryptoError::InvalidRecord);
        }
        let consumed = prefix
            .checked_add(length)
            .ok_or(CryptoError::InvalidRecord)?;
        let ciphertext = input
            .get(prefix..consumed)
            .ok_or(CryptoError::InvalidRecord)?
            .to_vec();
        Ok((Self::from_ciphertext(ciphertext)?, consumed))
    }
}

impl fmt::Debug for EncryptedRecord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EncryptedRecord")
            .field("ciphertext_len", &self.0.len())
            .finish()
    }
}

fn write_handshake(
    state: &mut HandshakeState,
    payload: &[u8],
) -> Result<EncryptedRecord, CryptoError> {
    let mut output = vec![0_u8; NOISE_MESSAGE_MAX];
    let written = state.write_message(payload, &mut output)?;
    output.truncate(written);
    EncryptedRecord::from_ciphertext(output)
}

fn read_handshake(
    state: &mut HandshakeState,
    message: &EncryptedRecord,
) -> Result<Vec<u8>, CryptoError> {
    let mut payload = vec![0_u8; NOISE_MESSAGE_MAX];
    let written = state.read_message(message.as_ciphertext(), &mut payload)?;
    payload.truncate(written);
    Ok(payload)
}

fn session_responder_state(
    local: &StaticKeypair,
    binding: &SessionBinding,
) -> Result<HandshakeState, CryptoError> {
    let prologue = binding.prologue();
    Ok(Builder::new(noise_params(SESSION_PATTERN)?)
        .local_private_key(&local.private)?
        .prologue(&prologue)?
        .build_responder()?)
}

fn verify_remote(state: &HandshakeState, expected: PublicKey) -> Result<(), CryptoError> {
    let Some(remote) = state.get_remote_static() else {
        return Err(CryptoError::RemoteIdentity);
    };
    let mismatch = remote.iter().zip(expected.0).fold(
        remote.len() ^ expected.0.len(),
        |difference, (left, right)| difference | usize::from(*left ^ right),
    );
    if mismatch == 0 {
        Ok(())
    } else {
        Err(CryptoError::RemoteIdentity)
    }
}

fn remote_public(state: &HandshakeState) -> Result<PublicKey, CryptoError> {
    let remote = state
        .get_remote_static()
        .ok_or(CryptoError::RemoteIdentity)?;
    let bytes = remote
        .try_into()
        .map_err(|_| CryptoError::InvalidKeyLength)?;
    Ok(PublicKey(bytes))
}

fn noise_params(pattern: &str) -> Result<NoiseParams, CryptoError> {
    pattern.parse().map_err(|_| CryptoError::Noise)
}

fn valid_relay_origin(origin: &str) -> bool {
    let Some(authority) = origin.strip_prefix("https://") else {
        return false;
    };
    !authority.is_empty()
        && authority.is_ascii()
        && !authority.contains(['/', '?', '#', '@', '*'])
        && authority.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b':' | b'[' | b']')
        })
}

/// A fail-closed cryptographic boundary error.
///
/// Noise's detailed internal error is deliberately not retained. The caller needs to close the link, and logging
/// cipher state detail would make a security boundary noisier without making recovery possible.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum CryptoError {
    /// A Noise initialization, authentication, size, or state transition failed.
    #[error("Noise channel operation failed")]
    Noise,

    /// A generated X25519 key was not the protocol's fixed size.
    #[error("X25519 key had an invalid length")]
    InvalidKeyLength,

    /// HKDF rejected the fixed pairing output size.
    #[error("pairing secret derivation failed")]
    KeyDerivation,

    /// The operating system would not supply fresh pairing material.
    #[error("cannot generate fresh pairing material")]
    RandomUnavailable,

    /// The pairing QR expiry cannot be represented safely.
    #[error("cannot establish the pairing expiry clock")]
    ClockUnavailable,

    /// The 120-second pairing window has elapsed.
    #[error("pairing offer expired")]
    PairingExpired,

    /// Five invalid first messages destroyed the offer.
    #[error("pairing offer is locked after five failed attempts")]
    PairingLocked,

    /// A valid first message already consumed the one-time QR secret.
    #[error("pairing offer was already spent")]
    PairingSpent,

    /// Untrusted display metadata cannot safely enter the local prompt.
    #[error("pairing identity display metadata is invalid")]
    InvalidPairingIdentity,

    /// The displayed identity belongs to another pairing attempt or key.
    #[error("pairing identity does not match the pending Noise peer")]
    PairingIdentityMismatch,

    /// Noise message two cannot be emitted without exact fresh PC presence.
    #[error("pairing needs approval at the PC for this exact device")]
    PairingNeedsPcApproval,

    /// A relay or direct-link binding was malformed.
    #[error("session link binding is invalid")]
    InvalidBinding,

    /// The authenticated static key did not match the paired peer.
    #[error("Noise peer does not match the pinned identity")]
    RemoteIdentity,

    /// An encrypted record's public structure was invalid.
    #[error("encrypted transport record is invalid")]
    InvalidRecord,

    /// The caller attempted to buffer more than the transport contract allows.
    #[error("transport frame length {length} exceeds the {max}-byte limit")]
    FrameTooLarge {
        /// Offered plaintext bytes.
        length: usize,
        /// Maximum accepted plaintext bytes.
        max: usize,
    },
}

impl From<snow::Error> for CryptoError {
    fn from(_: snow::Error) -> Self {
        Self::Noise
    }
}

#[cfg(test)]
mod pairing_lifecycle_tests {
    use super::*;

    #[test]
    fn an_expired_offer_destroys_its_secret_before_noise() {
        let pc = StaticKeypair::generate().expect("pc key");
        let (mut offer, _) = PairingOffer::generate().expect("offer");
        offer.expire_for_test();
        let shaped = EncryptedRecord::from_ciphertext(vec![0_u8; NOISE_TAG_LEN])
            .expect("minimum record shape");
        assert!(matches!(
            offer.receive(&pc, &shaped),
            Err(CryptoError::PairingExpired)
        ));
        assert_eq!(offer.remaining_attempts(), 0);
    }

    #[test]
    fn protected_private_bytes_rebuild_one_stable_public_identity() {
        let private = [0x5A; 32];
        let first = StaticKeypair::from_private(&private).expect("first identity");
        let second = StaticKeypair::from_private(&private).expect("second identity");
        assert_eq!(first.public_key(), second.public_key());
        assert_ne!(first.public_key().to_bytes(), [0; 32]);
    }

    #[test]
    fn a_session_responder_exposes_only_the_authenticated_key_before_approval() {
        let pc = StaticKeypair::generate().expect("pc key");
        let phone = StaticKeypair::generate().expect("phone key");
        let binding = SessionBinding::direct(LinkKind::Loopback, pc.public_key().to_bytes())
            .expect("loopback binding");
        let mut initiator =
            InitiatorHandshake::session(&phone, pc.public_key(), &binding).expect("initiator");
        let first = initiator.write_first(&[]).expect("message one");

        let pending = SessionResponder::new(&pc, &binding)
            .expect("responder")
            .receive(&first)
            .expect("authenticated message one");

        assert_eq!(pending.remote_public_key(), phone.public_key());
        let (mut responder, reply, early) = pending
            .approve(phone.public_key(), b"ready")
            .expect("paired identity approved");
        assert!(early.is_empty());
        let (mut phone_channel, response) = initiator.finish(&reply).expect("message two");
        assert_eq!(response, b"ready");
        let records = phone_channel
            .seal_frame(b"same core")
            .expect("sealed frame");
        let opened = records
            .into_iter()
            .find_map(|record| responder.open_record(&record).expect("opened record"));
        assert_eq!(opened.as_deref(), Some(b"same core".as_slice()));
    }

    #[test]
    fn a_session_responder_cannot_approve_a_different_paired_key() {
        let pc = StaticKeypair::generate().expect("pc key");
        let phone = StaticKeypair::generate().expect("phone key");
        let stranger = StaticKeypair::generate().expect("stranger key");
        let binding = SessionBinding::direct(LinkKind::Loopback, pc.public_key().to_bytes())
            .expect("loopback binding");
        let mut initiator =
            InitiatorHandshake::session(&phone, pc.public_key(), &binding).expect("initiator");
        let first = initiator.write_first(&[]).expect("message one");
        let pending = SessionResponder::new(&pc, &binding)
            .expect("responder")
            .receive(&first)
            .expect("authenticated message one");

        assert!(matches!(
            pending.approve(stranger.public_key(), &[]),
            Err(CryptoError::RemoteIdentity)
        ));
    }
}
