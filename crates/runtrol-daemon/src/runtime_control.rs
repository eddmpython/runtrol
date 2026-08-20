//! Single-owner public Runtime leases, mutations, and bounded event subscriptions.
//!
//! This module is an adapter over Core session authority. It records only structural mutation metadata and a keyed
//! authenticator. Caller input exists only in the request and the provider command handed out for asynchronous I/O.

use std::collections::BTreeMap;
use std::str::FromStr as _;

use hmac::{Hmac, Mac as _};
use runtrol_core::{
    ApprovalAuthority, ClosingReservation, OpenReservation, SessionManager, SessionView,
    TakenAgent, WorkspaceClaim,
};
use runtrol_core::{Lifecycle, SessionError, Waiting};
use runtrol_provider::{
    AbsPath, Agent, AgentCommand, ApprovalId, ApprovalKind, ApprovalRequest, ContentBlock,
    NativeSessionId, OpenIntent, OptionId, PermissionOptionKind, ProviderError, ProviderId,
    RiskClass, SessionId, StreamId, WallMs, WatchCursor,
};
use runtrol_runtime_protocol::{
    AcquireControlParams, CONTROL_LEASE_LIFETIME_MS, ControlLease, ControlLeaseParams,
    CoolSessionParams, EventCursor, ForgetSessionParams, IDEMPOTENCY_WINDOW_MS, LifecycleState,
    ListPendingApprovalsParams, MAX_INPUT_BYTES, MUTATION_CLOCK_SKEW_MS, MutationRequestId,
    PendingApproval, PendingApprovalList, RespondApprovalParams, RuntimeApprovalKind,
    RuntimeApprovalOption, RuntimeApprovalOptionKind, RuntimeApprovalRisk, RuntimeErrorKind,
    RuntimeMethod, SessionDescriptor, SessionOpenResult, SetModeParams, SetModelParams,
    SubmitInputParams, WaitingOn, WatchEventsParams, WatchEventsResult,
};
use runtrol_store::{
    IntegrationKey, IntegrationMutationKey, IntegrationMutationRow, IntegrationMutationState, Store,
};
use sha2::Sha256;
use tokio::sync::{mpsc, oneshot};

/// One request lent by a public connection to the single session owner.
pub(crate) struct RuntimeAsked {
    pub(crate) integration: IntegrationKey,
    pub(crate) request: RuntimeControlRequest,
    pub(crate) answered: oneshot::Sender<RuntimeControlReply>,
}

/// Public session operations after scope, root, and public identity authorization.
pub(crate) enum RuntimeControlRequest {
    PrepareOpen(RuntimeOpenRequest),
    Acquire {
        session: SessionId,
        params: AcquireControlParams,
    },
    Renew {
        session: SessionId,
        params: ControlLeaseParams,
    },
    Release {
        session: SessionId,
        params: ControlLeaseParams,
    },
    Submit {
        session: SessionId,
        params: SubmitInputParams,
    },
    SetModel {
        session: SessionId,
        params: SetModelParams,
    },
    /// Switch the governing permission mode under the caller's lease.
    SetMode {
        /// The exact managed session.
        session: SessionId,
        /// The validated public parameters.
        params: SetModeParams,
    },
    Interrupt {
        session: SessionId,
        params: ControlLeaseParams,
    },
    Cool {
        session: SessionId,
        params: CoolSessionParams,
    },
    Forget {
        session: SessionId,
        params: ForgetSessionParams,
    },
    ListApprovals {
        session: SessionId,
        params: ListPendingApprovalsParams,
        scopes: ApprovalScopes,
    },
    RespondApproval {
        session: SessionId,
        params: RespondApprovalParams,
        authority: ApprovalAuthority,
    },
    Watch {
        session: SessionId,
        params: WatchEventsParams,
        subscription_id: String,
    },
}

/// Owner response. Provider I/O remains outside the owner task.
pub(crate) enum RuntimeControlReply {
    Lease(ControlLease),
    Done,
    Approvals(PendingApprovalList),
    Watching {
        result: WatchEventsResult,
        view: Box<SessionView>,
    },
    Sending {
        mutation: IntegrationMutationKey,
        taken: TakenAgent,
        command: AgentCommand,
    },
    Cooling(RuntimeCooling),
    Opening(Box<RuntimeOpening>),
    Opened(SessionOpenResult),
    Failed(RuntimeControlFailure),
}

/// One authorized open mutation before it reserves Core process authority.
pub(crate) struct RuntimeOpenRequest {
    pub(crate) method: RuntimeMethod,
    pub(crate) request_id: MutationRequestId,
    pub(crate) provider: ProviderId,
    pub(crate) session: Option<SessionId>,
    pub(crate) native: Option<NativeSessionId>,
    pub(crate) workspace: AbsPath,
    pub(crate) claim: WorkspaceClaim,
    pub(crate) model: Option<Box<str>>,
    pub(crate) reasoning_effort: Option<Box<str>>,
    /// Already validated against the provider's switchable mode vocabulary at admission.
    pub(crate) permission: Option<Box<str>>,
    pub(crate) expected: Option<(LifecycleState, u64)>,
    pub(crate) proof: Option<Box<str>>,
}

/// One mutation intent and reserved Core slot handed to a connection for slow provider work.
pub(crate) struct RuntimeOpening {
    pub(crate) mutation: IntegrationMutationKey,
    pub(crate) integration: IntegrationKey,
    pub(crate) method: RuntimeMethod,
    pub(crate) provider: ProviderId,
    pub(crate) session: SessionId,
    pub(crate) native: Option<NativeSessionId>,
    pub(crate) workspace: AbsPath,
    pub(crate) model: Option<Box<str>>,
    pub(crate) reasoning_effort: Option<Box<str>>,
    pub(crate) permission: Option<Box<str>>,
    pub(crate) proof: Option<Box<str>>,
    pub(crate) reservation: OpenReservation,
    pub(crate) displaced_agent: Option<Box<dyn Agent>>,
    pub(crate) displaced_reservation: Option<ClosingReservation>,
    lease_id: String,
    lease_generation: u64,
}

/// One accepted cool mutation whose provider cleanup must run outside the session owner.
pub(crate) struct RuntimeCooling {
    pub(crate) mutation: IntegrationMutationKey,
    pub(crate) agent: Box<dyn Agent>,
    pub(crate) reservation: ClosingReservation,
}

/// Approval scopes derived from the current integration grant, never caller input.
#[derive(Clone, Copy)]
pub(crate) struct ApprovalScopes {
    pub(crate) low: bool,
    pub(crate) high: bool,
}

/// Cleanup authority returned to the connection because process stopping must not block the owner.
pub(crate) enum RuntimeOpenCleanup {
    Open(OpenReservation),
    Closing(ClosingReservation),
}

/// Intermediate result from attaching an asynchronously opened provider process.
pub(crate) enum RuntimeOpenCompletion {
    Answer(Result<SessionOpenResult, RuntimeControlFailure>),
    Cleanup {
        agent: Box<dyn Agent>,
        reservation: RuntimeOpenCleanup,
    },
}

/// A provider command returning to the session owner.
pub(crate) enum RuntimeReturned {
    Finished {
        mutation: IntegrationMutationKey,
        taken: TakenAgent,
        outcome: Result<(), runtrol_provider::ProviderError>,
        answered: oneshot::Sender<Result<(), RuntimeControlFailure>>,
    },
    Abandoned {
        mutation: IntegrationMutationKey,
        lease: runtrol_core::AgentLease,
    },
    Cooled {
        mutation: IntegrationMutationKey,
        reservation: ClosingReservation,
        outcome: Result<(), runtrol_provider::ProviderError>,
        answered: oneshot::Sender<Result<(), RuntimeControlFailure>>,
    },
    CoolAbandoned {
        mutation: IntegrationMutationKey,
        reservation: ClosingReservation,
    },
    Opened {
        opening: RuntimeOpening,
        intent: OpenIntent,
        agent: Box<dyn Agent>,
        answered: oneshot::Sender<RuntimeOpenCompletion>,
    },
    OpenDenied {
        opening: RuntimeOpening,
        failure: RuntimeControlFailure,
        answered: oneshot::Sender<RuntimeOpenCompletion>,
    },
    OpenUnknown {
        opening: RuntimeOpening,
        answered: oneshot::Sender<RuntimeOpenCompletion>,
    },
    OpenAbandoned {
        opening: RuntimeOpening,
    },
    OpenCleaned {
        reservation: RuntimeOpenCleanup,
        answered: oneshot::Sender<Result<SessionOpenResult, RuntimeControlFailure>>,
    },
}

/// Cancellation guard for one process slot and durable pending mutation held outside the owner task.
pub(crate) struct RuntimeOpenGuard {
    opening: Option<RuntimeOpening>,
    returning: mpsc::UnboundedSender<RuntimeReturned>,
}

impl RuntimeOpenGuard {
    pub(crate) fn new(
        opening: RuntimeOpening,
        returning: mpsc::UnboundedSender<RuntimeReturned>,
    ) -> Self {
        Self {
            opening: Some(opening),
            returning,
        }
    }

    pub(crate) fn take(mut self) -> Option<RuntimeOpening> {
        self.opening.take()
    }

    pub(crate) fn opening(&self) -> Option<&RuntimeOpening> {
        self.opening.as_ref()
    }

    pub(crate) fn take_displaced_agent(&mut self) -> Option<Box<dyn Agent>> {
        self.opening.as_mut()?.displaced_agent.take()
    }
}

impl Drop for RuntimeOpenGuard {
    fn drop(&mut self) {
        if let Some(opening) = self.opening.take() {
            drop(
                self.returning
                    .send(RuntimeReturned::OpenAbandoned { opening }),
            );
        }
    }
}

/// Guard that reports cancellation while a provider process is outside the owner.
pub(crate) struct RuntimeAgentGuard {
    mutation: IntegrationMutationKey,
    lease: Option<runtrol_core::AgentLease>,
    returning: mpsc::UnboundedSender<RuntimeReturned>,
}

impl RuntimeAgentGuard {
    pub(crate) fn new(
        mutation: IntegrationMutationKey,
        lease: runtrol_core::AgentLease,
        returning: mpsc::UnboundedSender<RuntimeReturned>,
    ) -> Self {
        Self {
            mutation,
            lease: Some(lease),
            returning,
        }
    }

    pub(crate) fn take(mut self) -> Option<runtrol_core::AgentLease> {
        self.lease.take()
    }
}

impl Drop for RuntimeAgentGuard {
    fn drop(&mut self) {
        if let Some(lease) = self.lease.take() {
            drop(self.returning.send(RuntimeReturned::Abandoned {
                mutation: self.mutation,
                lease,
            }));
        }
    }
}

/// Cancellation guard for a provider process already removed from the live manager for cooling.
pub(crate) struct RuntimeCoolGuard {
    mutation: IntegrationMutationKey,
    reservation: Option<ClosingReservation>,
    returning: mpsc::UnboundedSender<RuntimeReturned>,
}

impl RuntimeCoolGuard {
    pub(crate) fn new(
        mutation: IntegrationMutationKey,
        reservation: ClosingReservation,
        returning: mpsc::UnboundedSender<RuntimeReturned>,
    ) -> Self {
        Self {
            mutation,
            reservation: Some(reservation),
            returning,
        }
    }

    pub(crate) fn take(mut self) -> Option<ClosingReservation> {
        self.reservation.take()
    }
}

impl Drop for RuntimeCoolGuard {
    fn drop(&mut self) {
        if let Some(reservation) = self.reservation.take() {
            drop(self.returning.send(RuntimeReturned::CoolAbandoned {
                mutation: self.mutation,
                reservation,
            }));
        }
    }
}

/// Stable safe refusal returned through the public error envelope.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RuntimeControlFailure {
    pub(crate) kind: RuntimeErrorKind,
    pub(crate) message: &'static str,
}

impl RuntimeControlFailure {
    pub(crate) const fn new(kind: RuntimeErrorKind, message: &'static str) -> Self {
        Self { kind, message }
    }

    const fn internal() -> Self {
        Self::new(
            RuntimeErrorKind::Internal,
            "Runtime could not preserve the session control boundary",
        )
    }

    pub(crate) const fn outcome_unknown() -> Self {
        Self::new(
            RuntimeErrorKind::OutcomeUnknown,
            "the mutation may have happened and cannot be repeated safely",
        )
    }
}

