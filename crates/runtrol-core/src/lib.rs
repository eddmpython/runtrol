//! The supervisor kernel. Session lifetime, tiers, and the event pipeline.
//!
//! This crate holds runtrol's own decisions: which sessions exist, which of them have a process
//! attached, and in what order their events reach whoever is watching. It knows the shape of a
//! provider through a trait and never by name, which is why it carries no dependency on the drivers.
//! That one missing dependency edge is what makes "adding a provider does not touch the kernel" a
//! fact a gate can check rather than an intention.
//!
//! # Layout
//!
//! - [`home`] where runtrol keeps its own files, and every path inside that directory
//! - [`events`] the single point a driver's output enters a session, and the bounds it travels under
//! - [`registry`] which providers exist, and the seam that keeps their names out of this crate
//! - [`probe`] what each installed CLI actually is, asked rather than assumed, and remembered
//! - [`session`] the two names a session has, the one place its state may change, and the tiers

pub mod events;
pub mod home;
pub mod probe;
pub mod registry;
pub mod session;

pub use events::{
    CursorRegression, Delivery, FanOut, Published, Reach, ReplayRing, Sequencer, SessionHub,
    SessionView, SubscriberId, Subscription, WatchItem, WatchStart,
};
pub use home::{Endpoint, HomeError, Layout, RuntrolHome};
pub use probe::{
    BinFacts, Flags, LeadingArgFacts, LeadingFileFacts, ProbeCache, ProbeError, locate,
    locate_named, probe, probe_program,
};
pub use registry::{
    KindEntry, KindStatus, KindTable, Origin, Provider, ProviderRegistry, RegistryError,
};
pub use session::{
    AgentLease, AttachError, AttachedSession, CloseReason, ClosingReservation, ClosingSession,
    FailureCode, Identity, Lifecycle, LiveSession, Observed, OpenReservation, Pumped, ReservedOpen,
    SessionError, SessionManager, SessionState, TakenAgent, Tier,
};

// These edges are declared in this crate's manifest and enforced by
// `tests/audit/dependencyDirection.rs`. Until the modules that use them arrive, these lines are what
// make the declaration real: `cargo shear` reports a dependency nothing names, and a dependency table
// listing what nothing uses is the debt this repository refuses to carry.
use runtrol_childproc as _;
use runtrol_security as _;
use runtrol_store as _;
