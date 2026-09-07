//! One transient owner-extension delivery, admitted through the existing terminal lease and mutation ledger.

use std::time::{Duration, Instant};

use crate::Composed;
use runtrol_provider::ProcessIdentity;
use runtrol_runtime_protocol::{
    TerminalSendTextParams, TerminalTextOutcome, TerminalTextReceipt, WindowInputBinding,
    WindowInputClaim, WindowInputOutcome, WindowInputReceiptParams,
};
use tokio::sync::oneshot;

use super::{
    AuthorizedIntegration, HostedTerminal, MAX_TERMINAL_WRITE_BYTES, MutationKey, MutationOutcome,
    RootCheck, RootLease, RuntimeErrorKind, TerminalRuntimeAdapter, TerminalRuntimeFailure,
    TerminalView, WallMs, current_roots, ensure_mutation_capacity, fingerprint,
    mutation_in_progress, mutation_key, prior_from_state, private_terminal_id,
    remember_pending_done, root_authority_failure, root_proof, run_root_check,
    validate_input_grant, validate_lease_fields, validate_mutation_time, visible_in,
    visible_terminal, visible_terminal_in_view,
};
use crate::window_registry::input::InputTarget;

/// One response bound, shared with the existing terminal relay's stalled-peer budget.
pub(crate) const DELIVERY_DEADLINE: Duration = crate::runtime_serve::VIEW_WRITE_DEADLINE;

/// Transient payload ownership moves from the caller into one bounded owner receiver, then out on one claim.
/// The mutation ledger retains only the fingerprint and structural result.
pub(crate) struct OwnerOffer {
    pub(crate) sequence: u64,
    pub(crate) binding: WindowInputBinding,
    pub(crate) deadline: Instant,
    text: Option<String>,
    shell: ProcessIdentity,
    hosted: HostedTerminal,
    authority: AuthorizedIntegration,
    owner_authority: AuthorizedIntegration,
    root: RootLease,
    owner_root: RootLease,
    lease_id: String,
    lease_generation: u64,
    key: MutationKey,
    fingerprint: [u8; 32],
    receipt: TerminalTextReceipt,
    completion: oneshot::Sender<Result<TerminalTextReceipt, TerminalRuntimeFailure>>,
}

impl OwnerOffer {
    pub(crate) fn expired(&self) -> bool {
        self.completion.is_closed() || Instant::now() >= self.deadline
    }

    pub(crate) async fn claim(
        &mut self,
        composed: &Composed,
        subscription_id: &str,
    ) -> Result<WindowInputClaim, TerminalRuntimeFailure> {
        if self.expired() || self.text.is_none() {
            return Err(mutation_in_progress());
        }
        let shell = self.shell;
        let proof = run_root_check(composed.terminal_root_checks.clone(), move || {
            runtrol_childproc::process_identity(shell.pid()) == Some(shell)
        })
        .await
        .and_then(RootCheck::fresh)
        .map_err(|_| root_authority_failure())?;
        if !proof.value {
            return Err(TerminalRuntimeFailure::invalid(
                "the exact owner execution ended",
            ));
        }
        self.root
            .proof
            .ensure_fresh_until(Instant::now() + root_proof::ROOT_CHECK_DEADLINE)
            .await
            .map_err(|_| root_authority_failure())?;
        self.owner_root
            .proof
            .ensure_fresh_until(Instant::now() + root_proof::ROOT_CHECK_DEADLINE)
            .await
            .map_err(|_| root_authority_failure())?;
        let mut state = composed.runtime_terminals.state.lock().await;
        if !composed
            .windows
            .matches_input(&self.binding, self.shell, subscription_id)
            .await
        {
            return Err(TerminalRuntimeFailure::invalid(
                "the exact owner receiver or execution changed",
            ));
        }
        validate_input_grant(composed, &self.authority)?;
        validate_input_grant(composed, &self.owner_authority)?;
        self.root
            .proof
            .fresh()
            .map_err(|_| root_authority_failure())?;
        self.owner_root
            .proof
            .fresh()
            .map_err(|_| root_authority_failure())?;
        proof.fresh().map_err(|_| root_authority_failure())?;
        validate_lease_fields(
            &mut state,
            self.hosted.id,
            self.authority.key,
            &self.lease_id,
            self.lease_generation,
            WallMs::now().as_millis(),
        )?;
        if self.expired() || self.hosted.terminal.exit().is_some() {
            return Err(mutation_in_progress());
        }
        let text = self.text.take().ok_or_else(mutation_in_progress)?;
        Ok(WindowInputClaim { text })
    }

    pub(crate) async fn finish(self, composed: &Composed, params: &WindowInputReceiptParams) {
        let accepted = Instant::now() < self.deadline
            && self.text.is_none()
            && params.outcome == WindowInputOutcome::OwnerExtensionAccepted;
        let result = if accepted {
            let mut state = composed.runtime_terminals.state.lock().await;
            match state.mutations.get_mut(&self.key) {
                Some(stored) if stored.fingerprint == self.fingerprint => {
                    stored.outcome = MutationOutcome::Text(self.receipt.clone());
                    stored.recorded_at_ms = WallMs::now().as_millis();
                    Ok(self.receipt)
                }
                _ => Err(mutation_in_progress()),
            }
        } else {
            Err(mutation_in_progress())
        };
        // ok: A disconnected caller retains the structural mutation result but receives no replay.
        let _delivered = self.completion.send(result);
    }
}