struct LeaseState {
    integration: IntegrationKey,
    public: ControlLease,
    stream: StreamId,
}

#[derive(Clone)]
enum MutationOutcome {
    Pending,
    Lease(ControlLease),
    Done,
    Open(SessionOpenResult),
    Failed(RuntimeControlFailure),
}

struct MutationMemory {
    method: RuntimeMethod,
    authenticator: [u8; 32],
    outcome: MutationOutcome,
}

enum Begun {
    New(IntegrationMutationKey),
    Replay(Box<RuntimeControlReply>),
}

/// Content-free authority state owned beside `SessionManager`.
pub(crate) struct RuntimeControl {
    leases: BTreeMap<SessionId, LeaseState>,
    mutations: BTreeMap<IntegrationMutationKey, MutationMemory>,
    boot_id: [u8; 16],
    authenticator: Hmac<Sha256>,
    next_lease_generation: u64,
}

impl RuntimeControl {
    /// Mint one boot-local authenticator key and recovery identity.
    pub(crate) fn new() -> Result<Self, RuntimeControlFailure> {
        let mut boot_id = [0_u8; 16];
        let mut authenticator_key = [0_u8; 32];
        getrandom::fill(&mut boot_id).map_err(|_| RuntimeControlFailure::internal())?;
        getrandom::fill(&mut authenticator_key).map_err(|_| RuntimeControlFailure::internal())?;
        let authenticator = Hmac::<Sha256>::new_from_slice(&authenticator_key)
            .map_err(|_| RuntimeControlFailure::internal())?;
        Ok(Self {
            leases: BTreeMap::new(),
            mutations: BTreeMap::new(),
            boot_id,
            authenticator,
            next_lease_generation: 0,
        })
    }

    /// Answer one already authorized public operation without awaiting provider I/O.
    pub(crate) fn answer(
        &mut self,
        store: &Store,
        sessions: &mut SessionManager,
        integration: IntegrationKey,
        request: RuntimeControlRequest,
    ) -> RuntimeControlReply {
        match request {
            RuntimeControlRequest::PrepareOpen(request) => {
                self.prepare_open(store, sessions, integration, request)
            }
            RuntimeControlRequest::Acquire { session, params } => {
                self.acquire(store, sessions, integration, session, &params)
            }
            RuntimeControlRequest::Renew { session, params } => {
                self.renew(store, sessions, integration, session, &params)
            }
            RuntimeControlRequest::Release { session, params } => {
                self.release(store, sessions, integration, session, &params)
            }
            RuntimeControlRequest::Submit { session, params } => {
                self.submit(store, sessions, integration, session, params)
            }
            RuntimeControlRequest::SetModel { session, params } => {
                self.set_model(store, sessions, integration, session, params)
            }
            RuntimeControlRequest::SetMode { session, params } => {
                self.set_mode(store, sessions, integration, session, params)
            }
            RuntimeControlRequest::Interrupt { session, params } => {
                self.interrupt(store, sessions, integration, session, &params)
            }
            RuntimeControlRequest::Cool { session, params } => {
                self.cool(store, sessions, integration, session, &params)
            }
            RuntimeControlRequest::Forget { session, params } => {
                self.forget(store, sessions, integration, session, &params)
            }
            RuntimeControlRequest::ListApprovals {
                session,
                params,
                scopes,
            } => self.list_approvals(sessions, integration, session, &params, scopes),
            RuntimeControlRequest::RespondApproval {
                session,
                params,
                authority,
            } => self.respond_approval(store, sessions, integration, session, &params, authority),
            RuntimeControlRequest::Watch {
                session,
                params,
                subscription_id,
            } => Self::watch(sessions, session, &params, subscription_id),
        }
    }

    /// Attach one provider process, persist any known structural pointer, and grant initiating integration control.
    pub(crate) fn finish_open(
        &mut self,
        store: &Store,
        sessions: &mut SessionManager,
        opening: RuntimeOpening,
        intent: &OpenIntent,
        agent: Box<dyn Agent>,
    ) -> RuntimeOpenCompletion {
        let RuntimeOpening {
            mutation,
            integration,
            provider,
            session,
            reservation,
            displaced_agent: _,
            displaced_reservation,
            lease_id,
            lease_generation,
            ..
        } = opening;
        if let Some(displaced_reservation) = displaced_reservation {
            sessions.release_closing(displaced_reservation);
        }
        let attached = match sessions.attach_opened(reservation, provider, intent, agent) {
            Ok(attached) => attached,
            Err(error) => {
                let (_error, agent, reservation) = error.into_parts();
                return RuntimeOpenCompletion::Cleanup {
                    agent,
                    reservation: RuntimeOpenCleanup::Open(reservation),
                };
            }
        };
        if crate::dispatch::persist_live_from_store(store, sessions, attached.session).is_err() {
            return match sessions.close(attached.session) {
                Ok(closing) => RuntimeOpenCompletion::Cleanup {
                    agent: closing.agent,
                    reservation: RuntimeOpenCleanup::Closing(closing.reservation),
                },
                Err(_) => {
                    RuntimeOpenCompletion::Answer(Err(RuntimeControlFailure::outcome_unknown()))
                }
            };
        }
        let Some(live) = sessions.live_session(session) else {
            return RuntimeOpenCompletion::Answer(Err(RuntimeControlFailure::outcome_unknown()));
        };
        let control = ControlLease {
            lease_id,
            session_id: runtrol_runtime_protocol::RuntimeSessionId::new(session.to_string()),
            session_generation: live.state.generation(),
            lease_generation,
            expires_at_ms: WallMs::now()
                .as_millis()
                .saturating_add(CONTROL_LEASE_LIFETIME_MS),
        };
        self.leases.insert(
            session,
            LeaseState {
                integration,
                public: control.clone(),
                stream: live.stream,
            },
        );
        let result = SessionOpenResult {
            session: SessionDescriptor {
                session_id: control.session_id.clone(),
                provider_id: runtrol_runtime_protocol::ProviderId::new(provider.as_str()),
                native_session_id: live.native.map(str::to_owned),
                workspace: live.workspace.to_string(),
                hot: true,
                lifecycle: public_lifecycle(live.state.lifecycle()),
                looks_stuck: live.state.looks_stuck(),
                waiting_on: live.state.waiting().map(public_waiting),
                session_generation: live.state.generation(),
                label: None,
            },
            control,
        };
        match self.finish(store, mutation, MutationOutcome::Open(result.clone())) {
            Ok(()) => RuntimeOpenCompletion::Answer(Ok(result)),
            Err(failure) => RuntimeOpenCompletion::Answer(Err(failure)),
        }
    }

    /// Finish a deterministic pre-provider refusal and release both held Core slots.
    pub(crate) fn deny_open(
        &mut self,
        store: &Store,
        sessions: &mut SessionManager,
        opening: RuntimeOpening,
        failure: RuntimeControlFailure,
    ) -> RuntimeOpenCompletion {
        let mutation = opening.mutation;
        release_opening(sessions, opening);
        match self.deny(store, mutation, failure) {
            RuntimeControlReply::Failed(failure) => RuntimeOpenCompletion::Answer(Err(failure)),
            _ => RuntimeOpenCompletion::Answer(Err(RuntimeControlFailure::internal())),
        }
    }

    /// Release an ambiguous or cancelled open while deliberately preserving its pending ledger row.
    pub(crate) fn abandon_open(
        sessions: &mut SessionManager,
        opening: RuntimeOpening,
    ) -> RuntimeOpenCompletion {
        let RuntimeOpening {
            reservation,
            displaced_agent,
            displaced_reservation,
            ..
        } = opening;
        sessions.cancel_open(reservation);
        match (displaced_agent, displaced_reservation) {
            (Some(agent), Some(reservation)) => RuntimeOpenCompletion::Cleanup {
                agent,
                reservation: RuntimeOpenCleanup::Closing(reservation),
            },
            (None, Some(reservation)) => {
                sessions.release_closing(reservation);
                RuntimeOpenCompletion::Answer(Err(RuntimeControlFailure::outcome_unknown()))
            }
            (Some(agent), None) => {
                drop(agent);
                RuntimeOpenCompletion::Answer(Err(RuntimeControlFailure::outcome_unknown()))
            }
            (None, None) => {
                RuntimeOpenCompletion::Answer(Err(RuntimeControlFailure::outcome_unknown()))
            }
        }
    }

    /// Release one slot only after its process cleanup completed outside the owner task.
    pub(crate) fn finish_open_cleanup(
        sessions: &mut SessionManager,
        reservation: RuntimeOpenCleanup,
    ) -> Result<SessionOpenResult, RuntimeControlFailure> {
        match reservation {
            RuntimeOpenCleanup::Open(reservation) => sessions.cancel_open(reservation),
            RuntimeOpenCleanup::Closing(reservation) => sessions.release_closing(reservation),
        }
        Err(RuntimeControlFailure::outcome_unknown())
    }

    fn prepare_open(
        &mut self,
        store: &Store,
        sessions: &mut SessionManager,
        integration: IntegrationKey,
        request: RuntimeOpenRequest,
    ) -> RuntimeControlReply {
        let authenticator = self.authenticate_open(&request);
        let mutation = match self.begin(
            store,
            integration,
            request.method,
            &request.request_id,
            authenticator,
        ) {
            Ok(Begun::New(key)) => key,
            Ok(Begun::Replay(reply)) => return *reply,
            Err(failure) => return RuntimeControlReply::Failed(failure),
        };
        if let Some((expected_lifecycle, expected_generation)) = request.expected {
            let Some(expected_session) = request.session else {
                return self.deny(store, mutation, session_conflict());
            };
            let current = sessions.live_session(expected_session);
            let current_lifecycle = current.as_ref().map_or(LifecycleState::Cold, |live| {
                public_lifecycle(live.state.lifecycle())
            });
            let current_generation = current.as_ref().map_or(0, |live| live.state.generation());
            if current_lifecycle != expected_lifecycle || current_generation != expected_generation
            {
                return self.deny(store, mutation, session_conflict());
            }
        }
        let session = request.session.unwrap_or_else(SessionId::now);
        if request.method == RuntimeMethod::SessionsAdoptNative {
            let Some(native) = request.native.as_ref() else {
                return self.deny(store, mutation, session_conflict());
            };
            match store.find_by_native(request.provider, native) {
                Ok(Some(_)) => return self.deny(store, mutation, session_conflict()),
                Ok(None) => {}
                Err(_) => return self.deny(store, mutation, RuntimeControlFailure::internal()),
            }
        }
        if request.method == RuntimeMethod::SessionsResume {
            let stored = match store.get_session(session) {
                Ok(Some(stored)) => stored,
                Ok(None) => return self.deny(store, mutation, not_live()),
                Err(_) => return self.deny(store, mutation, RuntimeControlFailure::internal()),
            };
            if stored.provider != request.provider
                || Some(&stored.native) != request.native.as_ref()
                || stored.cwd != request.workspace
            {
                return self.deny(store, mutation, session_conflict());
            }
        }
        let lease_generation = match self.allocate_lease_generation() {
            Ok(generation) => generation,
            Err(failure) => return self.deny(store, mutation, failure),
        };
        let lease_id = match random_label("lease_") {
            Ok(lease_id) => lease_id,
            Err(failure) => return self.deny(store, mutation, failure),
        };
        match sessions.reserve_open_for_provider(request.provider, session, request.claim) {
            Ok(reserved) => {
                let (displaced_agent, displaced_reservation) =
                    reserved.displaced.map_or((None, None), |displaced| {
                        (Some(displaced.agent), Some(displaced.reservation))
                    });
                RuntimeControlReply::Opening(Box::new(RuntimeOpening {
                    mutation,
                    integration,
                    method: request.method,
                    provider: request.provider,
                    session,
                    native: request.native,
                    workspace: request.workspace,
                    model: request.model,
                    reasoning_effort: request.reasoning_effort,
                    permission: request.permission,
                    proof: request.proof,
                    reservation: reserved.reservation,
                    displaced_agent,
                    displaced_reservation,
                    lease_id,
                    lease_generation,
                }))
            }
            Err(error) => self.deny(store, mutation, open_failure(&error)),
        }
    }

