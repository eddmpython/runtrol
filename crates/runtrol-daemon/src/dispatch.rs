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
use runtrol_core::{ClosingReservation, OpenReservation, SessionManager, SessionView, TakenAgent};
use runtrol_ipc::wire::{ProviderLine, Request, Response, SessionLine, SessionListing, WireError};
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
    /// One exact Mission Task workspace was prepared outside the session owner.
    MissionWorkspace {
        /// Bound Mission identity.
        mission_id: Box<str>,
        /// Bound Task identity.
        task_id: Box<str>,
        /// Exact preparation result.
        response: Response,
    },
    /// One exact Mission Task verification completed outside the session owner.
    MissionVerification {
        /// Bound Mission identity.
        mission_id: Box<str>,
        /// Bound Task identity.
        task_id: Box<str>,
        /// Exact verification result.
        response: Response,
    },
    /// One exact Mission integrated-tree verification completed outside the session owner.
    MissionIntegration {
        /// Bound Mission identity.
        mission_id: Box<str>,
        /// Exact integration result.
        response: Response,
    },
    /// One exact capability candidate verification completed outside the session owner.
    CapabilityVerification {
        /// Bound canonical project text.
        project: Box<str>,
        /// Bound stable capability identity.
        capability_id: Box<str>,
        /// Bound exact candidate digest.
        version_sha256: Box<str>,
        /// Exact verification result.
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
        && crate::scope::allowed(&conversation.caller, &request, &composed.granted).is_ok()
    {
        let session = SessionId::now();
        match sessions.reserve_open_for_tests(session) {
            Ok(reserved) => Some(reserved),
            Err(error) => return Reply::One(from_session_error(&error)),
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
    let (reply, reservations) = finish_cleanup(answer_prepared(
        conversation,
        composed,
        sessions,
        request,
        prepared,
        reservation,
    ))
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
        || crate::scope::allowed(&conversation.caller, request, &composed.granted).is_err()
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
        || crate::scope::allowed(&conversation.caller, request, &composed.granted).is_err()
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
) -> Prepared {
    if !matches!(request, Request::ProviderUpdates)
        || !conversation.greeted()
        || crate::scope::allowed(&conversation.caller, request, &composed.granted).is_err()
    {
        return Prepared::None;
    }
    Prepared::ProviderUpdates {
        response: Response::ProviderUpdates(crate::provider_update::inspect_all(composed).await),
    }
}

/// Whether the request belongs to local public-integration administration.
pub(crate) const fn is_integration_admin(request: &Request) -> bool {
    matches!(
        request,
        Request::IntegrationEnrollments
            | Request::IntegrationApprovalBegin { .. }
            | Request::IntegrationApprovalFinish { .. }
            | Request::IntegrationEnrollmentDeny { .. }
            | Request::Integrations
            | Request::IntegrationRevoke { .. }
            | Request::IntegrationGrantChange { .. }
            | Request::RuntimeForgetRequests
            | Request::RuntimeForgetConfirm { .. }
            | Request::RuntimeKeyRotationRequests
            | Request::RuntimeKeyRotationConfirm { .. }
    )
}

/// Complete local integration administration outside the one session owner.
pub(crate) async fn prepare_integration_admin(
    conversation: &Conversation,
    composed: &Composed,
    request: &Request,
) -> Prepared {
    if !is_integration_admin(request)
        || !conversation.greeted()
        || crate::scope::allowed(conversation.caller(), request, &composed.granted).is_err()
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
        Request::IntegrationEnrollmentDeny { pending_id } => {
            crate::integration_admin::IntegrationAdmin::deny(composed, pending_id)
                .map(|()| Response::Done)
        }
        Request::Integrations => crate::integration_admin::IntegrationAdmin::integrations(composed)
            .map(Response::Integrations),
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
        Request::RuntimeForgetRequests => composed
            .integration_admin
            .forget_requests(composed)
            .await
            .map(Response::RuntimeForgetRequests),
        Request::RuntimeForgetConfirm { confirmation_id } => composed
            .integration_admin
            .confirm_forget(confirmation_id)
            .await
            .map(|()| Response::Done),
        Request::RuntimeKeyRotationRequests => composed
            .integration_admin
            .key_rotation_requests(composed)
            .await
            .map(Response::RuntimeKeyRotationRequests),
        Request::RuntimeKeyRotationConfirm { confirmation_id } => composed
            .integration_admin
            .confirm_key_rotation(confirmation_id)
            .await
            .map(|()| Response::Done),
        _ => return Prepared::None,
    }
    .unwrap_or_else(|error| refuse(&error.to_string()));
    Prepared::IntegrationAdmin { response }
}

/// Create or resolve one Mission Task workspace outside the single session owner.
pub(crate) async fn prepare_mission_workspace(
    conversation: &Conversation,
    composed: &Composed,
    request: &Request,
) -> Prepared {
    let Request::MissionPrepareTask {
        mission_id,
        task_id,
    } = request
    else {
        return Prepared::None;
    };
    if !conversation.greeted()
        || crate::scope::allowed(conversation.caller(), request, &composed.granted).is_err()
    {
        return Prepared::None;
    }
    let response = crate::mission::prepare_workspace(
        &composed.missions,
        &composed.ledger,
        &composed.containment,
        mission_id,
        task_id,
    )
    .await;
    Prepared::MissionWorkspace {
        mission_id: mission_id.clone(),
        task_id: task_id.clone(),
        response,
    }
}

/// Run exact Artifact sealing and Gate execution outside the single session owner.
pub(crate) async fn prepare_mission_verification(
    conversation: &Conversation,
    composed: &Composed,
    request: &Request,
) -> Prepared {
    let Request::MissionVerifyTask {
        mission_id,
        task_id,
    } = request
    else {
        return Prepared::None;
    };
    if !conversation.greeted()
        || crate::scope::allowed(conversation.caller(), request, &composed.granted).is_err()
    {
        return Prepared::None;
    }
    Prepared::MissionVerification {
        mission_id: mission_id.clone(),
        task_id: task_id.clone(),
        response: crate::mission::prepare_verification(composed, mission_id, task_id).await,
    }
}

/// Verify one manually integrated Mission tree outside the single session owner.
pub(crate) async fn prepare_mission_integration(
    conversation: &Conversation,
    composed: &Composed,
    request: &Request,
) -> Prepared {
    let Request::MissionCompleteIntegration { mission_id } = request else {
        return Prepared::None;
    };
    if !conversation.greeted()
        || crate::scope::allowed(conversation.caller(), request, &composed.granted).is_err()
    {
        return Prepared::None;
    }
    Prepared::MissionIntegration {
        mission_id: mission_id.clone(),
        response: crate::mission::prepare_integration(composed, mission_id).await,
    }
}

/// Run one capability candidate's reviewed fixed Gates outside the single session owner.
pub(crate) async fn prepare_capability_verification(
    conversation: &Conversation,
    composed: &Composed,
    request: &Request,
) -> Prepared {
    let Request::CapabilityVerify {
        project,
        capability_id,
        version_sha256,
    } = request
    else {
        return Prepared::None;
    };
    if !conversation.greeted()
        || crate::scope::allowed(conversation.caller(), request, &composed.granted).is_err()
    {
        return Prepared::None;
    }
    let gate_ids = {
        let growth = composed.growth.lock().await;
        growth.verification_gate_ids(project, capability_id, version_sha256)
    };
    let response = match gate_ids {
        Ok(gate_ids) => {
            let gates = {
                let missions = composed.missions.lock().await;
                missions.gate_requests(&gate_ids, runtrol_ledger::RunId::now())
            };
            match gates {
                Ok(gates) => {
                    let intent = {
                        let mut growth = composed.growth.lock().await;
                        growth.verification_intent(project, capability_id, version_sha256, gates)
                    };
                    match intent {
                        Ok(intent) => {
                            let results =
                                crate::growth::run_verification(&composed.containment, &intent)
                                    .await;
                            let mut growth = composed.growth.lock().await;
                            growth
                                .commit_verification(&intent, &results)
                                .unwrap_or_else(refuse)
                        }
                        Err(message) => refuse(message),
                    }
                }
                Err(message) => refuse(message),
            }
        }
        Err(message) => refuse(message),
    };
    Prepared::CapabilityVerification {
        project: project.clone(),
        capability_id: capability_id.clone(),
        version_sha256: version_sha256.clone(),
        response,
    }
}

/// Answer one request after any slow provider discovery has completed elsewhere.
#[expect(
    clippy::too_many_lines,
    reason = "one exhaustive request table keeps every scope-checked wire operation visible in one place"
)]
pub(crate) fn answer_prepared(
    conversation: &mut Conversation,
    composed: &Composed,
    sessions: &mut SessionManager,
    request: Request,
    prepared: Prepared,
    reservation: Option<OpenReservation>,
) -> Reply {
    // Before anything else looks at the request. A wall consulted after some other branch has acted is a wall on
    // the way out, and the thing it was supposed to prevent has already happened.
    if let Err(refusal) = crate::scope::allowed(&conversation.caller, &request, &composed.granted) {
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

    match request {
        // Answered above, and matched here so that adding a request cannot fall through to a wildcard that does nothing.
        Request::Hello { .. } => Reply::One(refuse("the wire format is already agreed")),

        Request::List => Reply::One(list(composed, sessions)),

        Request::WatchSessions => Reply::WatchingSessions,

        Request::Models { provider } => models(&provider, prepared),

        Request::ProviderUpdates => match prepared {
            Prepared::ProviderUpdates { response } => Reply::One(response),
            _ => Reply::One(refuse(
                "provider update inspection was not completed for this request",
            )),
        },

        mission @ (Request::MissionRegisterGate { .. }
        | Request::MissionValidate { .. }
        | Request::MissionList
        | Request::MissionGet { .. }
        | Request::MissionStart { .. }
        | Request::MissionPause { .. }
        | Request::MissionResumeSafe { .. }
        | Request::MissionCancel { .. }
        | Request::MissionBindSession { .. }
        | Request::MissionSendTaskInstruction { .. }
        | Request::MissionRetryTask { .. }
        | Request::MissionArchive { .. }) => {
            let runtime_ids: Vec<Box<str>> = composed
                .registry
                .usable()
                .map(|provider| provider.id().as_str().into())
                .collect();
            let capability_project = match &mission {
                Request::MissionValidate { project, .. } => Some(project.clone()),
                Request::MissionStart { mission_id, .. }
                | Request::MissionSendTaskInstruction { mission_id, .. } => {
                    let Ok(id) = mission_id.parse::<runtrol_ledger::MissionId>() else {
                        return Reply::One(refuse("the Mission identity is invalid"));
                    };
                    match composed.ledger.snapshot(id) {
                        Ok(Some(snapshot)) => Some(snapshot.mission.project_id),
                        Ok(None) => return Reply::One(refuse("the Mission does not exist")),
                        Err(_) => {
                            return Reply::One(refuse("the Mission ledger cannot be read"));
                        }
                    }
                }
                _ => None,
            };
            let approved_capabilities = if let Some(project) = capability_project {
                let Ok(mut growth) = composed.growth.try_lock() else {
                    return Reply::One(refuse("the capability controller lock is damaged"));
                };
                match growth.approved_capabilities(&project) {
                    Ok(capabilities) => capabilities,
                    Err(message) => return Reply::One(refuse(&message)),
                }
            } else {
                Vec::new()
            };
            match composed.missions.try_lock() {
                Ok(mut controller) => Reply::One(controller.answer(
                    &composed.ledger,
                    &runtime_ids,
                    &approved_capabilities,
                    &mission,
                )),
                Err(_) => Reply::One(refuse("the Mission controller lock is damaged")),
            }
        }

        Request::MissionPrepareTask {
            mission_id,
            task_id,
        } => match prepared {
            Prepared::MissionWorkspace {
                mission_id: prepared_mission,
                task_id: prepared_task,
                response,
            } if prepared_mission == mission_id && prepared_task == task_id => Reply::One(response),
            other => mismatched(other),
        },

        Request::MissionVerifyTask {
            mission_id,
            task_id,
        } => match prepared {
            Prepared::MissionVerification {
                mission_id: prepared_mission,
                task_id: prepared_task,
                response,
            } if prepared_mission == mission_id && prepared_task == task_id => Reply::One(response),
            other => mismatched(other),
        },

        Request::MissionCompleteIntegration { mission_id } => match prepared {
            Prepared::MissionIntegration {
                mission_id: prepared_mission,
                response,
            } if prepared_mission == mission_id => Reply::One(response),
            other => mismatched(other),
        },

        Request::CapabilityVerify {
            project,
            capability_id,
            version_sha256,
        } => match prepared {
            Prepared::CapabilityVerification {
                project: prepared_project,
                capability_id: prepared_capability,
                version_sha256: prepared_version,
                response,
            } if prepared_project == project
                && prepared_capability == capability_id
                && prepared_version == version_sha256 =>
            {
                Reply::One(response)
            }
            other => mismatched(other),
        },

        capability @ (Request::CapabilityPropose { .. }
        | Request::CapabilityList
        | Request::CapabilityApprove { .. }
        | Request::CapabilityReject { .. }
        | Request::CapabilityQuarantine { .. }
        | Request::CapabilityRollback { .. }
        | Request::CapabilityArchive { .. }) => match composed.growth.try_lock() {
            Ok(mut controller) => Reply::One(controller.answer(&composed.ledger, &capability)),
            Err(_) => Reply::One(refuse("the capability controller lock is damaged")),
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
                Err(error) => Reply::One(from_session_error(&error)),
            }
        }

        Request::IntegrationEnrollments
        | Request::IntegrationApprovalBegin { .. }
        | Request::IntegrationApprovalFinish { .. }
        | Request::IntegrationEnrollmentDeny { .. }
        | Request::Integrations
        | Request::IntegrationRevoke { .. }
        | Request::IntegrationGrantChange { .. }
        | Request::RuntimeForgetRequests
        | Request::RuntimeForgetConfirm { .. }
        | Request::RuntimeKeyRotationRequests
        | Request::RuntimeKeyRotationConfirm { .. } => match prepared {
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
        } => open(
            composed,
            sessions,
            &provider,
            PreparedKind::Start,
            prepared,
            reservation,
        ),

        Request::Resume {
            provider,
            native: _,
            workspace: _,
            workspace_access: _,
        } => open(
            composed,
            sessions,
            &provider,
            PreparedKind::Resume,
            prepared,
            reservation,
        ),

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
            match composed.store.set_session_label(session, label) {
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
            &composed.granted,
            session,
            approval,
            option,
            subject_digest,
        ) {
            Ok((taken, command)) => Reply::Sending { taken, command },
            Err(error) => Reply::One(from_session_error(&error)),
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
            match sessions.close(session) {
                Ok(closing) => match composed.store.remove_session(session) {
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
                Err(SessionError::NotLive { .. }) => match composed.store.remove_session(session) {
                    Ok(true) => Reply::One(Response::Done),
                    Ok(false) => Reply::One(from_session_error(&SessionError::NotLive { session })),
                    Err(error) => Reply::One(refuse(&error.to_string())),
                },
                Err(error) => Reply::One(from_session_error(&error)),
            }
        }

        // Consults nothing: no ledger, no scope, no configuration. The security posture requires this to work from
        // anywhere with no permission at all, and the worst a hostile caller achieves through it is stopping work.
        Request::StopEverything => match composed.containment.terminate_all() {
            Ok(()) => Reply::One(Response::Done),
            // Reported rather than swallowed. An operator who pressed the panic button has to know whether it worked.
            Err(error) => Reply::One(refuse(&error.to_string())),
        },

        // The exchange already happened in the connection task. What is verified here is the binding: the
        // answer must be the one computed for this exact request, the rule every prepared result follows.
        consult @ (Request::Consult
        | Request::ConsultWire { .. }
        | Request::ConsultUnwire { .. }) => match prepared {
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

/// Commit a session process that was opened by its connection task.
fn open(
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
    let response = match persist_live(composed, sessions, attached.session) {
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
        | Prepared::MissionWorkspace { .. }
        | Prepared::MissionVerification { .. }
        | Prepared::MissionIntegration { .. }
        | Prepared::CapabilityVerification { .. } => false,
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
        Err(error) => Reply::One(from_session_error(&error)),
    }
}

/// The sessions this daemon can see.
///
/// Joins runtrol's stored session pointers with the sessions that currently have a supervised process. Provider
/// transcript storage is not consulted, so listing never discovers, derives, or reads a transcript path.
pub(crate) fn list(composed: &Composed, sessions: &SessionManager) -> Response {
    let catalogue = match crate::session_catalogue::read(composed, sessions) {
        Ok(catalogue) => catalogue,
        Err(error) => return refuse(&error.to_string()),
    };
    Response::Sessions(SessionListing {
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
                looks_stuck: session.looks_stuck,
            })
            .collect(),
        warnings: catalogue.warnings,
    })
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
pub(crate) fn persist_live(
    composed: &Composed,
    sessions: &SessionManager,
    session: SessionId,
) -> Result<(), StoreError> {
    persist_live_from_store(&composed.store, sessions, session)
}

/// Persist one live pointer against an explicit store for public Runtime composition.
pub(crate) fn persist_live_from_store(
    store: &runtrol_store::Store,
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
    if let Some(prior) = prior_session
        && prior != session
    {
        store.remove_session(prior)?;
    }

    let now = WallMs::now();
    store.put_session(
        session,
        &StoredSession {
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
        },
    )
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

/// A refusal with a message and no claim about retrying.
pub(crate) fn refuse(message: &str) -> Response {
    Response::Failed(WireError::plain(message))
}

#[cfg(test)]
mod tests {
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
            Reply::One(Response::Welcome { wire, providers }) => {
                assert_eq!(wire, runtrol_ipc::WIRE_VERSION);
                assert!(!providers.is_empty(), "a fresh install has providers");
                assert!(providers.iter().any(|one| one.usable));
            }
            other => panic!("expected a welcome, got {}", shape(&other)),
        }
        assert!(conversation.greeted());
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
                Reply::One(Response::Failed(failure)) => {
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
        );

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
        );
        assert!(
            matches!(reply, Reply::One(Response::Failed(_))),
            "a model result was accepted as an opened session"
        );
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
            Reply::Watching(_) | Reply::WatchingSessions => "a subscription".to_owned(),
            Reply::Stopping { how, .. } => format!("a process still stopping, {how:?}"),
            Reply::Cleaning { agents, .. } => format!("{} processes still stopping", agents.len()),
            Reply::Sending { .. } => "a provider command in flight".to_owned(),
            Reply::Updating { provider, .. } => format!("provider {provider} updating"),
        }
    }
}
