//! Permission scopes as types. What a remote caller cannot be granted, it cannot name.
//!
//! runtrol exists so a phone can drive a coding agent on the operator's own machine, across the
//! internet. That is close to the most dangerous thing a hobbyist daemon can offer, and the security
//! posture answers it with five invariants. This crate is where two of them stop being sentences:
//!
//! - **An authenticated remote request is still denied by default.** A phone may shrink its authority
//!   and never grow it. [`GrantLedger::holds`] answers `false` for anything it has not been told, and
//!   the only function that adds authority takes a [`PcPresence`].
//! - **Some things are structurally ungrantable.** Pairing a device, writing configuration, answering
//!   approvals automatically, and bypassing a provider's permission prompt are [`LocalScope`] values.
//!   There is no conversion from [`LocalScope`] to [`DeviceScope`], so handing one to a grant does not
//!   fail a check, it fails to compile.
//!
//! # Why this is a crate and not a module
//!
//! [`PcPresence`] is worth something only if exactly one place can construct it. A module inside the
//! kernel would let all of the kernel mint a witness. A crate boundary lets one crate mint it, and
//! lets the dependency gate assert that whatever handles remote frames cannot reach the minting code.
//!
//! # Layout
//!
//! - [`caller`] who is asking, established by where a request arrived rather than by what it says
//! - [`scope`] the two walls, and every permission name in the product
//! - [`presence`] the unforgeable proof that somebody was at the machine for this decision
//! - [`grant`] who holds what, and the two authorities that never need holding
//! - [`workspace`] where work may happen, and the directories no configuration may open
//! - [`id`] the identifiers this crate owns
//! - [`error`] why authority was refused

pub mod caller;
pub mod error;
pub mod grant;
pub mod id;
pub mod presence;
pub mod scope;
pub mod workspace;

pub use caller::Caller;
pub use error::SecurityError;
pub use grant::{GrantLedger, LocalAuthorization};
pub use id::{DeviceId, WorkspaceRootId};
pub use presence::{GrantRequest, LocalConsole, PairingIdentity, PcPresence, PresenceChallenge};
pub use scope::{DeviceScope, LocalScope};
pub use workspace::{DeniedPath, DenyList, WorkspaceRoot};