    /// Restore a provider process and finish only an acknowledged mutation.
    pub(crate) fn finish_command(
        &mut self,
        store: &Store,
        sessions: &mut SessionManager,
        mutation: IntegrationMutationKey,
        taken: TakenAgent,
        outcome: &Result<(), runtrol_provider::ProviderError>,
    ) -> Result<(), RuntimeControlFailure> {
        let TakenAgent { agent, lease } = taken;
        if let Err(agent) = sessions.return_agent(lease, agent) {
            drop(agent);
            return Err(RuntimeControlFailure::outcome_unknown());
        }
        match outcome {
            Ok(()) => {
                self.finish(store, mutation, MutationOutcome::Done)?;
                Ok(())
            }
            Err(_) => Err(RuntimeControlFailure::outcome_unknown()),
        }
    }

    /// Release a cancelled provider handoff. Its durable pending intent remains ambiguous.
    pub(crate) fn abandon_command(
        sessions: &mut SessionManager,
        _mutation: IntegrationMutationKey,
        lease: runtrol_core::AgentLease,
    ) {
        sessions.abandon_agent(lease);
    }

    /// Finish provider cleanup for an accepted cool mutation.
    pub(crate) fn finish_cool(
        &mut self,
        store: &Store,
        sessions: &mut SessionManager,
        mutation: IntegrationMutationKey,
        reservation: ClosingReservation,
        outcome: &Result<(), runtrol_provider::ProviderError>,
    ) -> Result<(), RuntimeControlFailure> {
        sessions.release_closing(reservation);
        match outcome {
            Ok(()) => self.finish(store, mutation, MutationOutcome::Done),
            Err(_) => Err(RuntimeControlFailure::outcome_unknown()),
        }
    }

    /// Release a cancelled cool reservation while preserving its ambiguous mutation record.
    pub(crate) fn abandon_cool(
        sessions: &mut SessionManager,
        _mutation: IntegrationMutationKey,
        reservation: ClosingReservation,
    ) {
        sessions.release_closing(reservation);
    }

    fn acquire(
        &mut self,
        store: &Store,
        sessions: &SessionManager,
        integration: IntegrationKey,
        session: SessionId,
        params: &AcquireControlParams,
    ) -> RuntimeControlReply {
        let authenticator = self.authenticate_acquire(params);
        let mutation = match self.begin(
            store,
            integration,
            RuntimeMethod::SessionsAcquireControl,
            &params.request_id,
            authenticator,
        ) {
            Ok(Begun::New(key)) => key,
            Ok(Begun::Replay(reply)) => return *reply,
            Err(failure) => return RuntimeControlReply::Failed(failure),
        };
        let Some(live) = sessions.live_session(session) else {
            return self.deny(store, mutation, not_live());
        };
        let lifecycle = public_lifecycle(live.state.lifecycle());
        if lifecycle != params.expected_lifecycle
            || live.state.generation() != params.expected_session_generation
        {
            return self.deny(
                store,
                mutation,
                RuntimeControlFailure::new(
                    RuntimeErrorKind::SessionConflict,
                    "the session changed after the caller observed it",
                ),
            );
        }
        let now = WallMs::now().as_millis();
        if let Some(current) = self.leases.get(&session) {
            if current.public.expires_at_ms >= now {
                return self.deny(store, mutation, control_conflict());
            }
            if lifecycle == LifecycleState::HotRunning && current.integration != integration {
                return self.deny(store, mutation, control_conflict());
            }
        }
        let generation = match self.allocate_lease_generation() {
            Ok(generation) => generation,
            Err(failure) => return self.deny(store, mutation, failure),
        };
        let public = match random_label("lease_") {
            Ok(lease_id) => ControlLease {
                lease_id,
                session_id: params.session_id.clone(),
                session_generation: live.state.generation(),
                lease_generation: generation,
                expires_at_ms: now.saturating_add(CONTROL_LEASE_LIFETIME_MS),
            },
            Err(failure) => return self.deny(store, mutation, failure),
        };
        self.leases.insert(
            session,
            LeaseState {
                integration,
                public: public.clone(),
                stream: live.stream,
            },
        );
        match self.finish(store, mutation, MutationOutcome::Lease(public.clone())) {
            Ok(()) => RuntimeControlReply::Lease(public),
            Err(failure) => RuntimeControlReply::Failed(failure),
        }
    }

    fn renew(
        &mut self,
        store: &Store,
        sessions: &SessionManager,
        integration: IntegrationKey,
        session: SessionId,
        params: &ControlLeaseParams,
    ) -> RuntimeControlReply {
        let authenticator = self.authenticate_lease(RuntimeMethod::SessionsRenewControl, params);
        let mutation = match self.begin(
            store,
            integration,
            RuntimeMethod::SessionsRenewControl,
            &params.request_id,
            authenticator,
        ) {
            Ok(Begun::New(key)) => key,
            Ok(Begun::Replay(reply)) => return *reply,
            Err(failure) => return RuntimeControlReply::Failed(failure),
        };
        if let Err(failure) = self.verify_lease(sessions, integration, session, params) {
            return self.deny(store, mutation, failure);
        }
        let next = match self.allocate_lease_generation() {
            Ok(generation) => generation,
            Err(failure) => return self.deny(store, mutation, failure),
        };
        let Some(lease) = self.leases.get_mut(&session) else {
            return self.deny(store, mutation, lease_expired());
        };
        lease.public.lease_generation = next;
        lease.public.expires_at_ms = WallMs::now()
            .as_millis()
            .saturating_add(CONTROL_LEASE_LIFETIME_MS);
        let public = lease.public.clone();
        match self.finish(store, mutation, MutationOutcome::Lease(public.clone())) {
            Ok(()) => RuntimeControlReply::Lease(public),
            Err(failure) => RuntimeControlReply::Failed(failure),
        }
    }

    fn release(
        &mut self,
        store: &Store,
        sessions: &SessionManager,
        integration: IntegrationKey,
        session: SessionId,
        params: &ControlLeaseParams,
    ) -> RuntimeControlReply {
        let authenticator = self.authenticate_lease(RuntimeMethod::SessionsReleaseControl, params);
        let mutation = match self.begin(
            store,
            integration,
            RuntimeMethod::SessionsReleaseControl,
            &params.request_id,
            authenticator,
        ) {
            Ok(Begun::New(key)) => key,
            Ok(Begun::Replay(reply)) => return *reply,
            Err(failure) => return RuntimeControlReply::Failed(failure),
        };
        if let Err(failure) = self.verify_lease(sessions, integration, session, params) {
            return self.deny(store, mutation, failure);
        }
        self.leases.remove(&session);
        match self.finish(store, mutation, MutationOutcome::Done) {
            Ok(()) => RuntimeControlReply::Done,
            Err(failure) => RuntimeControlReply::Failed(failure),
        }
    }

    fn submit(
        &mut self,
        store: &Store,
        sessions: &mut SessionManager,
        integration: IntegrationKey,
        session: SessionId,
        params: SubmitInputParams,
    ) -> RuntimeControlReply {
        if params.input.len() > MAX_INPUT_BYTES {
            return RuntimeControlReply::Failed(RuntimeControlFailure::new(
                RuntimeErrorKind::InvalidRequest,
                "caller input exceeds the advertised byte limit",
            ));
        }
        let authenticator = self.authenticate_submit(&params);
        let mutation = match self.begin(
            store,
            integration,
            RuntimeMethod::SessionsSubmitInput,
            &params.request_id,
            authenticator,
        ) {
            Ok(Begun::New(key)) => key,
            Ok(Begun::Replay(reply)) => return *reply,
            Err(failure) => return RuntimeControlReply::Failed(failure),
        };
        if let Err(failure) = self.verify_lease_values(
            sessions,
            integration,
            session,
            &params.lease_id,
            params.lease_generation,
        ) {
            return self.deny(store, mutation, failure);
        }
        match sessions.take_agent(session) {
            Ok(taken) => RuntimeControlReply::Sending {
                mutation,
                taken,
                command: AgentCommand::Prompt(vec![ContentBlock::Text(params.input.into())]),
            },
            Err(_) => self.deny(
                store,
                mutation,
                RuntimeControlFailure::new(
                    RuntimeErrorKind::SessionConflict,
                    "the session cannot accept input in its current state",
                ),
            ),
        }
    }

    /// Relay the operator's model choice to the session's driver, under the same lease discipline as input.
    ///
    /// The same shape as [`Self::submit`] on purpose: switching what answers is as much a control action as
    /// speaking, so it takes the same lease and the same idempotent mutation record. Runtime carries the words
    /// and decides nothing about them; the provider's refusal or confirmation arrives on the event stream.
    fn set_model(
        &mut self,
        store: &Store,
        sessions: &mut SessionManager,
        integration: IntegrationKey,
        session: SessionId,
        params: SetModelParams,
    ) -> RuntimeControlReply {
        // Transport sanity rather than model knowledge: no provider names a model in kilobytes, so a request
        // that does is malformed, not a catalogue miss.
        let model_usable = !params.model.is_empty() && params.model.len() <= 256;
        let effort_usable = params
            .reasoning_effort
            .as_deref()
            .is_none_or(|effort| !effort.is_empty() && effort.len() <= 64);
        if !model_usable || !effort_usable {
            return RuntimeControlReply::Failed(RuntimeControlFailure::new(
                RuntimeErrorKind::InvalidRequest,
                "the model switch names no usable model or effort",
            ));
        }
        let authenticator = self.authenticate_set_model(&params);
        let mutation = match self.begin(
            store,
            integration,
            RuntimeMethod::SessionsSetModel,
            &params.request_id,
            authenticator,
        ) {
            Ok(Begun::New(key)) => key,
            Ok(Begun::Replay(reply)) => return *reply,
            Err(failure) => return RuntimeControlReply::Failed(failure),
        };
        if let Err(failure) = self.verify_lease_values(
            sessions,
            integration,
            session,
            &params.lease_id,
            params.lease_generation,
        ) {
            return self.deny(store, mutation, failure);
        }
        match sessions.take_agent(session) {
            Ok(taken) => RuntimeControlReply::Sending {
                mutation,
                taken,
                command: AgentCommand::SetModel {
                    model: params.model.into(),
                    reasoning_effort: params.reasoning_effort.map(Into::into),
                },
            },
            Err(_) => self.deny(
                store,
                mutation,
                RuntimeControlFailure::new(
                    RuntimeErrorKind::SessionConflict,
                    "the session cannot switch models in its current state",
                ),
            ),
        }
    }

    /// Relay the operator's mode choice to the session's driver, under the same lease discipline as input.
    ///
    /// Whether the name is one the provider accepts a runtrol switch to was already judged at the serve
    /// boundary, where the provider registry lives; here the words are carried under the same lease and
    /// idempotent mutation record as speaking, and the provider's confirmation or refusal arrives on the
    /// event stream.
    fn set_mode(
        &mut self,
        store: &Store,
        sessions: &mut SessionManager,
        integration: IntegrationKey,
        session: SessionId,
        params: SetModeParams,
    ) -> RuntimeControlReply {
        // Transport sanity rather than mode knowledge: every measured vocabulary is a short token.
        if params.mode.is_empty() || params.mode.len() > 64 {
            return RuntimeControlReply::Failed(RuntimeControlFailure::new(
                RuntimeErrorKind::InvalidRequest,
                "the mode switch names no usable mode",
            ));
        }
        let authenticator = self.authenticate_set_mode(&params);
        let mutation = match self.begin(
            store,
            integration,
            RuntimeMethod::SessionsSetMode,
            &params.request_id,
            authenticator,
        ) {
            Ok(Begun::New(key)) => key,
            Ok(Begun::Replay(reply)) => return *reply,
            Err(failure) => return RuntimeControlReply::Failed(failure),
        };
        if let Err(failure) = self.verify_lease_values(
            sessions,
            integration,
            session,
            &params.lease_id,
            params.lease_generation,
        ) {
            return self.deny(store, mutation, failure);
        }
        match sessions.take_agent(session) {
            Ok(taken) => RuntimeControlReply::Sending {
                mutation,
                taken,
                command: AgentCommand::SetMode {
                    mode: params.mode.into(),
                },
            },
            Err(_) => self.deny(
                store,
                mutation,
                RuntimeControlFailure::new(
                    RuntimeErrorKind::SessionConflict,
                    "the session cannot switch modes in its current state",
                ),
            ),
        }
    }

