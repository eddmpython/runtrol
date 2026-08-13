//! Local capability candidate inbox, independent fixed-Gate verification, activation, and rollback.

use std::{
    io::Write as _,
    time::{Duration, Instant},
};

use runtrol_childproc::{Containment, SpawnError, capture_in, resolve};
use runtrol_growth::{
    ApprovedVersion, CandidateRecord, CandidateState, CapabilityGateEvidence, CapabilityKind,
    CapabilityVerification, InspectedCandidate, TrustIndex, digest_text, inspect,
};
use runtrol_ipc::wire::{CapabilityGateLine, CapabilityLine, Request, Response};
use runtrol_orchestrator::{CapabilitySelection, GateOutcome, GateRequest};
use runtrol_provider::AbsPath;
use sha2::{Digest as _, Sha256};

const MAX_TRUST_FILE_BYTES: u64 = 256 * 1024;

#[derive(Debug)]
pub(crate) struct GrowthController {
    path: AbsPath,
    index: TrustIndex,
}

#[derive(Clone, Debug)]
pub(crate) struct VerificationIntent {
    project: AbsPath,
    capability_id: Box<str>,
    version_sha256: [u8; 32],
    candidate: InspectedCandidate,
    gates: Vec<GateRequest>,
}

#[derive(Clone, Debug)]
pub(crate) struct GateResult {
    request: GateRequest,
    outcome: GateOutcome,
    duration_ms: u64,
}

impl GrowthController {
    pub(crate) fn open(path: AbsPath) -> Result<Self, String> {
        Ok(Self {
            index: load(&path)?,
            path,
        })
    }

    pub(crate) fn answer(&mut self, request: &Request) -> Response {
        self.try_answer(request).unwrap_or_else(failed)
    }

