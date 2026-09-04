//! Authenticated public Runtime adapter for the existing Core-owned terminal table.
//!
//! The adapter owns only structural descriptors, connection-bound views, renewable control leases, and bounded
//! mutation outcomes. Terminal bytes pass directly between the PTY and the caller and never enter a diagnostic,
//! durable store, authorization record, or conversation model.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64ct::{Base64, Encoding as _};
use runtrol_core::terminal::{Attachment, TerminalError};
use runtrol_provider::{
    AbsPath, NativeTerminalAccess, NativeTerminalTarget, ProviderId as CoreProviderId, TerminalId,
    WallMs,
};
use runtrol_runtime_protocol::{
    AppScope, IDEMPOTENCY_WINDOW_MS, IntegrationGrant, MAX_IDEMPOTENCY_RECORDS,
    MAX_TERMINAL_COLUMNS, MAX_TERMINAL_INDEX_ITEMS, MAX_TERMINAL_ROWS, MAX_TERMINAL_SCREEN_BYTES,
    MAX_TERMINAL_WRITE_BYTES, MutationRequestId, ProviderList, RuntimeErrorKind, RuntimeTerminalId,
    RuntimeTerminalViewId, TerminalAcquireControlParams, TerminalAttachParams,
    TerminalControlLease, TerminalControlParams, TerminalDescriptor, TerminalGeometry,
    TerminalIndexSnapshot, TerminalOpenParams, TerminalOpenTarget, TerminalProcessState,
    TerminalResizeParams, TerminalStopParams, TerminalViewOpened, TerminalWriteParams,
};
#[cfg(windows)]
use runtrol_security::{ProjectRootGuard, ProjectRootIdentity};
use runtrol_store::IntegrationKey;
#[cfg(any(test, not(windows)))]
use runtrol_store::IntegrationRow;
use sha2::{Digest as _, Sha256};
use tokio::sync::Mutex;

use crate::Composed;
use crate::runtime_auth::AuthorizedIntegration;
#[cfg(any(test, not(windows)))]
use crate::runtime_inventory::approved_root_rows;
use crate::runtime_inventory::{AuthorizedRoot, RuntimeSessionCatalogue, authorized_roots};
use crate::runtime_native_sessions::NativeCursorCodec;
use crate::terminal_surface::HostedTerminal;

const LEASE_LIFETIME_MS: u64 = runtrol_runtime_protocol::CONTROL_LEASE_LIFETIME_MS;
/// Filesystem identity checks may block in the operating system, so they get a small separate global lane.
pub(crate) const ROOT_CHECK_SLOTS: usize = 2;
pub(crate) const ROOT_CHECK_DEADLINE: Duration = Duration::from_millis(400);

/// Run one filesystem proof without mistaking a delayed async poll for a late blocking result.
pub(crate) async fn run_root_check<T, F>(
    permits: Arc<tokio::sync::Semaphore>,
    check: F,
) -> Result<T, ()>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    let started = Instant::now();
    let deadline = started + ROOT_CHECK_DEADLINE;
    let permit = tokio::select! {
        biased;
        permit = permits.acquire_owned() => permit.map_err(drop)?,
        () = tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)) => return Err(()),
    };
    let mut worker = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let answer = check();
        (Instant::now(), answer)
    });
    let deadline_wait = tokio::time::sleep_until(tokio::time::Instant::from_std(deadline));
    tokio::pin!(deadline_wait);
    tokio::select! {
        biased;
        joined = &mut worker => match joined {
            Ok((finished, answer)) if finished <= deadline => Ok(answer),
            Ok(_) | Err(_) => Err(()),
        },
        () = &mut deadline_wait => Err(()),
    }
}

/// Public terminal state that is deliberately separate from the PTY host.
pub(crate) struct TerminalRuntimeAdapter {
    state: Mutex<TerminalAuthorityState>,
    /// Serializes admission and launch so concurrent native opens cannot create two processes.
    open_lane: Mutex<()>,
}

impl Default for TerminalRuntimeAdapter {
    fn default() -> Self {
        Self {
            state: Mutex::new(TerminalAuthorityState::default()),
            open_lane: Mutex::new(()),
        }
    }
}

#[derive(Default)]
struct TerminalAuthorityState {
    /// Exactly one control lease per terminal: the view that holds input and resize authority right now.
    leases: BTreeMap<TerminalId, ActiveLease>,
    /// Monotonic per terminal across transfers and renewals; it outlives a lease's expiry or release so a
    /// later holder's generation is still above every earlier one. Bounded by [`MAX_CONTROL_GENERATIONS`].
    control_generations: BTreeMap<TerminalId, u64>,
    mutations: BTreeMap<MutationKey, StoredMutation>,
}

/// How many terminals' control generations are remembered. Above this, the entries of terminals nobody holds
/// control of are dropped; a live holder's ordering is never touched.
const MAX_CONTROL_GENERATIONS: usize = 256;

/// What the public index says about control of one terminal.
#[derive(Clone, Copy, Default)]
struct ControlView {
    generation: u64,
    held: bool,
}

struct ActiveLease {
    owner: IntegrationKey,
    lease_id: String,
    terminal_generation: u64,
    lease_generation: u64,
    expires_at_ms: u64,
}

/// Connection-bound identity for one local view of a brokered terminal.
///
/// Local views do not occupy the public integration lease. The daemon serializes every byte stream at the
/// shared PTY, so the originating terminal and authenticated Runtime viewers can write to the same process.
pub(crate) struct LocalTerminalControl {
    terminal_id: TerminalId,
    terminal_generation: u64,
}

impl LocalTerminalControl {
    /// Bind the first local viewer to the exact process generation it opened.
    pub(crate) fn for_hosted(hosted: &HostedTerminal) -> Self {
        Self {
            terminal_id: hosted.id,
            terminal_generation: hosted.generation,
        }
    }
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
struct MutationKey {
    integration: [u8; 16],
    request: [u8; 16],
}

struct StoredMutation {
    fingerprint: [u8; 32],
    recorded_at_ms: u64,
    outcome: MutationOutcome,
}

#[derive(Clone)]
enum MutationOutcome {
    Opened(TerminalId),
    Lease(TerminalControlLease),
    /// The exact write, resize, or stop has been admitted and may already have reached the provider. Keeping
    /// this record across cancellation or an unknown I/O outcome prevents a retry from performing it twice.
    PendingDone,
    Done,
}

/// One admitted dedicated terminal stream.
pub(crate) struct TerminalView {
    pub(crate) opened: TerminalViewOpened,
    pub(crate) attachment: Attachment,
    pub(crate) hosted: HostedTerminal,
    pub(crate) authority: AuthorizedIntegration,
    #[cfg(windows)]
    pinned_root: PinnedTerminalRoot,
    root_grant_generation: u64,
    last_root_proof: tokio::time::Instant,
}

#[cfg(windows)]
#[derive(Debug)]
struct PinnedTerminalRoot {
    guard: Arc<tokio::sync::Mutex<ProjectRootGuard>>,
}

impl TerminalView {
    /// Rebind a changed grant, or require the blocking lane's recent successful root proof.
    pub(crate) fn refresh_root_authority(&mut self) -> Result<(), TerminalRuntimeFailure> {
        if self.root_grant_generation == self.authority.grant.grant_generation {
            return if self.last_root_proof.elapsed() <= std::time::Duration::from_secs(1) {
                Ok(())
            } else {
                Err(root_authority_failure())
            };
        }
        #[cfg(windows)]
        {
            self.pinned_root = pin_visible_root(&self.authority, &self.hosted)?;
        }
        #[cfg(not(windows))]
        ensure_visible(&self.hosted, &self.authority)?;
        self.root_grant_generation = self.authority.grant.grant_generation;
        self.last_root_proof = tokio::time::Instant::now();
        Ok(())
    }

    /// Record a successful check whose authority stamp still matches this view.
    pub(crate) fn remember_root_proof(&mut self) {
        self.last_root_proof = tokio::time::Instant::now();
    }

    #[cfg(windows)]
    pub(crate) fn pinned_root_guard(&self) -> Arc<tokio::sync::Mutex<ProjectRootGuard>> {
        Arc::clone(&self.pinned_root.guard)
    }
}

/// Safe stable failure before the Runtime server places it in a JSON-RPC envelope.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TerminalRuntimeFailure {
    pub(crate) kind: RuntimeErrorKind,
    pub(crate) message: &'static str,
}

impl TerminalRuntimeFailure {
    const fn new(kind: RuntimeErrorKind, message: &'static str) -> Self {
        Self { kind, message }
    }

    const fn invalid(message: &'static str) -> Self {
        Self::new(RuntimeErrorKind::InvalidRequest, message)
    }

    const fn unavailable(message: &'static str) -> Self {
        Self::new(RuntimeErrorKind::RuntimeUnavailable, message)
    }
}

const fn mutation_in_progress() -> TerminalRuntimeFailure {
    TerminalRuntimeFailure::new(
        RuntimeErrorKind::OutcomeUnknown,
        "the exact terminal mutation is still in progress or its outcome is unknown; it must not be performed twice",
    )
}

