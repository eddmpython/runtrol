//! Authenticated public Runtime adapter for the existing Core-owned terminal table.
//!
//! The adapter owns only structural descriptors, connection-bound views, renewable control leases, and bounded
//! mutation outcomes. Terminal bytes pass directly between the PTY and the caller and never enter a diagnostic,
//! durable store, authorization record, or conversation model.

use std::collections::BTreeMap;
use std::sync::Arc;

use base64ct::{Base64, Encoding as _};
use runtrol_core::terminal::{Attachment, ViewerKind};
use runtrol_provider::{AbsPath, ProviderId as CoreProviderId, TerminalId, WallMs};
use runtrol_runtime_protocol::{
    AppScope, IDEMPOTENCY_WINDOW_MS, IntegrationGrant, MAX_IDEMPOTENCY_RECORDS,
    MAX_TERMINAL_COLUMNS, MAX_TERMINAL_INDEX_ITEMS, MAX_TERMINAL_ROWS, MAX_TERMINAL_SCREEN_BYTES,
    MAX_TERMINAL_WRITE_BYTES, MutationRequestId, ProviderList, RuntimeErrorKind, RuntimeTerminalId,
    RuntimeTerminalViewId, TerminalAcquireControlParams, TerminalAttachParams,
    TerminalControlLease, TerminalControlParams, TerminalDescriptor, TerminalGeometry,
    TerminalIndexSnapshot, TerminalOpenParams, TerminalOpenTarget, TerminalProcessState,
    TerminalResizeParams, TerminalStopParams, TerminalViewOpened, TerminalWriteParams,
};
use runtrol_store::IntegrationKey;
use sha2::{Digest as _, Sha256};
use tokio::sync::Mutex;

use crate::Composed;
use crate::runtime_auth::AuthorizedIntegration;
use crate::runtime_inventory::{AuthorizedRoot, RuntimeSessionCatalogue, authorized_roots};
use crate::runtime_native_sessions::NativeCursorCodec;
use crate::terminal_surface::HostedTerminal;

const LEASE_LIFETIME_MS: u64 = runtrol_runtime_protocol::CONTROL_LEASE_LIFETIME_MS;

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
    leases: BTreeMap<(TerminalId, String), ActiveLease>,
    mutations: BTreeMap<MutationKey, StoredMutation>,
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
    /// What this local viewer is, so its input is translated or forwarded the way that viewer needs.
    viewer: ViewerKind,
}

impl LocalTerminalControl {
    /// Bind the first local viewer to the exact process generation it opened.
    pub(crate) fn for_hosted(hosted: &HostedTerminal, viewer: ViewerKind) -> Self {
        Self {
            terminal_id: hosted.id,
            terminal_generation: hosted.generation,
            viewer,
        }
    }

