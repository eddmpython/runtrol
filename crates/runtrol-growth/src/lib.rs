//! Project-only, non-executable capability candidates and exact local approval metadata.
//!
//! Candidate bodies remain ordinary project files. This crate accepts their bounded metadata and digests, never
//! provider conversation events, process output, environment values, credentials, or hidden prompt material.

use std::{collections::BTreeMap, io::Read as _, path::Path};

use runtrol_provider::AbsPath;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

/// Accepted capability metadata schema.
pub const CAPABILITY_SCHEMA: &str = "runtrol.dev/capability/v1alpha1";
/// Accepted verification plan schema.
pub const VERIFICATION_SCHEMA: &str = "runtrol.dev/capability-verification/v1alpha1";
/// Maximum files in one candidate.
pub const MAX_CANDIDATE_FILES: usize = 64;
/// Maximum bytes in one candidate tree.
pub const MAX_CANDIDATE_BYTES: u64 = 1024 * 1024;
/// Maximum approved versions retained for one capability.
pub const MAX_APPROVED_VERSIONS: usize = 8;

/// Non-executable capability kind.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityKind {
    /// Human-readable procedure and references.
    Skill,
    /// References to existing fixed Gate definitions.
    GateRecipe,
    /// Closed parameterized Mission template.
    Playbook,
}

/// Closed project capability metadata.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CandidateManifest {
    /// Exact schema identity.
    pub schema: Box<str>,
    /// Stable lowercase project capability identity.
    pub capability_id: Box<str>,
    /// Non-executable kind.
    pub kind: CapabilityKind,
    /// Must be `project` in v0.
    pub scope: Box<str>,
    /// Digest of `SKILL.md` and `references/` payload files.
    pub content_sha256: Box<str>,
    /// Source Mission identity.
    pub source_mission_id: Box<str>,
    /// Source Task identity.
    pub source_task_id: Box<str>,
    /// Source Run identity.
    pub source_run_id: Box<str>,
    /// Passing source Receipt identity.
    pub source_receipt_id: Box<str>,
    /// Reviewed policy digest.
    pub policy_sha256: Box<str>,
    /// Prior approved version digest, when updating.
    #[serde(default)]
    pub parent_version: Option<Box<str>>,
    /// Project-selected license identifier.
    pub license: Box<str>,
}

/// Closed candidate verification plan.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct VerificationPlan {
    /// Exact schema identity.
    pub schema: Box<str>,
    /// Author Run, distinct from verifier Run.
    pub author_run_id: Box<str>,
    /// Independent verifier Run.
    pub verifier_run_id: Box<str>,
    /// Explicit project-relative replay instruction.
    pub replay_instruction_ref: Box<str>,
    /// Explicit project-relative fixed input fixture.
    pub fixture_ref: Box<str>,
    /// Existing fixed Gate identities.
    pub gate_refs: Vec<Box<str>>,
}

/// One bounded file fact without file bytes.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct CandidateFile {
    /// Candidate-relative normalized path.
    pub path: Box<str>,
    /// Exact file digest.
    pub sha256: [u8; 32],
    /// Exact bytes.
    pub size: u64,
}

/// Fully inspected project candidate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InspectedCandidate {
    /// Canonical project root.
    pub project: AbsPath,
    /// Candidate-relative directory selected locally.
    pub candidate_ref: Box<str>,
    /// Closed metadata.
    pub manifest: CandidateManifest,
    /// Closed verification plan.
    pub verification: VerificationPlan,
    /// Stable sorted file facts.
    pub files: Vec<CandidateFile>,
    /// Digest of every candidate file, including closed metadata.
    pub version_sha256: [u8; 32],
}