fn terminal_lane_failure(error: &TerminalError) -> TerminalRuntimeFailure {
    match error {
        TerminalError::Busy => TerminalRuntimeFailure::new(
            RuntimeErrorKind::ResourceExhausted,
            "the bounded operation lane for this terminal is full",
        ),
        TerminalError::Spawn(_) | TerminalError::Input(_) | TerminalError::Runtime(_) => {
            TerminalRuntimeFailure::new(
                RuntimeErrorKind::OutcomeUnknown,
                "the terminal operation could not enter its ordered lane",
            )
        }
        TerminalError::NotFed | TerminalError::Feed(_) => TerminalRuntimeFailure::new(
            RuntimeErrorKind::InvalidRequest,
            "the terminal operation belongs to an observed mirror's feeding window",
        ),
    }
}

impl TerminalRuntimeAdapter {
    /// Root-filtered live terminal index from this exact Runtime generation.
    pub(crate) async fn list(
        &self,
        composed: &Composed,
        authority: &AuthorizedIntegration,
    ) -> Result<TerminalIndexSnapshot, TerminalRuntimeFailure> {
        let roots = self.validated_roots(composed, authority).await?;
        self.list_validated(composed, &roots).await
    }

    /// Revalidate filesystem authority on the bounded blocking lane used by terminal views.
    pub(crate) async fn validated_roots(
        &self,
        composed: &Composed,
        authority: &AuthorizedIntegration,
    ) -> Result<Vec<AuthorizedRoot>, TerminalRuntimeFailure> {
        let permits = composed.terminal_root_checks.clone();
        let authority = authority.clone();
        let checked = run_root_check(permits, move || current_roots(&authority)).await;
        match checked {
            Ok(Ok(roots)) => Ok(roots),
            Ok(Err(failure)) => Err(failure),
            Err(()) => Err(root_authority_failure()),
        }
    }

    /// Build a terminal index from roots already proven on the blocking lane.
    pub(crate) async fn list_validated(
        &self,
        composed: &Composed,
        roots: &[AuthorizedRoot],
    ) -> Result<TerminalIndexSnapshot, TerminalRuntimeFailure> {
        let generation = runtime_generation()?;
        let control = control_views(&mut *self.state.lock().await, WallMs::now().as_millis());
        let (hosted, changes) = {
            let terminals = composed.terminals.lock().await;
            (terminals.hosted_all(), terminals.change_sender())
        };
        let mut terminals = Vec::new();
        let mut omitted = 0_usize;
        for terminal in hosted {
            if !visible_in(&terminal, roots) {
                continue;
            }
            if terminals.len() >= usize::from(MAX_TERMINAL_INDEX_ITEMS) {
                omitted = omitted.saturating_add(1);
                continue;
            }
            terminals.push(descriptor(
                &terminal,
                generation,
                &changes,
                control.get(&terminal.id).copied().unwrap_or_default(),
            )?);
        }
        let warnings = if omitted == 0 {
            Vec::new()
        } else {
            vec![format!(
                "{omitted} live terminal descriptors exceeded the bounded index and were omitted"
            )]
        };
        Ok(TerminalIndexSnapshot {
            terminals,
            warnings,
        })
    }

    /// Structural terminal table changes. Callers re-run [`Self::list`] after every change.
    pub(crate) async fn changes(&self, composed: &Composed) -> tokio::sync::watch::Receiver<u64> {
        composed.terminals.lock().await.changes()
    }

    /// Keep resource figures current without rebuilding the public terminal index on a quiet authority tick.
    pub(crate) async fn refresh_memory(&self, composed: &Composed) {
        let (hosted, changes) = {
            let terminals = composed.terminals.lock().await;
            (terminals.hosted_all(), terminals.change_sender())
        };
        for terminal in hosted {
            if !terminal.stopping {
                let _resident_bytes = crate::runtime_inventory::resident_bytes_for_terminal(
                    terminal.terminal.pid(),
                    terminal.id,
                    terminal.generation,
                    &changes,
                );
            }
        }
    }

