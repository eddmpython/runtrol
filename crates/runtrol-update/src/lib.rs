//! Provider update channel verdicts and rollback version selection.
//!
//! This crate opens no socket and starts no process. It turns independently observed package ownership into a
//! closed channel verdict and chooses rollback candidates by semantic version order.

mod channel;

pub use channel::{
    ChannelId, ChannelObservation, ChannelVerdict, ConfirmedChannel, RollbackVerdict,
    confirm_channel, select_rollback,
};
