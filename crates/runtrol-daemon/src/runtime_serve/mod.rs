//! Public Runtime connection serving split by lifecycle responsibility.

mod audit_dispatch;
mod authority;
mod connection;
mod connection_state;
mod dispatch;
mod integration_requests;
mod provider_requests;
mod response;
mod session_control;
mod session_requests;
mod terminal_stream;
mod watch_relay;

pub(crate) use connection::serve_connection;
pub(crate) use provider_requests::{observe_native_activity, reconcile_native_activity};

#[cfg(test)]
#[path = "tests/official_attach.rs"]
mod official_attach_tests;