    /// Open or join one terminal and create a connection-bound view.
    #[expect(
        clippy::too_many_arguments,
        clippy::too_many_lines,
        reason = "terminal admission binds authority, provider observation, native proof, catalogue ownership, and PTY launch"
    )]
    pub(crate) async fn open(
        &self,
        composed: &Arc<Composed>,
        discovering: &crate::serve::DiscoveryGates,
        native_cursors: &NativeCursorCodec,
        providers: &ProviderList,
        sessions: &RuntimeSessionCatalogue,
        authority: AuthorizedIntegration,
        params: &TerminalOpenParams,
    ) -> Result<TerminalView, TerminalRuntimeFailure> {
        validate_geometry(params.geometry)?;
        validate_mutation_time(&params.request_id)?;
        if composed.draining.load(std::sync::atomic::Ordering::Acquire) {
            return Err(TerminalRuntimeFailure::unavailable(
                "this Runtime generation is draining and cannot open a new terminal",
            ));
        }
        let fingerprint = fingerprint(params)?;
        let key = mutation_key(authority.key, &params.request_id)?;
        let _opening = self.open_lane.lock().await;
        if let Some(outcome) = self.prior(&key, fingerprint).await? {
            let MutationOutcome::Opened(terminal_id) = outcome else {
                return Err(TerminalRuntimeFailure::new(
                    RuntimeErrorKind::IdempotencyConflict,
                    "the mutation identity belongs to another terminal operation",
                ));
            };
            return self
                .attach_hosted(composed, authority, terminal_id, false)
                .await;
        }

        let roots = current_roots(&authority)?;
        let workspace = AbsPath::canonicalize(&params.workspace).map_err(|_| {
            TerminalRuntimeFailure::new(
                RuntimeErrorKind::RootDenied,
                "the terminal workspace cannot be resolved inside an approved root",
            )
        })?;
        if !roots.iter().any(|root| workspace.is_under(&root.path)) {
            return Err(TerminalRuntimeFailure::new(
                RuntimeErrorKind::RootDenied,
                "the terminal workspace is outside the integration's approved roots",
            ));
        }
        if !providers.providers.iter().any(|provider| {
            provider.provider_id == params.provider_id
                && provider.installation.state
                    == runtrol_runtime_protocol::InstallationState::Usable
        }) {
            return Err(TerminalRuntimeFailure::new(
                RuntimeErrorKind::ProviderUnavailable,
                "the selected provider is not an observed usable Runtime provider",
            ));
        }
        let provider = CoreProviderId::parse(params.provider_id.as_str()).map_err(|_| {
            TerminalRuntimeFailure::new(
                RuntimeErrorKind::ProviderUnavailable,
                "the selected provider identity is not usable by this Runtime",
            )
        })?;
        let prepared = {
            let _lane = discovering.lane(provider).await.lock_owned().await;
            crate::provider_prepare::prepared_terminal_driver(composed, provider)
                .await
                .map_err(|_| {
                    TerminalRuntimeFailure::new(
                        RuntimeErrorKind::ProviderUnavailable,
                        "the selected provider could not be prepared",
                    )
                })?
        };
        // Whether the coding service was asked which conversations its live processes hold, and answered
        // without naming this one. It travels to the claim reservation, where an unnamed terminal in the same
        // folder would otherwise refuse this conversation on the chance of being it.
        let mut holder_known = false;
        // A provider-owned live peer is attached lazily after the adoption proof is revalidated. Keeping only
        // this structural decision here means an unviewed session allocates no TUI renderer or screen model.
        let mut live_route: Option<LiveTerminalRoute> = None;
        let native = match &params.target {
            TerminalOpenTarget::Fresh => None,
            TerminalOpenTarget::Native {
                native_session_id,
                adoption_token,
            } => {
                if sessions
                    .live_native_owner(provider, native_session_id)
                    .is_some()
                {
                    return Err(TerminalRuntimeFailure::new(
                        RuntimeErrorKind::NativeConversationBusy,
                        "the provider-native conversation is already live as a structured session",
                    ));
                }
                // A conversation a live provider process already owns is one session, and resuming it here
                // would fork it into two processes on one identity (measured 2026-08-30: the editor's Claude
                // panel session, resumed as a terminal, went on without the terminal seeing another byte).
                // A terminal this Runtime already hosts for it is the one exception: the open joins it below.
                if composed
                    .terminals
                    .lock()
                    .await
                    .open_for(provider, native_session_id)
                    .is_none()
                {
                    // `None` is a service that could not be asked, which proves nothing either way. It is
                    // told apart from a service that answered with no live conversation, because the second
                    // is proof that this conversation is free and the first is not.
                    let answered = match discovering.cached_native_activity(provider).await {
                        Some(activity) => Some(activity),
                        None => match prepared.driver.native_process_activity().await {
                            Ok(activity) => {
                                discovering
                                    .remember_native_activity(provider, activity.clone())
                                    .await;
                                Some(activity)
                            }
                            // A provider that cannot answer about its processes cannot block an open: the
                            // conversation may simply be stored, which is the common case.
                            Err(_) => None,
                        },
                    };
                    if let Some(activity) = answered.as_ref() {
                        if resume_would_fork(&activity.live, native_session_id.as_str()) {
                            live_route = live_terminal_route_for(
                                activity,
                                native_session_id.as_str(),
                                &workspace,
                            );
                            if live_route.is_none() {
                                return Err(TerminalRuntimeFailure::new(
                                    RuntimeErrorKind::NativeConversationBusy,
                                    "the conversation is open in the coding service's own window, and that process publishes no safe live terminal attachment",
                                ));
                            }
                        } else {
                            // The provider answered and did not name this conversation. That is the proof the
                            // claim reservation needs to distinguish a cold conversation from an unnamed owner.
                            holder_known = true;
                        }
                    }
                }
                native_cursors
                    .open_adoption(
                        &authority,
                        &roots,
                        provider,
                        prepared.binary_identity,
                        native_session_id,
                        &workspace,
                        adoption_token,
                    )
                    .map_err(|_| {
                        TerminalRuntimeFailure::new(
                            RuntimeErrorKind::CapabilityUnavailable,
                            "the native catalogue observation expired or no longer matches the provider",
                        )
                    })?;
                if let Some(existing) = composed
                    .terminals
                    .lock()
                    .await
                    .open_for(provider, native_session_id)
                    && existing.workspace != workspace
                {
                    return Err(TerminalRuntimeFailure::new(
                        RuntimeErrorKind::TerminalWorkspaceConflict,
                        "the provider-native terminal is already live in another workspace",
                    ));
                }
                Some(native_session_id.as_str())
            }
        };
        let exact_program = || {
            prepared.terminal_program.clone().ok_or_else(|| {
                TerminalRuntimeFailure::new(
                    RuntimeErrorKind::ProviderUnavailable,
                    "the selected provider has no exact prepared terminal program",
                )
            })
        };
        let opened = match live_route {
            Some(LiveTerminalRoute::Official(attach_target)) => {
                crate::terminal_surface::open_official_attach(
                    composed,
                    provider,
                    native.ok_or_else(|| {
                        TerminalRuntimeFailure::new(
                            RuntimeErrorKind::InvalidRequest,
                            "an official attachment requires one native conversation",
                        )
                    })?,
                    &attach_target,
                    workspace,
                    params.geometry.columns,
                    params.geometry.rows,
                    exact_program()?,
                )
                .await
            }
            None => {
                crate::terminal_surface::open_hosted(
                    composed,
                    provider,
                    native,
                    workspace,
                    params.geometry.columns,
                    params.geometry.rows,
                    Some(exact_program()?),
                    // The service was asked above which conversations its live processes hold, and this one was not
                    // among them (`resume_would_fork`). A terminal in this folder that nobody has named is therefore
                    // provably some other conversation, and must not hold this one back.
                    holder_known,
                )
                .await
            }
        };
        let (terminal_id, _terminal, attachment) = opened
        .map_err(|error| {
            use crate::native_claims::TerminalClaimError;
            use crate::terminal_surface::TerminalOpenError;
            match error {
                TerminalOpenError::Claim(TerminalClaimError::StructuredBusy) => {
                    TerminalRuntimeFailure::new(
                        RuntimeErrorKind::NativeConversationBusy,
                        "the provider-native conversation is already live as a structured session",
                    )
                }
                TerminalOpenError::Claim(TerminalClaimError::TerminalAlreadyLive) => {
                    TerminalRuntimeFailure::new(
                        RuntimeErrorKind::TerminalAlreadyLive,
                        "the provider-native terminal is already live in another Runtime generation",
                    )
                }
                TerminalOpenError::Claim(TerminalClaimError::WorkspaceConflict) => {
                    TerminalRuntimeFailure::new(
                        RuntimeErrorKind::TerminalWorkspaceConflict,
                        "the provider-native conversation is already live in another workspace",
                    )
                }
                TerminalOpenError::Claim(TerminalClaimError::State) => {
                    TerminalRuntimeFailure::new(
                        RuntimeErrorKind::RuntimeUnavailable,
                        "the native live-claim registry is unavailable",
                    )
                }
                TerminalOpenError::Claim(TerminalClaimError::LegacyGenerationBusy) => {
                    TerminalRuntimeFailure::new(
                        RuntimeErrorKind::LegacyGenerationBusy,
                        "a draining legacy Runtime generation cannot export native live claims",
                    )
                }
                TerminalOpenError::NoRoom { .. } => TerminalRuntimeFailure::new(
                    RuntimeErrorKind::ResourceExhausted,
                    "the bounded hosted terminal process table is full",
                ),
                TerminalOpenError::Provider(detail) => {
                    report_open_refusal(&detail);
                    TerminalRuntimeFailure::new(
                        RuntimeErrorKind::ProviderUnavailable,
                        "the provider terminal could not be opened",
                    )
                }
                TerminalOpenError::AlreadyBrokered | TerminalOpenError::NotFedByCaller => {
                    TerminalRuntimeFailure::new(
                        RuntimeErrorKind::Internal,
                        "an observed-mirror refusal reached a terminal open",
                    )
                }
            }
        })?;
        self.remember(key, fingerprint, MutationOutcome::Opened(terminal_id))
            .await?;
        self.finish_view(composed, authority, terminal_id, attachment, true)
            .await
    }

    /// Attach a view without changing shared geometry or granting control.
    pub(crate) async fn attach(
        &self,
        composed: &Arc<Composed>,
        authority: AuthorizedIntegration,
        params: &TerminalAttachParams,
    ) -> Result<TerminalView, TerminalRuntimeFailure> {
        let terminal_id = private_terminal_id(&params.terminal_id)?;
        self.attach_hosted(composed, authority, terminal_id, false)
            .await
    }

    async fn attach_hosted(
        &self,
        composed: &Arc<Composed>,
        authority: AuthorizedIntegration,
        terminal_id: TerminalId,
        initial_control: bool,
    ) -> Result<TerminalView, TerminalRuntimeFailure> {
        let (_hosted, attachment) = crate::terminal_surface::attach_current(composed, terminal_id)
            .await
            .map_err(|_| {
                TerminalRuntimeFailure::new(
                    RuntimeErrorKind::TerminalGone,
                    "the terminal ended in its recorded Runtime generation",
                )
            })?;
        self.finish_view(
            composed,
            authority,
            terminal_id,
            attachment,
            initial_control,
        )
        .await
    }

    async fn finish_view(
        &self,
        composed: &Arc<Composed>,
        authority: AuthorizedIntegration,
        terminal_id: TerminalId,
        attachment: Attachment,
        initial_control: bool,
    ) -> Result<TerminalView, TerminalRuntimeFailure> {
        if attachment.snapshot.len() > MAX_TERMINAL_SCREEN_BYTES {
            return Err(TerminalRuntimeFailure::new(
                RuntimeErrorKind::ResourceExhausted,
                "the bounded terminal screen snapshot exceeds the public limit",
            ));
        }
        let (hosted, changes) = {
            let terminals = composed.terminals.lock().await;
            let hosted = terminals.hosted(terminal_id).ok_or_else(|| {
                TerminalRuntimeFailure::new(
                    RuntimeErrorKind::TerminalGone,
                    "the terminal ended in its recorded Runtime generation",
                )
            })?;
            (hosted, terminals.change_sender())
        };
        #[cfg(windows)]
        let pinned_root = pin_visible_root(&authority, &hosted)?;
        #[cfg(not(windows))]
        ensure_visible(&hosted, &authority)?;
        let control_lease = if initial_control
            && authority
                .grant
                .scopes
                .contains(&AppScope::SessionInputWrite)
        {
            self.initial_lease(authority.key, &hosted).await?
        } else {
            None
        };
        let control = control_views(&mut *self.state.lock().await, WallMs::now().as_millis())
            .get(&hosted.id)
            .copied()
            .unwrap_or_default();
        let opened = TerminalViewOpened {
            terminal: descriptor(&hosted, runtime_generation()?, &changes, control)?,
            view_id: RuntimeTerminalViewId::now(),
            screen_base64: Base64::encode_string(&attachment.snapshot),
            checkpoint_available: attachment.checkpoint_available,
            control_lease,
        };
        Ok(TerminalView {
            opened,
            attachment,
            hosted,
            #[cfg(windows)]
            pinned_root,
            root_grant_generation: authority.grant.grant_generation,
            last_root_proof: tokio::time::Instant::now(),
            authority,
        })
    }

    async fn initial_lease(
        &self,
        owner: IntegrationKey,
        hosted: &HostedTerminal,
    ) -> Result<Option<TerminalControlLease>, TerminalRuntimeFailure> {
        let now = WallMs::now().as_millis();
        let mut state = self.state.lock().await;
        prune_expired_leases(&mut state, now);
        ensure_lease_capacity(&state)?;
        let lease_generation = next_control_generation(&mut state, hosted.id);
        let active = new_lease(owner, hosted.generation, lease_generation)?;
        let public = public_lease(hosted.id, &active)?;
        // The opener takes control: any earlier holder's lease is replaced, which its next write is told.
        state.leases.insert(hosted.id, active);
        Ok(Some(public))
    }

    /// Acquire this integration's current terminal control lease.
    pub(crate) async fn acquire(
        &self,
        composed: &Composed,
        authority: &AuthorizedIntegration,
        params: &TerminalAcquireControlParams,
    ) -> Result<TerminalControlLease, TerminalRuntimeFailure> {
        validate_mutation_time(&params.request_id)?;
        let terminal_id = private_terminal_id(&params.terminal_id)?;
        let hosted = visible_terminal(composed, authority, terminal_id).await?;
        if hosted.generation != params.expected_terminal_generation {
            return Err(TerminalRuntimeFailure::new(
                RuntimeErrorKind::SessionConflict,
                "the observed terminal process generation is stale",
            ));
        }
        let key = mutation_key(authority.key, &params.request_id)?;
        let fingerprint = fingerprint(params)?;
        if let Some(outcome) = self.prior(&key, fingerprint).await? {
            return match outcome {
                MutationOutcome::Lease(lease) => Ok(lease),
                MutationOutcome::Opened(_)
                | MutationOutcome::PendingDone
                | MutationOutcome::Done => Err(TerminalRuntimeFailure::new(
                    RuntimeErrorKind::IdempotencyConflict,
                    "the mutation identity belongs to another terminal operation",
                )),
            };
        }
        let mut state = self.state.lock().await;
        let now = WallMs::now().as_millis();
        prune_mutations(&mut state, now);
        prune_expired_leases(&mut state, now);
        ensure_lease_capacity(&state)?;
        ensure_mutation_capacity(&state)?;
        let lease_generation = next_control_generation(&mut state, terminal_id);
        let active = new_lease(authority.key, hosted.generation, lease_generation)?;
        let public = public_lease(terminal_id, &active)?;
        // Exactly one holder: whoever held control before loses it here, and learns so on its next write.
        state.leases.insert(terminal_id, active);
        state.mutations.insert(
            key,
            StoredMutation {
                fingerprint,
                recorded_at_ms: now,
                outcome: MutationOutcome::Lease(public.clone()),
            },
        );
        drop(state);
        // Visible and ordered: the index carries the new control generation to every reader.
        composed.terminals.lock().await.publish_control_change();
        Ok(public)
    }

    /// Renew one exact lease generation.
    pub(crate) async fn renew(
        &self,
        composed: &Composed,
        authority: &AuthorizedIntegration,
        params: &TerminalControlParams,
    ) -> Result<TerminalControlLease, TerminalRuntimeFailure> {
        let terminal_id = private_terminal_id(&params.terminal_id)?;
        drop(visible_terminal(composed, authority, terminal_id).await?);
        let key = mutation_key(authority.key, &params.request_id)?;
        let fingerprint = fingerprint(params)?;
        if let Some(outcome) = self.prior(&key, fingerprint).await? {
            return match outcome {
                MutationOutcome::Lease(lease) => Ok(lease),
                MutationOutcome::Opened(_)
                | MutationOutcome::PendingDone
                | MutationOutcome::Done => Err(TerminalRuntimeFailure::new(
                    RuntimeErrorKind::IdempotencyConflict,
                    "the mutation identity belongs to another terminal operation",
                )),
            };
        }
        validate_mutation_time(&params.request_id)?;
        let now = WallMs::now().as_millis();
        let mut state = self.state.lock().await;
        prune_mutations(&mut state, now);
        ensure_mutation_capacity(&state)?;
        current_lease_mut(&mut state, terminal_id, authority.key, params, now)?;
        let lease_generation = next_control_generation(&mut state, terminal_id);
        let active = state.leases.get_mut(&terminal_id).ok_or_else(|| {
            TerminalRuntimeFailure::new(
                RuntimeErrorKind::LeaseExpired,
                "the terminal lease expired",
            )
        })?;
        active.lease_generation = lease_generation;
        active.expires_at_ms = now.saturating_add(LEASE_LIFETIME_MS);
        let public = public_lease(terminal_id, active)?;
        state.mutations.insert(
            key,
            StoredMutation {
                fingerprint,
                recorded_at_ms: now,
                outcome: MutationOutcome::Lease(public.clone()),
            },
        );
        Ok(public)
    }

    /// Release one exact lease generation.
    pub(crate) async fn release(
        &self,
        composed: &Composed,
        authority: &AuthorizedIntegration,
        params: &TerminalControlParams,
    ) -> Result<(), TerminalRuntimeFailure> {
        let terminal_id = private_terminal_id(&params.terminal_id)?;
        drop(visible_terminal(composed, authority, terminal_id).await?);
        let key = mutation_key(authority.key, &params.request_id)?;
        let fingerprint = fingerprint(params)?;
        if self.prior_done(&key, fingerprint).await? {
            return Ok(());
        }
        validate_mutation_time(&params.request_id)?;
        let now = WallMs::now().as_millis();
        let mut state = self.state.lock().await;
        prune_mutations(&mut state, now);
        ensure_mutation_capacity(&state)?;
        current_lease_mut(&mut state, terminal_id, authority.key, params, now)?;
        state.leases.remove(&terminal_id);
        remember_done(&mut state, key, fingerprint, now);
        drop(state);
        composed.terminals.lock().await.publish_control_change();
        Ok(())
    }

    /// Write exact bytes once. A bounded pending record prevents a concurrent or cancelled duplicate while
    /// the daemon-wide authority lock is released before the per-terminal PTY operation.
    pub(crate) async fn write(
        &self,
        composed: &Composed,
        authority: &AuthorizedIntegration,
        params: &TerminalWriteParams,
    ) -> Result<(), TerminalRuntimeFailure> {
        let terminal_id = private_terminal_id(&params.terminal_id)?;
        let hosted = visible_terminal(composed, authority, terminal_id).await?;
        self.write_hosted(authority, params, hosted).await
    }

    /// Write through an admitted dedicated view whose Windows root handles remain pinned for its lifetime.
    pub(crate) async fn write_view(
        &self,
        composed: &Composed,
        view: &TerminalView,
        params: &TerminalWriteParams,
    ) -> Result<(), TerminalRuntimeFailure> {
        let terminal_id = private_terminal_id(&params.terminal_id)?;
        let hosted = visible_terminal_in_view(composed, view, terminal_id).await?;
        self.write_hosted(&view.authority, params, hosted).await
    }

    async fn write_hosted(
        &self,
        authority: &AuthorizedIntegration,
        params: &TerminalWriteParams,
        hosted: HostedTerminal,
    ) -> Result<(), TerminalRuntimeFailure> {
        let terminal_id = hosted.id;
        validate_mutation_time(&params.request_id)?;
        let key = mutation_key(authority.key, &params.request_id)?;
        let fingerprint = fingerprint(params)?;
        if self.prior_done(&key, fingerprint).await? {
            return Ok(());
        }
        let mut operation = hosted
            .terminal
            .operation()
            .await
            .map_err(|error| terminal_lane_failure(&error))?;
        let bytes = Base64::decode_vec(&params.bytes_base64).map_err(|_| {
            TerminalRuntimeFailure::invalid("terminal input bytes are not valid base64")
        })?;
        if bytes.len() > MAX_TERMINAL_WRITE_BYTES {
            return Err(TerminalRuntimeFailure::new(
                RuntimeErrorKind::ResourceExhausted,
                "terminal input exceeds the public byte limit",
            ));
        }
        let now = WallMs::now().as_millis();
        let mut state = self.state.lock().await;
        if prior_done_from_state(&mut state, &key, fingerprint, now)? {
            return Ok(());
        }
        ensure_mutation_capacity(&state)?;
        validate_lease_fields(
            &mut state,
            terminal_id,
            authority.key,
            &params.lease_id,
            params.lease_generation,
            now,
        )?;
        remember_pending_done(&mut state, key.clone(), fingerprint, now);
        drop(state);
        operation.input(&bytes).await.map_err(|_| {
            TerminalRuntimeFailure::new(
                RuntimeErrorKind::OutcomeUnknown,
                "the terminal input outcome is unknown and must not be retried automatically",
            )
        })?;
        self.finish_done(&key, fingerprint).await
    }

    /// Resize the shared PTY once under the current lease.
    pub(crate) async fn resize(
        &self,
        composed: &Composed,
        authority: &AuthorizedIntegration,
        params: &TerminalResizeParams,
    ) -> Result<(), TerminalRuntimeFailure> {
        validate_geometry(params.geometry)?;
        validate_mutation_time(&params.request_id)?;
        let terminal_id = private_terminal_id(&params.terminal_id)?;
        let hosted = visible_terminal(composed, authority, terminal_id).await?;
        let key = mutation_key(authority.key, &params.request_id)?;
        let fingerprint = fingerprint(params)?;
        if self.prior_done(&key, fingerprint).await? {
            return Ok(());
        }
        let mut operation = hosted
            .terminal
            .operation()
            .await
            .map_err(|error| terminal_lane_failure(&error))?;
        let now = WallMs::now().as_millis();
        let mut state = self.state.lock().await;
        if prior_done_from_state(&mut state, &key, fingerprint, now)? {
            return Ok(());
        }
        ensure_mutation_capacity(&state)?;
        validate_lease_fields(
            &mut state,
            terminal_id,
            authority.key,
            &params.lease_id,
            params.lease_generation,
            now,
        )?;
        remember_pending_done(&mut state, key.clone(), fingerprint, now);
        drop(state);
        operation
            .resize(runtrol_childproc::PtySize {
                cols: params.geometry.columns,
                rows: params.geometry.rows,
            })
            .await
            .map_err(|_| {
                TerminalRuntimeFailure::new(
                    RuntimeErrorKind::OutcomeUnknown,
                    "the terminal resize outcome is unknown",
                )
            })?;
        self.finish_done(&key, fingerprint).await?;
        composed.terminals.lock().await.publish_geometry_change();
        Ok(())
    }

    /// Stop only the hosted provider process under the current lease.
    pub(crate) async fn stop(
        &self,
        composed: &Composed,
        authority: &AuthorizedIntegration,
        params: &TerminalStopParams,
    ) -> Result<(), TerminalRuntimeFailure> {
        validate_mutation_time(&params.request_id)?;
        let terminal_id = private_terminal_id(&params.terminal_id)?;
        let hosted = visible_terminal(composed, authority, terminal_id).await?;
        let key = mutation_key(authority.key, &params.request_id)?;
        let fingerprint = fingerprint(params)?;
        if self.prior_done(&key, fingerprint).await? {
            return Ok(());
        }
        let _operation = hosted
            .terminal
            .operation()
            .await
            .map_err(|error| terminal_lane_failure(&error))?;
        let now = WallMs::now().as_millis();
        let mut state = self.state.lock().await;
        if prior_done_from_state(&mut state, &key, fingerprint, now)? {
            return Ok(());
        }
        ensure_mutation_capacity(&state)?;
        validate_lease_fields(
            &mut state,
            terminal_id,
            authority.key,
            &params.lease_id,
            params.lease_generation,
            now,
        )?;
        remember_pending_done(&mut state, key.clone(), fingerprint, now);
        drop(state);
        crate::terminal_surface::stop_hosted(&hosted)
            .await
            .map_err(|_| {
                TerminalRuntimeFailure::new(
                    RuntimeErrorKind::OutcomeUnknown,
                    "the terminal stop outcome is unknown",
                )
            })?;
        let mut state = self.state.lock().await;
        finish_done_from_state(&mut state, &key, fingerprint, WallMs::now().as_millis())?;
        state.leases.remove(&terminal_id);
        drop(state);
        composed.terminals.lock().await.mark_stopping(terminal_id);
        Ok(())
    }

    /// Write exact bytes from the local terminal that owns the brokered invocation.
    pub(crate) async fn write_local(
        &self,
        composed: &Composed,
        control: &LocalTerminalControl,
        bytes: &[u8],
    ) -> Result<(), TerminalRuntimeFailure> {
        if bytes.len() > MAX_TERMINAL_WRITE_BYTES {
            return Err(TerminalRuntimeFailure::new(
                RuntimeErrorKind::ResourceExhausted,
                "terminal input exceeds the public byte limit",
            ));
        }
        let hosted = composed
            .terminals
            .lock()
            .await
            .hosted(control.terminal_id)
            .ok_or_else(|| {
                TerminalRuntimeFailure::new(
                    RuntimeErrorKind::TerminalGone,
                    "the brokered terminal has ended",
                )
            })?;
        validate_local_generation(&hosted, control)?;
        hosted.terminal.input(bytes).await.map_err(|_| {
            TerminalRuntimeFailure::new(
                RuntimeErrorKind::OutcomeUnknown,
                "the brokered terminal input outcome is unknown",
            )
        })
    }

    /// Resize the shared PTY from the local terminal that owns the brokered invocation.
    pub(crate) async fn resize_local(
        &self,
        composed: &Composed,
        control: &LocalTerminalControl,
        cols: u16,
        rows: u16,
    ) -> Result<(), TerminalRuntimeFailure> {
        validate_geometry(TerminalGeometry {
            columns: cols,
            rows,
        })?;
        let hosted = composed
            .terminals
            .lock()
            .await
            .hosted(control.terminal_id)
            .ok_or_else(|| {
                TerminalRuntimeFailure::new(
                    RuntimeErrorKind::TerminalGone,
                    "the brokered terminal has ended",
                )
            })?;
        validate_local_generation(&hosted, control)?;
        hosted
            .terminal
            .resize(runtrol_childproc::PtySize { cols, rows })
            .await
            .map_err(|_| {
                TerminalRuntimeFailure::new(
                    RuntimeErrorKind::OutcomeUnknown,
                    "the brokered terminal resize outcome is unknown",
                )
            })?;
        composed.terminals.lock().await.publish_geometry_change();
        Ok(())
    }

    async fn prior(
        &self,
        key: &MutationKey,
        fingerprint: [u8; 32],
    ) -> Result<Option<MutationOutcome>, TerminalRuntimeFailure> {
        let mut state = self.state.lock().await;
        prior_from_state(&mut state, key, fingerprint, WallMs::now().as_millis())
    }

    async fn prior_done(
        &self,
        key: &MutationKey,
        fingerprint: [u8; 32],
    ) -> Result<bool, TerminalRuntimeFailure> {
        match self.prior(key, fingerprint).await? {
            None => Ok(false),
            Some(MutationOutcome::Done) => Ok(true),
            Some(MutationOutcome::PendingDone) => Err(mutation_in_progress()),
            Some(MutationOutcome::Lease(_) | MutationOutcome::Opened(_)) => {
                Err(TerminalRuntimeFailure::new(
                    RuntimeErrorKind::IdempotencyConflict,
                    "the mutation identity belongs to another terminal operation",
                ))
            }
        }
    }

    async fn finish_done(
        &self,
        key: &MutationKey,
        fingerprint: [u8; 32],
    ) -> Result<(), TerminalRuntimeFailure> {
        let mut state = self.state.lock().await;
        finish_done_from_state(&mut state, key, fingerprint, WallMs::now().as_millis())
    }

    async fn remember(
        &self,
        key: MutationKey,
        fingerprint: [u8; 32],
        outcome: MutationOutcome,
    ) -> Result<(), TerminalRuntimeFailure> {
        let now = WallMs::now().as_millis();
        let mut state = self.state.lock().await;
        prune_mutations(&mut state, now);
        ensure_mutation_capacity(&state)?;
        state.mutations.insert(
            key,
            StoredMutation {
                fingerprint,
                recorded_at_ms: now,
                outcome,
            },
        );
        Ok(())
    }

    /// Retire all control authority for an ended terminal.
    pub(crate) async fn terminal_ended(&self, terminal_id: TerminalId) {
        let mut state = self.state.lock().await;
        state.leases.remove(&terminal_id);
        state.control_generations.remove(&terminal_id);
    }
}

