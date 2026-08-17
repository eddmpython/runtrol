//! A provider-neutral Agent Client Protocol driver.
//!
//! The executable and its transport arguments come entirely from a provider manifest. This module knows the ACP
//! wire vocabulary and no provider names, so installing another ACP-speaking CLI is a TOML operation rather than
//! a core edit.
//!
//! Only the narrow fields runtrol makes decisions on are decoded. Every content block and every unbound frame
//! stays in its original byte buffer and crosses the provider seam as [`runtrol_provider::Opaque`].

mod agent;
mod catalogue;
mod history;
mod map;
mod provider;
mod wire;

pub use agent::AcpAgent;
pub use provider::AcpProvider;