/// Local trust state for one exact project capability.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum CandidateState {
    /// Files were explicitly proposed.
    Proposed,
    /// Closed schemas and bounds passed.
    Candidate,
    /// Fixed independent Gates are running.
    Verifying,
    /// Verification evidence passed and awaits local approval.
    Verified,
    /// Exact digest is locally active.
    Active,
    /// Active project bytes changed.
    Tampered,
    /// Local user removed the version from selection.
    Quarantined,
    /// Compatibility evidence requires revalidation.
    Stale,
    /// A prior approved version became active.
    RolledBack,
    /// Local user rejected the proposal.
    Rejected,
    /// Version is retained only as history.
    Archived,
}

impl CandidateState {
    /// Whether one explicit transition is legal.
    #[must_use]
    pub const fn allows(self, next: Self) -> bool {
        matches!(
            (self, next),
            (Self::Proposed, Self::Candidate | Self::Rejected)
                | (Self::Candidate, Self::Verifying | Self::Rejected)
                | (
                    Self::Verifying,
                    Self::Verified | Self::Candidate | Self::Rejected
                )
                | (Self::Verified, Self::Active | Self::Rejected)
                | (
                    Self::Active,
                    Self::Tampered | Self::Quarantined | Self::Stale | Self::Archived
                )
                | (
                    Self::Tampered | Self::Quarantined,
                    Self::Candidate | Self::RolledBack | Self::Archived
                )
                | (Self::Stale, Self::Candidate | Self::Archived)
                | (
                    Self::RolledBack,
                    Self::Tampered | Self::Quarantined | Self::Archived
                )
                | (Self::Rejected, Self::Archived)
                | (Self::Archived, Self::Candidate)
        )
    }
}

/// One bounded fixed-Gate fact retained for capability review.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct CapabilityGateEvidence {
    /// Stable local Gate identity.
    pub gate_id: Box<str>,
    /// Exact locally approved Gate definition digest.
    pub definition_sha256: [u8; 32],
    /// Closed outcome label.
    pub outcome: Box<str>,
    /// Observed wall duration for review, excluded from Receipt identity.
    pub duration_ms: u64,
}

/// Metadata-only independent verification attempt.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct CapabilityVerification {
    /// Author Run from the reviewed verification plan.
    pub author_run_id: Box<str>,
    /// Distinct verifier Run from the reviewed verification plan.
    pub verifier_run_id: Box<str>,
    /// Passing content-addressed Receipt identity, absent on failure.
    pub receipt_id: Option<Box<str>>,
    /// Stable Gate evidence in reviewed order.
    pub gates: Vec<CapabilityGateEvidence>,
}

/// One approved immutable version retained for rollback.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ApprovedVersion {
    /// Full candidate tree digest.
    pub version_sha256: [u8; 32],
    /// Candidate source reference when approved.
    pub source_ref: Box<str>,
    /// Exact passing independent verification evidence.
    pub verification: CapabilityVerification,
}

/// Metadata-only trust record.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct CandidateRecord {
    /// Stable capability identity.
    pub capability_id: Box<str>,
    /// Canonical project identity.
    pub project_id: Box<str>,
    /// Current project-relative source or active path.
    pub source_ref: Box<str>,
    /// Candidate kind.
    pub kind: CapabilityKind,
    /// Current state.
    pub state: CandidateState,
    /// Exact current candidate digest.
    pub version_sha256: [u8; 32],
    /// Source passing Receipt.
    pub source_receipt_id: Box<str>,
    /// Reviewed policy digest.
    pub policy_sha256: Box<str>,
    /// Most recent independent verification attempt.
    pub verification: Option<CapabilityVerification>,
    /// Exact approved version currently selected at the active path.
    #[serde(default)]
    pub active_version_sha256: Option<[u8; 32]>,
    /// Approved version history in approval order.
    pub approved_versions: Vec<ApprovedVersion>,
}

impl CandidateRecord {
    /// Apply one legal exact transition.
    ///
    /// # Errors
    /// Returns [`GrowthError::State`] for an undeclared edge.
    pub fn transition(&mut self, next: CandidateState) -> Result<(), GrowthError> {
        if !self.state.allows(next) {
            return Err(GrowthError::State);
        }
        self.state = next;
        Ok(())
    }
}

