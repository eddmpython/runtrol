//! Closed local `GateDefinition` registry and typed launch requests.

use std::collections::BTreeMap;

use runtrol_ledger::{GateRunId, RunId};
use runtrol_security::LocalScope;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

/// Symbolic working directory owned by the Mission contract.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum WorkingDirectoryRule {
    /// Exact Task working tree.
    TaskWorktree,
    /// Reviewed integrated tree.
    IntegratedTree,
}

/// Fixed local deterministic command definition.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct GateDefinition {
    /// Stable reference used by Mission files.
    pub id: Box<str>,
    /// Executable name resolved through the probed program boundary.
    pub program: Box<str>,
    /// Fixed argument vector, never shell source or interpolation.
    pub arguments: Vec<Box<str>>,
    /// Symbolic working directory.
    pub working_directory: WorkingDirectoryRule,
    /// Hard timeout.
    pub timeout_ms: u64,
    /// Platforms on which this exact definition is available.
    pub platforms: Vec<Box<str>>,
}

impl GateDefinition {
    /// Canonical digest of the reviewed fixed definition.
    #[must_use]
    pub fn digest(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        for part in [self.id.as_ref(), self.program.as_ref()] {
            hasher.update(part.len().to_be_bytes());
            hasher.update(part.as_bytes());
        }
        for argument in &self.arguments {
            hasher.update(argument.len().to_be_bytes());
            hasher.update(argument.as_bytes());
        }
        hasher.update(self.timeout_ms.to_be_bytes());
        hasher.update([match self.working_directory {
            WorkingDirectoryRule::TaskWorktree => 1,
            WorkingDirectoryRule::IntegratedTree => 2,
        }]);
        for platform in &self.platforms {
            hasher.update(platform.len().to_be_bytes());
            hasher.update(platform.as_bytes());
        }
        hasher.finalize().into()
    }

    fn validate(&self) -> Result<(), GateError> {
        if self.id.is_empty() || self.id.len() > 64 || self.program.is_empty() {
            return Err(GateError::InvalidDefinition);
        }
        if self.arguments.len() > 32 || self.arguments.iter().any(|argument| argument.len() > 256) {
            return Err(GateError::InvalidDefinition);
        }
        if self.timeout_ms == 0 || self.timeout_ms > 30 * 60 * 1_000 {
            return Err(GateError::InvalidDefinition);
        }
        Ok(())
    }
}

/// Exact local registry, changed only through local scope authority.
#[derive(Clone, Debug, Default)]
pub struct GateRegistry {
    definitions: BTreeMap<Box<str>, GateDefinition>,
}

impl GateRegistry {
    /// Register or replace one exact definition after daemon scope authorization.
    ///
    /// # Errors
    /// Returns [`GateError`] for the wrong local scope or a malformed definition.
    pub fn register(
        &mut self,
        authority: LocalScope,
        definition: GateDefinition,
    ) -> Result<(), GateError> {
        if authority != LocalScope::GateRegister {
            return Err(GateError::LocalOnly);
        }
        definition.validate()?;
        self.definitions.insert(definition.id.clone(), definition);
        Ok(())
    }

    /// Find one exact reviewed definition.
    #[must_use]
    pub fn get(&self, id: &str) -> Option<&GateDefinition> {
        self.definitions.get(id)
    }

    /// Every fixed definition in stable identity order.
    pub fn definitions(&self) -> impl Iterator<Item = &GateDefinition> {
        self.definitions.values()
    }

    /// Produce a typed launch request for daemon execution.
    ///
    /// # Errors
    /// Returns [`GateError::UnknownGate`] when no exact local definition exists.
    pub fn request(&self, id: &str, run_id: RunId) -> Result<GateRequest, GateError> {
        let definition = self.get(id).ok_or(GateError::UnknownGate)?;
        Ok(GateRequest {
            gate_run_id: GateRunId::now(),
            run_id,
            definition: definition.clone(),
            definition_sha256: definition.digest(),
        })
    }
}

/// Typed gate launch effect consumed by daemon-owned containment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GateRequest {
    /// Gate execution identity.
    pub gate_run_id: GateRunId,
    /// Exact Run whose tree is checked.
    pub run_id: RunId,
    /// Fixed reviewed definition.
    pub definition: GateDefinition,
    /// Digest stored in evidence.
    pub definition_sha256: [u8; 32],
}

/// Closed gate result. Process output is intentionally absent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GateOutcome {
    /// Declared exit class passed.
    Passed,
    /// Declared exit class failed.
    Failed,
    /// Hard timeout fired and the owned process tree was cleaned.
    TimedOut,
    /// Local cancellation cleaned the owned process tree.
    Cancelled,
    /// Fixed executable was unavailable.
    LaunchFailed,
}

/// Gate registry or request refusal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum GateError {
    /// Only a local registration action may change the registry.
    #[error("gate registration is local only")]
    LocalOnly,
    /// Definition exceeds a closed field or numeric bound.
    #[error("gate definition is invalid")]
    InvalidDefinition,
    /// Mission named no exact local definition.
    #[error("gate id is not in the local registry")]
    UnknownGate,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn definition() -> GateDefinition {
        GateDefinition {
            id: "check".into(),
            program: "cargo".into(),
            arguments: vec!["test".into()],
            working_directory: WorkingDirectoryRule::TaskWorktree,
            timeout_ms: 60_000,
            platforms: vec!["current".into()],
        }
    }

    #[test]
    fn only_local_exact_registry_entries_create_requests() {
        let mut registry = GateRegistry::default();
        assert_eq!(
            registry.register(LocalScope::MissionStart, definition()),
            Err(GateError::LocalOnly)
        );
        registry
            .register(LocalScope::GateRegister, definition())
            .expect("register");
        assert_eq!(
            registry.request("missing", RunId::now()),
            Err(GateError::UnknownGate)
        );
        assert!(registry.request("check", RunId::now()).is_ok());
    }
}