    fn try_answer(&mut self, request: &Request) -> Result<Response, &'static str> {
        match request {
            Request::CapabilityPropose {
                project,
                candidate_ref,
            } => self.propose(project, candidate_ref),
            Request::CapabilityList => {
                self.refresh_tamper()?;
                Ok(self.list())
            }
            Request::CapabilityApprove {
                project,
                capability_id,
                version_sha256,
            } => self.approve(project, capability_id, version_sha256),
            Request::CapabilityReject {
                project,
                capability_id,
            } => self.transition(project, capability_id, CandidateState::Rejected),
            Request::CapabilityQuarantine {
                project,
                capability_id,
            } => self.transition(project, capability_id, CandidateState::Quarantined),
            Request::CapabilityRollback {
                project,
                capability_id,
                version_sha256,
            } => self.rollback(project, capability_id, version_sha256),
            Request::CapabilityArchive {
                project,
                capability_id,
            } => self.archive(project, capability_id),
            _ => Err("the request is not a capability operation"),
        }
    }

    fn propose(&mut self, project: &str, candidate_ref: &str) -> Result<Response, &'static str> {
        let project =
            AbsPath::canonicalize(project).map_err(|_| "the capability project is unavailable")?;
        let candidate = inspect(&project, candidate_ref)
            .map_err(|_| "the capability candidate did not validate")?;
        self.index
            .propose(&candidate)
            .map_err(|_| "the capability candidate conflicts with current trust state")?;
        self.save()?;
        Ok(self.list())
    }

    pub(crate) fn verification_intent(
        &mut self,
        project: &str,
        capability_id: &str,
        version_sha256: &str,
        gates: Vec<GateRequest>,
    ) -> Result<VerificationIntent, &'static str> {
        let project =
            AbsPath::canonicalize(project).map_err(|_| "the capability project is unavailable")?;
        let record = self
            .index
            .get(project.as_str(), capability_id)
            .ok_or("the capability candidate does not exist")?;
        if record.state != CandidateState::Candidate {
            return Err("the capability candidate is not ready for verification");
        }
        let expected = parse_digest(version_sha256)?;
        if expected != record.version_sha256 {
            return Err("the capability candidate digest changed before verification");
        }
        let candidate = inspect(&project, &record.source_ref)
            .map_err(|_| "the capability candidate bytes no longer validate")?;
        if candidate.version_sha256 != expected {
            return Err("the capability candidate bytes changed before verification");
        }
        if candidate.verification.gate_refs.len() != gates.len() {
            return Err("the capability verification Gate set changed");
        }
        let record = self
            .index
            .get_mut(project.as_str(), capability_id)
            .ok_or("the capability candidate disappeared")?;
        record
            .transition(CandidateState::Verifying)
            .map_err(|_| "the capability cannot enter verification")?;
        self.save()?;
        Ok(VerificationIntent {
            project,
            capability_id: capability_id.into(),
            version_sha256: expected,
            candidate,
            gates,
        })
    }

    pub(crate) fn verification_gate_ids(
        &self,
        project: &str,
        capability_id: &str,
        version_sha256: &str,
    ) -> Result<Vec<Box<str>>, &'static str> {
        let project =
            AbsPath::canonicalize(project).map_err(|_| "the capability project is unavailable")?;
        let record = self
            .index
            .get(project.as_str(), capability_id)
            .ok_or("the capability candidate does not exist")?;
        if record.version_sha256 != parse_digest(version_sha256)? {
            return Err("the capability candidate digest changed");
        }
        let candidate = inspect(&project, &record.source_ref)
            .map_err(|_| "the capability candidate bytes no longer validate")?;
        Ok(candidate.verification.gate_refs)
    }

    pub(crate) fn commit_verification(
        &mut self,
        intent: &VerificationIntent,
        results: &[GateResult],
    ) -> Result<Response, &'static str> {
        let record = self
            .index
            .get_mut(intent.project.as_str(), &intent.capability_id)
            .ok_or("the capability candidate disappeared during verification")?;
        if record.state != CandidateState::Verifying
            || record.version_sha256 != intent.version_sha256
        {
            return Err("the capability trust state changed during verification");
        }
        let passed = !results.is_empty()
            && results.len() == intent.gates.len()
            && results
                .iter()
                .all(|result| result.outcome == GateOutcome::Passed);
        let receipt_id = passed.then(|| verification_receipt_id(intent, results).into());
        record.verification = Some(CapabilityVerification {
            author_run_id: intent.candidate.verification.author_run_id.clone(),
            verifier_run_id: intent.candidate.verification.verifier_run_id.clone(),
            receipt_id,
            gates: results
                .iter()
                .map(|result| CapabilityGateEvidence {
                    gate_id: result.request.definition.id.clone(),
                    definition_sha256: result.request.definition_sha256,
                    outcome: gate_outcome(result.outcome).into(),
                    duration_ms: result.duration_ms,
                })
                .collect(),
        });
        if passed {
            record
                .transition(CandidateState::Verified)
                .map_err(|_| "the capability cannot become verified")?;
        } else {
            record
                .transition(CandidateState::Candidate)
                .map_err(|_| "the capability cannot return to candidate review")?;
        }
        self.save()?;
        Ok(self.list())
    }

    fn approve(
        &mut self,
        project: &str,
        capability_id: &str,
        version_sha256: &str,
    ) -> Result<Response, &'static str> {
        let project =
            AbsPath::canonicalize(project).map_err(|_| "the capability project is unavailable")?;
        let expected = parse_digest(version_sha256)?;
        let record = self
            .index
            .get(project.as_str(), capability_id)
            .ok_or("the capability candidate does not exist")?;
        if record.state != CandidateState::Verified
            || record.version_sha256 != expected
            || record
                .verification
                .as_ref()
                .and_then(|verification| verification.receipt_id.as_ref())
                .is_none()
        {
            return Err("the exact capability version is not independently verified");
        }
        let candidate = inspect(&project, &record.source_ref)
            .map_err(|_| "the verified capability bytes no longer validate")?;
        if candidate.version_sha256 != expected {
            return Err("the verified capability bytes changed before approval");
        }
        let candidate_ref = record.source_ref.clone();
        let prior_active = record
            .active_version_sha256
            .and_then(|digest| {
                record
                    .approved_versions
                    .iter()
                    .find(|version| version.version_sha256 == digest)
            })
            .cloned();
        let activation = activate(
            &project,
            capability_id,
            &candidate_ref,
            prior_active.as_ref(),
        )?;
        let record = self
            .index
            .get_mut(project.as_str(), capability_id)
            .ok_or("the capability candidate disappeared during approval")?;
        if let Some((prior_digest, archive_ref)) = activation.archived_prior
            && let Some(prior) = record
                .approved_versions
                .iter_mut()
                .find(|version| version.version_sha256 == prior_digest)
        {
            prior.source_ref = archive_ref;
        }
        record.source_ref = activation.active_ref;
        let verification = record
            .verification
            .as_ref()
            .filter(|verification| verification.receipt_id.is_some())
            .cloned()
            .ok_or("the verification Receipt disappeared")?;
        record.approved_versions.push(ApprovedVersion {
            version_sha256: expected,
            source_ref: record.source_ref.clone(),
            verification,
        });
        if record.approved_versions.len() > runtrol_growth::MAX_APPROVED_VERSIONS {
            record.approved_versions.remove(0);
        }
        record
            .transition(CandidateState::Active)
            .map_err(|_| "the capability cannot become active")?;
        record.active_version_sha256 = Some(expected);
        self.save()?;
        Ok(self.list())
    }

    fn rollback(
        &mut self,
        project: &str,
        capability_id: &str,
        version_sha256: &str,
    ) -> Result<Response, &'static str> {
        let project =
            AbsPath::canonicalize(project).map_err(|_| "the capability project is unavailable")?;
        let expected = parse_digest(version_sha256)?;
        let record = self
            .index
            .get(project.as_str(), capability_id)
            .ok_or("the capability does not exist")?;
        if !matches!(
            record.state,
            CandidateState::Tampered | CandidateState::Quarantined
        ) {
            return Err("the capability is not eligible for rollback");
        }
        let prior = record
            .approved_versions
            .iter()
            .find(|version| version.version_sha256 == expected)
            .cloned()
            .ok_or("the requested rollback version was never approved")?;
        let prior_candidate = inspect(&project, &prior.source_ref)
            .map_err(|_| "the prior approved capability bytes are unavailable")?;
        if prior_candidate.version_sha256 != expected {
            return Err("the prior approved capability bytes changed");
        }
        let active_ref = format!(".runtrol/capabilities/active/{capability_id}");
        let active = project
            .join(&active_ref)
            .map_err(|_| "the active capability path is invalid")?;
        let prior_path = project
            .join(&prior.source_ref)
            .map_err(|_| "the prior capability path is invalid")?;
        let displaced = if active.as_std_path().exists() && active != prior_path {
            let displaced_digest = record
                .active_version_sha256
                .ok_or("the active capability has no selected digest")?;
            let displaced_ref = format!(
                ".runtrol/capabilities/archive/{capability_id}/replaced-{}",
                digest_text(&displaced_digest)
            );
            let displaced = project
                .join(&displaced_ref)
                .map_err(|_| "the displaced capability path is invalid")?;
            if displaced.as_std_path().exists() {
                return Err("the displaced capability archive already exists");
            }
            if let Some(parent) = displaced.parent() {
                std::fs::create_dir_all(parent.as_std_path())
                    .map_err(|_| "the displaced capability archive parent cannot be created")?;
            }
            std::fs::rename(active.as_std_path(), displaced.as_std_path())
                .map_err(|_| "the current capability could not be displaced for rollback")?;
            Some((displaced_digest, displaced_ref, displaced))
        } else {
            None
        };
        if prior_path != active
            && std::fs::rename(prior_path.as_std_path(), active.as_std_path()).is_err()
        {
            if let Some((_, _, displaced)) = &displaced {
                let _restored = std::fs::rename(displaced.as_std_path(), active.as_std_path());
            }
            return Err("the prior approved capability could not be restored");
        }
        let record = self
            .index
            .get_mut(project.as_str(), capability_id)
            .ok_or("the capability disappeared during rollback")?;
        record.version_sha256 = prior.version_sha256;
        record.source_ref = active_ref.into();
        record.verification = Some(prior.verification.clone());
        record.active_version_sha256 = Some(prior.version_sha256);
        if let Some((displaced_digest, displaced_ref, _)) = displaced
            && let Some(version) = record
                .approved_versions
                .iter_mut()
                .find(|version| version.version_sha256 == displaced_digest)
        {
            version.source_ref = displaced_ref.into();
        }
        if let Some(version) = record
            .approved_versions
            .iter_mut()
            .find(|version| version.version_sha256 == expected)
        {
            version.source_ref = record.source_ref.clone();
        }
        record
            .transition(CandidateState::RolledBack)
            .map_err(|_| "the capability cannot roll back")?;
        self.save()?;
        Ok(self.list())
    }

    fn transition(
        &mut self,
        project: &str,
        capability_id: &str,
        next: CandidateState,
    ) -> Result<Response, &'static str> {
        let project =
            AbsPath::canonicalize(project).map_err(|_| "the capability project is unavailable")?;
        self.index
            .get_mut(project.as_str(), capability_id)
            .ok_or("the capability does not exist")?
            .transition(next)
            .map_err(|_| "the capability state transition is not allowed")?;
        self.save()?;
        Ok(self.list())
    }

    fn archive(&mut self, project: &str, capability_id: &str) -> Result<Response, &'static str> {
        let project =
            AbsPath::canonicalize(project).map_err(|_| "the capability project is unavailable")?;
        let record = self
            .index
            .get(project.as_str(), capability_id)
            .ok_or("the capability does not exist")?;
        let archive_ref = format!(
            ".runtrol/capabilities/archive/{capability_id}/{}",
            digest_text(&record.version_sha256)
        );
        let source = project
            .join(&record.source_ref)
            .map_err(|_| "the capability source path is invalid")?;
        let archive = project
            .join(&archive_ref)
            .map_err(|_| "the capability archive path is invalid")?;
        if archive.as_std_path().exists() {
            return Err("the exact capability archive destination already exists");
        }
        if let Some(parent) = archive.parent() {
            std::fs::create_dir_all(parent.as_std_path())
                .map_err(|_| "the capability archive parent cannot be created")?;
        }
        std::fs::rename(source.as_std_path(), archive.as_std_path())
            .map_err(|_| "the capability files could not be archived atomically")?;
        let record = self
            .index
            .get_mut(project.as_str(), capability_id)
            .ok_or("the capability disappeared while archiving")?;
        record.source_ref = archive_ref.into();
        record
            .transition(CandidateState::Archived)
            .map_err(|_| "the capability cannot be archived")?;
        self.save()?;
        Ok(self.list())
    }

    fn refresh_tamper(&mut self) -> Result<(), &'static str> {
        let mut changed = false;
        for record in self.index.records.values_mut() {
            if record.active_version_sha256.is_some()
                && !matches!(
                    record.state,
                    CandidateState::Tampered | CandidateState::Quarantined | CandidateState::Stale
                )
                && !active_intact(record)
            {
                record.state = CandidateState::Tampered;
                changed = true;
            }
        }
        if changed {
            self.save()?;
        }
        Ok(())
    }

    pub(crate) fn approved_capabilities(
        &mut self,
        project: &str,
    ) -> Result<Vec<CapabilitySelection>, String> {
        let project = AbsPath::canonicalize(project)
            .map_err(|_| "the capability project is unavailable".to_owned())?;
        self.refresh_tamper().map_err(str::to_owned)?;
        Ok(self
            .index
            .records
            .values()
            .filter(|record| {
                record.project_id.as_ref() == project.as_str()
                    && !matches!(
                        record.state,
                        CandidateState::Tampered
                            | CandidateState::Quarantined
                            | CandidateState::Stale
                    )
                    && active_intact(record)
            })
            .filter_map(|record| {
                record
                    .active_version_sha256
                    .map(|digest| CapabilitySelection {
                        capability_id: record.capability_id.clone(),
                        version_sha256: digest_text(&digest).into(),
                    })
            })
            .collect())
    }

    fn list(&self) -> Response {
        Response::Capabilities(self.index.records.values().map(line).collect())
    }

    fn save(&self) -> Result<(), &'static str> {
        save(&self.path, &self.index)
            .map_err(|_| "the capability trust index could not be committed")
    }
}

