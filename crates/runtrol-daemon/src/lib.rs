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

pub mod compose;

pub use compose::{ComposeError, Composed};

// Declared in this crate's manifest and enforced by `tests/audit/dependencyDirection.rs`. The listener, the session
// rows, and the scope checks at the boundary arrive in the next step; until they do, these lines are what make the
// declaration real, because `cargo shear` reports a dependency nothing names.
use runtrol_ipc as _;
use runtrol_security as _;
use runtrol_store as _;