/// Whether resuming this conversation would fork it: a live provider process already owns the identity.
///
/// The caller has already ruled out a terminal this Runtime hosts for it (that open joins instead), so any
/// live owner left is outside: the service's own window or another program's child.
fn resume_would_fork(held: &[runtrol_provider::NativeSessionId], native: &str) -> bool {
    held.iter().any(|owned| owned.as_str() == native)
}

/// The provider-neutral route to one exact live terminal owner.
#[derive(Debug, Clone, PartialEq, Eq)]
enum LiveTerminalRoute {
    /// Launch the provider's official TUI attachment command with its opaque target.
    Official(NativeTerminalTarget),
}

/// Select the provider's exact terminal route for one native conversation and workspace.
///
/// The route is deliberately not inferred from the durable native identity. Some providers publish a shorter
/// live-job identity for attachment, and confusing the two either fails closed or starts the wrong renderer.
fn live_terminal_route_for(
    activity: &runtrol_provider::NativeProcessActivity,
    native: &str,
    workspace: &AbsPath,
) -> Option<LiveTerminalRoute> {
    activity.processes.iter().find_map(|process| {
        if process.native.as_str() != native
            || !process.cwd.as_deref().is_some_and(|folder| {
                AbsPath::new(folder).is_ok_and(|reported| reported == *workspace)
            })
        {
            return None;
        }
        match &process.terminal_access {
            NativeTerminalAccess::Unavailable => None,
            NativeTerminalAccess::Official { target } => {
                Some(LiveTerminalRoute::Official(target.clone()))
            }
        }
    })
}