struct Activation {
    active_ref: Box<str>,
    archived_prior: Option<([u8; 32], Box<str>)>,
}

fn activate(
    project: &AbsPath,
    capability_id: &str,
    candidate_ref: &str,
    prior_active: Option<&ApprovedVersion>,
) -> Result<Activation, &'static str> {
    let active_ref = format!(".runtrol/capabilities/active/{capability_id}");
    let active = project
        .join(&active_ref)
        .map_err(|_| "the active capability path is invalid")?;
    let source = project
        .join(candidate_ref)
        .map_err(|_| "the candidate capability path is invalid")?;
    if let Some(parent) = active.parent() {
        std::fs::create_dir_all(parent.as_std_path())
            .map_err(|_| "the active capability parent cannot be created")?;
    }
    let archived = if active.as_std_path().exists() {
        let prior =
            prior_active.ok_or("the existing active capability has no approved version record")?;
        let archive_ref = format!(
            ".runtrol/capabilities/archive/{capability_id}/{}",
            digest_text(&prior.version_sha256)
        );
        let archive = project
            .join(&archive_ref)
            .map_err(|_| "the prior capability archive path is invalid")?;
        if archive.as_std_path().exists() {
            return Err("the prior capability archive destination already exists");
        }
        if let Some(parent) = archive.parent() {
            std::fs::create_dir_all(parent.as_std_path())
                .map_err(|_| "the prior capability archive parent cannot be created")?;
        }
        std::fs::rename(active.as_std_path(), archive.as_std_path())
            .map_err(|_| "the prior active capability could not be archived")?;
        Some((prior.version_sha256, archive_ref.into(), archive))
    } else {
        None
    };
    if std::fs::rename(source.as_std_path(), active.as_std_path()).is_err() {
        if let Some((_, _, archive)) = &archived {
            let _restored = std::fs::rename(archive.as_std_path(), active.as_std_path());
        }
        return Err("the capability files could not be activated atomically");
    }
    Ok(Activation {
        active_ref: active_ref.into(),
        archived_prior: archived.map(|(digest, reference, _)| (digest, reference)),
    })
}

