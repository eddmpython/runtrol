//! Phone-facing transports and the security boundary in front of them.
//!
//! The local command surface does not use this crate. It already has an OS-authenticated endpoint. This crate
//! exists for the browser-reachable plane, where loopback is not authentication and every request is hostile until
//! its Host, Origin, browser metadata, protocol marker, and device credential have all been checked.

pub mod crypto;
pub mod egress;
pub mod http;

pub use crypto::{
    ApprovedPairing, Channel, CryptoError, EncryptedRecord, InitiatorHandshake, LinkKind,
    MAX_NOISE_PLAINTEXT, MAX_TRANSPORT_FRAME, PairingInvitation, PairingOffer, PairingSecret,
    PendingPairing, PublicKey, ResponderHandshake, SessionBinding, StaticKeypair,
};
pub use egress::{ApprovedDestination, EgressError, EgressPolicy};
pub use http::{AccessToken, DeviceCredential, PhoneBody, PhoneHttp, PhoneHttpError, response};
pub use hyper::StatusCode;