    fn interrupt(
        &mut self,
        store: &Store,
        sessions: &mut SessionManager,
        integration: IntegrationKey,
        session: SessionId,
        params: &ControlLeaseParams,
    ) -> RuntimeControlReply {
        let authenticator = self.authenticate_lease(RuntimeMethod::SessionsInterrupt, params);
        let mutation = match self.begin(
            store,
            integration,
            RuntimeMethod::SessionsInterrupt,
            &params.request_id,
            authenticator,
        ) {
            Ok(Begun::New(key)) => key,
            Ok(Begun::Replay(reply)) => return *reply,
            Err(failure) => return RuntimeControlReply::Failed(failure),
        };
        if let Err(failure) = self.verify_lease(sessions, integration, session, params) {
            return self.deny(store, mutation, failure);
        }
        match sessions.take_agent(session) {
            Ok(taken) => RuntimeControlReply::Sending {
                mutation,
                taken,
                command: AgentCommand::Interrupt,
            },
            Err(_) => self.deny(
                store,
                mutation,
                RuntimeControlFailure::new(
                    RuntimeErrorKind::SessionConflict,
                    "the session cannot be interrupted in its current state",
                ),
            ),
        }
    }

    fn cool(
        &mut self,
        store: &Store,
        sessions: &mut SessionManager,
        integration: IntegrationKey,
        session: SessionId,
        params: &CoolSessionParams,
    ) -> RuntimeControlReply {
        let authenticator = self.authenticate_cool(params);
        let mutation = match self.begin(
            store,
            integration,
            RuntimeMethod::SessionsCool,
            &params.request_id,
            authenticator,
        ) {
            Ok(Begun::New(key)) => key,
            Ok(Begun::Replay(reply)) => return *reply,
            Err(failure) => return RuntimeControlReply::Failed(failure),
        };
        if let Err(failure) = self.verify_lease_values(
            sessions,
            integration,
            session,
            &params.lease_id,
            params.lease_generation,
        ) {
            return self.deny(store, mutation, failure);
        }
        let Some(live) = sessions.live_session(session) else {
            return self.deny(store, mutation, not_live());
        };
        if public_lifecycle(live.state.lifecycle()) != LifecycleState::HotIdle
            || live.state.generation() != params.expected_session_generation
        {
            return self.deny(
                store,
                mutation,
                RuntimeControlFailure::new(
                    RuntimeErrorKind::SessionConflict,
                    "only the exact observed idle session can be cooled",
                ),
            );
        }
        match sessions.close(session) {
            Ok(closing) => {
                self.leases.remove(&session);
                RuntimeControlReply::Cooling(RuntimeCooling {
                    mutation,
                    agent: closing.agent,
                    reservation: closing.reservation,
                })
            }
            Err(_) => self.deny(
                store,
                mutation,
                RuntimeControlFailure::new(
                    RuntimeErrorKind::SessionConflict,
                    "the session cannot be cooled in its current state",
                ),
            ),
        }
    }

    fn forget(
        &mut self,
        store: &Store,
        sessions: &SessionManager,
        integration: IntegrationKey,
        session: SessionId,
        params: &ForgetSessionParams,
    ) -> RuntimeControlReply {
        let authenticator = self.authenticate_forget(params);
        let mutation = match self.begin(
            store,
            integration,
            RuntimeMethod::SessionsForget,
            &params.request_id,
            authenticator,
        ) {
            Ok(Begun::New(key)) => key,
            Ok(Begun::Replay(reply)) => return *reply,
            Err(failure) => return RuntimeControlReply::Failed(failure),
        };
        if sessions.live_session(session).is_some() {
            return self.deny(
                store,
                mutation,
                RuntimeControlFailure::new(
                    RuntimeErrorKind::SessionConflict,
                    "only the exact locally closed session pointer can be forgotten",
                ),
            );
        }
        match store.remove_session(session) {
            Ok(_) => {
                self.leases.remove(&session);
                match self.finish(store, mutation, MutationOutcome::Done) {
                    Ok(()) => RuntimeControlReply::Done,
                    Err(failure) => RuntimeControlReply::Failed(failure),
                }
            }
            Err(_) => self.deny(store, mutation, RuntimeControlFailure::internal()),
        }
    }

    fn list_approvals(
        &self,
        sessions: &SessionManager,
        integration: IntegrationKey,
        session: SessionId,
        params: &ListPendingApprovalsParams,
        scopes: ApprovalScopes,
    ) -> RuntimeControlReply {
        if let Err(failure) = self.verify_lease_values(
            sessions,
            integration,
            session,
            &params.lease_id,
            params.lease_generation,
        ) {
            return RuntimeControlReply::Failed(failure);
        }
        let Ok(requests) = sessions.pending_approvals(session) else {
            return RuntimeControlReply::Failed(RuntimeControlFailure::new(
                RuntimeErrorKind::SessionConflict,
                "pending approvals are unavailable while another provider command is in flight",
            ));
        };
        let approvals = match requests
            .into_iter()
            .map(|request| pending_approval(&request, scopes))
            .collect::<Result<Vec<_>, _>>()
        {
            Ok(approvals) => approvals,
            Err(failure) => return RuntimeControlReply::Failed(failure),
        };
        RuntimeControlReply::Approvals(PendingApprovalList { approvals })
    }

    fn respond_approval(
        &mut self,
        store: &Store,
        sessions: &mut SessionManager,
        integration: IntegrationKey,
        session: SessionId,
        params: &RespondApprovalParams,
        authority: ApprovalAuthority,
    ) -> RuntimeControlReply {
        let authenticator = self.authenticate_approval_response(params);
        let mutation = match self.begin(
            store,
            integration,
            RuntimeMethod::ApprovalsRespond,
            &params.request_id,
            authenticator,
        ) {
            Ok(Begun::New(key)) => key,
            Ok(Begun::Replay(reply)) => return *reply,
            Err(failure) => return RuntimeControlReply::Failed(failure),
        };
        if let Err(failure) = self.verify_lease_values(
            sessions,
            integration,
            session,
            &params.lease_id,
            params.lease_generation,
        ) {
            return self.deny(store, mutation, failure);
        }
        let Ok(approval) = ApprovalId::from_str(&params.approval_id) else {
            return self.deny(
                store,
                mutation,
                RuntimeControlFailure::new(
                    RuntimeErrorKind::ApprovalOptionInvalid,
                    "the approval identity is invalid or no longer pending",
                ),
            );
        };
        let option = OptionId(params.option_id);
        match sessions.take_for_answer_approval_with_authority(
            authority,
            session,
            approval,
            option,
            params.subject_digest,
        ) {
            Ok((taken, command)) => RuntimeControlReply::Sending {
                mutation,
                taken,
                command,
            },
            Err(error) => self.deny(store, mutation, approval_failure(&error)),
        }
    }

    fn watch(
        sessions: &mut SessionManager,
        session: SessionId,
        params: &WatchEventsParams,
        subscription_id: String,
    ) -> RuntimeControlReply {
        let requested = match params.after.as_ref().map(cursor_from_public).transpose() {
            Ok(cursor) => cursor,
            Err(failure) => return RuntimeControlReply::Failed(failure),
        };
        let Ok(view) = sessions.subscribe(session, requested) else {
            return RuntimeControlReply::Failed(not_live());
        };
        let start = view.start();
        RuntimeControlReply::Watching {
            result: WatchEventsResult {
                subscription_id,
                session_id: params.session_id.clone(),
                starts_at: cursor_to_public(start.starts_at),
                live_at: cursor_to_public(start.live_at),
                gap: start.gap.map(|gap| runtrol_runtime_protocol::EventGap {
                    requested: cursor_to_public(gap.requested),
                    live_at: cursor_to_public(gap.live_at),
                }),
            },
            view: Box::new(view),
        }
    }

    fn verify_lease(
        &self,
        sessions: &SessionManager,
        integration: IntegrationKey,
        session: SessionId,
        params: &ControlLeaseParams,
    ) -> Result<(), RuntimeControlFailure> {
        self.verify_lease_values(
            sessions,
            integration,
            session,
            &params.lease_id,
            params.lease_generation,
        )
    }

    fn verify_lease_values(
        &self,
        sessions: &SessionManager,
        integration: IntegrationKey,
        session: SessionId,
        lease_id: &str,
        lease_generation: u64,
    ) -> Result<(), RuntimeControlFailure> {
        let Some(current) = self.leases.get(&session) else {
            return Err(lease_expired());
        };
        if current.public.expires_at_ms < WallMs::now().as_millis() {
            return Err(lease_expired());
        }
        if current.integration != integration
            || current.public.lease_id != lease_id
            || current.public.lease_generation != lease_generation
        {
            return Err(control_conflict());
        }
        let Some(live) = sessions.live_session(session) else {
            return Err(not_live());
        };
        if live.stream != current.stream {
            return Err(control_conflict());
        }
        Ok(())
    }

    fn begin(
        &mut self,
        store: &Store,
        integration: IntegrationKey,
        method: RuntimeMethod,
        request_id: &MutationRequestId,
        authenticator: [u8; 32],
    ) -> Result<Begun, RuntimeControlFailure> {
        let now = WallMs::now().as_millis();
        let Some(created_at) = request_id.unix_millis() else {
            return Err(invalid_request_id());
        };
        if created_at > now.saturating_add(MUTATION_CLOCK_SKEW_MS) {
            return Err(invalid_request_id());
        }
        if created_at.saturating_add(IDEMPOTENCY_WINDOW_MS) < now {
            return Err(RuntimeControlFailure::outcome_unknown());
        }
        let Some(request_bytes) = request_id.to_bytes() else {
            return Err(invalid_request_id());
        };
        let key = IntegrationMutationKey::new(integration, request_bytes);
        if let Some(existing) = self.mutations.get(&key) {
            return compare_replay(existing, method, authenticator)
                .map(Box::new)
                .map(Begun::Replay);
        }
        let before = WallMs::from_millis(now.saturating_sub(IDEMPOTENCY_WINDOW_MS));
        store
            .purge_integration_mutations_before(before)
            .map_err(|_| RuntimeControlFailure::internal())?;
        if let Some(existing) = store
            .get_integration_mutation(key)
            .map_err(|_| RuntimeControlFailure::internal())?
        {
            if existing.boot_id != self.boot_id {
                return Err(RuntimeControlFailure::outcome_unknown());
            }
            if existing.method.as_ref() != method.as_str()
                || existing.authenticator != authenticator
            {
                return Err(idempotency_conflict());
            }
            return Err(RuntimeControlFailure::outcome_unknown());
        }
        let row = IntegrationMutationRow {
            boot_id: self.boot_id,
            created_at: WallMs::from_millis(now),
            method: method.as_str().into(),
            authenticator,
            state: IntegrationMutationState::Pending,
        };
        if !store
            .create_integration_mutation(key, &row)
            .map_err(|_| RuntimeControlFailure::internal())?
        {
            return Err(RuntimeControlFailure::new(
                RuntimeErrorKind::ResourceExhausted,
                "the bounded Runtime mutation ledger is full",
            ));
        }
        self.mutations.insert(
            key,
            MutationMemory {
                method,
                authenticator,
                outcome: MutationOutcome::Pending,
            },
        );
        Ok(Begun::New(key))
    }

    fn finish(
        &mut self,
        store: &Store,
        key: IntegrationMutationKey,
        outcome: MutationOutcome,
    ) -> Result<(), RuntimeControlFailure> {
        let state = match outcome {
            MutationOutcome::Pending => return Err(RuntimeControlFailure::internal()),
            MutationOutcome::Lease(_) | MutationOutcome::Done | MutationOutcome::Open(_) => {
                IntegrationMutationState::Completed
            }
            MutationOutcome::Failed(_) => IntegrationMutationState::Denied,
        };
        if !store
            .finish_integration_mutation(key, self.boot_id, state)
            .map_err(|_| RuntimeControlFailure::internal())?
        {
            return Err(RuntimeControlFailure::outcome_unknown());
        }
        let Some(memory) = self.mutations.get_mut(&key) else {
            return Err(RuntimeControlFailure::outcome_unknown());
        };
        memory.outcome = outcome;
        Ok(())
    }

    fn deny(
        &mut self,
        store: &Store,
        key: IntegrationMutationKey,
        failure: RuntimeControlFailure,
    ) -> RuntimeControlReply {
        match self.finish(store, key, MutationOutcome::Failed(failure)) {
            Ok(()) => RuntimeControlReply::Failed(failure),
            Err(storage) => RuntimeControlReply::Failed(storage),
        }
    }