/// Say on the Runtime's operational stderr why a terminal open was refused.
///
/// The public answer stays a fixed sentence; the exact cause (a missing terminal declaration, a spawn
/// failure) goes where an operator reads the daemon's own log. A probe measured this arm answering with no
/// recorded cause anywhere (2026-08-30).
#[expect(
    clippy::print_stderr,
    reason = "a refused open has no waiting log sink; stderr is the daemon's operational failure channel"
)]
fn report_open_refusal(detail: &str) {
    eprintln!("terminal open refused: {detail}");
}

fn validate_geometry(geometry: TerminalGeometry) -> Result<(), TerminalRuntimeFailure> {
    if !(2..=MAX_TERMINAL_COLUMNS).contains(&geometry.columns)
        || !(2..=MAX_TERMINAL_ROWS).contains(&geometry.rows)
    {
        return Err(TerminalRuntimeFailure::invalid(
            "terminal geometry is outside the advertised bounds",
        ));
    }
    Ok(())
}

fn validate_mutation_time(request: &MutationRequestId) -> Result<(), TerminalRuntimeFailure> {
    let Some(issued_at_ms) = request.unix_millis() else {
        return Err(TerminalRuntimeFailure::invalid(
            "terminal mutation identity is not a canonical UUIDv7",
        ));
    };
    let now = WallMs::now().as_millis();
    if issued_at_ms > now.saturating_add(runtrol_runtime_protocol::MUTATION_CLOCK_SKEW_MS)
        || now.saturating_sub(issued_at_ms) > IDEMPOTENCY_WINDOW_MS
    {
        return Err(TerminalRuntimeFailure::invalid(
            "terminal mutation identity is outside the accepted time window",
        ));
    }
    Ok(())
}

