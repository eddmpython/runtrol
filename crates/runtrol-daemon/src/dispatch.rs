//! One request in, one answer out.
//!
//! # Nothing here panics and nothing here is silent
//!
//! Every request produces an answer, including the ones that cannot be carried out. A dispatcher that returned nothing
//! for a case it did not expect would leave the command surface waiting forever, which looks exactly like a daemon that
//! has stopped and is much harder to diagnose than a refusal.
//!
//! # The greeting comes first, and it is enforced rather than assumed
//!
//! A connection that has not agreed on a wire format is answered with a refusal to every other request. Reading a
//! request from a build that speaks a different format would mean acting on somebody else's meaning, and the failure
//! that produces is a command landing somewhere the operator did not intend.
//!
//! # Where the scope wall goes
//!
//! Here, at the boundary, and not deeper. A request that arrives from somewhere other than this machine has to be
//! refused before it reaches anything that can act, and the place a request arrives is the only place that knows where
//! it came from. The wall itself lives in the security crate, the table of what each request needs lives in
//! [`crate::scope`], and this is where the two are asked.
//!
//! Consulted **before** the request is read for anything else, and before the greeting is answered. A check that ran
//! after some other branch had already acted would be a check on the way out.

use runtrol_core::registry::KindStatus;
use runtrol_core::session::SessionError;
use runtrol_core::{
    ClosingReservation, OpenReservation, SessionManager, SessionView, TakenAgent, Waiting,
};
use runtrol_ipc::wire::{
    ProviderLine, RemoteConnection, RemoteConnectionStage, RemoteConnectionState, Request,
    Response, SessionLine, SessionListing, SessionWaiting, WireError,
};
use runtrol_provider::{
    AbsPath, Agent, AgentCommand, CloseMode, ContentBlock, Disposition, ModelCatalog,
    NativeSessionId, OpenIntent, Provider, ProviderId, SessionId, WallMs,
};
use runtrol_security::Caller;
use runtrol_store::{SessionRow as StoredSession, StoreError};

use crate::compose::Composed;

/// What a request produced.
pub(crate) enum Reply {
    /// One answer, and the connection is free for the next request.
    One(Response),
    /// The caller is now watching a session.
    ///
    /// A separate shape because watching is not a question with an answer: it is the connection changing what it is for,
    /// and a dispatcher that pretended otherwise would have to answer once and then keep writing.
    Watching(Box<SessionView>),
    /// The caller is now watching the current session index.
    WatchingSessions,
    /// The session named is real but not this generation's to serve.
    ///
    /// A separate shape because the answer is the same refusal either way and the difference is what the
    /// connection may still try: a generation that replaced another can ask the one draining beside it, which is
    /// the only place a session started before the update still lives. Every other refusal is final here.
    NotHere(Response),
    /// A successor generation asked this daemon to drain.
    ///
    /// A separate shape because the session owner acts on it, not the connection: it releases the
    /// durable store so the successor can open it, stops taking new conversations, and ends this
    /// process once no turn is running. The connection only writes the acknowledgement.
    Draining,
    /// The session is closed, and its process is still being stopped.
    ///
    /// A separate shape because stopping is a wait, and the answer is not known until it is over. Handing the wait out
    /// is what keeps one session being closed from stopping every other session's output for as long as it takes: by
    /// the time this is returned the sessions are already correct, and all that is left is a process.
    Stopping {
        /// The driver, with nothing left holding it.
        agent: Box<dyn runtrol_provider::Agent>,
        /// How much time the process is given.
        how: CloseMode,
        /// The bounded slot to release after the process has stopped.
        reservation: ClosingReservation,
    },
    /// Session state is already committed, while detached processes still need to be stopped.
    Cleaning {
        /// The answer to write after cleanup.
        response: Response,
        /// Processes no longer owned by the session manager.
        agents: Vec<Cleanup>,
    },
    /// Provider stdin work moved out of the single session owner.
    Sending {
        /// The temporarily detached provider agent.
        taken: TakenAgent,
        /// The exact command to write.
        command: AgentCommand,
    },
    /// One provider has no process and is reserved while its package manager runs outside the owner.
    Updating {
        /// Provider whose confirmed package may change.
        provider: ProviderId,
        /// Opaque exclusion proof released only after verification or rollback finishes.
        reservation: runtrol_core::ProviderUpdateReservation,
    },
}

/// One process wait handed out by the single session owner.
pub(crate) struct Cleanup {
    /// The process to stop.
    pub(crate) agent: Box<dyn Agent>,
    /// How to stop it.
    pub(crate) how: CloseMode,
    /// The bounded slot to release after this process has stopped, when it has one.
    pub(crate) reservation: Option<CleanupReservation>,
}

pub(crate) enum CleanupReservation {
    Open(OpenReservation),
    Closing(ClosingReservation),
}

impl CleanupReservation {
    pub(crate) fn session(&self) -> SessionId {
        match self {
            Self::Open(reservation) => reservation.session(),
            Self::Closing(reservation) => reservation.session(),
        }
    }
}

/// The provider request shape a prepared result is bound to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PreparedKind {
    /// Model discovery.
    Models,
    /// A fresh session.
    Start,
    /// A resumed session.
    Resume,
}

/// A driver constructed while the probe cache is exclusively held.
pub(crate) enum Discovered {
    /// This request needs no provider work.
    None,
    /// The provider name was not valid, but the refusal is still bound to the request that produced it.
    Invalid {
        /// The request shape.
        kind: PreparedKind,
        /// The exact invalid provider text.
        provider: Box<str>,
        /// The refusal produced for it.
        response: Response,
    },
    /// A valid provider identifier and the driver construction result.
    Driver {
        /// The request shape.
        kind: PreparedKind,
        /// The parsed provider identifier.
        provider: ProviderId,
        /// The constructed driver or discovery refusal.
        driver: Result<Box<dyn Provider>, Response>,
    },
}

/// A session process opened before the single session owner receives it.
pub(crate) struct Opened {
    intent: OpenIntent,
    agent: Box<dyn Agent>,
}

/// The exact consult request a prepared consult answer belongs to.
///
/// Carried beside the answer so that one consult's answer cannot be replayed for a different consult request,
/// the same binding rule every other prepared result follows.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) enum ConsultAsked {
    /// The status of every direction.
    Status,
    /// One direction being wired.
    Wire {
        /// The registering provider.
        from: Box<str>,
        /// The provider being served.
        to: Box<str>,
    },
    /// One direction being unwired.
    Unwire {
        /// The registering provider.
        from: Box<str>,
        /// The provider being unregistered.
        to: Box<str>,
    },
    /// This executable being registered as Agent Tools in every usable provider CLI.
    AgentToolsWire,
    /// This executable's Agent Tools registration being removed from every usable provider CLI.
    AgentToolsUnwire,
}

impl ConsultAsked {
    /// The binding for one request, or `None` for a request that is not a consult.
    fn of(request: &Request) -> Option<Self> {
        match request {
            Request::Consult => Some(Self::Status),
            Request::ConsultWire { from, to } => Some(Self::Wire {
                from: from.clone(),
                to: to.clone(),
            }),
            Request::ConsultUnwire { from, to } => Some(Self::Unwire {
                from: from.clone(),
                to: to.clone(),
            }),
            Request::AgentToolsWire => Some(Self::AgentToolsWire),
            Request::AgentToolsUnwire => Some(Self::AgentToolsUnwire),
            _ => None,
        }
    }
}

/// Process work completed before the single session owner receives a request.
///
/// Probing a cold provider can take seconds. Keeping that wait outside the owner lets existing sessions continue to
/// publish events while a connection discovers the exact program it will use.
pub(crate) enum Prepared {
    /// This request needs no provider process work.
    None,
    /// A refusal produced before a provider identifier could be formed.
    Invalid {
        /// The request shape.
        kind: PreparedKind,
        /// The exact invalid provider text.
        provider: Box<str>,
        /// The refusal produced for it.
        response: Response,
    },
    /// Model discovery completed for this exact provider.
    Models {
        /// The provider whose models were queried.
        provider: ProviderId,
        /// The completed model result.
        result: Result<ModelCatalog, Response>,
    },
    /// Provider update inspection completed outside the session owner.
    ProviderUpdates {
        /// The complete bounded status list.
        response: Response,
    },
    /// Local integration administration completed outside the session owner.
    IntegrationAdmin {
        /// Exact response for the bound request.
        response: Response,
    },
    /// Local phone pairing administration completed outside the session owner.
    PairingAdmin {
        /// Exact response for the bound request.
        response: Response,
    },
    /// One exact ordinary-chat worktree preparation completed outside the session owner.
    IsolatedWorkspacePrepare {
        /// Bound idempotent request identity.
        request_id: Box<str>,
        /// Bound project text.
        project: Box<str>,
        /// Exact preparation result.
        response: Response,
    },
    /// One exact ordinary-chat worktree cleanup completed outside the session owner.
    IsolatedWorkspaceRelease {
        /// Bound optional Core ownership identity.
        workspace_id: Option<Box<str>>,
        /// Bound optional public Runtime session identity.
        session_id: Option<Box<str>>,
        /// Bound exact workspace text.
        workspace: Box<str>,
        /// Exact cleanup result.
        response: Response,
    },
    /// A fresh session process was opened for this exact provider.
    Start {
        /// The provider whose process was opened.
        provider: ProviderId,
        /// The opened process or refusal.
        result: Result<Opened, Response>,
    },
    /// A resumed session process was opened for this exact provider.
    Resume {
        /// The provider whose process was opened.
        provider: ProviderId,
        /// The opened process or refusal.
        result: Result<Opened, Response>,
    },

    /// A consult exchange completed by the connection task, outside the session owner.
    ///
    /// The whole exchange, not a handle to one: consult work is a few bounded process questions and holds no
    /// agent, so by the time the owner sees this there is nothing left that could wait.
    Consult {
        /// The exact consult request the answer was computed for.
        asked: ConsultAsked,
        /// The completed answer.
        response: Response,
    },
}

/// One connection's state.
///
/// Small on purpose: what a connection knows is who is on it and whether they have greeted, and nothing else. A
/// connection that remembered which session the caller "meant" would be the second place that notion lives.
#[derive(Debug)]
pub(crate) struct Conversation {
    /// Who is on the other end.
    ///
    /// Decided when the connection was accepted, from which endpoint it arrived on, and never afterwards. There is
    /// deliberately no way to set this from a request: a caller that could say who it was would say whatever got it
    /// the most authority.
    caller: Caller,
    /// The wire format has been agreed.
    greeted: bool,
}

impl Conversation {
    /// A connection from somebody at the machine, which has said nothing yet.
    ///
    /// The only constructor today, because the local endpoint is the only way in. A remote transport arrives with
    /// its own constructor taking the device it authenticated, and until then there is no way to build a
    /// conversation that claims to be one.
    #[must_use]
    pub(crate) const fn at_the_machine() -> Self {
        Self {
            caller: Caller::AtTheMachine,
            greeted: false,
        }
    }

    /// A connection from one device authenticated by the remote transport.
    ///
    /// This stays crate-private so request data cannot choose a caller. Only the daemon's transport boundary can
    /// construct it after matching a cryptographic identity to one restored paired device.
    #[must_use]
    pub(crate) const fn from_device(device: runtrol_security::DeviceId) -> Self {
        Self {
            caller: Caller::Device { device },
            greeted: false,
        }
    }

    /// Who is on the other end.
    #[must_use]
    pub(crate) const fn caller(&self) -> &Caller {
        &self.caller
    }

    /// Whether the wire format has been agreed.
    #[must_use]
    pub(crate) const fn greeted(&self) -> bool {
        self.greeted
    }
}

