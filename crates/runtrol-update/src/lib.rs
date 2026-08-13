//! Provider update channel verdicts and rollback version selection.
//!
//! This crate opens no socket and starts no process. It turns independently observed package ownership into a
//! closed channel verdict and chooses rollback candidates by semantic version order.

mod channel;
mod package;
mod transaction;

pub use channel::{
    ChannelId, ChannelObservation, ChannelVerdict, ConfirmedChannel, RollbackVerdict,
    confirm_channel, select_rollback,
};
pub use package::{MAX_PACKAGE_JSON_BYTES, NpmOwnership, OwnershipError, discover_npm_ownership};
pub use transaction::{TransactionError, UpdateAction, UpdateFinish, UpdateTransaction};