fn current_roots(
    authority: &AuthorizedIntegration,
) -> Result<Vec<AuthorizedRoot>, TerminalRuntimeFailure> {
    authorized_roots(authority).map_err(|_| root_authority_failure())
}

const fn root_authority_failure() -> TerminalRuntimeFailure {
    TerminalRuntimeFailure::new(
        RuntimeErrorKind::RootDenied,
        "an approved terminal root no longer has local authority",
    )
}

fn ensure_visible(
    hosted: &HostedTerminal,
    authority: &AuthorizedIntegration,
) -> Result<(), TerminalRuntimeFailure> {
    let roots = current_roots(authority)?;
    if visible_in(hosted, &roots) {
        Ok(())
    } else {
        Err(TerminalRuntimeFailure::new(
            RuntimeErrorKind::RootDenied,
            "the terminal is outside the integration's approved roots",
        ))
    }
}

fn visible_in(hosted: &HostedTerminal, roots: &[AuthorizedRoot]) -> bool {
    roots
        .iter()
        .any(|root| hosted.workspace.is_under(&root.path))
}

#[cfg(windows)]
fn pin_visible_root(
    authority: &AuthorizedIntegration,
    hosted: &HostedTerminal,
) -> Result<PinnedTerminalRoot, TerminalRuntimeFailure> {
    let row = authority
        .roots
        .iter()
        .find(|row| AbsPath::new(&row.path).is_ok_and(|path| hosted.workspace.is_under(&path)))
        .ok_or_else(|| {
            TerminalRuntimeFailure::new(
                RuntimeErrorKind::RootDenied,
                "the terminal is outside the integration's approved roots",
            )
        })?;
    let path = AbsPath::new(&row.path).map_err(|_| {
        TerminalRuntimeFailure::new(
            RuntimeErrorKind::RootDenied,
            "an approved terminal root no longer has local authority",
        )
    })?;
    let guard = ProjectRootGuard::acquire(&path, ProjectRootIdentity::from_bytes(row.identity))
        .map_err(|_| {
            TerminalRuntimeFailure::new(
                RuntimeErrorKind::RootDenied,
                "an approved terminal root no longer has local authority",
            )
        })?;
    Ok(PinnedTerminalRoot {
        guard: Arc::new(tokio::sync::Mutex::new(guard)),
    })
}

/// Revalidate a quiet terminal's output boundary away from its latency-sensitive relay task.
#[cfg(any(test, not(windows)))]
pub(crate) fn validate_workspace_roots(
    row: &IntegrationRow,
    workspace: &AbsPath,
) -> Result<(), TerminalRuntimeFailure> {
    let roots = approved_root_rows(&row.roots).map_err(|_| {
        TerminalRuntimeFailure::new(
            RuntimeErrorKind::RootDenied,
            "an approved terminal root no longer has local authority",
        )
    })?;
    if roots.iter().any(|root| workspace.is_under(&root.path)) {
        Ok(())
    } else {
        Err(TerminalRuntimeFailure::new(
            RuntimeErrorKind::RootDenied,
            "the terminal is outside the integration's approved roots",
        ))
    }
}

async fn visible_terminal(
    composed: &Composed,
    authority: &AuthorizedIntegration,
    terminal_id: TerminalId,
) -> Result<HostedTerminal, TerminalRuntimeFailure> {
    let hosted = composed
        .terminals
        .lock()
        .await
        .hosted(terminal_id)
        .ok_or_else(|| {
            TerminalRuntimeFailure::new(
                RuntimeErrorKind::TerminalNotFound,
                "the terminal does not exist in this Runtime generation",
            )
        })?;
    ensure_visible(&hosted, authority)?;
    Ok(hosted)
}

async fn visible_terminal_in_view(
    composed: &Composed,
    view: &TerminalView,
    terminal_id: TerminalId,
) -> Result<HostedTerminal, TerminalRuntimeFailure> {
    if terminal_id != view.hosted.id {
        return Err(TerminalRuntimeFailure::new(
            RuntimeErrorKind::TerminalNotFound,
            "the terminal view is not bound to this terminal",
        ));
    }
    let hosted = composed
        .terminals
        .lock()
        .await
        .hosted(terminal_id)
        .filter(|hosted| hosted.generation == view.hosted.generation)
        .ok_or_else(|| {
            TerminalRuntimeFailure::new(
                RuntimeErrorKind::TerminalGone,
                "the terminal ended in its recorded Runtime generation",
            )
        })?;
    #[cfg(not(windows))]
    ensure_visible(&hosted, &view.authority)?;
    Ok(hosted)
}

fn descriptor(
    hosted: &HostedTerminal,
    runtime_generation: &str,
    changes: &tokio::sync::watch::Sender<u64>,
    control: ControlView,
) -> Result<TerminalDescriptor, TerminalRuntimeFailure> {
    let size = hosted.terminal.size();
    let (origin, owner) = hosted.origin.projection();
    Ok(TerminalDescriptor {
        terminal_id: hosted.id.to_string().parse().map_err(|_| {
            TerminalRuntimeFailure::new(
                RuntimeErrorKind::Internal,
                "Runtime could not project a terminal identity",
            )
        })?,
        runtime_generation: runtime_generation.to_owned(),
        provider_id: runtrol_runtime_protocol::ProviderId::new(hosted.provider.to_string()),
        workspace: hosted.workspace.as_str().to_owned(),
        native_session_id: hosted.native.as_deref().map(str::to_owned),
        process_state: if hosted.stopping {
            TerminalProcessState::Stopping
        } else {
            TerminalProcessState::Running
        },
        opened_at_ms: hosted.opened_at_ms,
        terminal_generation: hosted.generation,
        geometry: TerminalGeometry {
            columns: size.cols,
            rows: size.rows,
        },
        control_generation: control.generation,
        control_held: control.held,
        origin,
        owner_window_session_id: owner.map(|owner| owner.window_session_id.clone()),
        owner_terminal_key: owner.map(|owner| owner.terminal_key.clone()),
        memory_bytes: if hosted.stopping {
            None
        } else {
            crate::runtime_inventory::resident_bytes_for_terminal(
                hosted.terminal.pid(),
                hosted.id,
                hosted.generation,
                changes,
            )
        },
    })
}

fn runtime_generation() -> Result<&'static str, TerminalRuntimeFailure> {
    crate::build_identity::build_digest().ok_or_else(|| {
        TerminalRuntimeFailure::unavailable("Runtime generation identity is unavailable")
    })
}

fn private_terminal_id(id: &RuntimeTerminalId) -> Result<TerminalId, TerminalRuntimeFailure> {
    id.as_str().parse().map_err(|_| {
        TerminalRuntimeFailure::invalid("terminal identity is not valid in this Runtime generation")
    })
}

fn mutation_key(
    integration: IntegrationKey,
    request: &MutationRequestId,
) -> Result<MutationKey, TerminalRuntimeFailure> {
    let Some(request) = request.to_bytes() else {
        return Err(TerminalRuntimeFailure::invalid(
            "terminal mutation identity is not a canonical UUIDv7",
        ));
    };
    Ok(MutationKey {
        integration: integration.to_bytes(),
        request,
    })
}

fn fingerprint<T: serde::Serialize>(value: &T) -> Result<[u8; 32], TerminalRuntimeFailure> {
    let encoded = serde_json::to_vec(value).map_err(|_| {
        TerminalRuntimeFailure::new(
            RuntimeErrorKind::Internal,
            "Runtime could not bind the terminal mutation parameters",
        )
    })?;
    Ok(Sha256::digest(encoded).into())
}

fn prior_from_state(
    state: &mut TerminalAuthorityState,
    key: &MutationKey,
    fingerprint: [u8; 32],
    now: u64,
) -> Result<Option<MutationOutcome>, TerminalRuntimeFailure> {
    prune_mutations(state, now);
    let Some(stored) = state.mutations.get(key) else {
        return Ok(None);
    };
    if stored.fingerprint != fingerprint {
        return Err(TerminalRuntimeFailure::new(
            RuntimeErrorKind::IdempotencyConflict,
            "the terminal mutation identity was reused with different parameters",
        ));
    }
    Ok(Some(stored.outcome.clone()))
}

fn prior_done_from_state(
    state: &mut TerminalAuthorityState,
    key: &MutationKey,
    fingerprint: [u8; 32],
    now: u64,
) -> Result<bool, TerminalRuntimeFailure> {
    match prior_from_state(state, key, fingerprint, now)? {
        None => Ok(false),
        Some(MutationOutcome::Done) => Ok(true),
        Some(MutationOutcome::PendingDone) => Err(mutation_in_progress()),
        Some(MutationOutcome::Lease(_) | MutationOutcome::Opened(_)) => {
            Err(TerminalRuntimeFailure::new(
                RuntimeErrorKind::IdempotencyConflict,
                "the mutation identity belongs to another terminal operation",
            ))
        }
    }
}