/// Answer one request.
///
/// Takes the assembled daemon and the sessions, because a request is about one or the other and usually both.
#[cfg(test)]
async fn answer(
    conversation: &mut Conversation,
    composed: &Composed,
    sessions: &mut SessionManager,
    request: Request,
) -> Reply {
    let reserved = if matches!(request, Request::Start { .. } | Request::Resume { .. })
        && conversation.greeted
        && crate::scope::allowed(
            &conversation.caller,
            &request,
            &composed.device_authority.grants(),
        )
        .is_ok()
    {
        let session = SessionId::now();
        match sessions.reserve_open_for_tests(session) {
            Ok(reserved) => Some(reserved),
            Err(error) => return reply_from_session_error(&error),
        }
    } else {
        None
    };
    let mut reserved = reserved;
    if let Some(displaced) = reserved.as_mut().and_then(|one| one.displaced.take()) {
        drop(
            displaced
                .agent
                .close(CloseMode::Graceful { grace_ms: 0 })
                .await,
        );
        sessions.release_closing(displaced.reservation);
    }
    let discovered = discover(conversation, composed, &request).await;
    let prepared = complete_prepare_for(
        &request,
        discovered,
        reserved.as_ref().map(|one| one.reservation.session()),
    )
    .await;
    let reservation = reserved.map(|one| one.reservation);
    let (reply, reservations) = finish_cleanup(
        answer_prepared(
            conversation,
            composed,
            sessions,
            request,
            prepared,
            reservation,
        )
        .await,
    )
    .await;
    for reservation in reservations {
        match reservation {
            CleanupReservation::Open(reservation) => sessions.cancel_open(reservation),
            CleanupReservation::Closing(reservation) => sessions.release_closing(reservation),
        }
    }
    reply
}

/// Construct a driver while the probe cache has one writer.
pub(crate) async fn discover(
    conversation: &Conversation,
    composed: &Composed,
    request: &Request,
) -> Discovered {
    if !needs_driver(request)
        || !conversation.greeted
        || crate::scope::allowed(
            &conversation.caller,
            request,
            &composed.device_authority.grants(),
        )
        .is_err()
    {
        return Discovered::None;
    }

    let Some((kind, provider)) = (match request {
        Request::Models { provider } => Some((PreparedKind::Models, provider.as_ref())),
        Request::Start { provider, .. } => Some((PreparedKind::Start, provider.as_ref())),
        Request::Resume { provider, .. } => Some((PreparedKind::Resume, provider.as_ref())),
        _ => None,
    }) else {
        return Discovered::None;
    };
    let Ok(id) = ProviderId::parse(provider) else {
        return Discovered::Invalid {
            kind,
            provider: provider.into(),
            response: refuse(&format!(
                "{provider:?} is not a provider name runtrol accepts"
            )),
        };
    };
    Discovered::Driver {
        kind,
        provider: id,
        driver: crate::provider_prepare::driver(composed, id)
            .await
            .map_err(|error| refuse(error.message())),
    }
}

/// Finish provider work for an optional slot reserved by the session owner.
pub(crate) async fn complete_prepare_for(
    request: &Request,
    discovered: Discovered,
    session: Option<SessionId>,
) -> Prepared {
    let (kind, provider, driver) = match discovered {
        Discovered::None => return Prepared::None,
        Discovered::Invalid {
            kind,
            provider,
            response,
        } => {
            return Prepared::Invalid {
                kind,
                provider,
                response,
            };
        }
        Discovered::Driver {
            kind,
            provider,
            driver,
        } => (kind, provider, driver),
    };

    let driver = match driver {
        Ok(driver) => driver,
        Err(response) => {
            return match kind {
                PreparedKind::Models => Prepared::Models {
                    provider,
                    result: Err(response),
                },
                PreparedKind::Start => Prepared::Start {
                    provider,
                    result: Err(response),
                },
                PreparedKind::Resume => Prepared::Resume {
                    provider,
                    result: Err(response),
                },
            };
        }
    };

    match (kind, request) {
        (PreparedKind::Models, Request::Models { .. }) => Prepared::Models {
            provider,
            result: driver
                .models()
                .await
                .map_err(|error| Response::Failed(WireError::from_provider(&error))),
        },
        (
            PreparedKind::Start,
            Request::Start {
                workspace,
                model,
                permission,
                ..
            },
        ) => Prepared::Start {
            provider,
            result: open_driver(
                driver.as_ref(),
                session,
                workspace,
                Disposition::Fresh,
                model.clone(),
                permission.clone(),
            )
            .await,
        },
        (
            PreparedKind::Resume,
            Request::Resume {
                native, workspace, ..
            },
        ) => Prepared::Resume {
            provider,
            result: open_driver(
                driver.as_ref(),
                session,
                workspace,
                Disposition::Resume {
                    native: native.clone(),
                },
                None,
                None,
            )
            .await,
        },
        _ => Prepared::Invalid {
            kind,
            provider: provider.as_str().into(),
            response: refuse("provider preparation was paired with a different request shape"),
        },
    }
}

/// Which managed session a request addresses, for the device workspace bound in [`answer`].
const fn addressed_session(request: &Request) -> Option<SessionId> {
    match request {
        Request::Prompt { session, .. }
        | Request::Rename { session, .. }
        | Request::Interrupt { session }
        | Request::AnswerApproval { session, .. }
        | Request::Watch { session, .. }
        | Request::Close { session, .. } => Some(*session),
        _ => None,
    }
}

/// Whether this device's live roots cover the session's workspace.
///
/// The workspace is the live process's when there is one and the stored pointer's otherwise, and a session
/// neither knows is nobody's to touch (fail closed): to a device outside the grant it does not exist, and
/// the refusal must not say more than the projection shows.
fn device_covers_session(
    composed: &Composed,
    sessions: &SessionManager,
    device: runtrol_security::DeviceId,
    session: SessionId,
) -> bool {
    let workspace = match sessions.live_session(session) {
        Some(live) => Some(live.workspace.clone()),
        None => match composed.store.get_session(session) {
            Ok(Some(row)) => Some(row.cwd),
            Ok(None) | Err(_) => None,
        },
    };
    let Some(workspace) = workspace else {
        return false;
    };
    composed
        .device_authority
        .live_roots(device)
        .iter()
        .any(|root| workspace.is_under(root))
}

/// Whether a request needs provider discovery before it reaches the session owner.
#[must_use]
pub(crate) const fn needs_driver(request: &Request) -> bool {
    matches!(
        request,
        Request::Models { .. } | Request::Start { .. } | Request::Resume { .. }
    )
}

/// Complete a consult exchange in the connection task, or nothing when the wall would refuse it anyway.
///
/// The wall is still asked by the owner before the answer is used. It is asked here first because consult
/// work runs provider processes, and an unauthorized request must cost nothing before it is refused.
pub(crate) async fn prepare_consult(
    conversation: &Conversation,
    composed: &Composed,
    request: &Request,
) -> Prepared {
    if !conversation.greeted()
        || crate::scope::allowed(
            &conversation.caller,
            request,
            &composed.device_authority.grants(),
        )
        .is_err()
    {
        return Prepared::None;
    }
    let Some(asked) = ConsultAsked::of(request) else {
        return Prepared::None;
    };
    Prepared::Consult {
        asked,
        response: crate::consult::answer(composed, request).await,
    }
}

/// Complete explicit provider update inspection outside the session owner.
pub(crate) async fn prepare_provider_updates(
    conversation: &Conversation,
    composed: &Composed,
    request: &Request,
    discovering: &crate::serve::DiscoveryGates,
) -> Prepared {
    if !matches!(request, Request::ProviderUpdates)
        || !conversation.greeted()
        || crate::scope::allowed(
            &conversation.caller,
            request,
            &composed.device_authority.grants(),
        )
        .is_err()
    {
        return Prepared::None;
    }
    Prepared::ProviderUpdates {
        response: Response::ProviderUpdates(
            crate::provider_update::inspect_all(composed, discovering).await,
        ),
    }
}

/// Whether the request belongs to local public-integration administration.
pub(crate) const fn is_integration_admin(request: &Request) -> bool {
    matches!(
        request,
        Request::IntegrationEnrollments
            | Request::IntegrationApprovalBegin { .. }
            | Request::IntegrationApprovalFinish { .. }
            | Request::IntegrationSelfApprove { .. }
            | Request::IntegrationEnrollmentDeny { .. }
            | Request::Integrations
            | Request::ProviderHelp { .. }
            | Request::IntegrationRevoke { .. }
            | Request::IntegrationGrantChange { .. }
            | Request::RuntimeForgetRequests
            | Request::RuntimeForgetConfirm { .. }
            | Request::RuntimeKeyRotationRequests
            | Request::RuntimeKeyRotationConfirm { .. }
            | Request::RuntimeSharedOpenRequests
            | Request::RuntimeSharedOpenConfirm { .. }
    )
}

/// Whether a request belongs to local paired-phone administration.
pub(crate) const fn is_pairing_admin(request: &Request) -> bool {
    matches!(
        request,
        Request::PairingBegin
            | Request::PairingProposals
            | Request::PairingApprovalBegin { .. }
            | Request::PairingApprovalFinish { .. }
            | Request::PairingDeny { .. }
            | Request::Devices
            | Request::DeviceRevoke { .. }
            | Request::DeviceAuthorityBegin { .. }
            | Request::DeviceAuthorityFinish { .. }
    )
}

/// Complete local phone pairing administration outside the one session owner.
pub(crate) async fn prepare_pairing_admin(
    conversation: &Conversation,
    composed: &Composed,
    request: &Request,
) -> Prepared {
    if !is_pairing_admin(request)
        || !conversation.greeted()
        || crate::scope::allowed(
            conversation.caller(),
            request,
            &composed.device_authority.grants(),
        )
        .is_err()
    {
        return Prepared::None;
    }
    let response = match request {
        Request::PairingBegin => composed
            .pairing_admin
            .begin(composed)
            .await
            .map(Response::PairingInvitation),
        Request::PairingProposals => Ok(Response::PairingProposals(
            composed.pairing_admin.proposals().await,
        )),
        Request::PairingApprovalBegin {
            proposal_id,
            scopes,
        } => composed
            .pairing_admin
            .begin_approval(proposal_id, scopes)
            .await
            .map(
                |(challenge_id, prompt)| Response::PairingApprovalChallenge {
                    challenge_id,
                    prompt,
                },
            ),
        Request::PairingApprovalFinish {
            challenge_id,
            answer,
        } => composed
            .pairing_admin
            .finish_approval(composed, challenge_id, answer)
            .await
            .map(|_| Response::Done),
        Request::PairingDeny { proposal_id } => composed
            .pairing_admin
            .deny(proposal_id)
            .await
            .map(|()| Response::Done),
        Request::Devices => Ok(Response::Devices(
            crate::pairing_admin::PairingAdmin::devices(composed),
        )),
        Request::DeviceRevoke { device_id } => {
            crate::pairing_admin::PairingAdmin::revoke(composed, device_id).map(|()| Response::Done)
        }
        Request::DeviceAuthorityBegin {
            device_id,
            scopes,
            roots,
            providers,
        } => composed
            .pairing_admin
            .begin_authority(composed, device_id, scopes, roots, providers)
            .await
            .map(
                |(challenge_id, prompt)| Response::DeviceAuthorityChallenge {
                    challenge_id,
                    prompt,
                },
            ),
        Request::DeviceAuthorityFinish {
            challenge_id,
            answer,
        } => composed
            .pairing_admin
            .finish_authority(composed, challenge_id, answer)
            .await
            .map(|()| Response::Done),
        _ => return Prepared::None,
    }
    .unwrap_or_else(|error| refuse(&error.to_string()));
    Prepared::PairingAdmin { response }
}

