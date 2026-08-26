//! Content-free provider-process ownership projected across daemon generations.
//!
//! A claim contains only provider identity, provider-native identity when it is already known, canonical
//! workspace, surface kind, and an opaque owner identity. Conversation bytes and provider output never enter it.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::RwLock;

use runtrol_core::SessionManager;
use runtrol_ipc::{GenerationLiveClaimLine, GenerationLiveClaimSurface};
use runtrol_provider::TerminalId;

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
    structured: Vec<GenerationLiveClaimLine>,
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
    /// Atomically reserve a terminal launch against every local and inherited process claim.
    pub(crate) fn reserve_terminal(
        &self,
        terminal: TerminalId,
        provider_id: &str,
        native_session_id: Option<&str>,
        workspace: &str,
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
            .iter()
            .chain(state.pending_terminals.values())
            .chain(state.terminals.values())
            .chain(state.remote.values().flatten())
        {
            if let Some(error) = claim_conflict(provider_id, native_session_id, workspace, claim) {
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
        let next = sessions
            .live_sessions()
            .map(|session| GenerationLiveClaimLine {
                provider_id: session.provider.to_string().into_boxed_str(),
                native_session_id: session.native.map(Into::into),
                workspace: session.workspace.as_str().into(),
                surface: GenerationLiveClaimSurface::Structured,
                owner_id: session.session.to_string().into_boxed_str(),
            })
            .collect();
        if let Ok(mut state) = self.state.write() {
            state.structured = next;
        }
    }

    /// Retire the exact terminal process claim when its exit observer removes the PTY.
    pub(crate) fn terminal_ended(&self, id: TerminalId) {
        if let Ok(mut state) = self.state.write() {
            state.pending_terminals.remove(&id);
            state.terminals.remove(&id);
        }
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
            .iter()
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
) -> Option<TerminalClaimError> {
    if claim.provider_id.as_ref() != provider_id {
        return None;
    }
    let exact_native =
        native_session_id.is_some() && claim.native_session_id.as_deref() == native_session_id;
    if exact_native && claim.workspace.as_ref() != workspace {
        return Some(TerminalClaimError::WorkspaceConflict);
    }
    let unresolved_collision =
        claim.native_session_id.is_none() && claim.workspace.as_ref() == workspace;
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
            registry.reserve_terminal(TerminalId::now(), "example", Some("native"), "/first"),
            Err(TerminalClaimError::StructuredBusy)
        ));
        assert!(matches!(
            registry.reserve_terminal(TerminalId::now(), "example", Some("native"), "/second"),
            Err(TerminalClaimError::WorkspaceConflict)
        ));
    }

    #[test]
    fn an_unresolved_launch_blocks_a_native_resume_in_the_same_workspace() {
        let registry = NativeLiveClaimRegistry::default();
        let first = match registry.reserve_terminal(TerminalId::now(), "example", None, "/work") {
            Ok(admission) => admission,
            Err(error) => panic!("the fresh terminal did not reserve its claim: {error}"),
        };
        assert!(matches!(&first, TerminalClaimAdmission::Reserved(_)));
        assert!(matches!(
            registry.reserve_terminal(TerminalId::now(), "example", Some("native"), "/work"),
            Err(TerminalClaimError::TerminalAlreadyLive)
        ));
    }

    #[test]
    fn an_unproven_legacy_generation_blocks_exact_native_resume() {
        let registry = NativeLiveClaimRegistry::default();
        registry.replace_legacy_generations(["legacy"].into_iter());
        assert!(matches!(
            registry.reserve_terminal(TerminalId::now(), "example", Some("native"), "/work"),
            Err(TerminalClaimError::LegacyGenerationBusy)
        ));
        registry.replace_legacy_generations(std::iter::empty());
        assert!(matches!(
            registry.reserve_terminal(TerminalId::now(), "example", Some("native"), "/work"),
            Ok(TerminalClaimAdmission::Reserved(_))
        ));
    }
}