fn active_intact(record: &CandidateRecord) -> bool {
    let Ok(project) = AbsPath::canonicalize(&record.project_id) else {
        return false;
    };
    let Some(active_digest) = record.active_version_sha256 else {
        return false;
    };
    let Some(active_version) = record
        .approved_versions
        .iter()
        .find(|version| version.version_sha256 == active_digest)
    else {
        return false;
    };
    let Ok(path) = project.join(&active_version.source_ref) else {
        return false;
    };
    if !path.as_std_path().exists() {
        return false;
    }
    match inspect(&project, &active_version.source_ref) {
        Ok(candidate) => candidate.version_sha256 == active_digest,
        Err(_) => false,
    }
}

pub(crate) async fn run_verification(
    containment: &Containment,
    intent: &VerificationIntent,
) -> Vec<GateResult> {
    let mut results = Vec::with_capacity(intent.gates.len());
    for request in &intent.gates {
        let started = Instant::now();
        let outcome = match resolve(&request.definition.program) {
            Ok(program) => {
                let arguments = request
                    .definition
                    .arguments
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>();
                match capture_in(
                    &program,
                    &arguments,
                    &intent.project,
                    Duration::from_millis(request.definition.timeout_ms),
                    containment,
                )
                .await
                {
                    Ok(output) if output.succeeded() => GateOutcome::Passed,
                    Ok(_) => GateOutcome::Failed,
                    Err(SpawnError::Timeout { .. }) => GateOutcome::TimedOut,
                    Err(_) => GateOutcome::LaunchFailed,
                }
            }
            Err(_) => GateOutcome::LaunchFailed,
        };
        results.push(GateResult {
            request: request.clone(),
            outcome,
            duration_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        });
    }
    results
}