/// Complete local integration administration outside the one session owner.
pub(crate) async fn prepare_integration_admin(
    conversation: &Conversation,
    composed: &Composed,
    request: &Request,
) -> Prepared {
    if !is_integration_admin(request)
        || !conversation.greeted()
        || crate::scope::allowed(
            conversation.caller(),
            request,
            &composed.device_authority.grants(),
        )
        .is_err()
    {
        return Prepared::None;
    }
    let response = match request {
        Request::IntegrationEnrollments => {
            crate::integration_admin::IntegrationAdmin::enrollments(composed)
                .map(Response::IntegrationEnrollments)
        }
        Request::IntegrationApprovalBegin {
            pending_id,
            scopes,
            roots,
        } => composed
            .integration_admin
            .begin(composed, pending_id, scopes, roots)
            .await
            .map(|challenge| Response::IntegrationApprovalChallenge {
                challenge_id: challenge.challenge_id,
                prompt: challenge.prompt,
            }),
        Request::IntegrationApprovalFinish {
            challenge_id,
            answer,
        } => composed
            .integration_admin
            .finish(composed, challenge_id, answer)
            .await
            .map(|integration_id| Response::IntegrationApproved {
                integration_id: integration_id.to_string().into(),
            }),
        Request::IntegrationSelfApprove {
            pending_id,
            signature,
        } => crate::integration_admin::IntegrationAdmin::self_approve(
            composed, pending_id, signature,
        )
        .map(|integration_id| Response::IntegrationApproved {
            integration_id: integration_id.to_string().into(),
        }),
        Request::IntegrationEnrollmentDeny { pending_id } => {
            crate::integration_admin::IntegrationAdmin::deny(composed, pending_id)
                .map(|()| Response::Done)
        }
        Request::Integrations => crate::integration_admin::IntegrationAdmin::integrations(composed)
            .map(Response::Integrations),
        Request::ProviderHelp { provider_id } => provider_help(composed, provider_id),
        Request::IntegrationRevoke { integration_id } => {
            crate::integration_admin::IntegrationAdmin::revoke(composed, integration_id)
                .map(|()| Response::Done)
        }
        Request::IntegrationGrantChange {
            integration_id,
            expected_grant_generation,
            scopes,
            roots,
        } => crate::integration_admin::IntegrationAdmin::change_grant(
            composed,
            integration_id,
            *expected_grant_generation,
            scopes,
            roots,
        )
        .map(|()| Response::Done),
        _ => match runtime_confirmation_administration(composed, request).await {
            Some(outcome) => outcome,
            None => return Prepared::None,
        },
    }
    .unwrap_or_else(|error| refuse(&error.to_string()));
    Prepared::IntegrationAdmin { response }
}

fn provider_help(
    composed: &Composed,
    provider_id: &str,
) -> Result<Response, crate::integration_admin::AdminError> {
    let provider = crate::runtime_inventory::providers(composed)
        .providers
        .into_iter()
        .find(|provider| provider.provider_id.as_str() == provider_id)
        .ok_or_else(|| {
            crate::integration_admin::AdminError::invalid(
                "the provider does not exist in this Runtime",
            )
        })?;
    let state = match provider.installation.state {
        runtrol_runtime_protocol::InstallationState::Usable => "usable",
        runtrol_runtime_protocol::InstallationState::Missing => "missing",
        runtrol_runtime_protocol::InstallationState::Unavailable => "unavailable",
    };
    let help = provider
        .help
        .unwrap_or(runtrol_runtime_protocol::ProviderHelp {
            sign_in: None,
            diagnose: None,
            install: None,
        });
    Ok(Response::ProviderHelp(Box::new(
        runtrol_ipc::wire::ProviderHelpLine {
            provider_id: provider.provider_id.as_str().into(),
            display_name: provider.display_name.into(),
            installation_state: state.into(),
            version: provider.installation.version.map(Into::into),
            why: provider.installation.why.map(Into::into),
            sign_in: help.sign_in.map(Into::into),
            diagnose: help.diagnose.map(Into::into),
            install: help.install.map(Into::into),
        },
    )))
}

/// The public Runtime's queued decisions (session forget, integration key rotation, shared-writer session
/// open), listed and confirmed at the machine. None when the request is not one of them.
async fn runtime_confirmation_administration(
    composed: &Composed,
    request: &Request,
) -> Option<Result<Response, crate::integration_admin::AdminError>> {
    let admin = &composed.integration_admin;
    Some(match request {
        Request::RuntimeForgetRequests => admin
            .forget_requests(composed)
            .await
            .map(Response::RuntimeForgetRequests),
        Request::RuntimeForgetConfirm { confirmation_id } => admin
            .confirm_forget(confirmation_id)
            .await
            .map(|()| Response::Done),
        Request::RuntimeKeyRotationRequests => admin
            .key_rotation_requests(composed)
            .await
            .map(Response::RuntimeKeyRotationRequests),
        Request::RuntimeKeyRotationConfirm { confirmation_id } => admin
            .confirm_key_rotation(confirmation_id)
            .await
            .map(|()| Response::Done),
        Request::RuntimeSharedOpenRequests => admin
            .shared_open_requests(composed)
            .await
            .map(Response::RuntimeSharedOpenRequests),
        Request::RuntimeSharedOpenConfirm { confirmation_id } => admin
            .confirm_shared_open(confirmation_id)
            .await
            .map(|()| Response::Done),
        _ => return None,
    })
}

/// Create or release one Core-owned ordinary-chat worktree outside the single session owner.
pub(crate) async fn prepare_isolated_workspace(
    conversation: &Conversation,
    composed: &Composed,
    request: &Request,
) -> Prepared {
    if !conversation.greeted()
        || crate::scope::allowed(
            conversation.caller(),
            request,
            &composed.device_authority.grants(),
        )
        .is_err()
    {
        return Prepared::None;
    }
    match request {
        Request::WorkspaceIsolatePrepare {
            request_id,
            project,
        } => {
            let response = composed
                .isolated_workspaces
                .lock()
                .await
                .prepare(&composed.containment, request_id, project)
                .await
                .unwrap_or_else(|message| refuse(&message));
            Prepared::IsolatedWorkspacePrepare {
                request_id: request_id.clone(),
                project: project.clone(),
                response,
            }
        }
        Request::WorkspaceIsolateRelease {
            workspace_id,
            session_id,
            workspace,
        } => {
            let response = composed
                .isolated_workspaces
                .lock()
                .await
                .release(
                    &composed.containment,
                    workspace_id.as_deref(),
                    session_id.as_deref(),
                    workspace,
                )
                .await
                .unwrap_or_else(|message| refuse(&message));
            Prepared::IsolatedWorkspaceRelease {
                workspace_id: workspace_id.clone(),
                session_id: session_id.clone(),
                workspace: workspace.clone(),
                response,
            }
        }
        _ => Prepared::None,
    }
}