impl TerminalRuntimeAdapter {
    async fn pin_input_owner(
        &self,
        composed: &Composed,
        authority: &AuthorizedIntegration,
        hosted: &HostedTerminal,
    ) -> Result<(RootLease, RootLease, InputTarget), TerminalRuntimeFailure> {
        let root = self
            .roots
            .pin(authority, hosted, composed.terminal_root_checks.clone())
            .await?;
        let target = composed.windows.input_target(hosted).await.ok_or_else(|| {
            TerminalRuntimeFailure::invalid("the observed terminal has no exact live input owner")
        })?;
        let owner_root = self
            .roots
            .pin(
                &target.authority,
                hosted,
                composed.terminal_root_checks.clone(),
            )
            .await?;
        Ok((root, owner_root, target))
    }

    /// Text is a distinct owner-extension operation. A mirror never pretends it owns a PTY writer.
    pub(crate) async fn send_text(
        &self,
        composed: &Composed,
        authority: &AuthorizedIntegration,
        params: TerminalSendTextParams,
        view: Option<&TerminalView>,
    ) -> Result<TerminalTextReceipt, TerminalRuntimeFailure> {
        validate_mutation_time(&params.request_id)?;
        if params.text.len() > MAX_TERMINAL_WRITE_BYTES {
            return Err(TerminalRuntimeFailure::new(
                RuntimeErrorKind::ResourceExhausted,
                "owner input text exceeds the public UTF-8 byte bound",
            ));
        }
        let key = mutation_key(authority.key, &params.request_id)?;
        let fingerprint = fingerprint(&params)?;
        if let Some(receipt) = prior_text(self.prior(&key, fingerprint).await?)? {
            return Ok(receipt);
        }
        let terminal_id = private_terminal_id(&params.terminal_id)?;
        let hosted = match view {
            Some(view) => visible_terminal_in_view(composed, view, terminal_id).await?,
            None => visible_terminal(composed, authority, terminal_id).await?,
        };
        let _operation = self
            .input_operation(
                composed,
                authority,
                &hosted,
                &params.lease_id,
                params.lease_generation,
            )
            .await?;
        let (root, owner_root, target) = self.pin_input_owner(composed, authority, &hosted).await?;
        let (completion, receipt) = oneshot::channel();
        let mut state = self.state.lock().await;
        validate_input_grant(composed, authority)?;
        validate_input_grant(composed, &target.authority)?;
        for pinned in [&root, &owner_root] {
            pinned.proof.fresh().map_err(|_| root_authority_failure())?;
        }
        if let Some(view) = view {
            view.require_fresh_root_proof()?;
        }
        let now = WallMs::now().as_millis();
        if let Some(receipt) = prior_text(prior_from_state(&mut state, &key, fingerprint, now)?)? {
            return Ok(receipt);
        }
        validate_lease_fields(
            &mut state,
            hosted.id,
            authority.key,
            &params.lease_id,
            params.lease_generation,
            now,
        )?;
        ensure_mutation_capacity(&state)?;
        state.next_text_sequence = state
            .next_text_sequence
            .checked_add(1)
            .ok_or_else(|| TerminalRuntimeFailure::unavailable("owner input sequence exhausted"))?;
        let sequence = state.next_text_sequence;
        let delivered = TerminalTextReceipt {
            request_id: params.request_id,
            delivery_sequence: sequence,
            owner_registration_generation: target.binding.registration_generation,
            outcome: TerminalTextOutcome::OwnerExtensionAccepted,
        };
        let deadline = Instant::now() + DELIVERY_DEADLINE;
        let InputTarget {
            binding,
            shell,
            authority: owner_authority,
            sender,
        } = target;
        let offer = OwnerOffer {
            sequence,
            binding,
            shell,
            deadline,
            text: Some(params.text),
            hosted: hosted.clone(),
            authority: authority.clone(),
            owner_authority,
            root,
            owner_root,
            lease_id: params.lease_id,
            lease_generation: params.lease_generation,
            key: key.clone(),
            fingerprint,
            receipt: delivered,
            completion,
        };
        // Reservation and bounded enqueue occur without another await. Every later failure is unknown,
        // including an owner's explicit refusal: callers must never silently mint a new mutation identity.
        remember_pending_done(&mut state, key, fingerprint, now);
        let queued = sender.try_send(offer);
        drop(sender);
        drop(state);
        if queued.is_err() {
            return Err(mutation_in_progress());
        }
        tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), receipt)
            .await
            .map_err(|_| mutation_in_progress())?
            .map_err(|_| mutation_in_progress())?
    }
}

fn prior_text(
    outcome: Option<MutationOutcome>,
) -> Result<Option<TerminalTextReceipt>, TerminalRuntimeFailure> {
    match outcome {
        None => Ok(None),
        Some(MutationOutcome::Text(receipt)) => Ok(Some(receipt)),
        Some(MutationOutcome::PendingDone) => Err(mutation_in_progress()),
        Some(_) => Err(TerminalRuntimeFailure::new(
            RuntimeErrorKind::IdempotencyConflict,
            "the mutation identity belongs to another terminal operation",
        )),
    }
}

/// Availability is only a structural hint. The claim repeats the exact checks before disclosing any text.
pub(crate) async fn input_available(
    composed: &Composed,
    authority: &AuthorizedIntegration,
    hosted: &HostedTerminal,
) -> bool {
    if validate_input_grant(composed, authority).is_err() {
        return false;
    }
    let Some(target) = composed.windows.input_target(hosted).await else {
        return false;
    };
    if validate_input_grant(composed, &target.authority).is_err() {
        return false;
    }
    let hosted = hosted.clone();
    run_root_check(composed.terminal_root_checks.clone(), move || {
        runtrol_childproc::process_identity(target.shell.pid()) == Some(target.shell)
            && current_roots(&target.authority).is_ok_and(|roots| visible_in(&hosted, &roots))
    })
    .await
    .and_then(RootCheck::fresh)
    .is_ok_and(|checked| checked.value)
}

#[cfg(test)]
mod tests;
