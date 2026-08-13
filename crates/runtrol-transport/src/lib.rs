//! Phone-facing transports and the security boundary in front of them.
//!
//! The local command surface does not use this crate. It already has an OS-authenticated endpoint. This crate
//! exists for the browser-reachable plane, where loopback is not authentication and every request is hostile until
//! its Host, Origin, browser metadata, protocol marker, and device credential have all been checked.

pub mod crypto;
pub mod egress;
pub mod http;
pub mod relay;
pub mod websocket;

pub use crypto::{
    ApprovedPairing, Channel, CryptoError, EncryptedRecord, InitiatorHandshake, LinkKind,
    MAX_ENCRYPTED_RECORD_WIRE, MAX_NOISE_PLAINTEXT, MAX_TRANSPORT_FRAME, PairingInvitation,
    PairingOffer, PairingSecret, PendingPairing, PendingSession, PublicKey, ResponderHandshake,
    SessionBinding, SessionResponder, StaticKeypair,
};
pub use egress::{ApprovedDestination, EgressError, EgressPolicy};
pub use http::{
    AccessToken, AdmittedNoiseUpgrade, CredentialFingerprint, DeviceCredential, NOISE_LINK_PATH,
    NOISE_LINK_PROTOCOL, PhoneBody, PhoneHttp, PhoneHttpError, response,
};
pub use hyper::StatusCode;
pub use relay::{
    RelayCredential, RelayEndpoint, RelayEnvelope, RelayError, RelayOrigin, RelayPairingMaterial,
    RelayRoute, RelaySeed, RelaySocket, ResolvedRelay,
};
pub use websocket::{NoiseUpgrade, NoiseWebSocket, PendingNoiseWebSocket, WebSocketLinkError};