/// Answer one request after any slow provider discovery has completed elsewhere.
#[expect(
    clippy::too_many_lines,
    reason = "one exhaustive request table keeps every scope-checked wire operation visible in one place"
)]
pub(crate) async fn answer_prepared(
    conversation: &mut Conversation,
    composed: &Composed,
    sessions: &mut SessionManager,
    request: Request,
    prepared: Prepared,
    reservation: Option<OpenReservation>,
) -> Reply {
    // Before anything else looks at the request. A wall consulted after some other branch has acted is a wall on
    // the way out, and the thing it was supposed to prevent has already happened.
    if let Err(refusal) = crate::scope::allowed_with_authority(
        &conversation.caller,
        &request,
        &composed.device_authority,
    ) {
        return Reply::One(refuse(&refusal.to_string()));
    }

    // The greeting is the one request that may arrive first, and everything else is refused until it has.
    if let Request::Hello { wire } = request {
        return match runtrol_ipc::wire::agree(wire) {
            Ok(agreed) => {
                conversation.greeted = true;
                Reply::One(Response::Welcome {
                    wire: agreed,
                    providers: providers_of(composed),
                    device: crate::pairing_admin::PairingAdmin::self_authority(
                        composed,
                        &conversation.caller,
                    ),
                    push_public_key: push_public_key(composed, &conversation.caller),
                    build_digest: crate::build_identity::build_digest().map(Into::into),
                })
            }
            Err(ours) => Reply::One(refuse(&format!(
                "this daemon speaks wire format {ours} and the caller speaks {wire}"
            ))),
        };
    }
    if !conversation.greeted {
        return Reply::One(refuse(
            "this connection has not agreed a wire format, so nothing on it can be acted on",
        ));
    }

    // A device acts only inside its live workspace roots. The scope wall already answered whether this
    // caller may perform this KIND of action; this answers whether it may perform it on THIS session, with
    // the same root verification that bounds what it can list, so acting and seeing cannot diverge: a
    // session id learned before a root was revoked stops working the moment the root does.
    if let (Caller::Device { device }, Some(session)) =
        (conversation.caller(), addressed_session(&request))
        && !device_covers_session(composed, sessions, *device, session)
    {
        return Reply::One(refuse(
            "this phone is not approved for that session's workspace",
        ));
    }

    match request {
        // Answered above, and matched here so that adding a request cannot fall through to a wildcard that does nothing.
        Request::Hello { .. } => Reply::One(refuse("the wire format is already agreed")),

        Request::List => Reply::One(list(composed, sessions, conversation.caller())),

        Request::WatchSessions => Reply::WatchingSessions,

        Request::Models { provider } => models(&provider, prepared),

        Request::ProviderUpdates => match prepared {
            Prepared::ProviderUpdates { response } => Reply::One(response),
            _ => Reply::One(refuse(
                "provider update inspection was not completed for this request",
            )),
        },

        Request::WorkspaceIsolatePrepare {
            request_id,
            project,
        } => match prepared {
            Prepared::IsolatedWorkspacePrepare {
                request_id: prepared_id,
                project: prepared_project,
                response,
            } if prepared_id == request_id && prepared_project == project => Reply::One(response),
            other => mismatched(other),
        },

        Request::WorkspaceIsolateRelease {
            workspace_id,
            session_id,
            workspace,
        } => match prepared {
            Prepared::IsolatedWorkspaceRelease {
                workspace_id: prepared_id,
                session_id: prepared_session,
                workspace: prepared_workspace,
                response,
            } if prepared_id == workspace_id
                && prepared_session == session_id
                && prepared_workspace == workspace =>
            {
                Reply::One(response)
            }
            other => mismatched(other),
        },

        Request::WorkspaceIsolateList => match composed.isolated_workspaces.try_lock() {
            Ok(controller) => Reply::One(controller.list()),
            Err(_) => Reply::One(refuse("the isolated workspace controller lock is damaged")),
        },

        Request::WorkspaceIsolateBind {
            workspace_id,
            session_id,
            workspace,
        } => match composed.isolated_workspaces.try_lock() {
            Ok(mut controller) => Reply::One(
                controller
                    .bind(&workspace_id, &session_id, &workspace)
                    .unwrap_or_else(|message| refuse(&message)),
            ),
            Err(_) => Reply::One(refuse("the isolated workspace controller lock is damaged")),
        },

        Request::ProviderUpdate { provider } => {
            let Ok(provider) = ProviderId::parse(&provider) else {
                return Reply::One(refuse("the update request names an invalid provider"));
            };
            match sessions.reserve_provider_update(provider) {
                Ok(reservation) => Reply::Updating {
                    provider,
                    reservation,
                },
                Err(error) => reply_from_session_error(&error),
            }
        }

        Request::RemoteConnection => Reply::One(remote_connection(composed)),

        Request::RemoteConfigure { relay_origin } => {
            if relay_origin.is_some()
                && (composed.pc_identity.is_none() || composed.relay_seed.is_none())
            {
                return Reply::One(refuse(
                    "remote connection requires a protected machine identity",
                ));
            }
            match composed.relay_control.configure(relay_origin.as_deref()) {
                Ok(()) => Reply::One(remote_connection(composed)),
                Err(error) => Reply::One(refuse(&error.to_string())),
            }
        }

        Request::PairingBegin
        | Request::PairingProposals
        | Request::PairingApprovalBegin { .. }
        | Request::PairingApprovalFinish { .. }
        | Request::PairingDeny { .. }
        | Request::Devices
        | Request::DeviceRevoke { .. }
        | Request::DeviceAuthorityBegin { .. }
        | Request::DeviceAuthorityFinish { .. } => match prepared {
            Prepared::PairingAdmin { response } => Reply::One(response),
            _ => Reply::One(refuse(
                "paired-phone administration was not completed for this request",
            )),
        },

        Request::PushSubscription { endpoint } => Reply::One(set_push_subscription(
            composed,
            &conversation.caller,
            endpoint.as_deref(),
        )),

        Request::IntegrationEnrollments
        | Request::IntegrationApprovalBegin { .. }
        | Request::IntegrationApprovalFinish { .. }
        | Request::IntegrationSelfApprove { .. }
        | Request::IntegrationEnrollmentDeny { .. }
        | Request::Integrations
        | Request::ProviderHelp { .. }
        | Request::IntegrationRevoke { .. }
        | Request::IntegrationGrantChange { .. }
        | Request::RuntimeForgetRequests
        | Request::RuntimeForgetConfirm { .. }
        | Request::RuntimeKeyRotationRequests
        | Request::RuntimeKeyRotationConfirm { .. }
        | Request::RuntimeSharedOpenRequests
        | Request::RuntimeSharedOpenConfirm { .. } => match prepared {
            Prepared::IntegrationAdmin { response } => Reply::One(response),
            _ => Reply::One(refuse(
                "integration administration was not completed for this request",
            )),
        },

        Request::Start {
            provider,
            workspace: _,
            workspace_access: _,
            model: _,
            permission: _,
        } => {
            open(
                composed,
                sessions,
                &provider,
                PreparedKind::Start,
                prepared,
                reservation,
            )
            .await
        }

        Request::Resume {
            provider,
            native: _,
            workspace: _,
            workspace_access: _,
        } => {
            open(
                composed,
                sessions,
                &provider,
                PreparedKind::Resume,
                prepared,
                reservation,
            )
            .await
        }

        Request::Prompt { session, text } => send(
            sessions,
            session,
            AgentCommand::Prompt(vec![ContentBlock::Text(text)]),
        ),

        Request::Rename { session, label } => {
            let label = match session_label(label) {
                Ok(label) => label,
                Err(why) => return Reply::One(refuse(why)),
            };
            let store = std::sync::Arc::clone(&composed.store);
            let labelled =
                tokio::task::spawn_blocking(move || store.set_session_label(session, label))
                    .await
                    .unwrap_or_else(|_worker| {
                        Err(StoreError::Codec {
                            field: "store worker",
                            why: "the store worker ended before the name was saved",
                        })
                    });
            match labelled {
                Ok(true) => Reply::One(Response::Done),
                Ok(false) => Reply::One(refuse(
                    "the provider has not established this session yet, so its name cannot be saved",
                )),
                Err(error) => Reply::One(refuse(&error.to_string())),
            }
        }

        Request::Interrupt { session } => send(sessions, session, AgentCommand::Interrupt),

        Request::AnswerApproval {
            session,
            approval,
            option,
            subject_digest,
        } => match sessions.take_for_answer_approval(
            conversation.caller(),
            &composed.device_authority.grants(),
            session,
            approval,
            option,
            subject_digest,
        ) {
            Ok((taken, command)) => Reply::Sending { taken, command },
            Err(error) => reply_from_session_error(&error),
        },

        Request::Watch { session, after } => match sessions.subscribe(session, after) {
            Ok(watching) => Reply::Watching(Box::new(watching)),
            Err(error) => Reply::One(refuse(&error.to_string())),
        },

        Request::Close { session, now } => {
            // The vocabulary's own answer, not one driver's. Reaching into a driver for it would give every
            // other provider that driver's patience, and adding a second one would change how long the first is
            // waited for depending on which import somebody wrote.
            let how = if now {
                CloseMode::Kill
            } else {
                CloseMode::graceful()
            };
            crate::serve::close_trace("dispatch: close received");
            let closing = sessions.close(session);
            crate::serve::close_trace("dispatch: session released");
            // Off the async thread, like every durable session write (see `persist_live_from_store`), and
            // only once the close itself is settled: a close the manager refuses keeps its stored pointer.
            let removed = if matches!(closing, Ok(_) | Err(SessionError::NotLive { .. })) {
                let store = std::sync::Arc::clone(&composed.store);
                tokio::task::spawn_blocking(move || store.remove_session(session))
                    .await
                    .unwrap_or_else(|_worker| {
                        Err(StoreError::Codec {
                            field: "store worker",
                            why: "the store worker ended before the session pointer was removed",
                        })
                    })
            } else {
                Ok(false)
            };
            match closing {
                Ok(closing) => match removed {
                    Ok(_) => Reply::Stopping {
                        agent: closing.agent,
                        how,
                        reservation: closing.reservation,
                    },
                    Err(error) => Reply::Cleaning {
                        response: refuse(&error.to_string()),
                        agents: vec![Cleanup {
                            agent: closing.agent,
                            how: CloseMode::Kill,
                            reservation: Some(CleanupReservation::Closing(closing.reservation)),
                        }],
                    },
                },
                Err(SessionError::NotLive { .. }) => match removed {
                    Ok(true) => Reply::One(Response::Done),
                    Ok(false) => reply_from_session_error(&SessionError::NotLive { session }),
                    Err(error) => Reply::One(refuse(&error.to_string())),
                },
                Err(error) => reply_from_session_error(&error),
            }
        }

        // Consults nothing: no ledger, no scope, no configuration. The security posture requires this to work from
        // anywhere with no permission at all, and the worst a hostile caller achieves through it is stopping work.
        Request::StopEverything => match composed.containment.terminate_all() {
            Ok(()) => Reply::One(Response::Done),
            // Reported rather than swallowed. An operator who pressed the panic button has to know whether it worked.
            Err(error) => Reply::One(refuse(&error.to_string())),
        },

        // Never refused: the successor is already listening, and what it needs is the store. The
        // owner loop releases it and decides when this process ends (once no turn is running); an
        // idle process does not keep a generation alive, because the conversation lives in the
        // provider's own store and the successor resumes it from there on demand.
        Request::Drain => Reply::Draining,

        Request::GenerationHandoff {
            successor_digest,
            authorities,
            claims,
        } => {
            if !composed.draining.load(std::sync::atomic::Ordering::Acquire) {
                return Reply::One(refuse(
                    "generation authority handoff is accepted only after drain begins",
                ));
            }
            if composed
                .generation_authority
                .apply(&successor_digest, &authorities)
                .is_err()
            {
                return Reply::One(refuse(
                    "generation authority handoff does not match the bound successor",
                ));
            }
            composed
                .native_claims
                .replace_remote(&successor_digest, claims);
            Reply::One(Response::GenerationHandoff {
                capabilities: runtrol_ipc::GenerationHandoffCapabilities {
                    public_terminal: true,
                    authority_relay: true,
                    native_live_claims: true,
                },
                claims: composed
                    .native_claims
                    .snapshot_except(Some(&successor_digest)),
            })
        }

        Request::GenerationAuthorityUpdate {
            successor_digest,
            authorities,
            claims,
        } => {
            if !composed.draining.load(std::sync::atomic::Ordering::Acquire)
                || composed
                    .generation_authority
                    .apply(&successor_digest, &authorities)
                    .is_err()
            {
                return Reply::One(refuse(
                    "generation authority update does not match a draining handoff",
                ));
            }
            composed
                .native_claims
                .replace_remote(&successor_digest, claims);
            Reply::One(Response::Done)
        }

        // The exchange already happened in the connection task. What is verified here is the binding: the
        // answer must be the one computed for this exact request, the rule every prepared result follows.
        consult @ (Request::Consult
        | Request::ConsultWire { .. }
        | Request::ConsultUnwire { .. }
        | Request::AgentToolsWire
        | Request::AgentToolsUnwire) => match prepared {
            Prepared::Consult { asked, response }
                if ConsultAsked::of(&consult).as_ref() == Some(&asked) =>
            {
                Reply::One(response)
            }
            other => mismatched(other),
        },

        // A request that arrived after this build was made. Refused by name, because a wildcard that answered "done"
        // would report something as carried out when nothing happened.
        other => Reply::One(refuse(&format!("this daemon has no binding for {other:?}"))),
    }
}

fn push_public_key(composed: &Composed, caller: &Caller) -> Option<Box<str>> {
    if !matches!(caller, Caller::Device { .. }) {
        return None;
    }
    composed
        .push_identity
        .as_ref()
        .map(|identity| identity.application_server_key().into_boxed_str())
}

fn set_push_subscription(composed: &Composed, caller: &Caller, endpoint: Option<&str>) -> Response {
    let Caller::Device { device } = caller else {
        return refuse("push subscriptions belong only to an authenticated paired phone");
    };
    let key = runtrol_store::DeviceKey::from_bytes(*device.as_bytes());
    let mut row = match composed.store.get_device(key) {
        Ok(Some(row)) => row,
        Ok(None) => return refuse("this phone is no longer paired"),
        Err(_) => return refuse("the device authorization store cannot be read"),
    };
    row.push_endpoint = match endpoint {
        None => None,
        Some(endpoint) => {
            let Some(identity) = &composed.push_identity else {
                return refuse("push delivery requires a protected machine identity");
            };
            match identity.seal_endpoint(*device.as_bytes(), endpoint) {
                Ok(encrypted) => Some(encrypted.into_boxed_slice()),
                Err(error) => return refuse(&error.to_string()),
            }
        }
    };
    if composed.store.put_device(key, &row).is_err() {
        return refuse("the push subscription could not be saved");
    }
    if composed.reload_device_authority().is_err() {
        return refuse("the updated device authorization could not be restored");
    }
    Response::Done
}

fn remote_connection(composed: &Composed) -> Response {
    let (relay_origin, status) = composed.relay_control.view();
    let (state, failure_boundary) = match status {
        crate::RelayStatus::Disabled => (RemoteConnectionState::Disabled, None),
        crate::RelayStatus::Connecting => (RemoteConnectionState::Connecting, None),
        crate::RelayStatus::Online => (RemoteConnectionState::Online, None),
        crate::RelayStatus::Offline(stage) => (
            RemoteConnectionState::Offline,
            Some(match stage {
                crate::RelayStage::Discovery => RemoteConnectionStage::Discovery,
                crate::RelayStage::Registration => RemoteConnectionStage::Registration,
                crate::RelayStage::Connection => RemoteConnectionStage::Connection,
                crate::RelayStage::Exchange => RemoteConnectionStage::Exchange,
            }),
        ),
    };
    Response::RemoteConnection(RemoteConnection {
        relay_origin,
        state,
        stage: failure_boundary,
    })
}

/// Commit a session process that was opened by its connection task.
async fn open(
    composed: &Composed,
    sessions: &mut SessionManager,
    requested_provider: &str,
    requested_kind: PreparedKind,
    prepared: Prepared,
    reservation: Option<OpenReservation>,
) -> Reply {
    let prepared = match bound(prepared, requested_kind, requested_provider) {
        Ok(prepared) => prepared,
        Err(reply) => {
            if let Some(reservation) = reservation {
                return hold_until_cleaned(reply, reservation, sessions);
            }
            return reply;
        }
    };
    let (provider, result) = match prepared {
        Prepared::Start { provider, result } | Prepared::Resume { provider, result } => {
            (provider, result)
        }
        Prepared::Invalid { response, .. } => {
            if let Some(reservation) = reservation {
                sessions.cancel_open(reservation);
            }
            return Reply::One(response);
        }
        other => return mismatched(other),
    };
    let Opened { intent, agent } = match result {
        Ok(opened) => opened,
        Err(response) => {
            if let Some(reservation) = reservation {
                sessions.cancel_open(reservation);
            }
            return Reply::One(response);
        }
    };
    let Some(reservation) = reservation else {
        return cleanup_opened(Opened { intent, agent });
    };

    let attached = match sessions.attach_opened(reservation, provider, &intent, agent) {
        Ok(attached) => attached,
        Err(error) => {
            let (error, agent, reservation) = error.into_parts();
            return Reply::Cleaning {
                response: from_session_error(&error),
                agents: vec![Cleanup {
                    agent,
                    how: CloseMode::Kill,
                    reservation: Some(CleanupReservation::Open(reservation)),
                }],
            };
        }
    };
    let mut agents = Vec::new();
    let response = match persist_live(composed, sessions, attached.session).await {
        Ok(()) => Response::Started {
            session: attached.session,
        },
        Err(error) => match sessions.close(attached.session) {
            Ok(closing) => {
                agents.push(Cleanup {
                    agent: closing.agent,
                    how: CloseMode::Kill,
                    reservation: Some(CleanupReservation::Closing(closing.reservation)),
                });
                refuse(&error.to_string())
            }
            Err(close_error) => refuse(&format!(
                "{error}; the unrecorded session also could not be detached: {close_error}"
            )),
        },
    };
    if agents.is_empty() {
        Reply::One(response)
    } else {
        Reply::Cleaning { response, agents }
    }
}

