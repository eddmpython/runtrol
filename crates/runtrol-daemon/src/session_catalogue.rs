//! One structural managed-session catalogue, adapted to each product surface without reading provider storage.

use std::collections::BTreeMap;

use runtrol_core::{Lifecycle, SessionManager};
use runtrol_provider::{ProviderId, SessionId};

use crate::Composed;

/// One managed session before a wire-specific adapter serializes it.
pub(crate) struct ManagedSession {
    pub(crate) session: SessionId,
    pub(crate) provider: ProviderId,
    pub(crate) native: Option<Box<str>>,
    pub(crate) label: Option<Box<str>>,
    pub(crate) workspace: Box<str>,
    pub(crate) hot: bool,
    pub(crate) lifecycle: ManagedLifecycle,
    pub(crate) looks_stuck: bool,
}

/// Structural lifecycle shared by private and public presentation adapters.
#[derive(Clone, Copy)]
pub(crate) enum ManagedLifecycle {
    Detached,
    Idle,
    Running,
    Failed,
    Closed,
}

impl ManagedLifecycle {
    pub(crate) const fn private_name(self) -> &'static str {
        match self {
            Self::Detached => "detached",
            Self::Idle => "idle",
            Self::Running => "busy",
            Self::Failed => "failed",
            Self::Closed => "closed",
        }
    }

    pub(crate) const fn public(self, hot: bool) -> runtrol_runtime_protocol::LifecycleState {
        use runtrol_runtime_protocol::LifecycleState;

        match self {
            Self::Running => LifecycleState::HotRunning,
            Self::Idle if hot => LifecycleState::HotIdle,
            Self::Failed => LifecycleState::Failed,
            Self::Detached | Self::Idle | Self::Closed => LifecycleState::Cold,
        }
    }
}

impl From<&Lifecycle> for ManagedLifecycle {
    fn from(lifecycle: &Lifecycle) -> Self {
        match lifecycle {
            Lifecycle::Detached | Lifecycle::Starting => Self::Detached,
            Lifecycle::Idle => Self::Idle,
            Lifecycle::Busy { .. } => Self::Running,
            Lifecycle::Failed { .. } => Self::Failed,
            Lifecycle::Closed { .. } => Self::Closed,
        }
    }
}

/// A structural snapshot plus damaged rows that were excluded.
pub(crate) struct Catalogue {
    pub(crate) sessions: Vec<ManagedSession>,
    pub(crate) warnings: Vec<Box<str>>,
}

/// Join Runtime metadata with the one live session owner.
pub(crate) fn read(
    composed: &Composed,
    sessions: &SessionManager,
) -> Result<Catalogue, runtrol_store::StoreError> {
    let stored = composed.store.list_sessions()?;
    let mut joined = BTreeMap::new();
    for (session, row) in stored.sessions {
        if row.archived {
            continue;
        }
        joined.insert(
            session,
            ManagedSession {
                session,
                provider: row.provider,
                native: Some(row.native.as_str().into()),
                label: row.label,
                workspace: row.cwd.as_str().into(),
                hot: false,
                lifecycle: ManagedLifecycle::Detached,
                looks_stuck: false,
            },
        );
    }
    for one in sessions.live_sessions() {
        let label = joined
            .get(&one.session)
            .and_then(|stored: &ManagedSession| stored.label.clone());
        joined.insert(
            one.session,
            ManagedSession {
                session: one.session,
                provider: one.provider,
                native: one.native.map(Into::into),
                label,
                workspace: one.workspace.as_str().into(),
                hot: one.tier.has_a_process(),
                lifecycle: one.state.lifecycle().into(),
                looks_stuck: one.state.looks_stuck(),
            },
        );
    }
    Ok(Catalogue {
        sessions: joined.into_values().collect(),
        warnings: stored
            .unreadable
            .into_iter()
            .map(|(session, error)| {
                format!("stored session {session} is unreadable: {error}").into()
            })
            .collect(),
    })
}