/// Durable metadata-only local trust index.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct TrustIndex {
    /// Fixed schema generation.
    pub schema: u8,
    /// Project and capability keyed records.
    pub records: BTreeMap<Box<str>, CandidateRecord>,
}

impl TrustIndex {
    /// Construct the current empty index.
    #[must_use]
    pub fn current() -> Self {
        Self {
            schema: 1,
            records: BTreeMap::new(),
        }
    }

    /// Stable composite key.
    #[must_use]
    pub fn key(project: &str, capability: &str) -> Box<str> {
        format!("{project}\u{0}{capability}").into()
    }

    /// Insert or replace one explicitly inspected proposal.
    ///
    /// # Errors
    /// Returns [`GrowthError::Conflict`] when an active unrelated version already owns the ID.
    pub fn propose(
        &mut self,
        candidate: &InspectedCandidate,
    ) -> Result<&CandidateRecord, GrowthError> {
        let key = Self::key(
            candidate.project.as_str(),
            &candidate.manifest.capability_id,
        );
        if let Some(record) = self.records.get(&key) {
            let expected_parent = record
                .active_version_sha256
                .map(|digest| digest_text(&digest));
            if expected_parent.as_deref() != candidate.manifest.parent_version.as_deref() {
                return Err(GrowthError::Conflict);
            }
        }
        let mut record = CandidateRecord {
            capability_id: candidate.manifest.capability_id.clone(),
            project_id: candidate.project.as_str().into(),
            source_ref: candidate.candidate_ref.clone(),
            kind: candidate.manifest.kind,
            state: CandidateState::Proposed,
            version_sha256: candidate.version_sha256,
            source_receipt_id: candidate.manifest.source_receipt_id.clone(),
            policy_sha256: candidate.manifest.policy_sha256.clone(),
            verification: None,
            active_version_sha256: self
                .records
                .get(&key)
                .and_then(|existing| existing.active_version_sha256),
            approved_versions: self
                .records
                .get(&key)
                .map_or_else(Vec::new, |existing| existing.approved_versions.clone()),
        };
        record.transition(CandidateState::Candidate)?;
        self.records.insert(key.clone(), record);
        self.records.get(&key).ok_or(GrowthError::State)
    }

    /// Find one project capability record.
    #[must_use]
    pub fn get(&self, project: &str, capability: &str) -> Option<&CandidateRecord> {
        self.records.get(&Self::key(project, capability))
    }

    /// Mutably find one project capability record.
    #[must_use]
    pub fn get_mut(&mut self, project: &str, capability: &str) -> Option<&mut CandidateRecord> {
        self.records.get_mut(&Self::key(project, capability))
    }
}