/// Build and open one driver process outside the single session owner.
async fn open_driver(
    driver: &dyn Provider,
    session: Option<SessionId>,
    workspace: &str,
    disposition: Disposition,
    model: Option<Box<str>>,
    permission: Option<Box<str>>,
) -> Result<Opened, Response> {
    let Ok(workspace) = AbsPath::canonicalize(workspace) else {
        return Err(refuse(&format!(
            "{workspace:?} is not a directory runtrol can work in"
        )));
    };
    let intent = OpenIntent {
        session: session.unwrap_or_else(SessionId::now),
        workspace,
        disposition,
        model,
        reasoning_effort: None,
        permission,
    };
    match driver.open(intent.clone()).await {
        Ok(agent) => Ok(Opened { intent, agent }),
        Err(error) => Err(Response::Failed(WireError::from_provider(&error))),
    }
}

/// Return a prepared model answer after verifying its request binding.
fn models(requested_provider: &str, prepared: Prepared) -> Reply {
    let prepared = match bound(prepared, PreparedKind::Models, requested_provider) {
        Ok(prepared) => prepared,
        Err(reply) => return reply,
    };
    match prepared {
        Prepared::Models { result, .. } => Reply::One(match result {
            Ok(catalogue) => Response::Models(catalogue),
            Err(response) => response,
        }),
        Prepared::Invalid { response, .. } => Reply::One(response),
        other => mismatched(other),
    }
}

/// Verify that provider process work cannot be replayed against a different request.
fn bound(
    prepared: Prepared,
    requested_kind: PreparedKind,
    requested_provider: &str,
) -> Result<Prepared, Reply> {
    let matches = match &prepared {
        // A consult answer never belongs to a provider request; its own binding is checked where it is used.
        Prepared::None
        | Prepared::Consult { .. }
        | Prepared::ProviderUpdates { .. }
        | Prepared::IntegrationAdmin { .. }
        | Prepared::PairingAdmin { .. }
        | Prepared::IsolatedWorkspacePrepare { .. }
        | Prepared::IsolatedWorkspaceRelease { .. } => false,
        Prepared::Invalid { kind, provider, .. } => {
            *kind == requested_kind && provider.as_ref() == requested_provider
        }
        Prepared::Models { provider, .. } => {
            requested_kind == PreparedKind::Models && provider.as_str() == requested_provider
        }
        Prepared::Start { provider, .. } => {
            requested_kind == PreparedKind::Start && provider.as_str() == requested_provider
        }
        Prepared::Resume { provider, .. } => {
            requested_kind == PreparedKind::Resume && provider.as_str() == requested_provider
        }
    };
    if matches {
        Ok(prepared)
    } else {
        Err(mismatched(prepared))
    }
}

fn mismatched(prepared: Prepared) -> Reply {
    let response = refuse("provider preparation does not belong to this request");
    match prepared {
        Prepared::Start {
            result: Ok(opened), ..
        }
        | Prepared::Resume {
            result: Ok(opened), ..
        } => cleanup_opened_with(response, opened),
        _ => Reply::One(response),
    }
}

fn cleanup_opened(opened: Opened) -> Reply {
    cleanup_opened_with(
        refuse("an opened process reached the session owner without a reservation"),
        opened,
    )
}

fn cleanup_opened_with(response: Response, opened: Opened) -> Reply {
    Reply::Cleaning {
        response,
        agents: vec![Cleanup {
            agent: opened.agent,
            how: CloseMode::Kill,
            reservation: None,
        }],
    }
}

fn hold_until_cleaned(
    mut reply: Reply,
    reservation: OpenReservation,
    sessions: &mut SessionManager,
) -> Reply {
    if let Reply::Cleaning { agents, .. } = &mut reply
        && let Some(cleanup) = agents.first_mut()
    {
        cleanup.reservation = Some(CleanupReservation::Open(reservation));
        return reply;
    }
    sessions.cancel_open(reservation);
    reply
}

/// Stop processes handed out by the owner when using the direct dispatcher convenience path.
#[cfg(test)]
async fn finish_cleanup(reply: Reply) -> (Reply, Vec<CleanupReservation>) {
    let Reply::Cleaning {
        mut response,
        agents,
    } = reply
    else {
        return (reply, Vec::new());
    };
    let mut failures = Vec::new();
    let mut reservations = Vec::new();
    for cleanup in agents {
        if let Err(error) = cleanup.agent.close(cleanup.how).await {
            failures.push(error.to_string());
        }
        reservations.extend(cleanup.reservation);
    }
    if !failures.is_empty()
        && let Response::Failed(error) = &response
    {
        response = refuse(&format!(
            "{}; cleanup also failed: {}",
            error.message,
            failures.join("; ")
        ));
    }
    (Reply::One(response), reservations)
}

#[cfg(test)]
fn checked_flags(
    provider: &str,
    driver: &runtrol_drivers::DriverKind,
    observed: runtrol_core::Flags,
) -> Result<crate::provider_prepare::CheckedFlags, Response> {
    crate::provider_prepare::checked_flags(provider, driver, observed)
        .map_err(|error| refuse(error.message()))
}

/// Hand a command to a live session.
fn send(sessions: &mut SessionManager, session: SessionId, command: AgentCommand) -> Reply {
    match sessions.take_agent(session) {
        Ok(taken) => Reply::Sending { taken, command },
        Err(error) => reply_from_session_error(&error),
    }
}

/// The sessions this daemon can see, projected to what this caller may see.
///
/// Joins runtrol's stored session pointers with the sessions that currently have a supervised process. Provider
/// transcript storage is not consulted, so listing never discovers, derives, or reads a transcript path.
pub(crate) fn list(composed: &Composed, sessions: &SessionManager, caller: &Caller) -> Response {
    let catalogue = match crate::session_catalogue::read(composed, sessions) {
        Ok(catalogue) => catalogue,
        Err(error) => return refuse(&error.to_string()),
    };
    let full = SessionListing {
        sessions: catalogue
            .sessions
            .into_iter()
            .map(|session| SessionLine {
                session: session.session,
                provider: session.provider.as_str().into(),
                native: session.native,
                label: session.label,
                workspace: session.workspace,
                hot: session.hot,
                doing: session.lifecycle.private_name().into(),
                waiting_on: session.waiting.map(private_waiting),
                looks_stuck: session.looks_stuck,
            })
            .collect(),
        warnings: catalogue.warnings,
        usage: usage_lines(composed, sessions),
    };
    Response::Sessions(sessions_visible_to(
        full,
        caller,
        &composed.device_authority,
    ))
}

/// Every service's latest account position, the turn reports and the probe reads merged, for the index.
fn usage_lines(
    composed: &Composed,
    sessions: &SessionManager,
) -> Vec<runtrol_ipc::wire::UsageLine> {
    let merged = crate::runtime_inventory::merge_probed_usage(
        &crate::runtime_inventory::provider_usage(&sessions.account_gauges()),
        composed,
    );
    let window = |window: runtrol_runtime_protocol::ProviderUsageWindow| {
        runtrol_ipc::wire::UsageWindowLine {
            id: window.id.into(),
            label: window.label.map(Into::into),
            scope: window.scope.map(Into::into),
            governing: window.governing,
            used_percent: window.used_percent,
            resets_at_ms: window.resets_at_ms,
            window_minutes: window.window_minutes,
        }
    };
    merged
        .providers
        .into_iter()
        .map(|gauge| runtrol_ipc::wire::UsageLine {
            provider: gauge.provider_id.as_str().into(),
            reached: gauge.reached,
            windows: gauge.windows.into_iter().map(window).collect(),
            tokens_today: gauge.tokens_today,
            at_ms: gauge.at_ms,
        })
        .collect()
}

const fn private_waiting(waiting: Waiting) -> SessionWaiting {
    match waiting {
        Waiting::Person => SessionWaiting::Person,
        Waiting::Quota => SessionWaiting::Quota,
    }
}

/// Project one full listing down to what a caller may know exists.
///
/// Somebody at the machine sees everything. A device sees exactly the rows inside its live workspace roots,
/// verified the same three ways that gate opening a session there, so a phone granted one project cannot
/// watch the names, paths, and timing of every other project on the machine. `session.list` is the scope to
/// ask the question; workspace grants bound the answer, and a device with none granted is answered with an
/// empty list rather than a refusal, because "you may ask, and nothing is yours to see" are both true.
///
/// A device view also carries no storage warnings: a warning names local rows and paths, which is operator
/// information a phone can do nothing about.
pub(crate) fn sessions_visible_to(
    full: SessionListing,
    caller: &Caller,
    authority: &crate::compose::DeviceAuthority,
) -> SessionListing {
    let Caller::Device { device } = caller else {
        return full;
    };
    let roots = authority.live_roots(*device);
    SessionListing {
        sessions: full
            .sessions
            .into_iter()
            .filter(|line| {
                AbsPath::new(&line.workspace)
                    .is_ok_and(|workspace| roots.iter().any(|root| workspace.is_under(root)))
            })
            .collect(),
        warnings: Vec::new(),
        // Account position is the operator's own, not a workspace's: the phone that may ask sees it whole.
        usage: full.usage,
    }
}

fn session_label(label: Option<Box<str>>) -> Result<Option<Box<str>>, &'static str> {
    let Some(label) = label else {
        return Ok(None);
    };
    let label = label.trim();
    if label.is_empty() {
        return Ok(None);
    }
    if label.chars().count() > 80 {
        return Err("a session name must be at most 80 characters");
    }
    if label.chars().any(forbidden_label_character) {
        return Err(
            "a session name must be one visible line without bidirectional control characters",
        );
    }
    Ok(Some(label.into()))
}

fn forbidden_label_character(character: char) -> bool {
    character.is_control()
        || matches!(
            character,
            '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'
        )
}

/// Persist the minimal pointer for one live session once its provider has named it.
///
/// No conversation value can enter this function: [`StoredSession`] has no field capable of holding one.
pub(crate) async fn persist_live(
    composed: &Composed,
    sessions: &SessionManager,
    session: SessionId,
) -> Result<(), StoreError> {
    persist_live_from_store(&composed.store, sessions, session).await
}

/// Persist one live pointer against an explicit store for public Runtime composition.
pub(crate) async fn persist_live_from_store(
    store: &std::sync::Arc<runtrol_store::Store>,
    sessions: &SessionManager,
    session: SessionId,
) -> Result<(), StoreError> {
    let Some(live) = sessions.live_session(session) else {
        return Ok(());
    };
    let Some(native) = live.native else {
        return Ok(());
    };
    let native = NativeSessionId::new(native).map_err(|_| StoreError::Codec {
        field: "native id",
        why: "the live provider identifier is not storable",
    })?;

    let prior_session = store.find_by_native(live.provider, &native)?;
    let prior_row = match prior_session {
        Some(prior) => store.get_session(prior)?,
        None => store.get_session(session)?,
    };
    let displaced = prior_session.filter(|prior| *prior != session);

    let now = WallMs::now();
    let row = StoredSession {
        provider: live.provider,
        native,
        cwd: live.workspace.clone(),
        label: prior_row.as_ref().and_then(|row| row.label.clone()),
        created_at: prior_row.as_ref().map_or(now, |row| row.created_at),
        last_seen_at: live.state.last_seen(),
        pinned: prior_row.as_ref().is_some_and(|row| row.pinned),
        archived: false,
        forked_from: prior_row.and_then(|row| row.forked_from),
        // The shared-daemon driver has no per-session process identity. A stale PID would be worse than
        // `None`, and hotness is joined from the live manager while this daemon is running.
        live: None,
    };
    // The durable writes run on a blocking worker: each is an fsync, and on the daemon's one async thread an
    // fsync held every accept and greeting for seconds on contended disks (measured 2026-08-27).
    let writer = std::sync::Arc::clone(store);
    match tokio::task::spawn_blocking(move || {
        if let Some(prior) = displaced {
            writer.remove_session(prior)?;
        }
        writer.put_session(session, &row)
    })
    .await
    {
        Ok(written) => written,
        Err(_worker) => Err(StoreError::Codec {
            field: "store worker",
            why: "the store worker ended before the session pointer was written",
        }),
    }
}