fn remember_pending_done(
    state: &mut TerminalAuthorityState,
    key: MutationKey,
    fingerprint: [u8; 32],
    now: u64,
) {
    state.mutations.insert(
        key,
        StoredMutation {
            fingerprint,
            recorded_at_ms: now,
            outcome: MutationOutcome::PendingDone,
        },
    );
}

fn finish_done_from_state(
    state: &mut TerminalAuthorityState,
    key: &MutationKey,
    fingerprint: [u8; 32],
    now: u64,
) -> Result<(), TerminalRuntimeFailure> {
    let Some(stored) = state.mutations.get_mut(key) else {
        return Err(TerminalRuntimeFailure::new(
            RuntimeErrorKind::OutcomeUnknown,
            "the terminal mutation completed without its bounded idempotency reservation",
        ));
    };
    if stored.fingerprint != fingerprint {
        return Err(TerminalRuntimeFailure::new(
            RuntimeErrorKind::IdempotencyConflict,
            "the terminal mutation identity was reused with different parameters",
        ));
    }
    match &stored.outcome {
        MutationOutcome::PendingDone | MutationOutcome::Done => {
            stored.outcome = MutationOutcome::Done;
            stored.recorded_at_ms = now;
            Ok(())
        }
        MutationOutcome::Lease(_) | MutationOutcome::Opened(_) => Err(TerminalRuntimeFailure::new(
            RuntimeErrorKind::IdempotencyConflict,
            "the mutation identity belongs to another terminal operation",
        )),
    }
}

fn prune_mutations(state: &mut TerminalAuthorityState, now: u64) {
    state
        .mutations
        .retain(|_, mutation| now.saturating_sub(mutation.recorded_at_ms) <= IDEMPOTENCY_WINDOW_MS);
}

fn ensure_mutation_capacity(state: &TerminalAuthorityState) -> Result<(), TerminalRuntimeFailure> {
    if state.mutations.len() >= usize::from(MAX_IDEMPOTENCY_RECORDS) {
        return Err(TerminalRuntimeFailure::new(
            RuntimeErrorKind::ResourceExhausted,
            "the bounded terminal mutation table is full",
        ));
    }
    Ok(())
}

fn remember_done(
    state: &mut TerminalAuthorityState,
    key: MutationKey,
    fingerprint: [u8; 32],
    now: u64,
) {
    state.mutations.insert(
        key,
        StoredMutation {
            fingerprint,
            recorded_at_ms: now,
            outcome: MutationOutcome::Done,
        },
    );
}

fn new_lease(
    owner: IntegrationKey,
    terminal_generation: u64,
    lease_generation: u64,
) -> Result<ActiveLease, TerminalRuntimeFailure> {
    let mut random = [0_u8; 16];
    getrandom::fill(&mut random).map_err(|_| {
        TerminalRuntimeFailure::new(
            RuntimeErrorKind::Internal,
            "Runtime could not allocate terminal control authority",
        )
    })?;
    let mut lease_id = String::from("terminal-lease-");
    for byte in random {
        use core::fmt::Write as _;
        write!(&mut lease_id, "{byte:02x}").map_err(|_| {
            TerminalRuntimeFailure::new(
                RuntimeErrorKind::Internal,
                "Runtime could not allocate terminal control authority",
            )
        })?;
    }
    Ok(ActiveLease {
        owner,
        lease_id,
        terminal_generation,
        lease_generation,
        expires_at_ms: WallMs::now().as_millis().saturating_add(LEASE_LIFETIME_MS),
    })
}

fn public_lease(
    terminal_id: TerminalId,
    active: &ActiveLease,
) -> Result<TerminalControlLease, TerminalRuntimeFailure> {
    let public_id = terminal_id.to_string().parse().map_err(|_| {
        TerminalRuntimeFailure::new(
            RuntimeErrorKind::Internal,
            "Runtime could not project terminal control authority",
        )
    })?;
    Ok(TerminalControlLease {
        lease_id: active.lease_id.clone(),
        terminal_id: public_id,
        terminal_generation: active.terminal_generation,
        lease_generation: active.lease_generation,
        expires_at_ms: active.expires_at_ms,
    })
}

fn prune_expired_leases(state: &mut TerminalAuthorityState, now: u64) {
    state.leases.retain(|_, lease| lease.expires_at_ms > now);
}

/// The next control generation of one terminal: one above every generation it ever handed out.
fn next_control_generation(state: &mut TerminalAuthorityState, terminal_id: TerminalId) -> u64 {
    if state.control_generations.len() >= MAX_CONTROL_GENERATIONS
        && !state.control_generations.contains_key(&terminal_id)
    {
        let held: std::collections::BTreeSet<TerminalId> = state.leases.keys().copied().collect();
        state
            .control_generations
            .retain(|terminal, _| held.contains(terminal));
    }
    let slot = state.control_generations.entry(terminal_id).or_insert(0);
    *slot = slot.saturating_add(1);
    *slot
}

/// What the index says about control of every terminal, read in one short lock.
fn control_views(
    state: &mut TerminalAuthorityState,
    now: u64,
) -> BTreeMap<TerminalId, ControlView> {
    prune_expired_leases(state, now);
    state
        .control_generations
        .iter()
        .map(|(terminal, generation)| {
            (
                *terminal,
                ControlView {
                    generation: *generation,
                    held: state.leases.contains_key(terminal),
                },
            )
        })
        .collect()
}

fn ensure_lease_capacity(state: &TerminalAuthorityState) -> Result<(), TerminalRuntimeFailure> {
    if state.leases.len() >= usize::from(MAX_IDEMPOTENCY_RECORDS) {
        return Err(TerminalRuntimeFailure::new(
            RuntimeErrorKind::ResourceExhausted,
            "the bounded terminal control lease table is full",
        ));
    }
    Ok(())
}

fn current_lease_mut<'a>(
    state: &'a mut TerminalAuthorityState,
    terminal_id: TerminalId,
    owner: IntegrationKey,
    params: &TerminalControlParams,
    now: u64,
) -> Result<&'a mut ActiveLease, TerminalRuntimeFailure> {
    validate_lease_fields(
        state,
        terminal_id,
        owner,
        &params.lease_id,
        params.lease_generation,
        now,
    )?;
    state.leases.get_mut(&terminal_id).ok_or_else(|| {
        TerminalRuntimeFailure::new(RuntimeErrorKind::LeaseExpired, "the terminal lease expired")
    })
}

fn validate_lease_fields(
    state: &mut TerminalAuthorityState,
    terminal_id: TerminalId,
    owner: IntegrationKey,
    lease_id: &str,
    lease_generation: u64,
    now: u64,
) -> Result<(), TerminalRuntimeFailure> {
    prune_expired_leases(state, now);
    let Some(active) = state.leases.get(&terminal_id) else {
        return Err(TerminalRuntimeFailure::new(
            RuntimeErrorKind::LeaseExpired,
            "the terminal control lease expired or was released",
        ));
    };
    if active.owner != owner || active.lease_id != lease_id {
        return Err(TerminalRuntimeFailure::new(
            RuntimeErrorKind::ControlConflict,
            "another view holds control of this terminal",
        ));
    }
    if active.lease_generation != lease_generation {
        return Err(TerminalRuntimeFailure::new(
            RuntimeErrorKind::ControlConflict,
            "the supplied terminal control lease is not current",
        ));
    }
    Ok(())
}

fn validate_local_generation(
    hosted: &HostedTerminal,
    control: &LocalTerminalControl,
) -> Result<(), TerminalRuntimeFailure> {
    if hosted.id != control.terminal_id || hosted.generation != control.terminal_generation {
        return Err(TerminalRuntimeFailure::new(
            RuntimeErrorKind::TerminalGone,
            "the brokered terminal process generation has ended",
        ));
    }
    Ok(())
}

