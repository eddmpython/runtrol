//! Live grant precedence and owner-local input share one bounded terminal holder.

use super::*;
use crate::window_registry::ConnectionToken;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LeaseOwner {
    Integration(IntegrationKey),
    Local(ConnectionToken),
}

impl From<IntegrationKey> for LeaseOwner {
    fn from(key: IntegrationKey) -> Self {
        Self::Integration(key)
    }
}

/// None means the holder's authority was positively removed. An unreadable relay is an error.
fn holder_priority(
    composed: &Composed,
    owner: LeaseOwner,
    hosted: &HostedTerminal,
) -> Result<Option<bool>, TerminalRuntimeFailure> {
    let LeaseOwner::Integration(key) = owner else {
        return Ok(Some(true));
    };
    let row = match crate::runtime_serve::current_integration_row(composed, key) {
        Ok(row) => row,
        Err(failure)
            if matches!(
                failure.kind,
                RuntimeErrorKind::IntegrationRevoked | RuntimeErrorKind::Unauthenticated
            ) =>
        {
            return Ok(None);
        }
        Err(failure) => {
            return Err(TerminalRuntimeFailure::new(failure.kind, failure.message));
        }
    };
    if row.revoked_at.is_some()
        || !row
            .scopes
            .iter()
            .any(|scope| scope.as_ref() == AppScope::SessionInputWrite.as_str())
        || !row.roots.iter().any(|root| {
            AbsPath::new(&root.path).is_ok_and(|path| hosted.project_root().is_under(&path))
        })
    {
        return Ok(None);
    }
    Ok(Some(row.scopes.iter().any(|scope| {
        scope.as_ref() == AppScope::SessionInputPriority.as_str()
    })))
}

pub(super) fn require_takeover(
    composed: &Composed,
    state: &mut TerminalAuthorityState,
    hosted: &HostedTerminal,
    priority: bool,
    only_if_free: bool,
) -> Result<(), TerminalRuntimeFailure> {
    let Some(holder) = state.leases.get(&hosted.id) else {
        return Ok(());
    };
    let Some(held_priority) = holder_priority(composed, holder.owner, hosted)? else {
        state.leases.remove(&hosted.id);
        return Ok(());
    };
    if only_if_free || (held_priority && !priority) {
        return Err(TerminalRuntimeFailure::new(
            RuntimeErrorKind::ControlConflict,
            "another terminal control lease has precedence",
        ));
    }
    Ok(())
}

