//! Content-free provider-process and terminal-surface admission projected across daemon generations.
//!
//! A claim contains only provider identity, provider-native identity when it is already known, canonical
//! workspace, surface kind, and an opaque owner identity. Conversation bytes and provider output never enter it.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::RwLock;

use runtrol_core::SessionManager;
use runtrol_ipc::{GenerationLiveClaimLine, GenerationLiveClaimSurface};
use runtrol_provider::{SessionId, TerminalId};

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum TerminalClaimError {
    #[error("the provider-native conversation is already live as a structured session")]
    StructuredBusy,
    #[error("the provider-native terminal is already live in another Runtime generation")]
    TerminalAlreadyLive,
    #[error("the provider-native conversation is already live in another workspace")]
    WorkspaceConflict,
    #[error("the native live-claim registry is unavailable")]
    State,
    #[error("a draining legacy Runtime generation cannot export native live claims")]
    LegacyGenerationBusy,
}

pub(crate) enum TerminalClaimAdmission<'registry> {
    Join(TerminalId),
    Reserved(TerminalClaimGuard<'registry>),
}

pub(crate) struct TerminalClaimGuard<'registry> {
    registry: &'registry NativeLiveClaimRegistry,
    terminal: TerminalId,
    committed: bool,
}

pub(crate) struct StructuredClaimGuard<'registry> {
    registry: &'registry NativeLiveClaimRegistry,
    session: SessionId,
    inserted: bool,
    committed: bool,
}

impl StructuredClaimGuard<'_> {
    pub(crate) fn commit(mut self) {
        self.committed = true;
    }
}

impl Drop for StructuredClaimGuard<'_> {
    fn drop(&mut self) {
        if self.committed || !self.inserted {
            return;
        }
        if let Ok(mut state) = self.registry.state.write() {
            state.structured.remove(&self.session);
        }
    }
}

impl TerminalClaimGuard<'_> {
    pub(crate) fn commit(mut self) -> Result<(), TerminalClaimError> {
        let mut state = self
            .registry
            .state
            .write()
            .map_err(|_| TerminalClaimError::State)?;
        let claim = state
            .pending_terminals
            .remove(&self.terminal)
            .ok_or(TerminalClaimError::State)?;
        state.terminals.insert(self.terminal, claim);
        self.committed = true;
        Ok(())
    }
}

impl Drop for TerminalClaimGuard<'_> {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        if let Ok(mut state) = self.registry.state.write() {
            state.pending_terminals.remove(&self.terminal);
        }
    }
}

/// Complete in-memory live claim view owned by the current generation.
pub(crate) struct NativeLiveClaimRegistry {
    state: RwLock<ClaimState>,
}

#[derive(Default)]
struct ClaimState {
    structured: BTreeMap<SessionId, GenerationLiveClaimLine>,
    pending_terminals: BTreeMap<TerminalId, GenerationLiveClaimLine>,
    terminals: BTreeMap<TerminalId, GenerationLiveClaimLine>,
    remote: BTreeMap<Box<str>, Vec<GenerationLiveClaimLine>>,
    legacy_generations: BTreeSet<Box<str>>,
}

impl Default for NativeLiveClaimRegistry {
    fn default() -> Self {
        Self {
            state: RwLock::new(ClaimState::default()),
        }
    }
}