    fn allocate_lease_generation(&mut self) -> Result<u64, RuntimeControlFailure> {
        self.next_lease_generation = self
            .next_lease_generation
            .checked_add(1)
            .ok_or_else(RuntimeControlFailure::internal)?;
        Ok(self.next_lease_generation)
    }

    fn authenticate_acquire(&self, params: &AcquireControlParams) -> [u8; 32] {
        let mut mac = self.mac(RuntimeMethod::SessionsAcquireControl);
        feed(&mut mac, params.session_id.as_str().as_bytes());
        feed(&mut mac, &[lifecycle_byte(params.expected_lifecycle)]);
        feed(&mut mac, &params.expected_session_generation.to_le_bytes());
        finish_mac(mac)
    }

    fn authenticate_approval_response(&self, params: &RespondApprovalParams) -> [u8; 32] {
        let mut mac = self.mac(RuntimeMethod::ApprovalsRespond);
        feed(&mut mac, params.session_id.as_str().as_bytes());
        feed(&mut mac, params.lease_id.as_bytes());
        feed(&mut mac, &params.lease_generation.to_le_bytes());
        feed(&mut mac, params.approval_id.as_bytes());
        feed(&mut mac, &params.option_id.to_le_bytes());
        feed(&mut mac, &params.subject_digest);
        finish_mac(mac)
    }

    fn authenticate_lease(&self, method: RuntimeMethod, params: &ControlLeaseParams) -> [u8; 32] {
        let mut mac = self.mac(method);
        feed(&mut mac, params.session_id.as_str().as_bytes());
        feed(&mut mac, params.lease_id.as_bytes());
        feed(&mut mac, &params.lease_generation.to_le_bytes());
        finish_mac(mac)
    }

    fn authenticate_submit(&self, params: &SubmitInputParams) -> [u8; 32] {
        let mut mac = self.mac(RuntimeMethod::SessionsSubmitInput);
        feed(&mut mac, params.session_id.as_str().as_bytes());
        feed(&mut mac, params.lease_id.as_bytes());
        feed(&mut mac, &params.lease_generation.to_le_bytes());
        feed(&mut mac, params.input.as_bytes());
        finish_mac(mac)
    }

    fn authenticate_set_model(&self, params: &SetModelParams) -> [u8; 32] {
        let mut mac = self.mac(RuntimeMethod::SessionsSetModel);
        feed(&mut mac, params.session_id.as_str().as_bytes());
        feed(&mut mac, params.lease_id.as_bytes());
        feed(&mut mac, &params.lease_generation.to_le_bytes());
        feed(&mut mac, params.model.as_bytes());
        // Presence is part of the identity: "no effort" and "empty effort" must not collide, and empty is
        // already refused before this runs.
        feed(
            &mut mac,
            params.reasoning_effort.as_deref().unwrap_or("").as_bytes(),
        );
        finish_mac(mac)
    }

    fn authenticate_set_mode(&self, params: &SetModeParams) -> [u8; 32] {
        let mut mac = self.mac(RuntimeMethod::SessionsSetMode);
        feed(&mut mac, params.session_id.as_str().as_bytes());
        feed(&mut mac, params.lease_id.as_bytes());
        feed(&mut mac, &params.lease_generation.to_le_bytes());
        feed(&mut mac, params.mode.as_bytes());
        finish_mac(mac)
    }

    fn authenticate_cool(&self, params: &CoolSessionParams) -> [u8; 32] {
        let mut mac = self.mac(RuntimeMethod::SessionsCool);
        feed(&mut mac, params.session_id.as_str().as_bytes());
        feed(&mut mac, &params.expected_session_generation.to_le_bytes());
        feed(&mut mac, params.lease_id.as_bytes());
        feed(&mut mac, &params.lease_generation.to_le_bytes());
        finish_mac(mac)
    }

    fn authenticate_forget(&self, params: &ForgetSessionParams) -> [u8; 32] {
        let mut mac = self.mac(RuntimeMethod::SessionsForget);
        feed(&mut mac, params.session_id.as_str().as_bytes());
        feed(&mut mac, &params.expected_session_generation.to_le_bytes());
        finish_mac(mac)
    }

    fn authenticate_open(&self, request: &RuntimeOpenRequest) -> [u8; 32] {
        let mut mac = self.mac(request.method);
        feed(&mut mac, request.provider.as_str().as_bytes());
        feed(&mut mac, request.workspace.as_str().as_bytes());
        feed(&mut mac, &[workspace_access_byte(request.claim.access())]);
        if let Some(session) = request.session {
            feed(&mut mac, session.as_bytes());
        } else {
            feed(&mut mac, &[]);
        }
        feed(
            &mut mac,
            request
                .native
                .as_ref()
                .map_or(&[][..], |native| native.as_str().as_bytes()),
        );
        feed(
            &mut mac,
            request.model.as_deref().map_or(&[][..], str::as_bytes),
        );
        feed(
            &mut mac,
            request
                .reasoning_effort
                .as_deref()
                .map_or(&[][..], str::as_bytes),
        );
        feed(
            &mut mac,
            request.proof.as_deref().map_or(&[][..], str::as_bytes),
        );
        if let Some((lifecycle, generation)) = request.expected {
            feed(&mut mac, &[lifecycle_byte(lifecycle)]);
            feed(&mut mac, &generation.to_le_bytes());
        }
        finish_mac(mac)
    }

    fn mac(&self, method: RuntimeMethod) -> Hmac<Sha256> {
        let mut mac = self.authenticator.clone();
        feed(&mut mac, b"runtrol-runtime-mutation-v1");
        feed(&mut mac, method.as_str().as_bytes());
        mac
    }
}

fn compare_replay(
    existing: &MutationMemory,
    method: RuntimeMethod,
    authenticator: [u8; 32],
) -> Result<RuntimeControlReply, RuntimeControlFailure> {
    if existing.method != method || existing.authenticator != authenticator {
        return Err(idempotency_conflict());
    }
    match &existing.outcome {
        MutationOutcome::Pending => Err(RuntimeControlFailure::outcome_unknown()),
        MutationOutcome::Lease(lease) => Ok(RuntimeControlReply::Lease(lease.clone())),
        MutationOutcome::Done => Ok(RuntimeControlReply::Done),
        MutationOutcome::Open(result) => Ok(RuntimeControlReply::Opened(result.clone())),
        MutationOutcome::Failed(failure) => Ok(RuntimeControlReply::Failed(*failure)),
    }
}

fn feed(mac: &mut Hmac<Sha256>, value: &[u8]) {
    mac.update(&u64::try_from(value.len()).unwrap_or(u64::MAX).to_le_bytes());
    mac.update(value);
}

fn finish_mac(mac: Hmac<Sha256>) -> [u8; 32] {
    let bytes = mac.finalize().into_bytes();
    let mut output = [0_u8; 32];
    output.copy_from_slice(&bytes);
    output
}

fn random_label(prefix: &str) -> Result<String, RuntimeControlFailure> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| RuntimeControlFailure::internal())?;
    let mut output = String::with_capacity(prefix.len().saturating_add(32));
    output.push_str(prefix);
    for byte in bytes {
        use core::fmt::Write as _;
        write!(&mut output, "{byte:02x}").map_err(|_| RuntimeControlFailure::internal())?;
    }
    Ok(output)
}

fn cursor_from_public(cursor: &EventCursor) -> Result<WatchCursor, RuntimeControlFailure> {
    let stream = StreamId::from_str(&cursor.stream).map_err(|_| {
        RuntimeControlFailure::new(
            RuntimeErrorKind::InvalidRequest,
            "the event cursor stream identity is malformed",
        )
    })?;
    Ok(WatchCursor {
        stream,
        epoch: cursor.epoch,
        seq: cursor.seq,
    })
}

pub(crate) fn cursor_to_public(cursor: WatchCursor) -> EventCursor {
    EventCursor {
        stream: cursor.stream.to_string(),
        epoch: cursor.epoch,
        seq: cursor.seq,
    }
}

/// The one place Core's waiting vocabulary becomes the public one.
///
/// Exhaustive by construction: both enums have exactly the two values, so adding a third to either without
/// deciding what it means here is a compile error rather than a state that silently never reaches a surface.
pub(crate) const fn public_waiting(waiting: Waiting) -> WaitingOn {
    match waiting {
        Waiting::Person => WaitingOn::Person,
        Waiting::Quota => WaitingOn::Quota,
    }
}

fn public_lifecycle(lifecycle: &Lifecycle) -> LifecycleState {
    match lifecycle {
        Lifecycle::Idle => LifecycleState::HotIdle,
        Lifecycle::Busy { .. } => LifecycleState::HotRunning,
        Lifecycle::Failed { .. } => LifecycleState::Failed,
        Lifecycle::Detached | Lifecycle::Starting | Lifecycle::Closed { .. } => {
            LifecycleState::Cold
        }
    }
}

const fn lifecycle_byte(lifecycle: LifecycleState) -> u8 {
    match lifecycle {
        LifecycleState::HotIdle => 0,
        LifecycleState::HotRunning => 1,
        LifecycleState::Cold => 2,
        LifecycleState::Failed => 3,
    }
}

const fn workspace_access_byte(access: runtrol_provider::WorkspaceAccess) -> u8 {
    match access {
        runtrol_provider::WorkspaceAccess::Exclusive => 0,
        runtrol_provider::WorkspaceAccess::Shared => 1,
    }
}

fn release_opening(sessions: &mut SessionManager, opening: RuntimeOpening) {
    sessions.cancel_open(opening.reservation);
    if let Some(displaced_reservation) = opening.displaced_reservation {
        sessions.release_closing(displaced_reservation);
    }
}

fn open_failure(error: &SessionError) -> RuntimeControlFailure {
    match error {
        SessionError::WorkspaceOccupied { .. } => RuntimeControlFailure::new(
            RuntimeErrorKind::WorkspaceConflict,
            "the working tree already has an incompatible writer",
        ),
        SessionError::NoRoom(_) | SessionError::OpeningCapacityReserved => {
            RuntimeControlFailure::new(
                RuntimeErrorKind::ResourceExhausted,
                "the bounded hot session capacity is busy",
            )
        }
        SessionError::ProviderUpdating { .. } | SessionError::ProviderBusyForUpdate { .. } => {
            RuntimeControlFailure::new(
                RuntimeErrorKind::ProviderUnavailable,
                "the selected provider is changing and cannot open a session",
            )
        }
        SessionError::Provider(provider) => provider_failure(provider),
        _ => session_conflict(),
    }
}

/// Why the coding service itself could not open a session.
///
/// Every arm exists because the operator's next move differs. Authenticating a CLI, installing one,
/// waiting out a quota and filing a vendor bug are four different errands, and a caller that receives
/// one category for all four can only say "it did not work".
///
/// This mattered more than it looks. Every one of these used to fall through to `session_conflict()`,
/// so a coding service that was merely not logged in reported "the session or native pointer changed
/// after the caller observed it" to a surface whose only honest response was to show that sentence to
/// a person. The kind is what a client branches on, so getting it wrong makes assistance impossible
/// no matter how good the surface is.
fn provider_failure(error: &ProviderError) -> RuntimeControlFailure {
    match error {
        // The CLI is not installed, or two of it are and choosing would silently pick one. Both are
        // resolved at the machine by installing or correcting `PATH`, never by retrying.
        ProviderError::BinNotFound { .. } | ProviderError::BinAmbiguous { .. } => {
            RuntimeControlFailure::new(
                RuntimeErrorKind::ProviderUnavailable,
                "this coding service is not installed where Runtrol can run it",
            )
        }
        // Authentication is the one provider failure a person can fix in seconds, and only at their
        // own machine: Runtrol holds no credential and will not accept one. `PresenceRequired` is
        // exactly that instruction, and it already means "a private local action is required".
        ProviderError::AuthRequired { .. } => RuntimeControlFailure::new(
            RuntimeErrorKind::PresenceRequired,
            "this coding service needs you to sign in to it at your own machine",
        ),
        // A clock lifts this one, so the caller may show a wait rather than a fault.
        ProviderError::Quota { .. } => RuntimeControlFailure::new(
            RuntimeErrorKind::RateLimited,
            "the coding service account has reached its limit",
        ),
        // Nothing is broken. The capability is absent, and saying so plainly stops the caller from
        // offering an action that cannot work.
        ProviderError::Unsupported { .. } | ProviderError::NativeRefused { .. } => {
            RuntimeControlFailure::new(
                RuntimeErrorKind::CapabilityUnavailable,
                "this coding service does not offer what the request needs",
            )
        }
        // The CLI changed shape underneath us. Distinct from the above because the fix is a vendor
        // bug report, and reporting it as anything else buries the one failure worth escalating.
        ProviderError::Protocol { .. } => RuntimeControlFailure::new(
            RuntimeErrorKind::ProviderUnavailable,
            "this coding service answered in a shape Runtrol cannot read",
        ),
        // Spawn, IO and timeout are all "it should have worked". Kept together because the next move
        // is the same (look at the machine, try again) and split from the four above because those
        // have specific next moves.
        ProviderError::Spawn { .. } | ProviderError::Io { .. } | ProviderError::Timeout { .. } => {
            RuntimeControlFailure::new(
                RuntimeErrorKind::ProviderUnavailable,
                "this coding service could not be started",
            )
        }
        // `ProviderError` is `#[non_exhaustive]` so a driver outside this repository keeps building.
        // A variant this build has never seen is still the coding service's failure, not a stale
        // pointer, so it lands in the honest category rather than the misleading one.
        _ => RuntimeControlFailure::new(
            RuntimeErrorKind::ProviderUnavailable,
            "this coding service could not open a session",
        ),
    }
}