/// Inspect exact project candidate files and closed schemas.
///
/// # Errors
/// Returns one closed refusal for path, schema, binary, link, size, digest, or provenance defects.
pub fn inspect(project: &AbsPath, candidate_ref: &str) -> Result<InspectedCandidate, GrowthError> {
    if !safe_relative(candidate_ref) {
        return Err(GrowthError::Path);
    }
    let candidate = project.join(candidate_ref).map_err(|_| GrowthError::Path)?;
    let candidate = AbsPath::canonicalize(candidate.as_str()).map_err(|_| GrowthError::Path)?;
    if !candidate.is_under(project) {
        return Err(GrowthError::Path);
    }
    let mut files = Vec::new();
    collect(candidate.as_std_path(), candidate.as_std_path(), &mut files)?;
    files.sort_by(|left, right| left.path.cmp(&right.path));
    if files.len() < 3 || files.len() > MAX_CANDIDATE_FILES {
        return Err(GrowthError::Bound);
    }
    let total = files
        .iter()
        .try_fold(0_u64, |sum, file| sum.checked_add(file.size))
        .ok_or(GrowthError::Bound)?;
    if total > MAX_CANDIDATE_BYTES {
        return Err(GrowthError::Bound);
    }
    for required in ["SKILL.md", "capability.toml", "verify.toml"] {
        if !files.iter().any(|file| file.path.as_ref() == required) {
            return Err(GrowthError::Schema);
        }
    }
    let manifest: CandidateManifest = read_toml(candidate.as_std_path(), "capability.toml")?;
    let verification: VerificationPlan = read_toml(candidate.as_std_path(), "verify.toml")?;
    validate_metadata(project, &manifest, &verification)?;
    let payload = tree_digest(files.iter().filter(|file| {
        file.path.as_ref() != "capability.toml" && file.path.as_ref() != "verify.toml"
    }));
    if digest_text(&payload) != manifest.content_sha256.as_ref() {
        return Err(GrowthError::Digest);
    }
    let version_sha256 = tree_digest(files.iter());
    Ok(InspectedCandidate {
        project: project.clone(),
        candidate_ref: candidate_ref.into(),
        manifest,
        verification,
        files,
        version_sha256,
    })
}

fn validate_metadata(
    project: &AbsPath,
    manifest: &CandidateManifest,
    verification: &VerificationPlan,
) -> Result<(), GrowthError> {
    if manifest.schema.as_ref() != CAPABILITY_SCHEMA
        || verification.schema.as_ref() != VERIFICATION_SCHEMA
        || manifest.scope.as_ref() != "project"
        || !valid_id(&manifest.capability_id)
        || !valid_digest(&manifest.content_sha256)
        || !valid_digest(&manifest.policy_sha256)
        || manifest.source_mission_id.is_empty()
        || manifest.source_task_id.is_empty()
        || manifest.source_run_id.is_empty()
        || !manifest.source_receipt_id.starts_with("rcp_")
        || manifest.license.is_empty()
        || verification.author_run_id == verification.verifier_run_id
        || verification.gate_refs.is_empty()
        || verification.gate_refs.len() > 64
        || !safe_relative(&verification.replay_instruction_ref)
        || !safe_relative(&verification.fixture_ref)
    {
        return Err(GrowthError::Schema);
    }
    for reference in [
        verification.replay_instruction_ref.as_ref(),
        verification.fixture_ref.as_ref(),
    ] {
        let path = project.join(reference).map_err(|_| GrowthError::Path)?;
        let canonical = AbsPath::canonicalize(path.as_str()).map_err(|_| GrowthError::Path)?;
        if !canonical.is_under(project) {
            return Err(GrowthError::Path);
        }
    }
    Ok(())
}

fn collect(root: &Path, path: &Path, files: &mut Vec<CandidateFile>) -> Result<(), GrowthError> {
    let metadata = std::fs::symlink_metadata(path).map_err(|_| GrowthError::Path)?;
    if metadata.file_type().is_symlink() {
        return Err(GrowthError::Link);
    }
    if metadata.is_dir() {
        let mut entries = std::fs::read_dir(path)
            .map_err(|_| GrowthError::Path)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| GrowthError::Path)?;
        entries.sort_by_key(std::fs::DirEntry::file_name);
        for entry in entries {
            if files.len() >= MAX_CANDIDATE_FILES {
                return Err(GrowthError::Bound);
            }
            collect(root, &entry.path(), files)?;
        }
        return Ok(());
    }
    if !metadata.is_file() || metadata.len() > MAX_CANDIDATE_BYTES {
        return Err(GrowthError::Bound);
    }
    let relative = path.strip_prefix(root).map_err(|_| GrowthError::Path)?;
    let relative = relative
        .to_str()
        .ok_or(GrowthError::Path)?
        .replace('\\', "/");
    if !allowed_file(&relative) {
        return Err(GrowthError::Executable);
    }
    let sha256 = hash_text_file(path)?;
    files.push(CandidateFile {
        path: relative.into(),
        sha256,
        size: metadata.len(),
    });
    Ok(())
}