impl NativeLiveClaimRegistry {
    /// Atomically reserve a structured launch against every local and inherited process claim.
    pub(crate) fn reserve_structured(
        &self,
        session: SessionId,
        provider_id: &str,
        native_session_id: Option<&str>,
        workspace: &str,
    ) -> Result<StructuredClaimGuard<'_>, TerminalClaimError> {
        let mut state = self.state.write().map_err(|_| TerminalClaimError::State)?;
        if native_session_id.is_some() && !state.legacy_generations.is_empty() {
            return Err(TerminalClaimError::LegacyGenerationBusy);
        }
        if let Some(existing) = state.structured.get(&session)
            && same_owner_claim(provider_id, native_session_id, workspace, existing)
        {
            return Ok(StructuredClaimGuard {
                registry: self,
                session,
                inserted: false,
                committed: false,
            });
        }
        for claim in state
            .structured
            .values()
            .chain(state.pending_terminals.values())
            .chain(state.terminals.values())
            .chain(state.remote.values().flatten())
        {
            if let Some(error) =
                claim_conflict(provider_id, native_session_id, workspace, claim, false)
            {
                return Err(error);
            }
        }
        state.structured.insert(
            session,
            GenerationLiveClaimLine {
                provider_id: provider_id.into(),
                native_session_id: native_session_id.map(Into::into),
                workspace: workspace.into(),
                surface: GenerationLiveClaimSurface::Structured,
                owner_id: session.to_string().into_boxed_str(),
            },
        );
        drop(state);
        Ok(StructuredClaimGuard {
            registry: self,
            session,
            inserted: true,
            committed: false,
        })
    }

    /// Atomically reserve a terminal launch against every local and inherited process claim.
    /// Take a terminal claim for one conversation, or say why it cannot be taken.
    ///
    /// `holder_known` says the caller has already asked the coding service which of its conversations a live
    /// process holds, and this one is not among them. It matters because of the rule below: a terminal whose
    /// conversation nobody has named yet blocks every conversation in its folder, since it might be any of
    /// them. That guard is right when nothing can tell them apart and wrong the moment something can. A
    /// generation that cannot name its own terminals (a build from before it could) would otherwise hold a
    /// whole project hostage for as long as it drains, which is what it did: on 2026-08-30 an operator's
    /// stored conversation in a project with one such terminal opened into an error instead of a session.
    pub(crate) fn reserve_terminal(
        &self,
        terminal: TerminalId,
        provider_id: &str,
        native_session_id: Option<&str>,
        workspace: &str,
        holder_known: bool,
    ) -> Result<TerminalClaimAdmission<'_>, TerminalClaimError> {
        let mut state = self.state.write().map_err(|_| TerminalClaimError::State)?;
        if native_session_id.is_some() && !state.legacy_generations.is_empty() {
            return Err(TerminalClaimError::LegacyGenerationBusy);
        }
        if let Some(native) = native_session_id {
            for (id, claim) in &state.terminals {
                if exact_terminal_join(provider_id, native, workspace, claim) {
                    return Ok(TerminalClaimAdmission::Join(*id));
                }
            }
        }
        for claim in state
            .structured
            .values()
            .chain(state.pending_terminals.values())
            .chain(state.terminals.values())
            .chain(state.remote.values().flatten())
        {
            if let Some(error) = claim_conflict(
                provider_id,
                native_session_id,
                workspace,
                claim,
                holder_known,
            ) {
                return Err(error);
            }
        }
        state.pending_terminals.insert(
            terminal,
            GenerationLiveClaimLine {
                provider_id: provider_id.into(),
                native_session_id: native_session_id.map(Into::into),
                workspace: workspace.into(),
                surface: GenerationLiveClaimSurface::Terminal,
                owner_id: terminal.to_string().into_boxed_str(),
            },
        );
        drop(state);
        Ok(TerminalClaimAdmission::Reserved(TerminalClaimGuard {
            registry: self,
            terminal,
            committed: false,
        }))
    }

    /// Replace the structured-process projection after the single session owner changes.
    pub(crate) fn replace_structured(&self, sessions: &SessionManager) {
        let active: BTreeSet<SessionId> = sessions.process_owner_ids().collect();
        let live: Vec<(SessionId, GenerationLiveClaimLine)> = sessions
            .live_sessions()
            .map(|session| {
                (
                    session.session,
                    GenerationLiveClaimLine {
                        provider_id: session.provider.to_string().into_boxed_str(),
                        native_session_id: session.native.map(Into::into),
                        workspace: session.workspace.as_str().into(),
                        surface: GenerationLiveClaimSurface::Structured,
                        owner_id: session.session.to_string().into_boxed_str(),
                    },
                )
            })
            .collect();
        if let Ok(mut state) = self.state.write() {
            state
                .structured
                .retain(|session, _| active.contains(session));
            state.structured.extend(live);
        }
    }

    /// Retire the exact terminal process claim when its exit observer removes the PTY.
    pub(crate) fn terminal_ended(&self, id: TerminalId) {
        if let Ok(mut state) = self.state.write() {
            state.pending_terminals.remove(&id);
            state.terminals.remove(&id);
        }
    }

    /// Bind the provider identity minted after an unnamed terminal process started.
    ///
    /// The same conflict scan as a native resume runs before the local claim changes, so a late provider roster
    /// observation cannot create a second owner for a conversation already claimed elsewhere.
    #[cfg(test)]
    pub(crate) fn bind_terminal_native(
        &self,
        terminal: TerminalId,
        provider_id: &str,
        native_session_id: &str,
        workspace: &str,
    ) -> Result<bool, TerminalClaimError> {
        self.bind_terminal_natives(&[(terminal, provider_id, native_session_id, workspace)])
            .map(|mut changed| changed.pop().unwrap_or(false))
    }

    /// Atomically bind one provider observation containing several previously unnamed terminals.
    ///
    /// Two fresh conversations may start in separate terminals of the same workspace before either provider-native
    /// identity exists. Binding them one at a time makes the still-unnamed sibling look like a possible duplicate of
    /// the first. A complete structural observation resolves both at once, so the batch excludes only its own exact
    /// terminal claims while checking every structured, remote, pending, and unrelated terminal owner.
    pub(crate) fn bind_terminal_natives(
        &self,
        bindings: &[(TerminalId, &str, &str, &str)],
    ) -> Result<Vec<bool>, TerminalClaimError> {
        let mut state = self.state.write().map_err(|_| TerminalClaimError::State)?;
        let terminal_ids = bindings
            .iter()
            .map(|(terminal, _, _, _)| *terminal)
            .collect::<BTreeSet<_>>();
        if terminal_ids.len() != bindings.len() {
            return Err(TerminalClaimError::State);
        }
        for (terminal, provider_id, _native_session_id, workspace) in bindings {
            let existing = state
                .terminals
                .get(terminal)
                .ok_or(TerminalClaimError::State)?;
            if existing.provider_id.as_ref() != *provider_id
                || existing.workspace.as_ref() != *workspace
            {
                return Err(TerminalClaimError::State);
            }
        }
        // A terminal already bound to one conversation may move to another: the CLI's own `/resume` and
        // `/clear` change the conversation a running process is in, and its roster then names the new one
        // for the same pid. Refusing that (measured 2026-08-29 on the operator's machine) left the claim on
        // the old conversation, so the row for the new one had no terminal to join and a click opened a
        // second process on the same conversation; every roster round after that answered with an error.
        // The new identity is checked against every other claim exactly as a first binding is.
        for (_terminal, provider_id, native_session_id, workspace) in bindings {
            for claim in state
                .structured
                .values()
                .chain(state.pending_terminals.values())
                .chain(
                    state
                        .terminals
                        .iter()
                        .filter(|(id, _)| !terminal_ids.contains(id))
                        .map(|(_, claim)| claim),
                )
                .chain(state.remote.values().flatten())
            {
                if let Some(error) = claim_conflict(
                    provider_id,
                    Some(native_session_id),
                    workspace,
                    claim,
                    false,
                ) {
                    return Err(error);
                }
            }
        }
        for (index, (_terminal, provider_id, native_session_id, workspace)) in
            bindings.iter().enumerate()
        {
            for (_other_terminal, other_provider, other_native, other_workspace) in
                bindings.iter().skip(index + 1)
            {
                if provider_id != other_provider || native_session_id != other_native {
                    continue;
                }
                return Err(if workspace == other_workspace {
                    TerminalClaimError::TerminalAlreadyLive
                } else {
                    TerminalClaimError::WorkspaceConflict
                });
            }
        }
        let mut changed = Vec::with_capacity(bindings.len());
        for (terminal, _provider_id, native_session_id, _workspace) in bindings {
            let claim = state
                .terminals
                .get_mut(terminal)
                .ok_or(TerminalClaimError::State)?;
            let is_changed = claim.native_session_id.as_deref() != Some(*native_session_id);
            if is_changed {
                claim.native_session_id = Some((*native_session_id).into());
            }
            changed.push(is_changed);
        }
        Ok(changed)
    }

    /// Whether a live process claim makes one provider-native mutation unsafe.
    ///
    /// An exact native identity blocks itself. A same-provider claim whose identity has not been minted yet blocks
    /// mutations in its workspace conservatively, because deleting underneath that original process would violate
    /// the non-destructive ownership boundary before the provider roster can bind it.
    pub(crate) fn blocks_native_mutation(
        &self,
        provider_id: &str,
        native_session_id: &str,
        workspace: &str,
    ) -> Result<bool, TerminalClaimError> {
        let state = self.state.read().map_err(|_| TerminalClaimError::State)?;
        Ok(state
            .structured
            .values()
            .chain(state.pending_terminals.values())
            .chain(state.terminals.values())
            .chain(state.remote.values().flatten())
            .any(|claim| {
                claim.provider_id.as_ref() == provider_id
                    && (claim.native_session_id.as_deref() == Some(native_session_id)
                        || (claim.native_session_id.is_none()
                            && claim.workspace.as_ref() == workspace))
            }))
    }

    /// Replace one peer generation's complete inherited claim set.
    pub(crate) fn replace_remote(&self, generation: &str, claims: Vec<GenerationLiveClaimLine>) {
        if let Ok(mut state) = self.state.write() {
            state.remote.insert(generation.into(), claims);
        }
    }

    /// Remove peer generations no longer present in the locator.
    pub(crate) fn retain_remote<'a>(&self, live: impl Iterator<Item = &'a str>) {
        let live: std::collections::BTreeSet<&str> = live.collect();
        if let Ok(mut state) = self.state.write() {
            state
                .remote
                .retain(|generation, _| live.contains(generation.as_ref()));
        }
    }

    /// Fail exact native resume closed while a live peer cannot export its process claims.
    pub(crate) fn replace_legacy_generations<'a>(
        &self,
        generations: impl Iterator<Item = &'a str>,
    ) {
        let next = generations.map(Into::into).collect();
        if let Ok(mut state) = self.state.write() {
            state.legacy_generations = next;
        }
    }

    /// Export local and inherited claims, excluding the peer that is receiving the response.
    pub(crate) fn snapshot_except(
        &self,
        excluded_generation: Option<&str>,
    ) -> Vec<GenerationLiveClaimLine> {
        let Ok(state) = self.state.read() else {
            return Vec::new();
        };
        state
            .structured
            .values()
            .chain(state.pending_terminals.values())
            .chain(state.terminals.values())
            .chain(
                state
                    .remote
                    .iter()
                    .filter(|(generation, _)| excluded_generation != Some(generation.as_ref()))
                    .flat_map(|(_, claims)| claims),
            )
            .cloned()
            .collect()
    }
}

