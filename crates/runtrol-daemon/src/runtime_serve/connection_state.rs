//! Connection-local protocol, authority, and watch state.

use std::sync::Arc;

use runtrol_runtime_protocol::{ProviderList, ProviderUsageList, RuntimeSessionId};
use runtrol_store::EnrollmentKey;
use tokio::sync::watch;

use crate::runtime_auth::{AuthorizedIntegration, ClientContext};
use crate::runtime_terminal::TerminalView;

pub(super) enum PublicAuthority {
    Anonymous,
    Pending(EnrollmentKey),
    Authorized(AuthorizedIntegration),
}

pub(super) enum PublicState {
    Fresh {
        challenge: runtrol_runtime_protocol::ServerChallenge,
    },
    Negotiated {
        context: ClientContext,
        authority: PublicAuthority,
    },
    Ready {
        context: ClientContext,
        authority: PublicAuthority,
    },
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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RelayOutcome {
    CloseConnection,
    ResumeRequests,
}