/// Re-check all required scopes against one refreshed grant.
pub(crate) fn has_scopes(grant: &IntegrationGrant, scopes: &[AppScope]) -> bool {
    scopes.iter().all(|scope| grant.scopes.contains(scope))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "current_thread")]
    async fn a_completed_root_check_survives_delayed_async_observation() {
        let permits = Arc::new(tokio::sync::Semaphore::new(1));
        let (started_tx, started_rx) = std::sync::mpsc::sync_channel(1);
        let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);
        let check = tokio::spawn(run_root_check(permits, move || {
            started_tx.send(()).expect("announce blocking check");
            release_rx.recv().expect("release blocking check");
            true
        }));
        loop {
            match started_rx.try_recv() {
                Ok(()) => break,
                Err(std::sync::mpsc::TryRecvError::Empty) => tokio::task::yield_now().await,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    panic!("blocking check ended before announcing itself")
                }
            }
        }
        release_tx.send(()).expect("finish blocking check");
        std::thread::sleep(ROOT_CHECK_DEADLINE + Duration::from_millis(100));
        assert_eq!(
            check.await.expect("join root check"),
            Ok(true),
            "a result completed within the deadline must win when the async runtime observes both it and the timer late"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_root_check_that_really_finishes_late_is_refused() {
        let permits = Arc::new(tokio::sync::Semaphore::new(1));
        let checked = run_root_check(permits, || {
            std::thread::sleep(ROOT_CHECK_DEADLINE + Duration::from_millis(100));
            true
        })
        .await;
        assert_eq!(checked, Err(()));
    }

    #[test]
    fn terminal_geometry_is_rejected_instead_of_silently_clamped() {
        assert!(
            validate_geometry(TerminalGeometry {
                columns: 1,
                rows: 24,
            })
            .is_err()
        );
        assert!(
            validate_geometry(TerminalGeometry {
                columns: 80,
                rows: 1,
            })
            .is_err()
        );
        assert!(
            validate_geometry(TerminalGeometry {
                columns: 80,
                rows: 24,
            })
            .is_ok()
        );
    }

    #[test]
    fn composite_scope_checks_require_every_scope() {
        let grant = IntegrationGrant {
            integration_id: runtrol_runtime_protocol::IntegrationId::new("integration"),
            scopes: vec![AppScope::SessionStart],
            roots: Vec::new(),
            key_generation: 1,
            grant_generation: 1,
        };
        assert!(has_scopes(&grant, &[AppScope::SessionStart]));
        assert!(!has_scopes(
            &grant,
            &[AppScope::SessionStart, AppScope::SessionOutputRead]
        ));
    }

    #[test]
    fn quiet_output_root_check_rejects_a_replaced_directory() {
        let base =
            std::env::temp_dir().join(format!("runtrol-terminal-root-check-{}", TerminalId::now()));
        let project_path = base.join("project");
        std::fs::create_dir_all(&project_path).expect("create project root");
        let project = AbsPath::canonicalize(project_path.to_str().expect("UTF-8 path"))
            .expect("canonical project root");
        let identity = runtrol_security::ProjectRootIdentity::read(&project)
            .expect("read project root identity");
        let row = IntegrationRow {
            public_key: [7; 32],
            client_instance_id: "client".into(),
            label: "Client".into(),
            manifest_digest: [8; 32],
            scopes: vec!["session.output.read".into()],
            roots: vec![runtrol_store::IntegrationRootRow {
                path: project.as_str().into(),
                identity: identity.to_bytes(),
            }],
            key_generation: 1,
            grant_generation: 1,
            approved_at: WallMs::from_millis(1),
            revoked_at: None,
        };

        validate_workspace_roots(&row, &project).expect("the approved directory is visible");
        std::fs::rename(&project_path, base.join("retired")).expect("retire approved root");
        std::fs::create_dir(&project_path).expect("replace root at the same path");
        assert!(
            validate_workspace_roots(&row, &project).is_err(),
            "the replacement must not inherit output authority"
        );
        std::fs::remove_dir_all(&base).expect("clean root fixture");
    }

    #[test]
    fn a_second_view_takes_control_and_the_first_is_told_on_its_next_write() {
        let terminal = TerminalId::now();
        let first_owner = IntegrationKey::from_bytes([1; 16]);
        let second_owner = IntegrationKey::from_bytes([2; 16]);
        let mut state = TerminalAuthorityState::default();
        let first = new_lease(
            first_owner,
            7,
            next_control_generation(&mut state, terminal),
        )
        .expect("the first lease is allocated");
        let first_id = first.lease_id.clone();
        state.leases.insert(terminal, first);
        assert!(validate_lease_fields(&mut state, terminal, first_owner, &first_id, 1, 0).is_ok());

        let second = new_lease(
            second_owner,
            7,
            next_control_generation(&mut state, terminal),
        )
        .expect("the second lease is allocated");
        let second_id = second.lease_id.clone();
        assert_eq!(
            second.lease_generation, 2,
            "control generations are ordered"
        );
        state.leases.insert(terminal, second);
        assert_eq!(state.leases.len(), 1, "exactly one view holds control");
        assert!(
            validate_lease_fields(&mut state, terminal, second_owner, &second_id, 2, 0).is_ok()
        );
        let refused = validate_lease_fields(&mut state, terminal, first_owner, &first_id, 1, 0)
            .expect_err("the first view lost control");
        assert_eq!(refused.kind, RuntimeErrorKind::ControlConflict);

        // Released, then held again: the generation keeps climbing past every earlier holder.
        state.leases.remove(&terminal);
        assert_eq!(next_control_generation(&mut state, terminal), 3);
        let views = control_views(&mut state, 0);
        assert_eq!(
            views
                .get(&terminal)
                .map(|view| (view.generation, view.held)),
            Some((3, false))
        );
    }

    #[test]
    fn an_in_flight_terminal_mutation_is_bounded_and_cannot_execute_twice() {
        let key = MutationKey {
            integration: [3; 16],
            request: [4; 16],
        };
        let fingerprint = [5; 32];
        let mut state = TerminalAuthorityState::default();
        remember_pending_done(&mut state, key.clone(), fingerprint, 10);

        let pending = prior_done_from_state(&mut state, &key, fingerprint, 11)
            .expect_err("a duplicate cannot pass an in-flight reservation");
        assert_eq!(pending.kind, RuntimeErrorKind::OutcomeUnknown);
        assert_eq!(
            state.mutations.len(),
            1,
            "pending work owns one bounded row"
        );

        finish_done_from_state(&mut state, &key, fingerprint, 12)
            .expect("the original operation finalizes its own reservation");
        assert!(
            prior_done_from_state(&mut state, &key, fingerprint, 13)
                .expect("the finished mutation is readable"),
            "an exact retry replays success without touching the terminal"
        );
        assert!(
            prior_done_from_state(&mut state, &key, [6; 32], 13).is_err(),
            "the same identity cannot change parameters"
        );
    }

    #[test]
    fn pending_terminal_mutations_count_toward_the_hard_table_limit() {
        let mut state = TerminalAuthorityState::default();
        for index in 0..MAX_IDEMPOTENCY_RECORDS {
            let mut request = [0; 16];
            request[..2].copy_from_slice(&index.to_le_bytes());
            remember_pending_done(
                &mut state,
                MutationKey {
                    integration: [7; 16],
                    request,
                },
                [8; 32],
                1,
            );
        }
        assert_eq!(state.mutations.len(), usize::from(MAX_IDEMPOTENCY_RECORDS));
        assert!(
            ensure_mutation_capacity(&state).is_err(),
            "an in-flight row cannot live outside the same frozen table ceiling"
        );
    }

    #[test]
    fn a_conversation_a_live_process_owns_is_not_resumed_into_a_second_process() {
        let held = vec![
            runtrol_provider::NativeSessionId::new("aaaaaaaa-0000-4000-8000-000000000001")
                .expect("a well-formed native identity parses"),
        ];
        assert!(super::resume_would_fork(
            &held,
            "aaaaaaaa-0000-4000-8000-000000000001"
        ));
        assert!(!super::resume_would_fork(
            &held,
            "aaaaaaaa-0000-4000-8000-000000000002"
        ));
        assert!(!super::resume_would_fork(
            &[],
            "aaaaaaaa-0000-4000-8000-000000000001"
        ));
    }

    #[test]
    fn live_attachment_requires_the_exact_native_identity_workspace_and_route() {
        let current = std::env::current_dir().expect("the test process has a current directory");
        let workspace = AbsPath::canonicalize(&current.to_string_lossy())
            .expect("the current directory is canonical");
        let native = runtrol_provider::NativeSessionId::new("aaaaaaaa-0000-4000-8000-000000000001")
            .expect("a well-formed native identity parses");
        let unavailable_native =
            runtrol_provider::NativeSessionId::new("aaaaaaaa-0000-4000-8000-000000000003")
                .expect("a well-formed native identity parses");
        let activity = runtrol_provider::NativeProcessActivity {
            live: vec![native.clone(), unavailable_native.clone()],
            active: Vec::new(),
            processes: vec![
                runtrol_provider::NativeProcessBinding {
                    pid: std::process::id(),
                    native,
                    cwd: Some(workspace.as_str().to_owned()),
                    terminal_access: NativeTerminalAccess::Official {
                        target: runtrol_provider::NativeTerminalTarget::new("job-opaque-1")
                            .expect("a valid opaque target"),
                    },
                },
                runtrol_provider::NativeProcessBinding {
                    pid: 45,
                    native: unavailable_native,
                    cwd: Some(workspace.as_str().to_owned()),
                    terminal_access: NativeTerminalAccess::Unavailable,
                },
            ],
        };

        assert!(matches!(
            live_terminal_route_for(
                &activity,
                "aaaaaaaa-0000-4000-8000-000000000001",
                &workspace,
            ),
            Some(LiveTerminalRoute::Official(target)) if target.as_str() == "job-opaque-1"
        ));
        assert!(
            live_terminal_route_for(
                &activity,
                "aaaaaaaa-0000-4000-8000-000000000003",
                &workspace,
            )
            .is_none()
        );
        let other_workspace = AbsPath::canonicalize(
            workspace
                .as_std_path()
                .parent()
                .expect("the current directory has a parent")
                .to_string_lossy()
                .as_ref(),
        )
        .expect("the parent directory is canonical");
        assert!(
            live_terminal_route_for(
                &activity,
                "aaaaaaaa-0000-4000-8000-000000000001",
                &other_workspace,
            )
            .is_none()
        );
    }
}