fn pending_approval(
    request: &ApprovalRequest,
    scopes: ApprovalScopes,
) -> Result<PendingApproval, RuntimeControlFailure> {
    let options = request
        .offerable(scopes.high)
        .into_iter()
        .map(|offered| {
            let unavailable = if !scopes.low && !scopes.high {
                Some("the integration has no approval response scope".to_owned())
            } else {
                offered.unavailable.map(str::to_owned)
            };
            RuntimeApprovalOption {
                option_id: offered.option.id.0,
                label: offered.option.label.into(),
                kind: public_approval_option_kind(offered.option.kind),
                unavailable,
            }
        })
        .collect();
    let subject = serde_json::to_value(&request.subject).map_err(|_| {
        RuntimeControlFailure::new(
            RuntimeErrorKind::Internal,
            "the normalized approval subject could not be transported",
        )
    })?;
    Ok(PendingApproval {
        approval_id: request.id.to_string(),
        kind: public_approval_kind(request.kind),
        risk: match request.risk {
            RiskClass::Low => RuntimeApprovalRisk::Low,
            RiskClass::High => RuntimeApprovalRisk::High,
        },
        options,
        subject,
        subject_incomplete: request.subject_incomplete,
        subject_digest: request.subject_digest,
        expires_at_ms: request.expires_at.as_millis(),
    })
}

const fn public_approval_kind(kind: ApprovalKind) -> RuntimeApprovalKind {
    match kind {
        ApprovalKind::Command => RuntimeApprovalKind::Command,
        ApprovalKind::FileChange => RuntimeApprovalKind::FileChange,
        ApprovalKind::Permissions => RuntimeApprovalKind::Permissions,
        ApprovalKind::Elicitation => RuntimeApprovalKind::Elicitation,
        ApprovalKind::Network => RuntimeApprovalKind::Network,
        _ => RuntimeApprovalKind::Other,
    }
}

const fn public_approval_option_kind(kind: PermissionOptionKind) -> RuntimeApprovalOptionKind {
    match kind {
        PermissionOptionKind::AllowOnce => RuntimeApprovalOptionKind::AllowOnce,
        PermissionOptionKind::AllowAlways => RuntimeApprovalOptionKind::AllowAlways,
        PermissionOptionKind::RejectOnce => RuntimeApprovalOptionKind::RejectOnce,
        PermissionOptionKind::RejectAlways => RuntimeApprovalOptionKind::RejectAlways,
    }
}

fn approval_failure(error: &SessionError) -> RuntimeControlFailure {
    match error {
        SessionError::ApprovalExpired { .. } => RuntimeControlFailure::new(
            RuntimeErrorKind::ApprovalExpired,
            "the pending approval expired before the response was accepted",
        ),
        SessionError::ApprovalNotPending { .. }
        | SessionError::ApprovalSubjectChanged { .. }
        | SessionError::ApprovalOptionNotOffered { .. }
        | SessionError::ApprovalOptionUnavailable { .. } => RuntimeControlFailure::new(
            RuntimeErrorKind::ApprovalOptionInvalid,
            "the approval, subject, or option is no longer an available exact choice",
        ),
        SessionError::NotLive { .. } => not_live(),
        SessionError::AgentInFlight { .. } => RuntimeControlFailure::new(
            RuntimeErrorKind::SessionConflict,
            "another provider command is already in flight for the session",
        ),
        SessionError::Security(_) => RuntimeControlFailure::new(
            RuntimeErrorKind::ScopeDenied,
            "the integration lacks the approval authority required by the pending request",
        ),
        // Answering an approval runs a provider command, so it fails the same ways opening does. A
        // sign-in that lapsed between the request and the answer is the common one, and it used to
        // report a stale pointer here too.
        SessionError::Provider(provider) => provider_failure(provider),
        _ => session_conflict(),
    }
}

const fn not_live() -> RuntimeControlFailure {
    RuntimeControlFailure::new(
        RuntimeErrorKind::SessionNotFound,
        "the authorized Runtime session is not live",
    )
}

const fn session_conflict() -> RuntimeControlFailure {
    RuntimeControlFailure::new(
        RuntimeErrorKind::SessionConflict,
        "the session or native pointer changed after the caller observed it",
    )
}

const fn control_conflict() -> RuntimeControlFailure {
    RuntimeControlFailure::new(
        RuntimeErrorKind::ControlConflict,
        "another control lease or generation is current",
    )
}

const fn lease_expired() -> RuntimeControlFailure {
    RuntimeControlFailure::new(
        RuntimeErrorKind::LeaseExpired,
        "the control lease expired or no longer exists",
    )
}

const fn idempotency_conflict() -> RuntimeControlFailure {
    RuntimeControlFailure::new(
        RuntimeErrorKind::IdempotencyConflict,
        "the mutation identity was reused with different parameters",
    )
}

const fn invalid_request_id() -> RuntimeControlFailure {
    RuntimeControlFailure::new(
        RuntimeErrorKind::InvalidRequest,
        "the mutation identity timestamp is outside the accepted window",
    )
}

#[cfg(test)]
async fn discard_fixture_open_completion(
    sessions: &mut SessionManager,
    completion: RuntimeOpenCompletion,
) {
    if let RuntimeOpenCompletion::Cleanup { agent, reservation } = completion {
        drop(agent.close(runtrol_provider::CloseMode::Kill).await);
        drop(RuntimeControl::finish_open_cleanup(sessions, reservation));
    }
}