/// Every provider this build knows about, usable or not.
fn providers_of(composed: &Composed) -> Vec<ProviderLine> {
    composed
        .registry
        .all()
        .map(|provider| ProviderLine {
            id: provider.id().as_str().into(),
            display_name: provider.manifest.display_name.clone(),
            usable: provider.is_usable(),
            why_not: match provider.kind {
                KindStatus::Available => None,
                KindStatus::Unavailable { why } => Some(why.into()),
                KindStatus::Unknown => Some("nothing in this build declares that kind".into()),
            },
            terminal_commands: provider
                .manifest
                .tui
                .as_ref()
                .map_or_else(Vec::new, |_| provider.manifest.bin.names.clone()),
        })
        .collect()
}

/// A session failure, as the caller sees it.
///
/// The provider's own variant is preserved where there is one, because "not installed" and "authenticate at your
/// machine" are different next moves for the operator.
fn from_session_error(error: &SessionError) -> Response {
    match error {
        SessionError::Provider(provider) => Response::Failed(WireError::from_provider(provider)),
        other => refuse(&other.to_string()),
    }
}

/// One session failure as the connection should see it.
///
/// Only one variant is ever worth trying elsewhere, so only that one is distinguished. Reading the variant
/// rather than the rendered sentence is what keeps this from breaking the first time the wording changes.
fn reply_from_session_error(error: &SessionError) -> Reply {
    let response = from_session_error(error);
    match error {
        SessionError::NotLive { .. } => Reply::NotHere(response),
        _ => Reply::One(response),
    }
}

