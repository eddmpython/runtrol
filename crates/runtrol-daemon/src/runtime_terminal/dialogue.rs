//! Local input authority controls the courier lifetime; no prompt content enters this operation.

use runtrol_runtime_protocol::TerminalSetDialogueParams;

use super::*;

impl TerminalRuntimeAdapter {
    pub(crate) async fn set_dialogue(
        &self,
        composed: &Composed,
        authority: &AuthorizedIntegration,
        params: &TerminalSetDialogueParams,
    ) -> Result<(), TerminalRuntimeFailure> {
        validate_mutation_time(&params.request_id)?;
        let terminal_id = private_terminal_id(&params.terminal_id)?;
        let hosted = visible_terminal(composed, authority, terminal_id).await?;
        self.set_dialogue_hosted(composed, authority, params, &hosted)
            .await
    }

    async fn set_dialogue_hosted(
        &self,
        composed: &Composed,
        authority: &AuthorizedIntegration,
        params: &TerminalSetDialogueParams,
        hosted: &HostedTerminal,
    ) -> Result<(), TerminalRuntimeFailure> {
        if matches!(
            hosted.origin,
            crate::terminal_surface::TerminalOrigin::ObservedMirror(_)
        ) {
            return Err(TerminalRuntimeFailure::invalid(
                "an observed terminal has no Runtime-owned courier process",
            ));
        }
        let terminal_id = hosted.id;
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
        let changes = composed.terminals.lock().await.change_sender();
        let mut state = self.state.lock().await;
        let mut now = WallMs::now().as_millis();
        if prior_done_from_state(&mut state, &key, fingerprint, now)? {
            return Ok(());
        }
        ensure_mutation_capacity(&state)?;
        // Recheck after the final asynchronous lock. Keep the lease and outcome locks through the
        // in-memory change, so neither a queued request nor cancellation can cross this operation.
        composed
            .courier_gate
            .set_dialogue_checked(
                terminal_id,
                params.enabled,
                Some(Arc::new(authority.clone())),
                || {
                    let current = crate::runtime_serve::refresh_current(composed, authority)
                        .map_err(|failure| {
                            TerminalRuntimeFailure::new(failure.kind, failure.message)
                        })?;
                    if !has_scopes(&current.grant, &[AppScope::SessionInputWrite]) {
                        return Err(TerminalRuntimeFailure::new(
                            RuntimeErrorKind::ScopeDenied,
                            "the integration grant lacks the required app scope",
                        ));
                    }
                    ensure_visible(hosted, &current)?;
                    validate_mutation_time(&params.request_id)?;
                    now = WallMs::now().as_millis();
                    validate_lease_fields(
                        &mut state,
                        terminal_id,
                        authority.key,
                        &params.lease_id,
                        params.lease_generation,
                        now,
                    )
                },
            )
            .await
            .map_err(|failure| match failure {
                crate::courier_gate::DialogueFailure::Control(failure) => failure,
                crate::courier_gate::DialogueFailure::Session(message) => {
                    TerminalRuntimeFailure::new(RuntimeErrorKind::TerminalGone, message)
                }
            })?;
        remember_done(&mut state, key, fingerprint, now);
        changes.send_modify(|revision| *revision = revision.wrapping_add(1));
        Ok(())
    }
}

#[cfg(test)]
#[path = "tests/dialogue.rs"]
mod tests;
