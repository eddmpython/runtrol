//! Explicit Artifact-only Handoff boundary.

use serde::{Deserialize, Serialize};

/// Closed Handoff between two reviewed Tasks.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Handoff {
    /// Exact schema identity.
    pub schema: Box<str>,
    /// Mission identity text from the project artifact.
    pub mission_id: Box<str>,
    /// Source Task identity text.
    pub source_task_id: Box<str>,
    /// Source Run identity text.
    pub source_run_id: Box<str>,
    /// Exact base Git identity.
    pub base_commit: Box<str>,
    /// Exact finish tree identity.
    pub finish_tree: Box<str>,
    /// Reviewed policy digest.
    pub policy_sha256: Box<str>,
    /// Evidence Receipt identity.
    pub receipt_id: Box<str>,
    /// Bounded Artifact references.
    pub artifacts: Vec<ArtifactManifestEntry>,
}

impl Handoff {
    /// Parse the closed TOML schema and enforce its Artifact count.
    ///
    /// # Errors
    /// Returns [`HandoffError`] for unknown fields, unsupported schema, or bounds.
    pub fn parse(source: &str) -> Result<Self, HandoffError> {
        let handoff: Self = toml::from_str(source).map_err(|_| HandoffError::Schema)?;
        if handoff.schema.as_ref() != "runtrol.dev/handoff/v1alpha1" {
            return Err(HandoffError::Schema);
        }
        if handoff.artifacts.is_empty() || handoff.artifacts.len() > crate::MAX_OUTPUT_ROOTS {
            return Err(HandoffError::ArtifactBound);
        }
        Ok(handoff)
    }
}

/// Bounded sorted Artifact manifest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArtifactManifest {
    /// Entries sorted by normalized relative path.
    pub entries: Vec<ArtifactManifestEntry>,
    /// Total declared bytes.
    pub total_bytes: u64,
}

impl ArtifactManifest {
    /// Validate count, paths, and byte ceiling before accepting the manifest.
    ///
    /// # Errors
    /// Returns [`HandoffError`] when an entry escapes or a bound is exceeded.
    pub fn seal(mut entries: Vec<ArtifactManifestEntry>) -> Result<Self, HandoffError> {
        if entries.is_empty() || entries.len() > runtrol_ledger::MAX_ARTIFACTS_PER_RUN {
            return Err(HandoffError::ArtifactBound);
        }
        if entries.iter().any(|entry| !safe_relative(&entry.path)) {
            return Err(HandoffError::PathEscape);
        }
        let total_bytes = entries
            .iter()
            .try_fold(0_u64, |total, entry| total.checked_add(entry.size))
            .ok_or(HandoffError::ArtifactBound)?;
        if total_bytes > runtrol_ledger::MAX_ARTIFACT_BYTES_PER_RUN {
            return Err(HandoffError::ArtifactBound);
        }
        entries.sort_by(|left, right| left.path.cmp(&right.path));
        Ok(Self {
            entries,
            total_bytes,
        })
    }
}

/// One project-owned Artifact reference, never its body.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ArtifactManifestEntry {
    /// Normalized project-relative path.
    pub path: Box<str>,
    /// Lowercase SHA-256 text.
    pub sha256: Box<str>,
    /// Declared media type.
    pub media_type: Box<str>,
    /// Declared bytes.
    #[serde(default)]
    pub size: u64,
}

fn safe_relative(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with(['/', '\\'])
        && !path
            .split(['/', '\\'])
            .any(|part| part.is_empty() || part == "." || part == "..")
        && !path.contains(':')
}

/// Handoff schema, path, or bound failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum HandoffError {
    /// Closed schema or version was not accepted.
    #[error("handoff schema is invalid or unsupported")]
    Schema,
    /// Artifact count or bytes exceeded the fixed bound.
    #[error("handoff artifact bound exceeded")]
    ArtifactBound,
    /// Artifact path was not a safe relative project path.
    #[error("handoff artifact path escapes the project")]
    PathEscape,
}