fn verification_receipt_id(intent: &VerificationIntent, results: &[GateResult]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"runtrol.dev/capability-verification-receipt/v1alpha1");
    hasher.update(intent.capability_id.as_bytes());
    hasher.update(intent.version_sha256);
    hasher.update(intent.candidate.manifest.source_receipt_id.as_bytes());
    hasher.update(intent.candidate.verification.author_run_id.as_bytes());
    hasher.update(intent.candidate.verification.verifier_run_id.as_bytes());
    for result in results {
        hasher.update(result.request.definition.id.as_bytes());
        hasher.update(result.request.definition_sha256);
        hasher.update([gate_outcome_byte(result.outcome)]);
    }
    format!("rcp_{}", digest_text(&hasher.finalize().into()))
}

const fn gate_outcome(outcome: GateOutcome) -> &'static str {
    match outcome {
        GateOutcome::Passed => "passed",
        GateOutcome::Failed => "failed",
        GateOutcome::TimedOut => "timedOut",
        GateOutcome::LaunchFailed => "launchFailed",
        GateOutcome::Cancelled => "cancelled",
    }
}

const fn gate_outcome_byte(outcome: GateOutcome) -> u8 {
    match outcome {
        GateOutcome::Passed => 1,
        GateOutcome::Failed => 2,
        GateOutcome::TimedOut => 3,
        GateOutcome::LaunchFailed => 4,
        GateOutcome::Cancelled => 5,
    }
}

