//! The built-in drivers. The kind table lives here, and so does every provider proper noun.
//!
//! This is the only place in `crates/` where a CLI may be named. The kernel selects code by `kind` and has
//! nowhere to write a name, which is what makes "adding a provider does not touch the kernel" a fact a gate can
//! check.
//!
//! # Layout
//!
//! - [`framing`] getting bytes to and from a child in the shapes these CLIs speak
//! - [`claude`] the driver for the CLI that runs one process per session, and its measured surface
//! - [`codex`] the driver for the CLI whose sessions share one daemon, and its measured surface
//! - [`consult`] how a driver takes part in cross-consult wiring, as a declared surface
//! - [`kinds`] the kind table and the manifests compiled into this binary
//!
//! # The whole public surface is one function and two tables
//!
//! [`builtin`] hands back what this build ships: the manifest text and the kind table. Whoever composes the build
//! parses the one and reads the other, and neither the kernel nor this crate needs to know about the other's shape.

pub mod acp;
mod catalogue;
pub mod claude;
pub mod codex;
pub mod consult;
pub mod framing;
pub mod kinds;

pub use consult::{ConsultSurface, ConsultTool, McpConsultServer, McpRegistrar};
pub use framing::{FrameError, Incoming, LineError, Lines, Pending, RequestId};
pub use kinds::{DriverContext, DriverKind, KINDS, MANIFESTS, MakeDriver};

/// What this build ships.
///
/// One call, two tables. The manifests are text because the loader owns parsing and there is exactly one parser; the
/// kinds are data because selecting code is this crate's job and naming a provider is nobody else's.
#[must_use]
pub const fn builtin() -> Builtin {
    Builtin {
        manifests: MANIFESTS,
        kinds: KINDS,
    }
}

/// The manifests and kinds compiled into this binary.
#[derive(Clone, Copy, Debug)]
pub struct Builtin {
    /// Manifest text, one per provider this build ships.
    pub manifests: &'static [&'static str],
    /// Every kind this build knows about, served or not.
    pub kinds: &'static [DriverKind],
}
