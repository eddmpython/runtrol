//! Single-owner public Runtime leases, mutations, and bounded event subscriptions.
//!
//! This module is an adapter over Core session authority. It records only structural mutation metadata and a keyed
//! authenticator. Caller input exists only in the request and the provider command handed out for asynchronous I/O.

use std::collections::BTreeMap;
use std::str::FromStr as _;

use hmac::{Hmac, Mac as _};
use runtrol_core::{
    ClosingReservation, OpenReservation, SessionManager, SessionView, TakenAgent, WorkspaceClaim,
};
use runtrol_core::{Lifecycle, SessionError};
use runtrol_provider::{
    AbsPath, Agent, AgentCommand, ContentBlock, NativeSessionId, OpenIntent, ProviderId, SessionId,
    StreamId, WallMs, WatchCursor,
};
use runtrol_runtime_protocol::{
    AcquireControlParams, CONTROL_LEASE_LIFETIME_MS, ControlLease, ControlLeaseParams, EventCursor,
    IDEMPOTENCY_WINDOW_MS, LifecycleState, MAX_INPUT_BYTES, MUTATION_CLOCK_SKEW_MS,
    MutationRequestId, RuntimeErrorKind, RuntimeMethod, SessionDescriptor, SessionOpenResult,
    SubmitInputParams, WatchEventsParams, WatchEventsResult,
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
    Interrupt {
        session: SessionId,
        params: ControlLeaseParams,
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
    Watching {
        result: WatchEventsResult,
        view: Box<SessionView>,
    },
    Sending {
        mutation: IntegrationMutationKey,
        taken: TakenAgent,
        command: AgentCommand,
    },
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
    pub(crate) proof: Option<Box<str>>,
    pub(crate) reservation: OpenReservation,
    pub(crate) displaced_agent: Option<Box<dyn Agent>>,
    pub(crate) displaced_reservation: Option<ClosingReservation>,
    lease_id: String,
    lease_generation: u64,
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
            RuntimeControlRequest::Interrupt { session, params } => {
                self.interrupt(store, sessions, integration, session, &params)
            }
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
                lifecycle: public_lifecycle(live.state.lifecycle()),
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
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use runtrol_provider::{
        AbsPath, Agent, CloseMode, Disposition, OpenIntent, Produced, Provider, ProviderError,
        ProviderId, WorkspaceAccess,
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
}
