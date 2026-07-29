//! The supervisor kernel. Session lifetime, tiers, and the event pipeline.
//!
//! Skeleton. This crate's own surface arrives in its own step, bottom up.

// The dependency edges of this crate are already declared and already enforced by
// `tests/audit/dependencyDirection.rs`. Until this crate has code that names them, these lines are
// what make the declaration real: an unreferenced dependency is one `cargo shear` reports as dead,
// and a dependency table that lists what nothing uses is the debt this repository refuses to carry.
use runtrol_childproc as _;
use runtrol_provider as _;
use runtrol_security as _;
use runtrol_store as _;