fn line(record: &CandidateRecord) -> CapabilityLine {
    CapabilityLine {
        project: record.project_id.clone(),
        capability_id: record.capability_id.clone(),
        kind: match record.kind {
            CapabilityKind::Skill => "skill".into(),
            CapabilityKind::GateRecipe => "gateRecipe".into(),
            CapabilityKind::Playbook => "playbook".into(),
        },
        state: state(record.state).into(),
        version_sha256: digest_text(&record.version_sha256).into(),
        source_ref: record.source_ref.clone(),
        source_receipt_id: record.source_receipt_id.clone(),
        verification_receipt_id: record
            .verification
            .as_ref()
            .and_then(|verification| verification.receipt_id.clone()),
        verification_gates: record
            .verification
            .as_ref()
            .map_or_else(Vec::new, |verification| {
                verification
                    .gates
                    .iter()
                    .map(|gate| CapabilityGateLine {
                        gate_id: gate.gate_id.clone(),
                        definition_sha256: digest_text(&gate.definition_sha256).into(),
                        outcome: gate.outcome.clone(),
                        duration_ms: gate.duration_ms,
                    })
                    .collect()
            }),
        active_version_sha256: record
            .active_version_sha256
            .map(|digest| digest_text(&digest).into()),
        approved_versions: record
            .approved_versions
            .iter()
            .map(|version| digest_text(&version.version_sha256).into())
            .collect(),
    }
}

const fn state(state: CandidateState) -> &'static str {
    match state {
        CandidateState::Proposed => "proposed",
        CandidateState::Candidate => "candidate",
        CandidateState::Verifying => "verifying",
        CandidateState::Verified => "verified",
        CandidateState::Active => "active",
        CandidateState::Tampered => "tampered",
        CandidateState::Quarantined => "quarantined",
        CandidateState::Stale => "stale",
        CandidateState::RolledBack => "rolledBack",
        CandidateState::Rejected => "rejected",
        CandidateState::Archived => "archived",
    }
}

fn load(path: &AbsPath) -> Result<TrustIndex, String> {
    let metadata = match std::fs::symlink_metadata(path.as_std_path()) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(TrustIndex::current());
        }
        Err(error) => return Err(error.to_string()),
    };
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.len() > MAX_TRUST_FILE_BYTES
    {
        return Err("the capability trust index is not a bounded regular file".to_owned());
    }
    let index: TrustIndex = serde_json::from_slice(
        &std::fs::read(path.as_std_path()).map_err(|error| error.to_string())?,
    )
    .map_err(|_| "the capability trust index is malformed".to_owned())?;
    if index.schema != 1 {
        return Err("the capability trust index schema is unsupported".to_owned());
    }
    Ok(index)
}

fn save(path: &AbsPath, index: &TrustIndex) -> Result<(), String> {
    let bytes = serde_json::to_vec(index).map_err(|error| error.to_string())?;
    if u64::try_from(bytes.len()).map_or(true, |size| size > MAX_TRUST_FILE_BYTES) {
        return Err("the capability trust index exceeds its byte bound".to_owned());
    }
    let temporary = path
        .parent()
        .ok_or_else(|| "the capability trust index has no parent".to_owned())?
        .join("capability-trust.json.writing")
        .map_err(|error| error.to_string())?;
    let mut file =
        std::fs::File::create(temporary.as_std_path()).map_err(|error| error.to_string())?;
    file.write_all(&bytes).map_err(|error| error.to_string())?;
    file.sync_all().map_err(|error| error.to_string())?;
    drop(file);
    std::fs::rename(temporary.as_std_path(), path.as_std_path()).map_err(|error| error.to_string())
}

