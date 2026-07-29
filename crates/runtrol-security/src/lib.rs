//! Permission scopes as types. What a remote caller cannot be granted, it cannot name.
//!
//! Skeleton. This crate's own surface arrives in its own step, bottom up.

// The dependency edges of this crate are already declared and already enforced by
// `tests/audit/dependencyDirection.rs`. Until this crate has code that names them, these lines are
// what make the declaration real: an unreferenced dependency is one `cargo shear` reports as dead,
// and a dependency table that lists what nothing uses is the debt this repository refuses to carry.
use runtrol_provider as _;