#[cfg(test)]
#[expect(
    clippy::too_many_lines,
    reason = "the fixture owner exhaustively mirrors every production owner handoff and cancellation path"
)]
pub(crate) async fn fixture_runtime_owner(
    composed: std::sync::Arc<crate::Composed>,
    mut asked: mpsc::Receiver<RuntimeAsked>,
    mut returned: mpsc::UnboundedReceiver<RuntimeReturned>,
) {
    let mut control = RuntimeControl::new().expect("Runtime control");
    let mut sessions = SessionManager::new();
    loop {
        tokio::select! {
            request = asked.recv() => {
                let Some(RuntimeAsked { integration, request, answered }) = request else {
                    break;
                };
                let reply = control.answer(&composed.store, &mut sessions, integration, request);
                if let Err(reply) = answered.send(reply) {
                    match reply {
                        RuntimeControlReply::Opening(opening) => {
                            let completion = RuntimeControl::abandon_open(&mut sessions, *opening);
                            discard_fixture_open_completion(&mut sessions, completion).await;
                        }
                        RuntimeControlReply::Sending { taken, .. } => {
                            let TakenAgent { agent, lease } = taken;
                            drop(agent);
                            sessions.abandon_agent(lease);
                        }
                        RuntimeControlReply::Cooling(cooling) => {
                            let RuntimeCooling {
                                mutation,
                                agent,
                                reservation,
                            } = cooling;
                            drop(agent.close(runtrol_provider::CloseMode::graceful()).await);
                            RuntimeControl::abandon_cool(
                                &mut sessions,
                                mutation,
                                reservation,
                            );
                        }
                        _ => {}
                    }
                }
            }
            returned = returned.recv() => {
                let Some(returned) = returned else {
                    break;
                };
                match returned {
                    RuntimeReturned::Opened { opening, intent, agent, answered } => {
                        let completion = control.finish_open(
                            &composed.store,
                            &mut sessions,
                            opening,
                            &intent,
                            agent,
                        );
                        if let Err(completion) = answered.send(completion) {
                            discard_fixture_open_completion(&mut sessions, completion).await;
                        }
                    }
                    RuntimeReturned::OpenDenied { opening, failure, answered } => {
                        let completion = control.deny_open(
                            &composed.store,
                            &mut sessions,
                            opening,
                            failure,
                        );
                        if let Err(completion) = answered.send(completion) {
                            discard_fixture_open_completion(&mut sessions, completion).await;
                        }
                    }
                    RuntimeReturned::OpenUnknown { opening, answered } => {
                        let completion = RuntimeControl::abandon_open(&mut sessions, opening);
                        if let Err(completion) = answered.send(completion) {
                            discard_fixture_open_completion(&mut sessions, completion).await;
                        }
                    }
                    RuntimeReturned::OpenAbandoned { opening } => {
                        let completion = RuntimeControl::abandon_open(&mut sessions, opening);
                        discard_fixture_open_completion(&mut sessions, completion).await;
                    }
                    RuntimeReturned::OpenCleaned { reservation, answered } => {
                        let result = RuntimeControl::finish_open_cleanup(&mut sessions, reservation);
                        drop(answered.send(result));
                    }
                    RuntimeReturned::Finished { mutation, taken, outcome, answered } => {
                        let result = control.finish_command(
                            &composed.store,
                            &mut sessions,
                            mutation,
                            taken,
                            &outcome,
                        );
                        let _ignored = answered.send(result);
                    }
                    RuntimeReturned::Abandoned { mutation, lease } => {
                        RuntimeControl::abandon_command(&mut sessions, mutation, lease);
                    }
                    RuntimeReturned::Cooled {
                        mutation,
                        reservation,
                        outcome,
                        answered,
                    } => {
                        let result = control.finish_cool(
                            &composed.store,
                            &mut sessions,
                            mutation,
                            reservation,
                            &outcome,
                        );
                        if answered.send(result).is_err() {}
                    }
                    RuntimeReturned::CoolAbandoned {
                        mutation,
                        reservation,
                    } => {
                        RuntimeControl::abandon_cool(&mut sessions, mutation, reservation);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use runtrol_provider::{
        AbsPath, Agent, ApprovalOption, CloseMode, Disposition, Opaque, OpenIntent,
        PermissionOptionKind, Produced, Provider, ProviderError, ProviderId, WorkspaceAccess,
    };

    use super::*;

    struct QuietAgent(SessionId);

    #[async_trait]
    impl Agent for QuietAgent {
        fn session(&self) -> SessionId {
            self.0
        }

        fn native(&self) -> Option<&str> {
            Some("runtime-control-native")
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

    struct QuietProvider;

    #[async_trait]
    impl Provider for QuietProvider {
        fn id(&self) -> ProviderId {
            ProviderId::parse("runtime-control-fixture").expect("valid fixture provider")
        }

        async fn open(&self, intent: OpenIntent) -> Result<Box<dyn Agent>, ProviderError> {
            Ok(Box::new(QuietAgent(intent.session)))
        }
    }

    struct ApprovalAgent {
        session: SessionId,
        approval: ApprovalRequest,
    }

    #[async_trait]
    impl Agent for ApprovalAgent {
        fn session(&self) -> SessionId {
            self.session
        }

        fn native(&self) -> Option<&str> {
            Some("runtime-approval-native")
        }

        fn approval(&self, id: ApprovalId) -> Option<&ApprovalRequest> {
            (self.approval.id == id).then_some(&self.approval)
        }

        fn approvals(&self) -> Vec<&ApprovalRequest> {
            vec![&self.approval]
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

    struct ApprovalProvider;

    #[async_trait]
    impl Provider for ApprovalProvider {
        fn id(&self) -> ProviderId {
            ProviderId::parse("runtime-approval-fixture").expect("valid fixture provider")
        }

        async fn open(&self, intent: OpenIntent) -> Result<Box<dyn Agent>, ProviderError> {
            Ok(Box::new(ApprovalAgent {
                session: intent.session,
                approval: ApprovalRequest {
                    id: ApprovalId::now(),
                    turn: None,
                    tool_call: None,
                    kind: ApprovalKind::Command,
                    risk: RiskClass::High,
                    options: vec![
                        ApprovalOption {
                            id: OptionId(0),
                            label: "allow once".into(),
                            kind: PermissionOptionKind::AllowOnce,
                        },
                        ApprovalOption {
                            id: OptionId(1),
                            label: "reject once".into(),
                            kind: PermissionOptionKind::RejectOnce,
                        },
                    ],
                    subject: Opaque::owned(r#"{"command":"cargo test"}"#.to_owned()),
                    subject_incomplete: false,
                    subject_digest: [7; 32],
                    expires_at: WallMs::now().plus_millis(90_000),
                },
            }))
        }
    }

    #[tokio::test]
    #[expect(
        clippy::too_many_lines,
        reason = "one approval fixture proves lease binding, risk authority, exact response fields, and replay together"
    )]
    async fn approvals_bind_listing_and_response_to_the_exact_lease_and_held_risk() {
        let directory = std::env::temp_dir().join(format!(
            "runtrol-runtime-approval-{}-{}",
            std::process::id(),
            SessionId::now()
        ));
        std::fs::create_dir_all(&directory).expect("create approval fixture");
        let workspace =
            AbsPath::canonicalize(directory.to_str().expect("UTF-8 fixture")).expect("workspace");
        let store_path = workspace.join("state.redb").expect("store path");
        let store = Store::open(&store_path).expect("open store");
        let session = SessionId::now();
        let mut sessions = SessionManager::new();
        sessions
            .start(
                &ApprovalProvider,
                OpenIntent {
                    session,
                    workspace,
                    disposition: Disposition::Fresh,
                    model: None,
                    reasoning_effort: None,
                    permission: None,
                },
                WorkspaceAccess::Shared,
            )
            .await
            .expect("start approval fixture");
        let integration = IntegrationKey::from_bytes([8; 16]);
        let mut control = RuntimeControl::new().expect("control authority");
        let generation = sessions.state(session).expect("live state").generation();
        let RuntimeControlReply::Lease(lease) = control.answer(
            &store,
            &mut sessions,
            integration,
            RuntimeControlRequest::Acquire {
                session,
                params: AcquireControlParams {
                    request_id: MutationRequestId::now(),
                    session_id: runtrol_runtime_protocol::RuntimeSessionId::new(
                        session.to_string(),
                    ),
                    expected_lifecycle: LifecycleState::HotIdle,
                    expected_session_generation: generation,
                },
            },
        ) else {
            panic!("expected control lease");
        };
        let list = ListPendingApprovalsParams {
            session_id: lease.session_id.clone(),
            lease_id: lease.lease_id.clone(),
            lease_generation: lease.lease_generation,
        };
        let RuntimeControlReply::Approvals(low_list) = control.answer(
            &store,
            &mut sessions,
            integration,
            RuntimeControlRequest::ListApprovals {
                session,
                params: list.clone(),
                scopes: ApprovalScopes {
                    low: true,
                    high: false,
                },
            },
        ) else {
            panic!("expected pending approval list");
        };
        let low_pending = low_list.approvals.first().expect("one pending approval");
        assert_eq!(low_pending.risk, RuntimeApprovalRisk::High);
        assert!(
            low_pending
                .options
                .iter()
                .all(|option| option.unavailable.is_some())
        );
        let RuntimeControlReply::Approvals(high_list) = control.answer(
            &store,
            &mut sessions,
            integration,
            RuntimeControlRequest::ListApprovals {
                session,
                params: list,
                scopes: ApprovalScopes {
                    low: false,
                    high: true,
                },
            },
        ) else {
            panic!("expected high-authority pending approval list");
        };
        let pending = high_list.approvals.first().expect("one pending approval");
        assert!(
            pending
                .options
                .iter()
                .all(|option| option.unavailable.is_none())
        );

        let response = RespondApprovalParams {
            request_id: MutationRequestId::now(),
            session_id: lease.session_id,
            lease_id: lease.lease_id,
            lease_generation: lease.lease_generation,
            approval_id: pending.approval_id.clone(),
            option_id: 0,
            subject_digest: pending.subject_digest,
        };
        assert!(matches!(
            control.answer(
                &store,
                &mut sessions,
                integration,
                RuntimeControlRequest::RespondApproval {
                    session,
                    params: response.clone(),
                    authority: ApprovalAuthority::Low,
                },
            ),
            RuntimeControlReply::Failed(RuntimeControlFailure {
                kind: RuntimeErrorKind::ApprovalOptionInvalid,
                ..
            })
        ));
        let mut high_response = response;
        high_response.request_id = MutationRequestId::now();
        let RuntimeControlReply::Sending {
            mutation,
            taken,
            command:
                AgentCommand::Answer {
                    id,
                    option,
                    subject_digest,
                },
        } = control.answer(
            &store,
            &mut sessions,
            integration,
            RuntimeControlRequest::RespondApproval {
                session,
                params: high_response.clone(),
                authority: ApprovalAuthority::High,
            },
        )
        else {
            panic!("expected exact high-risk response handoff");
        };
        assert_eq!(id.to_string(), high_response.approval_id);
        assert_eq!(option, OptionId(0));
        assert_eq!(subject_digest, [7; 32]);
        control
            .finish_command(&store, &mut sessions, mutation, taken, &Ok(()))
            .expect("finish approval response");
        assert!(matches!(
            control.answer(
                &store,
                &mut sessions,
                integration,
                RuntimeControlRequest::RespondApproval {
                    session,
                    params: high_response,
                    authority: ApprovalAuthority::High,
                },
            ),
            RuntimeControlReply::Done
        ));

        drop((control, sessions, store));
        std::fs::remove_dir_all(directory).expect("clean approval fixture");
    }

    #[tokio::test]
    #[expect(
        clippy::too_many_lines,
        reason = "one fixture proves successful attach, durable pointer, exact replay, conflict, cancellation, and restart ambiguity together"
    )]
    async fn open_commits_replays_and_keeps_cancelled_provider_work_ambiguous() {
        let directory = std::env::temp_dir().join(format!(
            "runtrol-runtime-open-{}-{}",
            std::process::id(),
            SessionId::now()
        ));
        let first_directory = directory.join("first");
        let second_directory = directory.join("second");
        std::fs::create_dir_all(&first_directory).expect("create first workspace");
        std::fs::create_dir_all(&second_directory).expect("create second workspace");
        let first = AbsPath::canonicalize(first_directory.to_str().expect("UTF-8 first path"))
            .expect("first workspace");
        let second = AbsPath::canonicalize(second_directory.to_str().expect("UTF-8 second path"))
            .expect("second workspace");
        let store_path = AbsPath::canonicalize(directory.to_str().expect("UTF-8 fixture path"))
            .expect("fixture root")
            .join("state.redb")
            .expect("store path");
        let store = Store::open(&store_path).expect("open store");
        let integration = IntegrationKey::from_bytes([9; 16]);
        let provider = ProviderId::parse("runtime-control-fixture").expect("provider");
        let request_id = MutationRequestId::now();
        let mut control = RuntimeControl::new().expect("control authority");
        let mut sessions = SessionManager::new();

        let request = RuntimeOpenRequest {
            method: RuntimeMethod::SessionsStart,
            request_id: request_id.clone(),
            provider,
            session: None,
            native: None,
            workspace: first.clone(),
            claim: WorkspaceClaim::discover(first.clone(), WorkspaceAccess::Exclusive)
                .expect("first claim"),
            model: None,
            reasoning_effort: None,
            permission: None,
            expected: None,
            proof: None,
        };
        let RuntimeControlReply::Opening(opening) = control.answer(
            &store,
            &mut sessions,
            integration,
            RuntimeControlRequest::PrepareOpen(request),
        ) else {
            panic!("expected an open reservation");
        };
        let session = opening.session;
        let intent = OpenIntent {
            session,
            workspace: first.clone(),
            disposition: Disposition::Fresh,
            model: None,
            reasoning_effort: None,
            permission: None,
        };
        let RuntimeOpenCompletion::Answer(Ok(opened)) = control.finish_open(
            &store,
            &mut sessions,
            *opening,
            &intent,
            Box::new(QuietAgent(session)),
        ) else {
            panic!("expected an attached public session");
        };
        assert_eq!(opened.session.session_id.as_str(), session.to_string());
        assert_eq!(opened.control.session_id, opened.session.session_id);
        assert!(
            store
                .get_session(session)
                .expect("read optional stored pointer")
                .is_none(),
            "fresh native identity becomes durable only with its first provider event"
        );

        let repeated = RuntimeOpenRequest {
            method: RuntimeMethod::SessionsStart,
            request_id: request_id.clone(),
            provider,
            session: None,
            native: None,
            workspace: first.clone(),
            claim: WorkspaceClaim::discover(first.clone(), WorkspaceAccess::Exclusive)
                .expect("replay claim"),
            model: None,
            reasoning_effort: None,
            permission: None,
            expected: None,
            proof: None,
        };
        assert!(matches!(
            control.answer(
                &store,
                &mut sessions,
                integration,
                RuntimeControlRequest::PrepareOpen(repeated),
            ),
            RuntimeControlReply::Opened(result) if result == opened
        ));

        let changed = RuntimeOpenRequest {
            method: RuntimeMethod::SessionsStart,
            request_id,
            provider,
            session: None,
            native: None,
            workspace: first.clone(),
            claim: WorkspaceClaim::discover(first, WorkspaceAccess::Exclusive)
                .expect("changed claim"),
            model: Some("different-model".into()),
            reasoning_effort: None,
            permission: None,
            expected: None,
            proof: None,
        };
        assert!(matches!(
            control.answer(
                &store,
                &mut sessions,
                integration,
                RuntimeControlRequest::PrepareOpen(changed),
            ),
            RuntimeControlReply::Failed(RuntimeControlFailure {
                kind: RuntimeErrorKind::IdempotencyConflict,
                ..
            })
        ));

        let cancelled_id = MutationRequestId::now();
        let cancelled = RuntimeOpenRequest {
            method: RuntimeMethod::SessionsStart,
            request_id: cancelled_id.clone(),
            provider,
            session: None,
            native: None,
            workspace: second.clone(),
            claim: WorkspaceClaim::discover(second.clone(), WorkspaceAccess::Exclusive)
                .expect("second claim"),
            model: None,
            reasoning_effort: None,
            permission: None,
            expected: None,
            proof: None,
        };
        let RuntimeControlReply::Opening(cancelled_opening) = control.answer(
            &store,
            &mut sessions,
            integration,
            RuntimeControlRequest::PrepareOpen(cancelled),
        ) else {
            panic!("expected cancellable reservation");
        };
        assert!(matches!(
            RuntimeControl::abandon_open(&mut sessions, *cancelled_opening),
            RuntimeOpenCompletion::Answer(Err(RuntimeControlFailure {
                kind: RuntimeErrorKind::OutcomeUnknown,
                ..
            }))
        ));
        let replay_cancelled = RuntimeOpenRequest {
            method: RuntimeMethod::SessionsStart,
            request_id: cancelled_id,
            provider,
            session: None,
            native: None,
            workspace: second.clone(),
            claim: WorkspaceClaim::discover(second, WorkspaceAccess::Exclusive)
                .expect("cancelled replay claim"),
            model: None,
            reasoning_effort: None,
            permission: None,
            expected: None,
            proof: None,
        };
        assert!(matches!(
            control.answer(
                &store,
                &mut sessions,
                integration,
                RuntimeControlRequest::PrepareOpen(replay_cancelled),
            ),
            RuntimeControlReply::Failed(RuntimeControlFailure {
                kind: RuntimeErrorKind::OutcomeUnknown,
                ..
            })
        ));

        drop((control, sessions, store));
        std::fs::remove_dir_all(directory).expect("clean fixture directory");
    }

    #[tokio::test]
    #[expect(
        clippy::too_many_lines,
        reason = "one lifecycle fixture proves lease binding, asynchronous cleanup, replay, and conflict together"
    )]
    async fn cooling_requires_the_exact_idle_lease_and_replays_only_after_cleanup() {
        let directory = std::env::temp_dir().join(format!(
            "runtrol-runtime-cool-{}-{}",
            std::process::id(),
            SessionId::now()
        ));
        std::fs::create_dir_all(&directory).expect("create cooling fixture");
        let workspace =
            AbsPath::canonicalize(directory.to_str().expect("UTF-8 fixture")).expect("workspace");
        let store_path = workspace.join("state.redb").expect("store path");
        let store = Store::open(&store_path).expect("open store");
        let session = SessionId::now();
        let mut sessions = SessionManager::new();
        sessions
            .start(
                &QuietProvider,
                OpenIntent {
                    session,
                    workspace,
                    disposition: Disposition::Fresh,
                    model: None,
                    reasoning_effort: None,
                    permission: None,
                },
                WorkspaceAccess::Shared,
            )
            .await
            .expect("start cooling fixture");
        let integration = IntegrationKey::from_bytes([6; 16]);
        let mut control = RuntimeControl::new().expect("control authority");
        let generation = sessions.state(session).expect("live state").generation();
        let acquire = AcquireControlParams {
            request_id: MutationRequestId::now(),
            session_id: runtrol_runtime_protocol::RuntimeSessionId::new(session.to_string()),
            expected_lifecycle: LifecycleState::HotIdle,
            expected_session_generation: generation,
        };
        let RuntimeControlReply::Lease(lease) = control.answer(
            &store,
            &mut sessions,
            integration,
            RuntimeControlRequest::Acquire {
                session,
                params: acquire,
            },
        ) else {
            panic!("expected control lease");
        };
        let params = CoolSessionParams {
            request_id: MutationRequestId::now(),
            session_id: lease.session_id.clone(),
            expected_session_generation: generation,
            lease_id: lease.lease_id,
            lease_generation: lease.lease_generation,
        };
        let RuntimeControlReply::Cooling(cooling) = control.answer(
            &store,
            &mut sessions,
            integration,
            RuntimeControlRequest::Cool {
                session,
                params: params.clone(),
            },
        ) else {
            panic!("expected provider cooling handoff");
        };
        assert!(sessions.live_session(session).is_none());
        let RuntimeCooling {
            mutation,
            agent,
            reservation,
        } = cooling;
        let outcome = agent.close(CloseMode::graceful()).await;
        control
            .finish_cool(&store, &mut sessions, mutation, reservation, &outcome)
            .expect("finish provider cooling");
        assert!(matches!(
            control.answer(
                &store,
                &mut sessions,
                integration,
                RuntimeControlRequest::Cool {
                    session,
                    params: params.clone(),
                },
            ),
            RuntimeControlReply::Done
        ));

        let mut changed = params;
        changed.expected_session_generation = changed.expected_session_generation.saturating_add(1);
        assert!(matches!(
            control.answer(
                &store,
                &mut sessions,
                integration,
                RuntimeControlRequest::Cool {
                    session,
                    params: changed,
                },
            ),
            RuntimeControlReply::Failed(RuntimeControlFailure {
                kind: RuntimeErrorKind::IdempotencyConflict,
                ..
            })
        ));
        drop((control, sessions, store));
        std::fs::remove_dir_all(directory).expect("clean cooling fixture");
    }

    #[tokio::test]
    #[expect(
        clippy::too_many_lines,
        reason = "one fixture proves exact replay, conflict, input non-retention, and restart ambiguity together"
    )]
    async fn control_replays_exactly_and_restart_keeps_ambiguity_safe() {
        let directory =
            std::env::temp_dir().join(format!("runtrol-runtime-control-{}", std::process::id()));
        drop(std::fs::remove_dir_all(&directory));
        std::fs::create_dir_all(&directory).expect("create fixture directory");
        let workspace =
            AbsPath::canonicalize(directory.to_str().expect("UTF-8 fixture")).expect("workspace");
        let store_path = workspace.join("state.redb").expect("store path");
        let store = Store::open(&store_path).expect("open store");
        let session = SessionId::now();
        let mut sessions = SessionManager::new();
        sessions
            .start(
                &QuietProvider,
                OpenIntent {
                    session,
                    workspace: workspace.clone(),
                    disposition: Disposition::Fresh,
                    model: None,
                    reasoning_effort: None,
                    permission: None,
                },
                WorkspaceAccess::Shared,
            )
            .await
            .expect("start fixture session");
        let request_id = MutationRequestId::now();
        let params = AcquireControlParams {
            request_id: request_id.clone(),
            session_id: runtrol_runtime_protocol::RuntimeSessionId::new(session.to_string()),
            expected_lifecycle: LifecycleState::HotIdle,
            expected_session_generation: sessions.state(session).expect("live state").generation(),
        };
        let integration = IntegrationKey::from_bytes([5; 16]);
        let mut control = RuntimeControl::new().expect("control authority");
        let RuntimeControlReply::Lease(first) = control.answer(
            &store,
            &mut sessions,
            integration,
            RuntimeControlRequest::Acquire {
                session,
                params: params.clone(),
            },
        ) else {
            panic!("expected a lease");
        };
        let RuntimeControlReply::Lease(repeated) = control.answer(
            &store,
            &mut sessions,
            integration,
            RuntimeControlRequest::Acquire {
                session,
                params: params.clone(),
            },
        ) else {
            panic!("expected exact replay");
        };
        assert_eq!(repeated, first);

        let mut conflicting = params.clone();
        conflicting.expected_session_generation =
            conflicting.expected_session_generation.saturating_add(1);
        assert!(matches!(
            control.answer(
                &store,
                &mut sessions,
                integration,
                RuntimeControlRequest::Acquire {
                    session,
                    params: conflicting,
                },
            ),
            RuntimeControlReply::Failed(RuntimeControlFailure {
                kind: RuntimeErrorKind::IdempotencyConflict,
                ..
            })
        ));

        let input = "runtrol-input-must-not-enter-storage-7f49b8";
        let submitted = control.answer(
            &store,
            &mut sessions,
            integration,
            RuntimeControlRequest::Submit {
                session,
                params: SubmitInputParams {
                    request_id: MutationRequestId::now(),
                    session_id: first.session_id.clone(),
                    lease_id: first.lease_id.clone(),
                    lease_generation: first.lease_generation,
                    input: input.to_owned(),
                },
            },
        );
        let RuntimeControlReply::Sending {
            mutation: _,
            taken,
            command,
        } = submitted
        else {
            panic!("expected provider handoff");
        };
        assert!(matches!(
            &command,
            AgentCommand::Prompt(blocks)
                if matches!(blocks.as_slice(), [ContentBlock::Text(text)] if text.as_ref() == input)
        ));
        let TakenAgent { agent, lease } = taken;
        if let Err(agent) = sessions.return_agent(lease, agent) {
            drop(agent);
            panic!("restore fixture agent");
        }

        let mut restarted = RuntimeControl::new().expect("new boot authority");
        assert!(matches!(
            restarted.answer(
                &store,
                &mut sessions,
                integration,
                RuntimeControlRequest::Acquire { session, params },
            ),
            RuntimeControlReply::Failed(RuntimeControlFailure {
                kind: RuntimeErrorKind::OutcomeUnknown,
                ..
            })
        ));
        drop((restarted, control, sessions, store));
        let durable = std::fs::read(store_path.as_std_path()).expect("read durable metadata");
        assert!(
            !durable
                .windows(input.len())
                .any(|window| window == input.as_bytes()),
            "caller input entered Runtime storage"
        );
        std::fs::remove_dir_all(directory).expect("clean fixture directory");
    }

    /// One of each `ProviderError` this build can construct, for the failure-translation tests.
    ///
    /// Written out rather than generated so that adding a variant to `ProviderError` shows up here as
    /// a gap somebody has to fill, which is the point: a new variant with no considered category is
    /// how the whole set drifted into one wrong answer the first time.
    fn every_provider_error() -> Vec<ProviderError> {
        let provider = ProviderId::parse("mapped").expect("provider id");
        vec![
            ProviderError::BinNotFound {
                provider,
                searched: "PATH".into(),
            },
            ProviderError::BinAmbiguous {
                provider,
                candidates: "one, two".into(),
            },
            ProviderError::Spawn {
                provider,
                program: "cli".into(),
                source: std::io::Error::other("no"),
            },
            ProviderError::Protocol {
                provider,
                doing: "opening",
                detail: "unreadable".into(),
            },
            ProviderError::Timeout {
                provider,
                doing: "opening",
                waited_ms: 1,
            },
            ProviderError::AuthRequired {
                provider,
                how: "cli auth login".into(),
            },
            ProviderError::Quota {
                provider,
                resets_in_ms: None,
            },
            ProviderError::Unsupported {
                provider,
                what: "resume".into(),
                why: "this CLI has no such command",
            },
            ProviderError::NativeRefused {
                provider,
                doing: "resuming",
                detail: "declined".into(),
            },
            ProviderError::Io {
                provider,
                doing: "reading",
                source: std::io::Error::other("no"),
            },
        ]
    }

    #[test]
    fn a_coding_service_that_needs_signing_in_says_so_and_not_something_else() {
        // The whole reason this translation exists. A CLI that is merely not logged in is the most
        // common provider failure and the easiest to fix, and it can only be fixed at the operator's
        // own machine, which is exactly what `PresenceRequired` means.
        let failure = provider_failure(&ProviderError::AuthRequired {
            provider: ProviderId::parse("mapped").expect("provider id"),
            how: "cli auth login".into(),
        });
        assert_eq!(failure.kind, RuntimeErrorKind::PresenceRequired);
    }

    #[test]
    fn no_coding_service_failure_is_reported_as_a_stale_pointer() {
        // The regression this file carried: every provider failure fell through to the session
        // conflict arm, so a surface received "the session or native pointer changed after the caller
        // observed it" for a CLI that was not installed, not logged in, or out of quota. A client
        // branches on the kind, so that one wrong value made assistance impossible.
        for error in every_provider_error() {
            let failure = provider_failure(&error);
            assert_ne!(
                failure.kind,
                RuntimeErrorKind::SessionConflict,
                "{error} was reported as a stale pointer"
            );
            assert_ne!(
                failure.kind,
                RuntimeErrorKind::Internal,
                "{error} was reported as an internal fault"
            );
        }
    }

    #[test]
    fn the_four_errands_a_person_can_act_on_stay_apart() {
        // Installing, signing in, waiting out a limit and hitting an absent capability are four
        // different next moves. Collapsing any two of them removes the only information the surface
        // has to offer the right action.
        let provider = ProviderId::parse("mapped").expect("provider id");
        let kinds = [
            provider_failure(&ProviderError::BinNotFound {
                provider,
                searched: "PATH".into(),
            })
            .kind,
            provider_failure(&ProviderError::AuthRequired {
                provider,
                how: "cli auth login".into(),
            })
            .kind,
            provider_failure(&ProviderError::Quota {
                provider,
                resets_in_ms: None,
            })
            .kind,
            provider_failure(&ProviderError::Unsupported {
                provider,
                what: "resume".into(),
                why: "this CLI has no such command",
            })
            .kind,
        ];
        for (index, kind) in kinds.iter().enumerate() {
            for other in kinds.iter().skip(index + 1) {
                assert_ne!(kind, other, "two distinct errands share one category");
            }
        }
    }

    #[test]
    fn opening_and_answering_an_approval_translate_a_provider_failure_the_same_way() {
        // A sign-in can lapse between a request and its answer, so the approval path fails the same
        // ways opening does. Two translations would drift, and the one nobody looked at would be the
        // one that kept reporting a stale pointer.
        for error in every_provider_error() {
            let direct = provider_failure(&error);
            let opening = open_failure(&SessionError::Provider(error));
            assert_eq!(direct.kind, opening.kind);
            assert_eq!(direct.message, opening.message);
        }
        for error in every_provider_error() {
            let direct = provider_failure(&error);
            let approving = approval_failure(&SessionError::Provider(error));
            assert_eq!(direct.kind, approving.kind);
            assert_eq!(direct.message, approving.message);
        }
    }

    #[test]
    fn every_translated_message_reads_as_a_sentence_about_the_coding_service() {
        // The kind is what a client branches on, but the message is what a person reads when the
        // surface has nothing better to show. A wire-shaped or empty one puts protocol vocabulary in
        // front of somebody trying to get work done.
        for error in every_provider_error() {
            let failure = provider_failure(&error);
            assert!(
                failure.message.len() > 20,
                "{error} produced a message too short to explain anything"
            );
            assert!(
                !failure.message.contains("pointer") && !failure.message.contains("Runtime"),
                "{error} produced a message about the transport rather than the service"
            );
        }
    }
}