fn parse_digest(value: &str) -> Result<[u8; 32], &'static str> {
    if value.len() != 64
        || value
            .bytes()
            .any(|byte| !byte.is_ascii_hexdigit() || byte.is_ascii_uppercase())
    {
        return Err("the capability version digest is invalid");
    }
    let mut digest = [0_u8; 32];
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        let pair = core::str::from_utf8(pair).map_err(|_| "the capability digest is invalid")?;
        let byte = digest
            .get_mut(index)
            .ok_or("the capability digest is too long")?;
        *byte = u8::from_str_radix(pair, 16).map_err(|_| "the capability digest is invalid")?;
    }
    Ok(digest)
}

fn failed(message: &'static str) -> Response {
    Response::Failed(runtrol_ipc::wire::WireError {
        message: message.into(),
        retryable: false,
        needs_the_operator: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtrol_ledger::RunId;
    use runtrol_orchestrator::{GateDefinition, GateRegistry, WorkingDirectoryRule};
    use runtrol_security::LocalScope;

    struct Scratch {
        root: std::path::PathBuf,
        project: AbsPath,
    }

    impl Scratch {
        fn make() -> Self {
            let root = std::env::temp_dir().join(format!(
                "runtrol-growth-controller-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            if root.exists() {
                std::fs::remove_dir_all(&root).expect("clear Growth fixture");
            }
            std::fs::create_dir_all(root.join("instructions")).expect("instruction parent");
            std::fs::create_dir_all(root.join("fixtures")).expect("fixture parent");
            std::fs::write(
                root.join("instructions/replay.md"),
                b"replay exact fixture\n",
            )
            .expect("replay instruction");
            std::fs::write(root.join("fixtures/input.txt"), b"fixed input\n").expect("fixed input");
            let project =
                AbsPath::canonicalize(root.to_str().expect("UTF-8 fixture")).expect("project");
            Self { root, project }
        }

        fn write_candidate(&self, name: &str, body: &[u8], parent: Option<&str>) -> [u8; 32] {
            let relative = format!(".runtrol/capabilities/candidates/{name}");
            let path = self.root.join(&relative);
            std::fs::create_dir_all(&path).expect("candidate parent");
            std::fs::write(path.join("SKILL.md"), body).expect("Skill body");
            let content_sha256 = payload_digest(body);
            let parent = parent.map_or_else(String::new, |digest| {
                format!("parent_version = \"{digest}\"\n")
            });
            let manifest = format!(
                concat!(
                    "schema = \"runtrol.dev/capability/v1alpha1\"\n",
                    "capability_id = \"reviewed-skill\"\n",
                    "kind = \"skill\"\n",
                    "scope = \"project\"\n",
                    "content_sha256 = \"{}\"\n",
                    "source_mission_id = \"msn_fixture\"\n",
                    "source_task_id = \"tsk_fixture\"\n",
                    "source_run_id = \"run_author\"\n",
                    "source_receipt_id = \"rcp_source\"\n",
                    "policy_sha256 = \"2222222222222222222222222222222222222222222222222222222222222222\"\n",
                    "{}",
                    "license = \"MIT\"\n"
                ),
                digest_text(&content_sha256),
                parent,
            );
            std::fs::write(path.join("capability.toml"), manifest).expect("manifest");
            std::fs::write(
                path.join("verify.toml"),
                concat!(
                    "schema = \"runtrol.dev/capability-verification/v1alpha1\"\n",
                    "author_run_id = \"run_author\"\n",
                    "verifier_run_id = \"run_verifier\"\n",
                    "replay_instruction_ref = \"instructions/replay.md\"\n",
                    "fixture_ref = \"fixtures/input.txt\"\n",
                    "gate_refs = [\"cap-check\"]\n"
                ),
            )
            .expect("verification plan");
            inspect(&self.project, &relative)
                .expect("inspect candidate")
                .version_sha256
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ignored = std::fs::remove_dir_all(&self.root);
        }
    }

    fn payload_digest(body: &[u8]) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(8_u64.to_be_bytes());
        hasher.update(b"SKILL.md");
        hasher.update(Sha256::digest(body));
        hasher.update(
            u64::try_from(body.len())
                .expect("bounded fixture")
                .to_be_bytes(),
        );
        hasher.finalize().into()
    }

    fn gate() -> GateRequest {
        let mut registry = GateRegistry::default();
        registry
            .register(
                LocalScope::GateRegister,
                GateDefinition {
                    id: "cap-check".into(),
                    program: "fixture".into(),
                    arguments: Vec::new(),
                    working_directory: WorkingDirectoryRule::TaskWorktree,
                    timeout_ms: 1_000,
                    platforms: vec![std::env::consts::OS.into()],
                },
            )
            .expect("register Gate");
        registry
            .request("cap-check", RunId::now())
            .expect("Gate request")
    }

    fn propose(controller: &mut GrowthController, scratch: &Scratch, name: &str) -> Response {
        controller.answer(&Request::CapabilityPropose {
            project: scratch.project.as_str().into(),
            candidate_ref: format!(".runtrol/capabilities/candidates/{name}").into(),
        })
    }

    fn verify_and_approve(controller: &mut GrowthController, scratch: &Scratch, version: [u8; 32]) {
        let version_text = digest_text(&version);
        let intent = controller
            .verification_intent(
                scratch.project.as_str(),
                "reviewed-skill",
                &version_text,
                vec![gate()],
            )
            .expect("verification intent");
        let request = intent.gates.first().expect("Gate").clone();
        let Response::Capabilities(verified) = controller
            .commit_verification(
                &intent,
                &[GateResult {
                    request,
                    outcome: GateOutcome::Passed,
                    duration_ms: 7,
                }],
            )
            .expect("commit verification")
        else {
            panic!("verified capability list");
        };
        assert!(
            verified
                .first()
                .and_then(|line| line.verification_receipt_id.as_deref())
                .is_some_and(|receipt| receipt.starts_with("rcp_"))
        );
        assert!(matches!(
            controller.answer(&Request::CapabilityApprove {
                project: scratch.project.as_str().into(),
                capability_id: "reviewed-skill".into(),
                version_sha256: version_text.into(),
            }),
            Response::Capabilities(_)
        ));
    }

    #[test]
    fn update_preserves_active_version_then_tamper_disables_reuse_and_rollback_restores() {
        let scratch = Scratch::make();
        let trust_path = scratch.project.join("trust.json").expect("trust path");
        let mut controller = GrowthController::open(trust_path).expect("controller");

        let first = scratch.write_candidate("first", b"# First\n", None);
        assert!(matches!(
            propose(&mut controller, &scratch, "first"),
            Response::Capabilities(_)
        ));
        verify_and_approve(&mut controller, &scratch, first);

        let second = scratch.write_candidate("second", b"# Second\n", Some(&digest_text(&first)));
        assert!(matches!(
            propose(&mut controller, &scratch, "second"),
            Response::Capabilities(_)
        ));
        assert_eq!(
            controller
                .approved_capabilities(scratch.project.as_str())
                .expect("old active remains")
                .first()
                .expect("selection")
                .version_sha256
                .as_ref(),
            digest_text(&first)
        );
        verify_and_approve(&mut controller, &scratch, second);

        let active = scratch
            .root
            .join(".runtrol/capabilities/active/reviewed-skill/SKILL.md");
        std::fs::write(&active, b"tampered\n").expect("tamper active bytes");
        assert!(
            controller
                .approved_capabilities(scratch.project.as_str())
                .expect("refresh tamper")
                .is_empty()
        );
        assert!(matches!(
            controller.answer(&Request::CapabilityRollback {
                project: scratch.project.as_str().into(),
                capability_id: "reviewed-skill".into(),
                version_sha256: digest_text(&first).into(),
            }),
            Response::Capabilities(_)
        ));
        let selected = controller
            .approved_capabilities(scratch.project.as_str())
            .expect("restored selection");
        assert_eq!(selected.len(), 1);
        assert_eq!(
            selected
                .first()
                .expect("restored capability")
                .version_sha256
                .as_ref(),
            digest_text(&first)
        );
        assert_eq!(std::fs::read(active).expect("restored bytes"), b"# First\n");
    }
}