    pub(crate) const fn viewer(&self) -> ViewerKind {
        self.viewer
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
    Done,
}

/// One admitted dedicated terminal stream.
pub(crate) struct TerminalView {
    pub(crate) opened: TerminalViewOpened,
    pub(crate) attachment: Attachment,
    pub(crate) hosted: HostedTerminal,
    pub(crate) authority: AuthorizedIntegration,
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

impl TerminalRuntimeAdapter {
    /// Root-filtered live terminal index from this exact Runtime generation.
    pub(crate) async fn list(
        &self,
        composed: &Composed,
        authority: &AuthorizedIntegration,
    ) -> Result<TerminalIndexSnapshot, TerminalRuntimeFailure> {
        let roots = current_roots(authority)?;
        let generation = runtime_generation()?;
        let hosted = composed.terminals.lock().await.hosted_all();
        let mut terminals = Vec::new();
        let mut omitted = 0_usize;
        for terminal in hosted {
            if !visible_in(&terminal, &roots) {
                continue;
            }
            if terminals.len() >= usize::from(MAX_TERMINAL_INDEX_ITEMS) {
                omitted = omitted.saturating_add(1);
                continue;
            }
            terminals.push(descriptor(&terminal, generation)?);
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
                    let held = match discovering.cached_native_activity(provider).await {
                        Some(activity) => activity.live,
                        None => match prepared.driver.native_process_activity().await {
                            Ok(activity) => {
                                let live = activity.live.clone();
                                discovering
                                    .remember_native_activity(provider, activity)
                                    .await;
                                live
                            }
                            // A provider that cannot answer about its processes cannot block an open: the
                            // conversation may simply be stored, which is the common case.
                            Err(_) => Vec::new(),
                        },
                    };
                    if resume_would_fork(&held, native_session_id.as_str()) {
                        return Err(TerminalRuntimeFailure::new(
                            RuntimeErrorKind::NativeConversationBusy,
                            "the conversation is open in the coding service's own window; a second process is not started for it",
                        ));
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
        let program = prepared.terminal_program.ok_or_else(|| {
            TerminalRuntimeFailure::new(
                RuntimeErrorKind::ProviderUnavailable,
                "the selected provider has no exact prepared terminal program",
            )
        })?;
        let (terminal_id, _terminal, attachment) = crate::terminal_surface::open_hosted(
            composed,
            provider,
            native,
            workspace,
            params.geometry.columns,
            params.geometry.rows,
            Some(program),
            // The public surface is an editor's terminal, which has its own mouse.
            ViewerKind::Terminal,
        )
        .await
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
                TerminalOpenError::Provider(_) => TerminalRuntimeFailure::new(
                    RuntimeErrorKind::ProviderUnavailable,
                    "the provider terminal could not be opened",
                ),
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

    /// Revalidate one live view after a grant generation, root, or terminal table change.
    pub(crate) async fn validate_view(
        &self,
        composed: &Composed,
        authority: &AuthorizedIntegration,
        terminal_id: TerminalId,
    ) -> Result<(), TerminalRuntimeFailure> {
        drop(visible_terminal(composed, authority, terminal_id).await?);
        Ok(())
    }

    async fn attach_hosted(
        &self,
        composed: &Arc<Composed>,
        authority: AuthorizedIntegration,
        terminal_id: TerminalId,
        initial_control: bool,
    ) -> Result<TerminalView, TerminalRuntimeFailure> {
        let (hosted, attachment) =
            crate::terminal_surface::attach_current(composed, terminal_id, ViewerKind::Terminal)
                .await
                .map_err(|_| {
                    TerminalRuntimeFailure::new(
                        RuntimeErrorKind::TerminalGone,
                        "the terminal ended in its recorded Runtime generation",
                    )
                })?;
        ensure_visible(&hosted, &authority)?;
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
        let hosted = composed
            .terminals
            .lock()
            .await
            .hosted(terminal_id)
            .ok_or_else(|| {
                TerminalRuntimeFailure::new(
                    RuntimeErrorKind::TerminalGone,
                    "the terminal ended in its recorded Runtime generation",
                )
            })?;
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
        let opened = TerminalViewOpened {
            terminal: descriptor(&hosted, runtime_generation()?)?,
            view_id: RuntimeTerminalViewId::now(),
            screen_base64: Base64::encode_string(&attachment.snapshot),
            control_lease,
        };
        Ok(TerminalView {
            opened,
            attachment,
            hosted,
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
        let active = new_lease(owner, hosted.generation)?;
        let public = public_lease(hosted.id, &active)?;
        state
            .leases
            .insert((hosted.id, active.lease_id.clone()), active);
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
                MutationOutcome::Opened(_) | MutationOutcome::Done => {
                    Err(TerminalRuntimeFailure::new(
                        RuntimeErrorKind::IdempotencyConflict,
                        "the mutation identity belongs to another terminal operation",
                    ))
                }
            };
        }
        let mut state = self.state.lock().await;
        let now = WallMs::now().as_millis();
        prune_mutations(&mut state, now);
        prune_expired_leases(&mut state, now);
        ensure_lease_capacity(&state)?;
        ensure_mutation_capacity(&state)?;
        let active = new_lease(authority.key, hosted.generation)?;
        let public = public_lease(terminal_id, &active)?;
        state
            .leases
            .insert((terminal_id, active.lease_id.clone()), active);
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
                MutationOutcome::Opened(_) | MutationOutcome::Done => {
                    Err(TerminalRuntimeFailure::new(
                        RuntimeErrorKind::IdempotencyConflict,
                        "the mutation identity belongs to another terminal operation",
                    ))
                }
            };
        }
        validate_mutation_time(&params.request_id)?;
        let now = WallMs::now().as_millis();
        let mut state = self.state.lock().await;
        prune_mutations(&mut state, now);
        ensure_mutation_capacity(&state)?;
        let active = current_lease_mut(&mut state, terminal_id, authority.key, params, now)?;
        active.lease_generation = active.lease_generation.saturating_add(1);
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
        let _active = current_lease_mut(&mut state, terminal_id, authority.key, params, now)?;
        state.leases.remove(&(terminal_id, params.lease_id.clone()));
        remember_done(&mut state, key, fingerprint, now);
        Ok(())
    }

    /// Write exact bytes once. The state lock spans the PTY write so a concurrent duplicate cannot pass twice.
    pub(crate) async fn write(
        &self,
        composed: &Composed,
        authority: &AuthorizedIntegration,
        params: &TerminalWriteParams,
    ) -> Result<(), TerminalRuntimeFailure> {
        let terminal_id = private_terminal_id(&params.terminal_id)?;
        let hosted = visible_terminal(composed, authority, terminal_id).await?;
        let bytes = Base64::decode_vec(&params.bytes_base64).map_err(|_| {
            TerminalRuntimeFailure::invalid("terminal input bytes are not valid base64")
        })?;
        if bytes.len() > MAX_TERMINAL_WRITE_BYTES {
            return Err(TerminalRuntimeFailure::new(
                RuntimeErrorKind::ResourceExhausted,
                "terminal input exceeds the public byte limit",
            ));
        }
        validate_mutation_time(&params.request_id)?;
        let key = mutation_key(authority.key, &params.request_id)?;
        let fingerprint = fingerprint(params)?;
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
        hosted
            .terminal
            .input(&bytes, ViewerKind::Terminal)
            .await
            .map_err(|_| {
                TerminalRuntimeFailure::new(
                    RuntimeErrorKind::OutcomeUnknown,
                    "the terminal input outcome is unknown and must not be retried automatically",
                )
            })?;
        remember_done(&mut state, key, fingerprint, now);
        Ok(())
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
        hosted
            .terminal
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
        remember_done(&mut state, key, fingerprint, now);
        drop(state);
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
        hosted.terminal.kill().map_err(|_| {
            TerminalRuntimeFailure::new(
                RuntimeErrorKind::OutcomeUnknown,
                "the terminal stop outcome is unknown",
            )
        })?;
        remember_done(&mut state, key, fingerprint, now);
        state
            .leases
            .retain(|(leased_terminal, _), _| *leased_terminal != terminal_id);
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
        hosted
            .terminal
            .input(bytes, control.viewer())
            .await
            .map_err(|_| {
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
            Some(MutationOutcome::Lease(_) | MutationOutcome::Opened(_)) => {
                Err(TerminalRuntimeFailure::new(
                    RuntimeErrorKind::IdempotencyConflict,
                    "the mutation identity belongs to another terminal operation",
                ))
            }
        }
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
        self.state
            .lock()
            .await
            .leases
            .retain(|(leased_terminal, _), _| *leased_terminal != terminal_id);
    }
}

/// Whether resuming this conversation would fork it: a live provider process already owns the identity.
///
/// The caller has already ruled out a terminal this Runtime hosts for it (that open joins instead), so any
/// live owner left is outside: the service's own window or another program's child.
fn resume_would_fork(held: &[runtrol_provider::NativeSessionId], native: &str) -> bool {
    held.iter().any(|owned| owned.as_str() == native)
}

fn validate_geometry(geometry: TerminalGeometry) -> Result<(), TerminalRuntimeFailure> {
    if !(2..=MAX_TERMINAL_COLUMNS).contains(&geometry.columns)
        || !(1..=MAX_TERMINAL_ROWS).contains(&geometry.rows)
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
    authorized_roots(authority).map_err(|_| {
        TerminalRuntimeFailure::new(
            RuntimeErrorKind::RootDenied,
            "an approved terminal root no longer has local authority",
        )
    })
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

fn descriptor(
    hosted: &HostedTerminal,
    runtime_generation: &str,
) -> Result<TerminalDescriptor, TerminalRuntimeFailure> {
    let size = hosted.terminal.size();
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
        memory_bytes: if hosted.stopping {
            None
        } else {
            crate::runtime_inventory::resident_bytes_now(hosted.terminal.pid())
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
        Some(MutationOutcome::Lease(_) | MutationOutcome::Opened(_)) => {
            Err(TerminalRuntimeFailure::new(
                RuntimeErrorKind::IdempotencyConflict,
                "the mutation identity belongs to another terminal operation",
            ))
        }
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
        lease_generation: 1,
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
    state
        .leases
        .get_mut(&(terminal_id, params.lease_id.clone()))
        .ok_or_else(|| {
            TerminalRuntimeFailure::new(
                RuntimeErrorKind::LeaseExpired,
                "the terminal lease expired",
            )
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
    let Some(active) = state.leases.get(&(terminal_id, lease_id.to_owned())) else {
        return Err(TerminalRuntimeFailure::new(
            RuntimeErrorKind::LeaseExpired,
            "the terminal control lease expired or was released",
        ));
    };
    if active.owner != owner
        || active.lease_id != lease_id
        || active.lease_generation != lease_generation
    {
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
    fn two_views_of_one_integration_hold_independent_writer_leases_for_one_pty() {
        let terminal = TerminalId::now();
        let owner = IntegrationKey::from_bytes([1; 16]);
        let first_lease = new_lease(owner, 7).expect("the first lease is allocated");
        let second_lease = new_lease(owner, 7).expect("the second lease is allocated");
        let first_id = first_lease.lease_id.clone();
        let second_id = second_lease.lease_id.clone();
        let mut state = TerminalAuthorityState::default();
        state
            .leases
            .insert((terminal, first_id.clone()), first_lease);
        state
            .leases
            .insert((terminal, second_id.clone()), second_lease);

        assert!(validate_lease_fields(&mut state, terminal, owner, &first_id, 1, 0).is_ok());
        assert!(validate_lease_fields(&mut state, terminal, owner, &second_id, 1, 0).is_ok());
        assert_eq!(
            state.leases.len(),
            2,
            "both viewers still write to the one PTY"
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
}