impl TerminalRuntimeAdapter {
    /// Lease validation and bounded registration share the takeover lock. A displaced caller cannot
    /// reserve a slot after its successor synchronously retires the old pending reservations.
    pub(super) async fn input_operation<'a>(
        &self,
        composed: &Composed,
        authority: &AuthorizedIntegration,
        hosted: &'a HostedTerminal,
        lease_id: &str,
        lease_generation: u64,
    ) -> Result<runtrol_core::terminal::TerminalOperation<'a>, TerminalRuntimeFailure> {
        let pending = {
            let mut state = self.state.lock().await;
            validate_input_grant(composed, authority)?;
            validate_lease_fields(
                &mut state,
                hosted.id,
                authority.key,
                lease_id,
                lease_generation,
                WallMs::now().as_millis(),
            )?;
            hosted
                .terminal
                .reserve_operation()
                .map_err(|error| terminal_lane_failure(&error))?
        };
        pending
            .wait()
            .await
            .map_err(|error| terminal_lane_failure(&error))
    }

    /// Register the already authenticated owner-local broker in the same holder table as public views.
    pub(crate) async fn local_control(
        &self,
        composed: &Composed,
        hosted: &HostedTerminal,
    ) -> Result<LocalTerminalControl, TerminalRuntimeFailure> {
        let control = LocalTerminalControl {
            terminal_id: hosted.id,
            terminal_generation: hosted.generation,
            owner: ConnectionToken::next(),
        };
        self.claim_local(composed, &control, hosted, false).await?;
        Ok(control)
    }

    async fn claim_local(
        &self,
        composed: &Composed,
        control: &LocalTerminalControl,
        hosted: &HostedTerminal,
        only_if_free: bool,
    ) -> Result<u64, TerminalRuntimeFailure> {
        let mut state = self.state.lock().await;
        let now = WallMs::now().as_millis();
        validate_local_generation(hosted, control)?;
        prune_expired_leases(&mut state, now);
        let owner = LeaseOwner::Local(control.owner);
        if let Some(held) = state.leases.get_mut(&hosted.id)
            && held.owner == owner
        {
            held.expires_at_ms = now.saturating_add(LEASE_LIFETIME_MS);
            return Ok(held.lease_generation);
        }
        require_takeover(composed, &mut state, hosted, true, only_if_free)?;
        ensure_lease_capacity(&state)?;
        let generation = next_control_generation(&mut state, hosted.id);
        let lease = new_lease(owner, hosted.generation, generation)?;
        state.leases.insert(hosted.id, lease);
        hosted.terminal.supersede_pending_operations();
        drop(state);
        composed.terminals.lock().await.publish_control_change();
        Ok(generation)
    }

    /// Release only this connection's current holder, preserving a later public or local takeover.
    pub(crate) async fn release_local(&self, composed: &Composed, control: &LocalTerminalControl) {
        let mut state = self.state.lock().await;
        if !state.leases.get(&control.terminal_id).is_some_and(|held| {
            held.owner == LeaseOwner::Local(control.owner)
                && held.terminal_generation == control.terminal_generation
        }) {
            return;
        }
        state.leases.remove(&control.terminal_id);
        drop(state);
        composed.terminals.lock().await.publish_control_change();
    }

    async fn local_hosted(
        composed: &Composed,
        control: &LocalTerminalControl,
    ) -> Result<HostedTerminal, TerminalRuntimeFailure> {
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
        Ok(hosted)
    }

    async fn local_operation<'a>(
        &self,
        hosted: &'a HostedTerminal,
        control: &LocalTerminalControl,
        claimed: u64,
    ) -> Result<runtrol_core::terminal::TerminalOperation<'a>, TerminalRuntimeFailure> {
        let pending = {
            let state = self.state.lock().await;
            validate_local_lease(&state, hosted, control, claimed)?;
            hosted
                .terminal
                .reserve_operation()
                .map_err(|error| terminal_lane_failure(&error))?
        };
        let operation = pending
            .wait()
            .await
            .map_err(|error| terminal_lane_failure(&error))?;
        let state = self.state.lock().await;
        validate_local_lease(&state, hosted, control, claimed)?;
        Ok(operation)
    }

    /// Exact owner-local typing may take over an equal or lower holder before its ordered write.
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
        let hosted = Self::local_hosted(composed, control).await?;
        let claimed = self.claim_local(composed, control, &hosted, false).await?;
        let mut operation = self.local_operation(&hosted, control, claimed).await?;
        operation.input(bytes).await.map_err(|_| {
            TerminalRuntimeFailure::new(
                RuntimeErrorKind::OutcomeUnknown,
                "the brokered terminal input outcome is unknown",
            )
        })
    }

    /// Geometry follows this exact holder; an old local window cannot reclaim it by resizing.
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
        let hosted = Self::local_hosted(composed, control).await?;
        let claimed = self.claim_local(composed, control, &hosted, true).await?;
        let mut operation = self.local_operation(&hosted, control, claimed).await?;
        operation
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
}

fn validate_local_lease(
    state: &TerminalAuthorityState,
    hosted: &HostedTerminal,
    control: &LocalTerminalControl,
    generation: u64,
) -> Result<(), TerminalRuntimeFailure> {
    validate_local_generation(hosted, control)?;
    if state.leases.get(&hosted.id).is_some_and(|held| {
        held.owner == LeaseOwner::Local(control.owner)
            && held.lease_generation == generation
            && held.expires_at_ms > WallMs::now().as_millis()
    }) {
        return Ok(());
    }
    Err(TerminalRuntimeFailure::new(
        RuntimeErrorKind::ControlConflict,
        "the brokered terminal control changed while the operation waited",
    ))
}