/// A refusal with a message and no claim about retrying.
pub(crate) fn refuse(message: &str) -> Response {
    Response::Failed(WireError::plain(message))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;
    use runtrol_provider::{Produced, ProviderError};

    use super::*;

    struct IdleAgent(SessionId);

    #[async_trait]
    impl Agent for IdleAgent {
        fn session(&self) -> SessionId {
            self.0
        }

        fn native(&self) -> Option<&str> {
            None
        }

        async fn send(&mut self, _command: AgentCommand) -> Result<(), ProviderError> {
            Ok(())
        }

        async fn next(&mut self) -> Option<Result<Produced, ProviderError>> {
            core::future::pending().await
        }

        async fn close(self: Box<Self>, _how: CloseMode) -> Result<(), ProviderError> {
            Ok(())
        }
    }

    fn composed_for(name: &str) -> (crate::compose::Composed, String) {
        let root = std::env::temp_dir().join(format!("runtrol-dispatch-{name}"));
        if root.exists() {
            std::fs::remove_dir_all(&root).expect("clear the previous run");
        }
        let text = root
            .to_str()
            .expect("the temporary path is UTF-8")
            .to_owned();
        // Composing without establishing containment: doing that in a test terminates the runner on one platform.
        let composed = crate::compose::Composed::for_tests(&text, runtrol_drivers::builtin())
            .expect("a fresh home composes");
        (composed, text)
    }

    fn clean(composed: crate::compose::Composed, path: &str) {
        // The store owns an exclusive file handle. Release it before removing the scratch home, especially on Windows.
        drop(composed);
        std::fs::remove_dir_all(path).expect("remove the scratch home");
    }

    fn attach_and_store(
        composed: &Composed,
        sessions: &mut SessionManager,
        session: SessionId,
        path: &str,
    ) {
        let provider = ProviderId::parse("stored-provider").expect("valid provider id");
        let workspace = AbsPath::canonicalize(path).expect("the scratch home exists");
        let intent = OpenIntent {
            session,
            workspace: workspace.clone(),
            disposition: Disposition::Fresh,
            model: None,
            reasoning_effort: None,
            permission: None,
        };
        let claim = runtrol_core::WorkspaceClaim::discover(
            workspace.clone(),
            runtrol_provider::WorkspaceAccess::Shared,
        )
        .expect("the scratch workspace has an identity");
        let reserved = sessions
            .reserve_open(session, claim)
            .expect("one process slot");
        sessions
            .attach_opened(
                reserved.reservation,
                provider,
                &intent,
                Box::new(IdleAgent(session)),
            )
            .expect("the test process attaches");
        let now = WallMs::now();
        composed
            .store
            .put_session(
                session,
                &StoredSession {
                    provider,
                    native: NativeSessionId::new("provider-session-1").expect("valid native id"),
                    cwd: workspace,
                    label: None,
                    created_at: now,
                    last_seen_at: now,
                    pinned: false,
                    archived: false,
                    forked_from: None,
                    live: None,
                },
            )
            .expect("store the pointer");
    }

    fn store_test_phone(composed: &Composed, device: runtrol_security::DeviceId) {
        composed
            .store
            .put_device(
                runtrol_store::DeviceKey::from_bytes(*device.as_bytes()),
                &runtrol_store::DeviceRow {
                    remote_static_key: [0x73; 32],
                    credential_fingerprint: [0x74; 32],
                    name: "Test phone".into(),
                    platform: "Web Push".into(),
                    scopes: vec!["session.output.read".into()],
                    roots: Vec::new(),
                    push_endpoint: None,
                    paired_at: WallMs::now(),
                },
            )
            .expect("store paired phone");
        composed
            .reload_device_authority()
            .expect("restore paired phone");
    }

    /// Pair a phone against one on-disk root, optionally granting the workspace scope for it.
    ///
    /// Returns the device so a test can speak as it. The root row is always stored; whether the matching
    /// `workspace(...)` scope is granted is the variable under test, because a stored path without a live
    /// grant must disclose nothing.
    fn pair_phone_for_root(
        composed: &Composed,
        root: &AbsPath,
        grant_workspace: bool,
    ) -> runtrol_security::DeviceId {
        let device = runtrol_security::DeviceId::now();
        let root_id = runtrol_security::WorkspaceRootId::now();
        let identity = runtrol_security::ProjectRootIdentity::read(root)
            .expect("the scratch root has a filesystem identity");
        let mut scopes: Vec<Box<str>> = vec![
            runtrol_security::DeviceScope::SessionList
                .to_string()
                .into(),
            runtrol_security::DeviceScope::SessionInputWrite
                .to_string()
                .into(),
        ];
        if grant_workspace {
            scopes.push(
                runtrol_security::DeviceScope::Workspace(root_id)
                    .to_string()
                    .into(),
            );
        }
        composed
            .store
            .put_device(
                runtrol_store::DeviceKey::from_bytes(*device.as_bytes()),
                &runtrol_store::DeviceRow {
                    remote_static_key: [0x51; 32],
                    credential_fingerprint: [0x52; 32],
                    name: "Projection phone".into(),
                    platform: "Test OS".into(),
                    scopes,
                    roots: vec![runtrol_store::DeviceRootRow {
                        id: *root_id.as_bytes(),
                        path: root.as_str().into(),
                        identity: identity.to_bytes(),
                    }],
                    push_endpoint: None,
                    paired_at: WallMs::now(),
                },
            )
            .expect("store paired phone");
        composed
            .reload_device_authority()
            .expect("restore paired phone");
        device
    }

    fn listing_row(workspace: &str) -> SessionLine {
        SessionLine {
            session: SessionId::now(),
            provider: "stored-provider".into(),
            native: None,
            label: None,
            workspace: workspace.into(),
            hot: false,
            doing: "idle".into(),
            waiting_on: None,
            looks_stuck: false,
        }
    }

    #[test]
    fn a_phone_sees_only_the_sessions_inside_its_granted_roots() {
        let (composed, path) = composed_for("phone-projection");
        let granted_dir = std::path::Path::new(&path).join("granted");
        std::fs::create_dir(&granted_dir).expect("create the granted root");
        let granted = AbsPath::canonicalize(granted_dir.to_str().expect("UTF-8 scratch path"))
            .expect("canonical granted root");
        let device = pair_phone_for_root(&composed, &granted, true);

        let mut inside = listing_row(granted.as_str());
        inside.waiting_on = Some(SessionWaiting::Person);
        let nested = listing_row(&format!(
            "{}{}deeper",
            granted.as_str(),
            std::path::MAIN_SEPARATOR
        ));
        let elsewhere = listing_row(&path);
        let full = SessionListing {
            sessions: vec![inside.clone(), nested.clone(), elsewhere.clone()],
            warnings: vec!["one damaged row was skipped".into()],
            usage: Vec::new(),
        };

        let machine = sessions_visible_to(
            full.clone(),
            &Caller::AtTheMachine,
            &composed.device_authority,
        );
        assert_eq!(machine.sessions.len(), 3, "the machine sees everything");
        assert_eq!(machine.warnings.len(), 1, "the machine keeps warnings");

        let phone =
            sessions_visible_to(full, &Caller::Device { device }, &composed.device_authority);
        let seen: Vec<_> = phone.sessions.iter().map(|line| line.session).collect();
        assert_eq!(
            seen,
            vec![inside.session, nested.session],
            "exactly the rows under the granted root, nothing beside it"
        );
        assert_eq!(
            phone.sessions.first().and_then(|line| line.waiting_on),
            Some(SessionWaiting::Person),
            "the root projection keeps the bounded attention fact"
        );
        assert!(
            phone.warnings.is_empty(),
            "storage warnings are operator information"
        );
        clean(composed, &path);
    }

    #[test]
    fn a_phone_without_the_workspace_grant_sees_an_empty_list() {
        let (composed, path) = composed_for("phone-ungranted");
        let granted_dir = std::path::Path::new(&path).join("almost");
        std::fs::create_dir(&granted_dir).expect("create the root");
        let root = AbsPath::canonicalize(granted_dir.to_str().expect("UTF-8 scratch path"))
            .expect("canonical root");
        let device = pair_phone_for_root(&composed, &root, false);

        let full = SessionListing {
            sessions: vec![listing_row(root.as_str())],
            warnings: Vec::new(),
            usage: Vec::new(),
        };
        let phone =
            sessions_visible_to(full, &Caller::Device { device }, &composed.device_authority);
        assert!(
            phone.sessions.is_empty(),
            "a stored root without a live workspace grant discloses nothing"
        );
        clean(composed, &path);
    }

    #[test]
    fn a_replaced_directory_disappears_from_the_phone_view() {
        let (composed, path) = composed_for("phone-swapped-root");
        let granted_dir = std::path::Path::new(&path).join("swapped");
        std::fs::create_dir(&granted_dir).expect("create the root");
        let root = AbsPath::canonicalize(granted_dir.to_str().expect("UTF-8 scratch path"))
            .expect("canonical root");
        let device = pair_phone_for_root(&composed, &root, true);

        std::fs::remove_dir(&granted_dir).expect("remove the approved directory");
        std::fs::create_dir(&granted_dir).expect("replace the approved directory");

        let full = SessionListing {
            sessions: vec![listing_row(root.as_str())],
            warnings: Vec::new(),
            usage: Vec::new(),
        };
        let phone =
            sessions_visible_to(full, &Caller::Device { device }, &composed.device_authority);
        assert!(
            phone.sessions.is_empty(),
            "disclosure uses the same identity verification as opening, so a replacement directory shows nothing"
        );
        clean(composed, &path);
    }

    #[tokio::test]
    async fn a_phone_cannot_touch_a_session_outside_its_roots() {
        // The action-side twin of the listing projection: a session id learned before a root was revoked
        // (or guessed) must stop working the moment the root does, with a refusal that says no more than
        // the projection shows.
        let (composed, path) = composed_for("phone-touch-bound");
        let mut sessions = SessionManager::new();
        let session = SessionId::now();
        attach_and_store(&composed, &mut sessions, session, &path);

        let outside_dir = std::path::Path::new(&path).join("elsewhere");
        std::fs::create_dir(&outside_dir).expect("create the ungranted root");
        let outside = AbsPath::canonicalize(outside_dir.to_str().expect("UTF-8 scratch path"))
            .expect("canonical ungranted root");
        let device = pair_phone_for_root(&composed, &outside, true);

        let mut conversation = Conversation::from_device(device);
        match answer(
            &mut conversation,
            &composed,
            &mut sessions,
            Request::Hello {
                wire: runtrol_ipc::WIRE_VERSION,
            },
        )
        .await
        {
            Reply::One(Response::Welcome { .. }) => {}
            other => panic!("expected the greeting, got {}", shape(&other)),
        }
        match answer(
            &mut conversation,
            &composed,
            &mut sessions,
            Request::Prompt {
                session,
                text: "hello".into(),
            },
        )
        .await
        {
            Reply::One(Response::Failed(failure)) => {
                assert!(
                    failure
                        .message
                        .contains("not approved for that session's workspace"),
                    "{}",
                    failure.message
                );
            }
            other => panic!("expected the workspace refusal, got {}", shape(&other)),
        }

        // The same phone, granted the session's own root, reaches the agent.
        let covering = AbsPath::canonicalize(&path).expect("the scratch home canonicalizes");
        let allowed_device = pair_phone_for_root(&composed, &covering, true);
        let mut allowed = Conversation::from_device(allowed_device);
        match answer(
            &mut allowed,
            &composed,
            &mut sessions,
            Request::Hello {
                wire: runtrol_ipc::WIRE_VERSION,
            },
        )
        .await
        {
            Reply::One(Response::Welcome { .. }) => {}
            other => panic!("expected the greeting, got {}", shape(&other)),
        }
        match answer(
            &mut allowed,
            &composed,
            &mut sessions,
            Request::Prompt {
                session,
                text: "hello".into(),
            },
        )
        .await
        {
            Reply::Sending { taken, .. } => {
                assert!(sessions.return_agent(taken.lease, taken.agent).is_ok());
            }
            other => panic!(
                "expected the prompt to reach the agent, got {}",
                shape(&other)
            ),
        }
        clean(composed, &path);
    }

    #[tokio::test]
    async fn nothing_can_be_asked_before_the_wire_format_is_agreed() {
        // Acting on a request from a build that speaks a different format means acting on somebody else's meaning, and
        // the failure that produces is a command landing where the operator did not intend.
        let (composed, path) = composed_for("ungreeted");
        let mut sessions = SessionManager::new();
        let mut conversation = Conversation::at_the_machine();
        assert!(!conversation.greeted());

        match answer(&mut conversation, &composed, &mut sessions, Request::List).await {
            Reply::One(Response::Failed(failure)) => {
                assert!(
                    failure.message.contains("wire format"),
                    "{}",
                    failure.message
                );
            }
            other => panic!("expected a refusal, got {}", shape(&other)),
        }
        clean(composed, &path);
    }

    #[tokio::test]
    async fn the_greeting_answers_with_every_provider_this_build_knows() {
        let (composed, path) = composed_for("greeting");
        let mut sessions = SessionManager::new();
        let mut conversation = Conversation::at_the_machine();

        match answer(
            &mut conversation,
            &composed,
            &mut sessions,
            Request::Hello {
                wire: runtrol_ipc::WIRE_VERSION,
            },
        )
        .await
        {
            Reply::One(Response::Welcome {
                wire,
                providers,
                device,
                push_public_key,
                build_digest,
            }) => {
                assert_eq!(wire, runtrol_ipc::WIRE_VERSION);
                assert!(!providers.is_empty(), "a fresh install has providers");
                assert!(providers.iter().any(|one| one.usable));
                assert!(device.is_none());
                assert!(push_public_key.is_none());
                assert!(
                    build_digest.is_some_and(|digest| digest.len() == 64),
                    "the greeting announces this executable's digest for supersession",
                );
            }
            other => panic!("expected a welcome, got {}", shape(&other)),
        }
        assert!(conversation.greeted());
        clean(composed, &path);
    }

    #[tokio::test]
    async fn an_authenticated_phone_owns_one_encrypted_push_subscription() {
        let (mut composed, path) = composed_for("push-subscription");
        composed.push_identity = Some(Arc::new(
            runtrol_transport::PushIdentity::derive(&[0x71; 32]).expect("test push identity"),
        ));
        let device = runtrol_security::DeviceId::now();
        let key = runtrol_store::DeviceKey::from_bytes(*device.as_bytes());
        store_test_phone(&composed, device);

        let mut sessions = SessionManager::new();
        let mut phone = Conversation::from_device(device);
        assert!(matches!(
            answer(
                &mut phone,
                &composed,
                &mut sessions,
                Request::Hello {
                    wire: runtrol_ipc::WIRE_VERSION,
                },
            )
            .await,
            Reply::One(Response::Welcome {
                push_public_key: Some(_),
                ..
            })
        ));

        let endpoint = "https://fcm.googleapis.com/fcm/send/private-capability";
        assert!(matches!(
            answer(
                &mut phone,
                &composed,
                &mut sessions,
                Request::PushSubscription {
                    endpoint: Some(endpoint.into()),
                },
            )
            .await,
            Reply::One(Response::Done)
        ));
        let stored = composed
            .store
            .get_device(key)
            .expect("read paired phone")
            .expect("paired phone remains");
        let sealed = stored.push_endpoint.expect("encrypted subscription");
        assert!(
            !sealed
                .windows(endpoint.len())
                .any(|window| window == endpoint.as_bytes())
        );
        composed
            .push_identity
            .as_ref()
            .expect("push identity")
            .validate_stored_endpoint(*device.as_bytes(), &sealed)
            .expect("same phone restores subscription");
        assert_eq!(composed.device_authority.push_targets().len(), 1);

        let mut local = Conversation::at_the_machine();
        assert!(matches!(
            answer(
                &mut local,
                &composed,
                &mut sessions,
                Request::Hello {
                    wire: runtrol_ipc::WIRE_VERSION,
                },
            )
            .await,
            Reply::One(Response::Welcome { .. })
        ));
        assert!(matches!(
            answer(
                &mut local,
                &composed,
                &mut sessions,
                Request::PushSubscription {
                    endpoint: Some(endpoint.into()),
                },
            )
            .await,
            Reply::One(Response::Failed(_))
        ));

        assert!(matches!(
            answer(
                &mut phone,
                &composed,
                &mut sessions,
                Request::PushSubscription { endpoint: None },
            )
            .await,
            Reply::One(Response::Done)
        ));
        assert!(
            composed
                .store
                .get_device(key)
                .expect("read cleared phone")
                .expect("paired phone remains")
                .push_endpoint
                .is_none()
        );
        clean(composed, &path);
    }

    #[tokio::test]
    async fn a_caller_speaking_another_wire_format_is_told_both_numbers() {
        let (composed, path) = composed_for("mismatch");
        let mut sessions = SessionManager::new();
        let mut conversation = Conversation::at_the_machine();

        match answer(
            &mut conversation,
            &composed,
            &mut sessions,
            Request::Hello {
                wire: runtrol_ipc::WIRE_VERSION + 1,
            },
        )
        .await
        {
            Reply::One(Response::Failed(failure)) => {
                assert!(
                    failure
                        .message
                        .contains(&runtrol_ipc::WIRE_VERSION.to_string()),
                    "{}",
                    failure.message
                );
                assert!(
                    failure
                        .message
                        .contains(&(runtrol_ipc::WIRE_VERSION + 1).to_string()),
                    "{}",
                    failure.message
                );
            }
            other => panic!("expected a refusal, got {}", shape(&other)),
        }
        assert!(
            !conversation.greeted(),
            "a connection that did not agree must not count as greeted"
        );
        clean(composed, &path);
    }

    #[tokio::test]
    async fn a_command_for_a_session_that_is_not_live_is_refused_by_name() {
        let (composed, path) = composed_for("absent");
        let mut sessions = SessionManager::new();
        let mut conversation = Conversation::at_the_machine();
        greet(&mut conversation, &composed, &mut sessions).await;

        let absent = SessionId::now();
        for request in [
            Request::Prompt {
                session: absent,
                text: "anything".into(),
            },
            Request::Interrupt { session: absent },
            Request::Watch {
                session: absent,
                after: None,
            },
            Request::Close {
                session: absent,
                now: true,
            },
        ] {
            match answer(&mut conversation, &composed, &mut sessions, request).await {
                // Refused either way. Some of these requests reach the session table and are marked as the one
                // refusal a connection may still try elsewhere (a session this generation does not hold may be
                // held by the one draining beside it); others are refused before they get that far. What this
                // test is about is that the refusal names the session, which both shapes must do.
                Reply::One(Response::Failed(failure))
                | Reply::NotHere(Response::Failed(failure)) => {
                    assert!(
                        failure.message.contains(&absent.to_string()),
                        "the refusal has to name the session: {}",
                        failure.message
                    );
                }
                other => panic!("expected a refusal, got {}", shape(&other)),
            }
        }
        clean(composed, &path);
    }

    #[tokio::test]
    async fn a_provider_nobody_declared_is_refused_rather_than_started() {
        let (composed, path) = composed_for("noprovider");
        let mut sessions = SessionManager::new();
        let mut conversation = Conversation::at_the_machine();
        greet(&mut conversation, &composed, &mut sessions).await;

        match answer(
            &mut conversation,
            &composed,
            &mut sessions,
            Request::Start {
                provider: "nothing-declares-this".into(),
                workspace: std::env::temp_dir().to_string_lossy().into_owned().into(),
                workspace_access: runtrol_provider::WorkspaceAccess::Exclusive,
                model: None,
                permission: None,
            },
        )
        .await
        {
            Reply::One(Response::Failed(failure)) => {
                assert!(
                    failure.message.contains("nothing-declares-this"),
                    "{}",
                    failure.message
                );
            }
            other => panic!("expected a refusal, got {}", shape(&other)),
        }
        assert_eq!(sessions.hot(), 0, "nothing was started");
        clean(composed, &path);
    }

    #[tokio::test]
    async fn a_workspace_that_is_not_a_directory_is_refused_before_anything_starts() {
        // The one field on this path that names a place on the operator's disk. A start that accepted it would put an
        // agent somewhere nobody chose.
        let (composed, path) = composed_for("noworkspace");
        let mut sessions = SessionManager::new();
        let mut conversation = Conversation::at_the_machine();
        greet(&mut conversation, &composed, &mut sessions).await;

        let provider = composed
            .registry
            .usable()
            .next()
            .expect("a usable provider")
            .id()
            .as_str()
            .to_owned();

        match answer(
            &mut conversation,
            &composed,
            &mut sessions,
            Request::Start {
                provider: provider.into(),
                workspace: "this/is/not/a/real/place".into(),
                workspace_access: runtrol_provider::WorkspaceAccess::Exclusive,
                model: None,
                permission: None,
            },
        )
        .await
        {
            Reply::One(Response::Failed(_)) => {}
            other => panic!("expected a refusal, got {}", shape(&other)),
        }
        assert_eq!(sessions.hot(), 0);
        clean(composed, &path);
    }

    #[tokio::test]
    async fn listing_with_nothing_running_is_an_empty_list_and_not_a_failure() {
        let (composed, path) = composed_for("emptylist");
        let mut sessions = SessionManager::new();
        let mut conversation = Conversation::at_the_machine();
        greet(&mut conversation, &composed, &mut sessions).await;

        match answer(&mut conversation, &composed, &mut sessions, Request::List).await {
            Reply::One(Response::Sessions(listing)) => {
                assert!(listing.sessions.is_empty());
                assert!(listing.warnings.is_empty());
            }
            other => panic!("expected a listing, got {}", shape(&other)),
        }
        clean(composed, &path);
    }

    #[tokio::test]
    async fn a_stored_session_is_listed_cold_and_can_be_removed_without_a_process() {
        // A daemon restart begins with an empty live manager. The durable pointer must still appear, and closing that
        // cold row removes only runtrol's pointer rather than requiring a process that no longer exists.
        let (composed, path) = composed_for("storedlist");
        let mut sessions = SessionManager::new();
        let mut conversation = Conversation::at_the_machine();
        greet(&mut conversation, &composed, &mut sessions).await;

        let session = SessionId::now();
        let provider = ProviderId::parse("stored-provider").expect("valid provider id");
        let native = NativeSessionId::new("provider-session-1").expect("valid native id");
        let workspace = AbsPath::canonicalize(&path).expect("the scratch home exists");
        let now = WallMs::now();
        composed
            .store
            .put_session(
                session,
                &StoredSession {
                    provider,
                    native: native.clone(),
                    cwd: workspace.clone(),
                    label: None,
                    created_at: now,
                    last_seen_at: now,
                    pinned: false,
                    archived: false,
                    forked_from: None,
                    live: None,
                },
            )
            .expect("store the pointer");

        match answer(&mut conversation, &composed, &mut sessions, Request::List).await {
            Reply::One(Response::Sessions(listing)) => {
                assert!(listing.warnings.is_empty());
                let [line] = listing.sessions.as_slice() else {
                    panic!("expected one stored session, got {:?}", listing.sessions);
                };
                assert_eq!(line.session, session);
                assert_eq!(line.provider.as_ref(), provider.as_str());
                assert_eq!(line.native.as_deref(), Some(native.as_str()));
                assert_eq!(line.workspace.as_ref(), workspace.as_str());
                assert!(!line.hot);
                assert_eq!(line.doing.as_ref(), "detached");
            }
            other => panic!("expected a listing, got {}", shape(&other)),
        }

        assert!(matches!(
            answer(
                &mut conversation,
                &composed,
                &mut sessions,
                Request::Close {
                    session,
                    now: false
                },
            )
            .await,
            Reply::One(Response::Done)
        ));
        assert!(
            composed
                .store
                .get_session(session)
                .expect("the store remains readable")
                .is_none(),
            "closing a cold row removes its pointer"
        );
        clean(composed, &path);
    }

    #[tokio::test]
    async fn renaming_changes_only_the_display_name_and_rejects_spoofing_controls() {
        let (composed, path) = composed_for("rename-session");
        let mut sessions = SessionManager::new();
        let mut conversation = Conversation::at_the_machine();
        greet(&mut conversation, &composed, &mut sessions).await;
        let session = SessionId::now();
        attach_and_store(&composed, &mut sessions, session, &path);

        assert!(matches!(
            answer(
                &mut conversation,
                &composed,
                &mut sessions,
                Request::Rename {
                    session,
                    label: Some("  Release repair  ".into()),
                },
            )
            .await,
            Reply::One(Response::Done)
        ));
        let renamed = composed
            .store
            .get_session(session)
            .expect("the store remains readable")
            .expect("the session remains present");
        assert_eq!(renamed.label.as_deref(), Some("Release repair"));

        match answer(
            &mut conversation,
            &composed,
            &mut sessions,
            Request::Rename {
                session,
                label: Some("safe\u{202E}fake".into()),
            },
        )
        .await
        {
            Reply::One(Response::Failed(failure)) => {
                assert!(
                    failure.message.contains("bidirectional"),
                    "{}",
                    failure.message
                );
            }
            other => panic!("expected a refusal, got {}", shape(&other)),
        }
        clean(composed, &path);
    }

    #[tokio::test]
    async fn a_close_refused_while_an_agent_is_in_flight_keeps_the_stored_session() {
        let (composed, path) = composed_for("close-in-flight-row");
        let mut sessions = SessionManager::new();
        let mut conversation = Conversation::at_the_machine();
        greet(&mut conversation, &composed, &mut sessions).await;
        let session = SessionId::now();
        attach_and_store(&composed, &mut sessions, session, &path);
        let taken = sessions
            .take_agent(session)
            .expect("a provider command owns the agent");

        let reply = answer(
            &mut conversation,
            &composed,
            &mut sessions,
            Request::Close { session, now: true },
        )
        .await;
        assert!(
            matches!(reply, Reply::One(Response::Failed(_))),
            "an in-flight close must be refused"
        );
        assert!(
            composed
                .store
                .get_session(session)
                .expect("the store remains readable")
                .is_some(),
            "a refused close must not delete its durable pointer"
        );

        sessions.abandon_agent(taken.lease);
        drop(taken.agent);
        clean(composed, &path);
    }

    #[tokio::test]
    async fn a_close_refused_by_reservation_generation_exhaustion_keeps_the_stored_session() {
        let (composed, path) = composed_for("close-generation-row");
        let mut sessions = SessionManager::new();
        let mut conversation = Conversation::at_the_machine();
        greet(&mut conversation, &composed, &mut sessions).await;
        let session = SessionId::now();
        attach_and_store(&composed, &mut sessions, session, &path);
        sessions.exhaust_reservation_generations_for_tests();

        let reply = answer(
            &mut conversation,
            &composed,
            &mut sessions,
            Request::Close {
                session,
                now: false,
            },
        )
        .await;
        assert!(
            matches!(reply, Reply::One(Response::Failed(_))),
            "generation exhaustion must refuse the close"
        );
        assert!(
            sessions.is_live(session),
            "the manager kept the live process"
        );
        assert!(
            composed
                .store
                .get_session(session)
                .expect("the store remains readable")
                .is_some(),
            "a refused close must not delete its durable pointer"
        );

        drop(sessions);
        clean(composed, &path);
    }

    #[tokio::test]
    async fn the_panic_button_consults_nothing_and_reports_what_happened() {
        // It has to work from anywhere with no permission at all. What it must not do is report a success it did not
        // achieve, and this daemon holds a containment that deliberately holds nothing.
        let (composed, path) = composed_for("panic");
        let mut sessions = SessionManager::new();
        let mut conversation = Conversation::at_the_machine();
        greet(&mut conversation, &composed, &mut sessions).await;

        match answer(
            &mut conversation,
            &composed,
            &mut sessions,
            Request::StopEverything,
        )
        .await
        {
            Reply::One(Response::Failed(failure)) => {
                assert!(
                    failure.message.contains("holds nothing"),
                    "a kill that did nothing must say so: {}",
                    failure.message
                );
            }
            other => panic!(
                "expected the refusal this containment gives, got {}",
                shape(&other)
            ),
        }
        clean(composed, &path);
    }

    #[test]
    fn a_missing_required_driver_flag_is_refused_with_its_consequence() {
        let driver = runtrol_drivers::kinds::lookup("claude-stream-json").expect("built-in kind");
        let observed = runtrol_core::Flags::Observed(
            driver
                .flags
                .iter()
                .filter(|flag| flag.flag != "--permission-prompt-tool")
                .map(|flag| flag.flag.to_owned())
                .collect(),
        );

        match checked_flags("fixture", driver, observed) {
            Err(Response::Failed(failure)) => {
                assert!(failure.message.contains("--permission-prompt-tool"));
                assert!(failure.message.contains("approvals cannot be brokered"));
            }
            other => panic!("a required flag was accepted: {other:?}"),
        }
    }

    #[test]
    fn a_missing_optional_driver_flag_is_left_out_of_the_available_set() {
        let driver = runtrol_drivers::kinds::lookup("claude-stream-json").expect("built-in kind");
        let observed = runtrol_core::Flags::Observed(
            driver
                .flags
                .iter()
                .filter(|flag| flag.flag != "--include-partial-messages")
                .map(|flag| flag.flag.to_owned())
                .collect(),
        );

        let checked = checked_flags("fixture", driver, observed).expect("required flags remain");
        assert!(!checked.available.contains("--include-partial-messages"));
        assert_eq!(
            checked
                .unavailable
                .get("--include-partial-messages")
                .copied(),
            Some("a message appears all at once instead of as it is written")
        );
    }

    #[test]
    fn an_unknown_parser_result_only_refuses_required_flags() {
        let optional = runtrol_drivers::DriverKind {
            kind: "fixture",
            make: None,
            consult: runtrol_drivers::ConsultSurface::NONE,
            flags: &[runtrol_drivers::kinds::DriverFlag {
                flag: "--optional",
                required: false,
                without_it: "the optional feature remains unavailable to this session",
            }],
            unavailable: Some("a test fixture with no implementation"),
        };
        let checked = checked_flags(
            "fixture",
            &optional,
            runtrol_core::Flags::Unknown {
                why: "the parser said nothing".to_owned(),
            },
        )
        .expect("an optional-only driver can degrade explicitly");

        assert!(checked.available.is_empty());
        assert_eq!(
            checked.unavailable.get("--optional").copied(),
            Some("the optional feature remains unavailable to this session")
        );
    }

    #[tokio::test]
    async fn prepared_provider_work_cannot_be_replayed_for_another_provider() {
        let (composed, path) = composed_for("prepared-binding");
        let mut sessions = SessionManager::new();
        let mut conversation = Conversation::at_the_machine();
        greet(&mut conversation, &composed, &mut sessions).await;
        let prepared_for = ProviderId::parse("prepared-for").expect("valid test provider");

        let reply = answer_prepared(
            &mut conversation,
            &composed,
            &mut sessions,
            Request::Models {
                provider: "asked-for".into(),
            },
            Prepared::Models {
                provider: prepared_for,
                result: Ok(ModelCatalog::unknown("test catalogue")),
            },
            None,
        )
        .await;

        match reply {
            Reply::One(Response::Failed(error)) => {
                assert!(
                    error.message.contains("does not belong"),
                    "{}",
                    error.message
                );
            }
            other => panic!("expected a fail-closed refusal, got {}", shape(&other)),
        }

        let reply = answer_prepared(
            &mut conversation,
            &composed,
            &mut sessions,
            Request::Start {
                provider: prepared_for.as_str().into(),
                workspace: std::env::temp_dir().to_string_lossy().into_owned().into(),
                workspace_access: runtrol_provider::WorkspaceAccess::Exclusive,
                model: None,
                permission: None,
            },
            Prepared::Models {
                provider: prepared_for,
                result: Ok(ModelCatalog::unknown("test catalogue")),
            },
            None,
        )
        .await;
        assert!(
            matches!(reply, Reply::One(Response::Failed(_))),
            "a model result was accepted as an opened session"
        );
        clean(composed, &path);
    }

    #[tokio::test]
    async fn drain_is_never_refused_whatever_the_sessions_are_doing() {
        let (composed, path) = composed_for("drain-never-refused");
        let mut sessions = SessionManager::new();
        let session = SessionId::now();
        attach_and_store(&composed, &mut sessions, session, &path);

        // The successor is already listening and needs the store; whether a turn is running only
        // decides when this process ends, and that decision belongs to the owner loop.
        let mut conversation = Conversation::at_the_machine();
        greet(&mut conversation, &composed, &mut sessions).await;
        match answer(&mut conversation, &composed, &mut sessions, Request::Drain).await {
            Reply::Draining => {}
            other => panic!(
                "expected the drain past a live process, got {}",
                shape(&other)
            ),
        }

        let mut idle = SessionManager::new();
        let mut fresh = Conversation::at_the_machine();
        greet(&mut fresh, &composed, &mut idle).await;
        match answer(&mut fresh, &composed, &mut idle, Request::Drain).await {
            Reply::Draining => {}
            other => panic!("expected the drain, got {}", shape(&other)),
        }
        clean(composed, &path);
    }

    /// Agree a wire format, so the rest of a test can ask for something.
    async fn greet(
        conversation: &mut Conversation,
        composed: &Composed,
        sessions: &mut SessionManager,
    ) {
        let reply = answer(
            conversation,
            composed,
            sessions,
            Request::Hello {
                wire: runtrol_ipc::WIRE_VERSION,
            },
        )
        .await;
        assert!(matches!(reply, Reply::One(Response::Welcome { .. })));
    }

    /// What a reply is, for a message that has to say what arrived instead.
    fn shape(reply: &Reply) -> String {
        match reply {
            Reply::One(response) => format!("{response:?}"),
            Reply::NotHere(response) => format!("not this generation: {response:?}"),
            Reply::Watching(_) | Reply::WatchingSessions => "a subscription".to_owned(),
            Reply::Draining => "a drain".to_owned(),
            Reply::Stopping { how, .. } => format!("a process still stopping, {how:?}"),
            Reply::Cleaning { agents, .. } => format!("{} processes still stopping", agents.len()),
            Reply::Sending { .. } => "a provider command in flight".to_owned(),
            Reply::Updating { provider, .. } => format!("provider {provider} updating"),
        }
    }
}
