//! The daemon: where every crate that cannot see the others is joined into something that runs.
//!
//! # Why this is a library and not the binary
//!
//! An integration test cannot link a binary. The joining, the dispatch, and the scope checks at the boundary are the
//! highest-risk code in the product, so they live where a test can reach them and the binary contains nothing but the
//! choice of which personality to be.
//!
//! # Layout
//!
//! - [`compose`] establishing containment, finding the home, and reading which providers exist
//! - [`scope`] what each request needs before anybody is allowed to make it
//! - [`dispatch`] one request in, one answer out
//! - [`consult`] wiring one CLI into another as a consultant, through the CLIs' own commands
//! - [`serve`] one owner of the sessions, and a task for every connection beside it

mod account_probe;
mod build_identity;
pub mod compose;
mod consult;
mod crash;
pub mod dispatch;
pub mod generations;
mod growth;
mod integration_admin;
mod isolated_workspace;
mod mission;
mod mission_schedule;
mod pairing_admin;
mod provider_prepare;
mod provider_update;
mod relay;
mod runtime_audit;
mod runtime_auth;
mod runtime_control;
mod runtime_inventory;
mod runtime_native_sessions;
mod runtime_serve;
pub mod scope;
pub mod serve;
mod session_catalogue;

pub use compose::{ComposeError, Composed};
pub use crash::record_panics_at;
pub use generations::{
    GenerationIdentity, GenerationStatus, assemble_superseding, generation_endpoint, status,
};
pub use relay::{RelayIngress, RelayStage, RelayStatus};
pub use scope::{Needed, WallRefusal, allowed, needed};
pub use serve::{
    MAX_BLOCKING_PROVIDER_OPERATIONS, MODEL_PREPARATION_BUDGET_MS, PhoneIngress, PhoneIngressError,
    ServeError, serve, serve_with_phone, serve_with_relay,
};

/// One place for tests in this crate to take the process-wide console before claiming it.
///
/// `LocalConsole` is deliberately once per process, because there is one operator at one machine. Tests in
/// different modules of this crate drive paths that claim it, and `cargo test` runs them in parallel, so without
/// a shared lock whichever one lost the race failed with "the local approval surface is already in use".
///
/// Observed as an intermittent red in CI and reproduced locally: a test passes alone and fails beside its
/// siblings. Serializing here rather than in each module keeps the call sites from drifting onto separate locks,
/// which would serialize nothing.
/// Async-aware on purpose. The async call sites hold this across an await, which a
/// blocking guard would not survive, and this crate disallows the blocking one for exactly that reason.
#[cfg(test)]
pub(crate) fn console_lock() -> &'static tokio::sync::Mutex<()> {
    static CONSOLE: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
    CONSOLE.get_or_init(|| tokio::sync::Mutex::new(()))
}
