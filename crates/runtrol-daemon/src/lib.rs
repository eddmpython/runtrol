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
//! - [`serve`] one owner of the sessions, and a task for every connection beside it

pub mod compose;
pub mod dispatch;
pub mod scope;
pub mod serve;

pub use compose::{ComposeError, Composed};
pub use scope::{Needed, WallRefusal, allowed, needed};
pub use serve::{ServeError, serve};

/// Where a daemon for this home listens.
///
/// Asked for rather than worked out, so that the two ends of the local connection cannot derive it differently. The
/// daemon decides where it listens; everything else asks. Establishing containment is not part of answering this,
/// which is what lets a command surface ask without becoming something that owns child processes.
///
/// # Errors
///
/// [`ComposeError::Home`] when runtrol's directory cannot be established.
pub fn endpoint(home: Option<&str>) -> Result<String, ComposeError> {
    let home = match home {
        Some(chosen) => runtrol_core::RuntrolHome::open_at(chosen)?,
        None => runtrol_core::RuntrolHome::open()?,
    };
    Ok(home.paths().endpoint().address().to_owned())
}
