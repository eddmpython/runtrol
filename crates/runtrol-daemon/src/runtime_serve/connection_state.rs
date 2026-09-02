//! Connection-local protocol, authority, and watch state.

use std::sync::Arc;

use runtrol_runtime_protocol::{ProviderList, ProviderUsageList, RuntimeSessionId};
use runtrol_store::EnrollmentKey;
use tokio::sync::watch;

use crate::runtime_auth::{AuthorizedIntegration, ClientContext};
use crate::runtime_terminal::TerminalView;
use crate::window_registry::ConnectionToken;

pub(super) enum PublicAuthority {
    Anonymous,
    Pending(EnrollmentKey),
    Authorized(AuthorizedIntegration),
}

pub(super) enum PublicState {
    Fresh {
        challenge: runtrol_runtime_protocol::ServerChallenge,
        token: ConnectionToken,
    },
    Negotiated {
        context: ClientContext,
        authority: PublicAuthority,
        token: ConnectionToken,
    },
    Ready {
        context: ClientContext,
        authority: PublicAuthority,
        token: ConnectionToken,
    },
}

impl PublicState {
    /// The connection this state belongs to, for what the connection registers and takes with it.
    pub(super) const fn token(&self) -> ConnectionToken {
        match self {
            Self::Fresh { token, .. }
            | Self::Negotiated { token, .. }
            | Self::Ready { token, .. } => *token,
        }
    }
}

pub(super) enum Watching {
    Events {
        subscription_id: String,
        session_id: RuntimeSessionId,
        view: Box<runtrol_core::SessionView>,
    },
    SessionIndex {
        subscription_id: String,
        last: runtrol_runtime_protocol::ManagedSessionList,
        authority: AuthorizedIntegration,
    },
    Providers {
        subscription_id: String,
        last: ProviderList,
        updates: watch::Receiver<Arc<ProviderList>>,
        usage: watch::Receiver<Arc<ProviderUsageList>>,
        authority: AuthorizedIntegration,
    },
    Terminal(Box<TerminalView>),
    TerminalIndex {
        subscription_id: String,
        last: runtrol_runtime_protocol::TerminalIndexSnapshot,
        updates: watch::Receiver<u64>,
        authority: AuthorizedIntegration,
    },
    WindowIndex {
        subscription_id: String,
        last: runtrol_runtime_protocol::WindowIndexSnapshot,
        updates: watch::Receiver<u64>,
    },
    WindowReveals {
        subscription_id: String,
        requests: tokio::sync::broadcast::Receiver<crate::window_registry::RevealRequest>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RelayOutcome {
    CloseConnection,
    ResumeRequests,
}