fn same_owner_claim(
    provider_id: &str,
    native_session_id: Option<&str>,
    workspace: &str,
    claim: &GenerationLiveClaimLine,
) -> bool {
    claim.surface == GenerationLiveClaimSurface::Structured
        && claim.provider_id.as_ref() == provider_id
        && claim.native_session_id.as_deref() == native_session_id
        && claim.workspace.as_ref() == workspace
}

fn exact_terminal_join(
    provider_id: &str,
    native_session_id: &str,
    workspace: &str,
    claim: &GenerationLiveClaimLine,
) -> bool {
    claim.surface == GenerationLiveClaimSurface::Terminal
        && claim.provider_id.as_ref() == provider_id
        && claim.native_session_id.as_deref() == Some(native_session_id)
        && claim.workspace.as_ref() == workspace
}

fn claim_conflict(
    provider_id: &str,
    native_session_id: Option<&str>,
    workspace: &str,
    claim: &GenerationLiveClaimLine,
    holder_known: bool,
) -> Option<TerminalClaimError> {
    if claim.provider_id.as_ref() != provider_id {
        return None;
    }
    let exact_native =
        native_session_id.is_some() && claim.native_session_id.as_deref() == native_session_id;
    if exact_native && claim.workspace.as_ref() != workspace {
        return Some(TerminalClaimError::WorkspaceConflict);
    }
    // A claim whose conversation nobody has named could be this one, so it blocks. Unless the service itself
    // has been asked and did not name this conversation among the ones its live processes hold: then the
    // unnamed terminal is provably some other conversation, and refusing this one protects nothing.
    let unresolved_collision = !holder_known
        && native_session_id.is_some()
        && claim.native_session_id.is_none()
        && claim.workspace.as_ref() == workspace;
    let collision = exact_native || unresolved_collision;
    if !collision {
        return None;
    }
    Some(match claim.surface {
        GenerationLiveClaimSurface::Structured => TerminalClaimError::StructuredBusy,
        GenerationLiveClaimSurface::Terminal => TerminalClaimError::TerminalAlreadyLive,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A terminal another generation cannot name blocks every conversation in its folder, because it might be
    /// any of them. It stops blocking the moment the coding service itself has been asked and did not name the
    /// conversation being opened: then the unnamed terminal is provably a different one.
    ///
    /// Measured 2026-08-30 on the operator's machine: a generation from a build that could not name its own
    /// terminals held one in a project, and every stored conversation in that project opened into an error
    /// instead of a session for as long as that generation drained.
    #[test]
    fn an_unnamed_terminal_elsewhere_stops_blocking_once_the_service_has_answered() {
        let registry = NativeLiveClaimRegistry::default();
        registry.replace_remote(
            "draining",
            vec![GenerationLiveClaimLine {
                provider_id: "example".into(),
                // The other generation has a terminal here and cannot say which conversation is in it.
                native_session_id: None,
                workspace: "/work".into(),
                surface: GenerationLiveClaimSurface::Terminal,
                owner_id: "terminal".into(),
            }],
        );
        // Nobody asked the service: the guard holds, because the unnamed terminal could be this conversation.
        assert!(matches!(
            registry.reserve_terminal(TerminalId::now(), "example", Some("stored"), "/work", false),
            Err(TerminalClaimError::TerminalAlreadyLive)
        ));
        // The service answered and did not name this conversation: it is free, and refusing it protects
        // nothing.
        assert!(matches!(
            registry.reserve_terminal(TerminalId::now(), "example", Some("stored"), "/work", true),
            Ok(TerminalClaimAdmission::Reserved(_))
        ));
    }

    /// Having asked the service never overrides a claim that names this exact conversation.
    #[test]
    fn a_named_claim_refuses_however_well_the_service_answered() {
        let registry = NativeLiveClaimRegistry::default();
        registry.replace_remote(
            "draining",
            vec![GenerationLiveClaimLine {
                provider_id: "example".into(),
                native_session_id: Some("stored".into()),
                workspace: "/work".into(),
                surface: GenerationLiveClaimSurface::Terminal,
                owner_id: "terminal".into(),
            }],
        );
        assert!(matches!(
            registry.reserve_terminal(TerminalId::now(), "example", Some("stored"), "/work", true),
            Err(TerminalClaimError::TerminalAlreadyLive)
        ));
    }

    #[test]
    fn a_peer_does_not_receive_its_own_claims_back() {
        let registry = NativeLiveClaimRegistry::default();
        registry.replace_remote(
            "new",
            vec![GenerationLiveClaimLine {
                provider_id: "example".into(),
                native_session_id: Some("native".into()),
                workspace: "/work".into(),
                surface: GenerationLiveClaimSurface::Terminal,
                owner_id: "terminal".into(),
            }],
        );
        assert!(registry.snapshot_except(Some("new")).is_empty());
        assert_eq!(registry.snapshot_except(None).len(), 1);
    }

    #[test]
    fn a_pending_terminal_claim_is_exported_until_its_reservation_ends() {
        let registry = NativeLiveClaimRegistry::default();
        let admission = registry
            .reserve_terminal(TerminalId::now(), "example", Some("native"), "/work", false)
            .expect("reserve the terminal claim");
        assert!(matches!(&admission, TerminalClaimAdmission::Reserved(_)));
        assert_eq!(
            registry.snapshot_except(None).len(),
            1,
            "generation handoff must see a launch before its process finishes starting"
        );
        drop(admission);
        assert!(registry.snapshot_except(None).is_empty());
    }

    #[test]
    fn an_exact_native_claim_cannot_cross_surfaces_or_workspaces() {
        let registry = NativeLiveClaimRegistry::default();
        registry.replace_remote(
            "old",
            vec![GenerationLiveClaimLine {
                provider_id: "example".into(),
                native_session_id: Some("native".into()),
                workspace: "/first".into(),
                surface: GenerationLiveClaimSurface::Structured,
                owner_id: "session".into(),
            }],
        );
        assert!(matches!(
            registry.reserve_terminal(
                TerminalId::now(),
                "example",
                Some("native"),
                "/first",
                false
            ),
            Err(TerminalClaimError::StructuredBusy)
        ));
        assert!(matches!(
            registry.reserve_terminal(
                TerminalId::now(),
                "example",
                Some("native"),
                "/second",
                false
            ),
            Err(TerminalClaimError::WorkspaceConflict)
        ));
    }

    #[test]
    fn an_unresolved_launch_blocks_a_native_resume_in_the_same_workspace() {
        let registry = NativeLiveClaimRegistry::default();
        let first =
            match registry.reserve_terminal(TerminalId::now(), "example", None, "/work", false) {
                Ok(admission) => admission,
                Err(error) => panic!("the fresh terminal did not reserve its claim: {error}"),
            };
        assert!(matches!(&first, TerminalClaimAdmission::Reserved(_)));
        assert!(matches!(
            registry.reserve_terminal(TerminalId::now(), "example", Some("native"), "/work", false),
            Err(TerminalClaimError::TerminalAlreadyLive)
        ));
    }

    #[test]
    fn unresolved_fresh_work_does_not_block_other_fresh_work() {
        let registry = NativeLiveClaimRegistry::default();
        let terminal = registry
            .reserve_terminal(TerminalId::now(), "example", None, "/work", false)
            .expect("fresh terminal claim");
        let structured = registry
            .reserve_structured(SessionId::now(), "example", None, "/work")
            .expect("fresh structured claim");
        assert!(matches!(terminal, TerminalClaimAdmission::Reserved(_)));
        structured.commit();
    }

    #[test]
    fn a_provider_roster_can_bind_an_unresolved_terminal_once() {
        let registry = NativeLiveClaimRegistry::default();
        let terminal_id = TerminalId::now();
        let TerminalClaimAdmission::Reserved(terminal) = registry
            .reserve_terminal(terminal_id, "example", None, "/work", false)
            .expect("fresh terminal claim")
        else {
            panic!("fresh terminal unexpectedly joined");
        };
        terminal.commit().expect("committed terminal claim");

        assert!(
            registry
                .bind_terminal_native(terminal_id, "example", "native", "/work")
                .expect("provider identity binds")
        );
        assert!(
            !registry
                .bind_terminal_native(terminal_id, "example", "native", "/work")
                .expect("the same binding is idempotent")
        );
        let claims = registry.snapshot_except(None);
        assert_eq!(
            claims
                .first()
                .and_then(|claim| claim.native_session_id.as_deref()),
            Some("native")
        );
    }

    #[test]
    fn one_provider_observation_atomically_names_two_fresh_sibling_terminals() {
        let registry = NativeLiveClaimRegistry::default();
        let first = TerminalId::now();
        let second = TerminalId::now();
        for terminal_id in [first, second] {
            let TerminalClaimAdmission::Reserved(terminal) = registry
                .reserve_terminal(terminal_id, "example", None, "/work", false)
                .expect("fresh terminal claim")
            else {
                panic!("fresh terminal unexpectedly joined");
            };
            terminal.commit().expect("committed terminal claim");
        }

        assert_eq!(
            registry
                .bind_terminal_natives(&[
                    (first, "example", "native-first", "/work"),
                    (second, "example", "native-second", "/work"),
                ])
                .expect("the complete provider observation binds atomically"),
            vec![true, true]
        );
        let claims = registry.snapshot_except(None);
        assert_eq!(
            claims
                .iter()
                .filter_map(|claim| claim.native_session_id.as_deref())
                .collect::<BTreeSet<_>>(),
            BTreeSet::from(["native-first", "native-second"])
        );
    }

    #[test]
    fn a_process_that_moved_to_another_conversation_rebinds_its_terminal() {
        let registry = NativeLiveClaimRegistry::default();
        let terminal_id = TerminalId::now();
        let TerminalClaimAdmission::Reserved(terminal) = registry
            .reserve_terminal(terminal_id, "example", None, "/work", false)
            .expect("fresh terminal claim")
        else {
            panic!("fresh terminal unexpectedly joined");
        };
        terminal.commit().expect("committed terminal claim");
        assert!(
            registry
                .bind_terminal_native(terminal_id, "example", "first", "/work")
                .expect("the first identity binds")
        );
        // The CLI resumed another conversation inside the same process.
        assert!(
            registry
                .bind_terminal_native(terminal_id, "example", "second", "/work")
                .expect("the moved identity rebinds")
        );
        // A resume of the new conversation joins this terminal instead of opening a second process, and the
        // old identity no longer joins anything.
        assert!(matches!(
            registry.reserve_terminal(TerminalId::now(), "example", Some("second"), "/work", false),
            Ok(TerminalClaimAdmission::Join(id)) if id == terminal_id
        ));
        assert!(matches!(
            registry.reserve_terminal(TerminalId::now(), "example", Some("first"), "/work", false),
            Ok(TerminalClaimAdmission::Reserved(_))
        ));
        // Another live claim on the target still refuses, exactly as a first binding would.
        registry
            .reserve_structured(SessionId::now(), "example", Some("third"), "/work")
            .expect("structured claim")
            .commit();
        assert!(matches!(
            registry.bind_terminal_native(terminal_id, "example", "third", "/work"),
            Err(TerminalClaimError::StructuredBusy)
        ));
    }

    #[test]
    fn a_late_identity_binding_cannot_take_an_existing_native_claim() {
        let registry = NativeLiveClaimRegistry::default();
        registry
            .reserve_structured(SessionId::now(), "example", Some("native"), "/work")
            .expect("structured claim")
            .commit();
        let terminal_id = TerminalId::now();
        let TerminalClaimAdmission::Reserved(terminal) = registry
            .reserve_terminal(terminal_id, "example", None, "/work", false)
            .expect("fresh unresolved terminal claim")
        else {
            panic!("fresh terminal unexpectedly joined");
        };
        terminal.commit().expect("committed terminal claim");

        assert!(matches!(
            registry.bind_terminal_native(terminal_id, "example", "native", "/work"),
            Err(TerminalClaimError::StructuredBusy)
        ));
    }

    #[test]
    fn live_and_identity_pending_claims_block_native_mutation() {
        let registry = NativeLiveClaimRegistry::default();
        registry
            .reserve_structured(SessionId::now(), "example", Some("exact"), "/one")
            .expect("structured claim")
            .commit();
        let terminal_id = TerminalId::now();
        let TerminalClaimAdmission::Reserved(terminal) = registry
            .reserve_terminal(terminal_id, "example", None, "/two", false)
            .expect("identity-pending terminal")
        else {
            panic!("fresh terminal unexpectedly joined");
        };
        terminal.commit().expect("committed terminal claim");

        assert!(
            registry
                .blocks_native_mutation("example", "exact", "/elsewhere")
                .expect("registry answers")
        );
        assert!(
            registry
                .blocks_native_mutation("example", "another", "/two")
                .expect("registry answers")
        );
        assert!(
            !registry
                .blocks_native_mutation("example", "another", "/three")
                .expect("registry answers")
        );
        assert!(
            !registry
                .blocks_native_mutation("other", "exact", "/one")
                .expect("registry answers")
        );
    }

    #[test]
    fn committed_structured_and_terminal_claims_exclude_each_other() {
        let registry = NativeLiveClaimRegistry::default();
        registry
            .reserve_structured(SessionId::now(), "example", Some("native"), "/work")
            .expect("structured claim")
            .commit();
        assert!(matches!(
            registry.reserve_terminal(TerminalId::now(), "example", Some("native"), "/work", false),
            Err(TerminalClaimError::StructuredBusy)
        ));

        let other = NativeLiveClaimRegistry::default();
        let TerminalClaimAdmission::Reserved(terminal) = other
            .reserve_terminal(TerminalId::now(), "example", Some("native"), "/work", false)
            .expect("terminal claim")
        else {
            panic!("first terminal unexpectedly joined");
        };
        terminal.commit().expect("committed terminal claim");
        assert!(matches!(
            other.reserve_structured(SessionId::now(), "example", Some("native"), "/work"),
            Err(TerminalClaimError::TerminalAlreadyLive)
        ));
    }

    #[test]
    fn an_unproven_legacy_generation_blocks_exact_native_resume() {
        let registry = NativeLiveClaimRegistry::default();
        registry.replace_legacy_generations(["legacy"].into_iter());
        assert!(matches!(
            registry.reserve_terminal(TerminalId::now(), "example", Some("native"), "/work", false),
            Err(TerminalClaimError::LegacyGenerationBusy)
        ));
        registry.replace_legacy_generations(std::iter::empty());
        assert!(matches!(
            registry.reserve_terminal(TerminalId::now(), "example", Some("native"), "/work", false),
            Ok(TerminalClaimAdmission::Reserved(_))
        ));
    }
}