fn allowed_file(path: &str) -> bool {
    path == "SKILL.md"
        || path == "capability.toml"
        || path == "verify.toml"
        || path.strip_prefix("references/").is_some_and(|name| {
            safe_relative(name)
                && matches!(
                    Path::new(name).extension().and_then(|one| one.to_str()),
                    Some("md" | "txt" | "toml" | "json")
                )
        })
}

fn hash_text_file(path: &Path) -> Result<[u8; 32], GrowthError> {
    let file = std::fs::File::open(path).map_err(|_| GrowthError::Path)?;
    let mut bytes = Vec::new();
    file.take(MAX_CANDIDATE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| GrowthError::Path)?;
    if u64::try_from(bytes.len()).map_or(true, |size| size > MAX_CANDIDATE_BYTES)
        || bytes.contains(&0)
        || core::str::from_utf8(&bytes).is_err()
    {
        return Err(GrowthError::Binary);
    }
    Ok(Sha256::digest(bytes).into())
}

fn read_toml<T: serde::de::DeserializeOwned>(root: &Path, name: &str) -> Result<T, GrowthError> {
    let source = std::fs::read_to_string(root.join(name)).map_err(|_| GrowthError::Schema)?;
    toml::from_str(&source).map_err(|_| GrowthError::Schema)
}

fn tree_digest<'a>(files: impl Iterator<Item = &'a CandidateFile>) -> [u8; 32] {
    let mut hasher = Sha256::new();
    for file in files {
        hasher.update(
            u64::try_from(file.path.len())
                .unwrap_or(u64::MAX)
                .to_be_bytes(),
        );
        hasher.update(file.path.as_bytes());
        hasher.update(file.sha256);
        hasher.update(file.size.to_be_bytes());
    }
    hasher.finalize().into()
}

fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && !value.starts_with('-')
        && !value.ends_with('-')
}

fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn safe_relative(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with(['/', '\\'])
        && !path.contains(':')
        && !path
            .split(['/', '\\'])
            .any(|part| part.is_empty() || part == "." || part == "..")
}

/// Lowercase digest text.
#[must_use]
pub fn digest_text(digest: &[u8; 32]) -> String {
    use core::fmt::Write as _;
    let mut text = String::with_capacity(64);
    for byte in digest {
        let _written = write!(&mut text, "{byte:02x}");
    }
    text
}

/// Candidate validation or trust refusal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum GrowthError {
    /// Candidate path escaped or was unavailable.
    #[error("candidate path is invalid or unavailable")]
    Path,
    /// Candidate contains a symbolic link.
    #[error("candidate tree contains a symbolic link")]
    Link,
    /// Candidate count or bytes exceeded a hard bound.
    #[error("candidate tree exceeds a hard bound")]
    Bound,
    /// Candidate contains an unapproved or executable-shaped file.
    #[error("candidate tree contains an unsupported file")]
    Executable,
    /// Candidate contains binary or non-UTF-8 bytes.
    #[error("candidate tree contains binary content")]
    Binary,
    /// Closed metadata is malformed or incomplete.
    #[error("candidate metadata schema is invalid")]
    Schema,
    /// Declared content digest does not match exact project bytes.
    #[error("candidate content digest does not match")]
    Digest,
    /// Stable ID already belongs to another active exact version.
    #[error("capability identity conflicts with an active version")]
    Conflict,
    /// Trust state transition is not allowed.
    #[error("capability state transition is not allowed")]
    State,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_machine_requires_verification_before_activation() {
        assert!(CandidateState::Candidate.allows(CandidateState::Verifying));
        assert!(CandidateState::Verified.allows(CandidateState::Active));
        assert!(!CandidateState::Candidate.allows(CandidateState::Active));
        assert!(!CandidateState::Tampered.allows(CandidateState::Active));
    }
}
