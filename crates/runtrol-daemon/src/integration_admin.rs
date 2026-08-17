//! Local-only listing, physical-presence approval, denial, and revocation for public integrations.

use std::collections::{BTreeMap, BTreeSet};

use runtrol_ipc::wire::{
    IntegrationEnrollmentLine, IntegrationLine, RuntimeForgetLine, RuntimeKeyRotationLine,
};
use runtrol_provider::{AbsPath, WallMs};
use runtrol_runtime_protocol::{
    AppScope, IDEMPOTENCY_WINDOW_MS, IntegrationId, MUTATION_CLOCK_SKEW_MS, MutationRequestId,
    PendingEnrollmentId,
};
use runtrol_security::{
    DenyList, GrantRequest, IntegrationProposal, LocalConsole, PresenceChallenge, WorkspaceRoot,
};
use runtrol_store::{
    EnrollmentKey, EnrollmentState, IntegrationAuditOutcome, IntegrationGrantChange,
    IntegrationKey, IntegrationMutationKey, IntegrationRootRow, IntegrationRow,
};
use tokio::sync::Mutex;

use crate::Composed;
use crate::runtime_auth::{enrollment_key, integration_id, integration_key, pending_id};

const MAX_APPROVAL_CHALLENGES: usize = 16;
const APPROVAL_CHALLENGE_MS: u64 = runtrol_security::presence::CHALLENGE_WINDOW_MS;
const MAX_FORGET_CONFIRMATIONS: usize = 64;
const FORGET_CONFIRMATION_MS: u64 = 10 * 60_000;
const MAX_KEY_ROTATION_CONFIRMATIONS: usize = 64;
const KEY_ROTATION_CONFIRMATION_MS: u64 = 10 * 60_000;

/// One local approval challenge returned to the VS Code surface.
pub(crate) struct ApprovalChallenge {
    pub(crate) challenge_id: Box<str>,
    pub(crate) prompt: Box<str>,
}

struct PendingApproval {
    enrollment: EnrollmentKey,
    request: GrantRequest,
    challenge: PresenceChallenge,
    expires_at: WallMs,
}

struct PendingForgetConfirmation {
    integration: IntegrationKey,
    session: runtrol_provider::SessionId,
    expected_session_generation: u64,
    confirmation_id: Box<str>,
    expires_at: WallMs,
    confirmed: bool,
}

struct PendingKeyRotationConfirmation {
    integration: IntegrationKey,
    expected_key_generation: u64,
    new_public_key: [u8; 32],
    confirmation_id: Box<str>,
    expires_at: WallMs,
    confirmed: bool,
}

/// Whether one exact public forget request still needs a local approval action.
#[derive(Clone)]
pub(crate) enum ForgetConfirmation {
    Awaiting { confirmation_id: Box<str> },
    Confirmed,
}

/// Closed admission failures for the bounded local forget confirmation table.
#[derive(Clone, Copy)]
pub(crate) enum ForgetConfirmationError {
    InvalidRequestId,
    IdempotencyConflict,
    ResourceExhausted,
    StateUnavailable,
}

/// Whether one exact public key rotation still needs a local approval action.
#[derive(Clone)]
pub(crate) enum KeyRotationConfirmation {
    Awaiting { confirmation_id: Box<str> },
    Confirmed,
}

/// Closed admission failures for the bounded local key rotation confirmation table.
#[derive(Clone, Copy)]
pub(crate) enum KeyRotationConfirmationError {
    InvalidRequestId,
    IdempotencyConflict,
    ResourceExhausted,
    StateUnavailable,
}

/// Bounded one-use local integration approval state.
#[derive(Default)]
pub(crate) struct IntegrationAdmin {
    challenges: Mutex<BTreeMap<[u8; 16], PendingApproval>>,
    forget_confirmations: Mutex<BTreeMap<IntegrationMutationKey, PendingForgetConfirmation>>,
    key_rotation_confirmations:
        Mutex<BTreeMap<IntegrationMutationKey, PendingKeyRotationConfirmation>>,
}

impl IntegrationAdmin {
    pub(crate) async fn existing_forget_confirmation(
        &self,
        integration: IntegrationKey,
        request_id: &MutationRequestId,
        session: runtrol_provider::SessionId,
        expected_session_generation: u64,
    ) -> Result<Option<ForgetConfirmation>, ForgetConfirmationError> {
        let (key, now) = forget_key(integration, request_id)?;
        let mut confirmations = self.forget_confirmations.lock().await;
        confirmations.retain(|_, pending| pending.expires_at >= now);
        let Some(pending) = confirmations.get(&key) else {
            return Ok(None);
        };
        if pending.session != session
            || pending.expected_session_generation != expected_session_generation
        {
            return Err(ForgetConfirmationError::IdempotencyConflict);
        }
        Ok(Some(if pending.confirmed {
            ForgetConfirmation::Confirmed
        } else {
            ForgetConfirmation::Awaiting {
                confirmation_id: pending.confirmation_id.clone(),
            }
        }))
    }

    pub(crate) async fn request_forget_confirmation(
        &self,
        integration: IntegrationKey,
        request_id: &MutationRequestId,
        session: runtrol_provider::SessionId,
        expected_session_generation: u64,
    ) -> Result<ForgetConfirmation, ForgetConfirmationError> {
        let (key, now) = forget_key(integration, request_id)?;
        let mut confirmations = self.forget_confirmations.lock().await;
        confirmations.retain(|_, pending| pending.expires_at >= now);
        if let Some(pending) = confirmations.get(&key) {
            if pending.session != session
                || pending.expected_session_generation != expected_session_generation
            {
                return Err(ForgetConfirmationError::IdempotencyConflict);
            }
            return Ok(if pending.confirmed {
                ForgetConfirmation::Confirmed
            } else {
                ForgetConfirmation::Awaiting {
                    confirmation_id: pending.confirmation_id.clone(),
                }
            });
        }
        if confirmations.len() >= MAX_FORGET_CONFIRMATIONS {
            return Err(ForgetConfirmationError::ResourceExhausted);
        }
        let confirmation_id = format!(
            "fgt_{}",
            hex(&random_key().map_err(|_| { ForgetConfirmationError::StateUnavailable })?)
        )
        .into_boxed_str();
        confirmations.insert(
            key,
            PendingForgetConfirmation {
                integration,
                session,
                expected_session_generation,
                confirmation_id: confirmation_id.clone(),
                expires_at: now.plus_millis(FORGET_CONFIRMATION_MS),
                confirmed: false,
            },
        );
        Ok(ForgetConfirmation::Awaiting { confirmation_id })
    }

    pub(crate) async fn forget_requests(
        &self,
        composed: &Composed,
    ) -> Result<Vec<RuntimeForgetLine>, AdminError> {
        let now = WallMs::now();
        let mut confirmations = self.forget_confirmations.lock().await;
        confirmations.retain(|_, pending| pending.expires_at >= now);
        confirmations
            .values()
            .filter(|pending| !pending.confirmed)
            .map(|pending| {
                let integration = composed
                    .store
                    .get_integration(pending.integration)
                    .map_err(|_| AdminError::state())?
                    .ok_or_else(AdminError::state)?;
                Ok(RuntimeForgetLine {
                    confirmation_id: pending.confirmation_id.clone(),
                    integration_id: integration_id(pending.integration).to_string().into(),
                    integration_label: integration.label,
                    session_id: pending.session.to_string().into(),
                    expires_at_ms: pending.expires_at.as_millis(),
                })
            })
            .collect()
    }

    pub(crate) async fn confirm_forget(&self, confirmation_id: &str) -> Result<(), AdminError> {
        let now = WallMs::now();
        let mut confirmations = self.forget_confirmations.lock().await;
        confirmations.retain(|_, pending| pending.expires_at >= now);
        let pending = confirmations
            .values_mut()
            .find(|pending| pending.confirmation_id.as_ref() == confirmation_id)
            .ok_or_else(|| AdminError::invalid("the Runtime forget confirmation does not exist"))?;
        pending.confirmed = true;
        Ok(())
    }

    pub(crate) async fn request_key_rotation_confirmation(
        &self,
        integration: IntegrationKey,
        request_id: &MutationRequestId,
        expected_key_generation: u64,
        new_public_key: [u8; 32],
    ) -> Result<KeyRotationConfirmation, KeyRotationConfirmationError> {
        let (key, now) = key_rotation_key(integration, request_id)?;
        let mut confirmations = self.key_rotation_confirmations.lock().await;
        confirmations.retain(|_, pending| pending.expires_at >= now);
        if let Some(pending) = confirmations.get(&key) {
            if pending.expected_key_generation != expected_key_generation
                || pending.new_public_key != new_public_key
            {
                return Err(KeyRotationConfirmationError::IdempotencyConflict);
            }
            return Ok(if pending.confirmed {
                KeyRotationConfirmation::Confirmed
            } else {
                KeyRotationConfirmation::Awaiting {
                    confirmation_id: pending.confirmation_id.clone(),
                }
            });
        }
        if confirmations.len() >= MAX_KEY_ROTATION_CONFIRMATIONS {
            return Err(KeyRotationConfirmationError::ResourceExhausted);
        }
        let confirmation_id = format!(
            "rot_{}",
            hex(&random_key().map_err(|_| KeyRotationConfirmationError::StateUnavailable)?)
        )
        .into_boxed_str();
        confirmations.insert(
            key,
            PendingKeyRotationConfirmation {
                integration,
                expected_key_generation,
                new_public_key,
                confirmation_id: confirmation_id.clone(),
                expires_at: now.plus_millis(KEY_ROTATION_CONFIRMATION_MS),
                confirmed: false,
            },
        );
        Ok(KeyRotationConfirmation::Awaiting { confirmation_id })
    }

    pub(crate) async fn key_rotation_requests(
        &self,
        composed: &Composed,
    ) -> Result<Vec<RuntimeKeyRotationLine>, AdminError> {
        let now = WallMs::now();
        let mut confirmations = self.key_rotation_confirmations.lock().await;
        confirmations.retain(|_, pending| pending.expires_at >= now);
        confirmations
            .values()
            .filter(|pending| !pending.confirmed)
            .map(|pending| {
                let integration = composed
                    .store
                    .get_integration(pending.integration)
                    .map_err(|_| AdminError::state())?
                    .ok_or_else(AdminError::state)?;
                Ok(RuntimeKeyRotationLine {
                    confirmation_id: pending.confirmation_id.clone(),
                    integration_id: integration_id(pending.integration).to_string().into(),
                    integration_label: integration.label,
                    current_key_generation: pending.expected_key_generation,
                    new_key_fingerprint: fingerprint(&pending.new_public_key).into(),
                    expires_at_ms: pending.expires_at.as_millis(),
                })
            })
            .collect()
    }

    pub(crate) async fn confirm_key_rotation(
        &self,
        confirmation_id: &str,
    ) -> Result<(), AdminError> {
        let now = WallMs::now();
        let mut confirmations = self.key_rotation_confirmations.lock().await;
        confirmations.retain(|_, pending| pending.expires_at >= now);
        let pending = confirmations
            .values_mut()
            .find(|pending| pending.confirmation_id.as_ref() == confirmation_id)
            .ok_or_else(|| {
                AdminError::invalid("the Runtime key rotation confirmation does not exist")
            })?;
        pending.confirmed = true;
        Ok(())
    }

    pub(crate) fn enrollments(
        composed: &Composed,
    ) -> Result<Vec<IntegrationEnrollmentLine>, AdminError> {
        let now = WallMs::now();
        composed
            .store
            .purge_expired_enrollments(now)
            .map_err(|_| AdminError::state())?;
        let rows = composed
            .store
            .list_enrollments()
            .map_err(|_| AdminError::state())?;
        Ok(rows
            .into_iter()
            .filter(|(_, row)| row.state == EnrollmentState::Pending && row.expires_at >= now)
            .map(|(key, row)| IntegrationEnrollmentLine {
                pending_id: pending_id(key).to_string().into(),
                client_name: row.client_name,
                client_version: row.client_version,
                client_instance_id: row.client_instance_id,
                key_fingerprint: fingerprint(&row.public_key).into(),
                scopes: row.scopes,
                roots: row.roots,
                expires_at_ms: row.expires_at.as_millis(),
            })
            .collect())
    }

    pub(crate) fn integrations(composed: &Composed) -> Result<Vec<IntegrationLine>, AdminError> {
        let rows = composed
            .store
            .list_integrations()
            .map_err(|_| AdminError::state())?;
        Ok(rows
            .into_iter()
            .map(|(key, row)| IntegrationLine {
                integration_id: integration_id(key).to_string().into(),
                label: row.label,
                client_instance_id: row.client_instance_id,
                scopes: row.scopes,
                available_scopes: AppScope::ALL
                    .iter()
                    .map(|scope| Box::<str>::from(scope.as_str()))
                    .collect(),
                roots: row.roots.into_iter().map(|root| root.path).collect(),
                grant_generation: row.grant_generation,
                revoked: row.revoked_at.is_some(),
            })
            .collect())
    }

    pub(crate) async fn begin(
        &self,
        composed: &Composed,
        pending: &str,
        scopes: &[Box<str>],
        roots: &[Box<str>],
    ) -> Result<ApprovalChallenge, AdminError> {
        let enrollment = parse_enrollment(pending)?;
        let now = WallMs::now();
        let row = composed
            .store
            .get_enrollment(enrollment)
            .map_err(|_| AdminError::state())?
            .ok_or_else(|| AdminError::invalid("the pending enrollment does not exist"))?;
        if row.state != EnrollmentState::Pending || row.expires_at < now {
            return Err(AdminError::invalid("the enrollment is terminal or expired"));
        }
        validate_narrowing(scopes, &row.scopes, "scope")?;
        validate_narrowing(roots, &row.roots, "root")?;
        if scopes.iter().any(|scope| scope.starts_with("session.")) && roots.is_empty() {
            return Err(AdminError::invalid(
                "session authority requires at least one approved project root",
            ));
        }
        let deny = deny_list(composed)?;
        let canonical_roots = approve_roots(roots, &deny)?
            .into_iter()
            .map(|root| root.path().clone())
            .collect();
        let proposal = IntegrationProposal::new(
            enrollment.to_bytes(),
            row.public_key,
            row.manifest_digest,
            &row.client_name,
            &row.client_version,
            &row.client_instance_id,
            scopes.to_vec(),
            canonical_roots,
        )
        .map_err(|_| AdminError::invalid("the integration proposal is unsafe for local display"))?;
        let request = GrantRequest::ApproveIntegration { proposal };
        let challenge = {
            let console = LocalConsole::claim().ok_or_else(|| {
                AdminError::unavailable("the local approval surface is already in use")
            })?;
            PresenceChallenge::issue(&console, request.clone())
                .map_err(|_| AdminError::unavailable("a local challenge could not be generated"))?
        };
        let prompt = challenge.prompt().into();
        let mut challenges = self.challenges.lock().await;
        challenges.retain(|_, pending| pending.expires_at >= now);
        if challenges.len() >= MAX_APPROVAL_CHALLENGES {
            return Err(AdminError::unavailable(
                "too many local integration approvals are awaiting an answer",
            ));
        }
        let mut chosen = None;
        for _ in 0..4 {
            let key = random_key()?;
            if !challenges.contains_key(&key) {
                chosen = Some(key);
                break;
            }
        }
        let Some(key) = chosen else {
            return Err(AdminError::state());
        };
        let previous = challenges.insert(
            key,
            PendingApproval {
                enrollment,
                request,
                challenge,
                expires_at: now.plus_millis(APPROVAL_CHALLENGE_MS),
            },
        );
        if previous.is_some() {
            return Err(AdminError::state());
        }
        Ok(ApprovalChallenge {
            challenge_id: format!("apc_{}", hex(&key)).into(),
            prompt,
        })
    }

    pub(crate) async fn finish(
        &self,
        composed: &Composed,
        challenge_id: &str,
        answer: &str,
    ) -> Result<IntegrationId, AdminError> {
        let challenge_key = parse_opaque(challenge_id, "apc_")?;
        let pending = self
            .challenges
            .lock()
            .await
            .remove(&challenge_key)
            .ok_or_else(|| AdminError::invalid("the local approval challenge does not exist"))?;
        let witness = pending
            .challenge
            .answer(answer)
            .map_err(|_| AdminError::invalid("the local approval phrase was wrong or expired"))?;
        witness.check(&pending.request).map_err(|_| {
            AdminError::invalid("the local approval witness is stale or mismatched")
        })?;
        let GrantRequest::ApproveIntegration { proposal } = pending.request else {
            return Err(AdminError::state());
        };
        if proposal.pending_id() != pending.enrollment.to_bytes() {
            return Err(AdminError::state());
        }
        let now = WallMs::now();
        let row = composed
            .store
            .get_enrollment(pending.enrollment)
            .map_err(|_| AdminError::state())?
            .ok_or_else(|| AdminError::invalid("the pending enrollment no longer exists"))?;
        if row.state != EnrollmentState::Pending
            || row.expires_at < now
            || row.public_key != proposal.public_key()
            || row.manifest_digest != proposal.manifest_digest()
            || row.client_instance_id.as_ref() != proposal.client_instance_id()
        {
            return Err(AdminError::invalid(
                "the pending enrollment changed or expired before approval",
            ));
        }
        validate_narrowing(proposal.scopes(), &row.scopes, "scope")?;
        let deny = deny_list(composed)?;
        let current_roots = approve_roots(&row.roots, &deny)?;
        if !proposal
            .roots()
            .iter()
            .all(|selected| current_roots.iter().any(|root| root.path() == selected))
        {
            return Err(AdminError::invalid(
                "a selected project root no longer matches the pending proposal",
            ));
        }
        let approved_roots = proposal
            .roots()
            .iter()
            .map(|selected| {
                let root = current_roots
                    .iter()
                    .find(|root| root.path() == selected)
                    .ok_or_else(AdminError::state)?;
                Ok(IntegrationRootRow {
                    path: root.path().as_str().into(),
                    identity: root.identity().to_bytes(),
                })
            })
            .collect::<Result<Vec<_>, AdminError>>()?;
        let grant = IntegrationRow {
            public_key: row.public_key,
            client_instance_id: row.client_instance_id,
            label: proposal.client_name().into(),
            manifest_digest: row.manifest_digest,
            scopes: proposal.scopes().to_vec(),
            roots: approved_roots,
            key_generation: 1,
            grant_generation: 1,
            approved_at: now,
            revoked_at: None,
        };
        commit_approval(composed, pending.enrollment, &grant, "integrations/approve")
    }

    /// Approve one pending enrollment for the key that requested it, without a typed phrase.
    ///
    /// Reaching this method already means reaching the owner-only private endpoint, which is what the phrase
    /// otherwise stands in for. The phrase cannot add anything against a caller that is already there: the local
    /// challenge prompt carries its own answer, so any program with this reach can read and return it. What the
    /// phrase does not establish is which enrollment the caller is, and the signature here does establish exactly
    /// that. The grant is the enrollment as requested; narrowing stays a reviewed decision through `begin`.
    pub(crate) fn self_approve(
        composed: &Composed,
        pending: &str,
        signature: &str,
    ) -> Result<IntegrationId, AdminError> {
        let enrollment = parse_enrollment(pending)?;
        let now = WallMs::now();
        let row = composed
            .store
            .get_enrollment(enrollment)
            .map_err(|_| AdminError::state())?
            .ok_or_else(|| AdminError::invalid("the pending enrollment does not exist"))?;
        if row.state != EnrollmentState::Pending || row.expires_at < now {
            return Err(AdminError::invalid("the enrollment is terminal or expired"));
        }
        crate::runtime_auth::verify_self_approval(
            &row.public_key,
            signature,
            &pending_id(enrollment),
        )
        .map_err(|_| {
            AdminError::invalid("the self-approval proof does not match the enrolling key")
        })?;
        // The same rule `begin` and `change_grant` enforce. Authority over sessions or approvals with no root to
        // exercise it in is not a smaller grant, it is a broken one: every later call fails root-denied and
        // nothing repairs it, because the grant itself is valid and the client has no reason to re-enroll.
        if row
            .scopes
            .iter()
            .any(|scope| scope.starts_with("session.") || scope.starts_with("approval."))
            && row.roots.is_empty()
        {
            return Err(AdminError::invalid(
                "session authority requires at least one approved project root",
            ));
        }
        let deny = deny_list(composed)?;
        let approved_roots = approve_roots(&row.roots, &deny)?
            .iter()
            .map(|root| IntegrationRootRow {
                path: root.path().as_str().into(),
                identity: root.identity().to_bytes(),
            })
            .collect::<Vec<_>>();
        let grant = IntegrationRow {
            public_key: row.public_key,
            client_instance_id: row.client_instance_id,
            label: row.client_name,
            manifest_digest: row.manifest_digest,
            scopes: row.scopes,
            roots: approved_roots,
            key_generation: 1,
            grant_generation: 1,
            approved_at: now,
            revoked_at: None,
        };
        commit_approval(composed, enrollment, &grant, "integrations/selfApprove")
    }

    pub(crate) fn deny(composed: &Composed, pending: &str) -> Result<(), AdminError> {
        let key = parse_enrollment(pending)?;
        let row = composed
            .store
            .get_enrollment(key)
            .map_err(|_| AdminError::state())?
            .ok_or_else(|| AdminError::invalid("the pending enrollment does not exist"))?;
        if row.expires_at < WallMs::now() || row.state != EnrollmentState::Pending {
            return Err(AdminError::invalid("the enrollment is terminal or expired"));
        }
        crate::runtime_audit::local(
            &composed.store,
            None,
            None,
            "integrations/deny",
            IntegrationAuditOutcome::Attempted,
            "attempted",
        )
        .map_err(|_| AdminError::state())?;
        composed
            .store
            .deny_enrollment(key)
            .map_err(|_| AdminError::state())?;
        crate::runtime_audit::local(
            &composed.store,
            None,
            None,
            "integrations/deny",
            IntegrationAuditOutcome::Allowed,
            "allowed",
        )
        .map_err(|_| AdminError::state())
    }

    pub(crate) fn revoke(composed: &Composed, integration: &str) -> Result<(), AdminError> {
        let id = IntegrationId::new(integration);
        let key = integration_key(&id)
            .map_err(|_| AdminError::invalid("the integration identity is malformed"))?;
        let row = composed
            .store
            .get_integration(key)
            .map_err(|_| AdminError::state())?;
        let key_generation = row.as_ref().map(|row| row.key_generation);
        crate::runtime_audit::local(
            &composed.store,
            row.as_ref().map(|_| key),
            key_generation,
            "integrations/revoke",
            IntegrationAuditOutcome::Attempted,
            "attempted",
        )
        .map_err(|_| AdminError::state())?;
        if composed
            .store
            .revoke_integration(key, WallMs::now())
            .map_err(|_| AdminError::state())?
        {
            crate::runtime_audit::local(
                &composed.store,
                Some(key),
                key_generation,
                "integrations/revoke",
                IntegrationAuditOutcome::Allowed,
                "allowed",
            )
            .map_err(|_| AdminError::state())
        } else {
            crate::runtime_audit::local(
                &composed.store,
                None,
                None,
                "integrations/revoke",
                IntegrationAuditOutcome::Denied,
                "integrationNotFound",
            )
            .map_err(|_| AdminError::state())?;
            Err(AdminError::invalid("the integration does not exist"))
        }
    }

    pub(crate) fn change_grant(
        composed: &Composed,
        integration: &str,
        expected_grant_generation: u64,
        scopes: &[Box<str>],
        roots: &[Box<str>],
    ) -> Result<(), AdminError> {
        let id = IntegrationId::new(integration);
        let key = integration_key(&id)
            .map_err(|_| AdminError::invalid("the integration identity is malformed"))?;
        let row = composed
            .store
            .get_integration(key)
            .map_err(|_| AdminError::state())?
            .ok_or_else(|| AdminError::invalid("the integration does not exist"))?;
        if row.revoked_at.is_some() {
            return Err(AdminError::invalid("the integration is revoked"));
        }
        if row.grant_generation != expected_grant_generation {
            return Err(AdminError::invalid(
                "the integration grant changed before local review completed",
            ));
        }
        let mut unique_scopes = BTreeSet::new();
        if scopes.is_empty()
            || scopes.len() > AppScope::ALL.len()
            || !scopes.iter().all(|scope| {
                scope.parse::<AppScope>().is_ok() && unique_scopes.insert(scope.as_ref())
            })
        {
            return Err(AdminError::invalid(
                "the replacement integration scopes are invalid or duplicated",
            ));
        }
        if scopes
            .iter()
            .any(|scope| scope.starts_with("session.") || scope.starts_with("approval."))
            && roots.is_empty()
        {
            return Err(AdminError::invalid(
                "session authority requires at least one approved project root",
            ));
        }
        let deny = deny_list(composed)?;
        let approved = approve_roots(roots, &deny)?;
        let mut unique_roots = BTreeSet::new();
        if !approved
            .iter()
            .all(|root| unique_roots.insert(root.path().as_str()))
        {
            return Err(AdminError::invalid(
                "the replacement project roots contain the same directory more than once",
            ));
        }
        let roots = approved
            .into_iter()
            .map(|root| IntegrationRootRow {
                path: root.path().as_str().into(),
                identity: root.identity().to_bytes(),
            })
            .collect();
        crate::runtime_audit::local(
            &composed.store,
            Some(key),
            Some(row.key_generation),
            "integrations/changeGrant",
            IntegrationAuditOutcome::Attempted,
            "attempted",
        )
        .map_err(|_| AdminError::state())?;
        match composed
            .store
            .change_integration_grant(key, expected_grant_generation, scopes.to_vec(), roots)
            .map_err(|_| AdminError::state())?
        {
            IntegrationGrantChange::Changed(_) | IntegrationGrantChange::Unchanged(_) => {
                crate::runtime_audit::local(
                    &composed.store,
                    Some(key),
                    Some(row.key_generation),
                    "integrations/changeGrant",
                    IntegrationAuditOutcome::Allowed,
                    "allowed",
                )
                .map_err(|_| AdminError::state())
            }
            IntegrationGrantChange::Conflict => Err(AdminError::invalid(
                "the integration grant changed before it could be committed",
            )),
            IntegrationGrantChange::Missing => {
                Err(AdminError::invalid("the integration no longer exists"))
            }
            IntegrationGrantChange::Revoked => {
                Err(AdminError::invalid("the integration was revoked"))
            }
        }
    }
}

fn validate_narrowing(
    selected: &[Box<str>],
    requested: &[Box<str>],
    field: &'static str,
) -> Result<(), AdminError> {
    let empty_root_set = field == "root" && selected.is_empty() && requested.is_empty();
    if (!empty_root_set && selected.is_empty()) || selected.len() > requested.len() {
        return Err(AdminError::invalid(match field {
            "scope" => "at least one requested scope must remain",
            "root" => "at least one requested root must remain",
            _ => "the narrowed authority is invalid",
        }));
    }
    let mut unique = BTreeSet::new();
    if !selected
        .iter()
        .all(|value| requested.contains(value) && unique.insert(value.as_ref()))
    {
        return Err(AdminError::invalid(match field {
            "scope" => "approved scopes must be a unique subset of the request",
            "root" => "approved roots must be a unique subset of the request",
            _ => "the narrowed authority is invalid",
        }));
    }
    Ok(())
}

pub(crate) fn approve_roots(
    roots: &[Box<str>],
    deny: &DenyList,
) -> Result<Vec<WorkspaceRoot>, AdminError> {
    roots
        .iter()
        .map(|root| {
            WorkspaceRoot::approve(root, deny).map_err(|_| {
                AdminError::invalid("a requested project root is unavailable or denied")
            })
        })
        .collect()
}

pub(crate) fn deny_list(composed: &Composed) -> Result<DenyList, AdminError> {
    let variable = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    let home = std::env::var(variable)
        .map_err(|_| AdminError::unavailable("the operator home directory is unavailable"))?;
    let home = AbsPath::canonicalize(&home)
        .map_err(|_| AdminError::unavailable("the operator home directory cannot be resolved"))?;
    let declared = composed
        .registry
        .all()
        .flat_map(|provider| provider.manifest.secrets.under_home.iter())
        .map(AsRef::as_ref)
        .collect::<Vec<_>>();
    DenyList::new(&home, composed.home.paths().root(), &declared).map_err(|_| AdminError::state())
}

/// Mint one unused integration key and durably record the grant, bracketed by audit.
///
/// Both approval paths end here so that the audit pair, the key-collision retry, and the durable write are
/// written once. What differs between them is who proved the decision, which is settled before this call and
/// carried in `method` so the ledger keeps the two apart.
///
/// The method is not cosmetic. The audit ledger is where someone goes to ask whether a person reviewed a grant
/// or a program spent its own enrollment, and one shared name would make that unanswerable after the fact.
fn commit_approval(
    composed: &Composed,
    enrollment: EnrollmentKey,
    grant: &IntegrationRow,
    method: &'static str,
) -> Result<IntegrationId, AdminError> {
    for _ in 0..4 {
        let key = IntegrationKey::from_bytes(random_key()?);
        if composed
            .store
            .get_integration(key)
            .map_err(|_| AdminError::state())?
            .is_none()
        {
            crate::runtime_audit::local(
                &composed.store,
                Some(key),
                Some(grant.key_generation),
                method,
                IntegrationAuditOutcome::Attempted,
                "attempted",
            )
            .map_err(|_| AdminError::state())?;
            composed
                .store
                .approve_enrollment(enrollment, key, grant)
                .map_err(|_| AdminError::state())?;
            crate::runtime_audit::local(
                &composed.store,
                Some(key),
                Some(grant.key_generation),
                method,
                IntegrationAuditOutcome::Allowed,
                "allowed",
            )
            .map_err(|_| AdminError::state())?;
            return Ok(integration_id(key));
        }
    }
    Err(AdminError::state())
}

fn parse_enrollment(value: &str) -> Result<EnrollmentKey, AdminError> {
    enrollment_key(&PendingEnrollmentId::new(value))
        .map_err(|_| AdminError::invalid("the pending enrollment identity is malformed"))
}

fn forget_key(
    integration: IntegrationKey,
    request_id: &MutationRequestId,
) -> Result<(IntegrationMutationKey, WallMs), ForgetConfirmationError> {
    let now = WallMs::now();
    let Some(created_at) = request_id.unix_millis() else {
        return Err(ForgetConfirmationError::InvalidRequestId);
    };
    if created_at > now.as_millis().saturating_add(MUTATION_CLOCK_SKEW_MS)
        || created_at.saturating_add(IDEMPOTENCY_WINDOW_MS) < now.as_millis()
    {
        return Err(ForgetConfirmationError::InvalidRequestId);
    }
    let Some(request_bytes) = request_id.to_bytes() else {
        return Err(ForgetConfirmationError::InvalidRequestId);
    };
    Ok((IntegrationMutationKey::new(integration, request_bytes), now))
}

fn key_rotation_key(
    integration: IntegrationKey,
    request_id: &MutationRequestId,
) -> Result<(IntegrationMutationKey, WallMs), KeyRotationConfirmationError> {
    let now = WallMs::now();
    let Some(created_at) = request_id.unix_millis() else {
        return Err(KeyRotationConfirmationError::InvalidRequestId);
    };
    if created_at > now.as_millis().saturating_add(MUTATION_CLOCK_SKEW_MS)
        || created_at.saturating_add(IDEMPOTENCY_WINDOW_MS) < now.as_millis()
    {
        return Err(KeyRotationConfirmationError::InvalidRequestId);
    }
    let Some(request_bytes) = request_id.to_bytes() else {
        return Err(KeyRotationConfirmationError::InvalidRequestId);
    };
    Ok((IntegrationMutationKey::new(integration, request_bytes), now))
}

fn random_key() -> Result<[u8; 16], AdminError> {
    let mut key = [0_u8; 16];
    getrandom::fill(&mut key).map_err(|_| AdminError::state())?;
    Ok(key)
}

fn parse_opaque(value: &str, prefix: &str) -> Result<[u8; 16], AdminError> {
    let Some(encoded) = value.strip_prefix(prefix) else {
        return Err(AdminError::invalid(
            "the local challenge identity is malformed",
        ));
    };
    if encoded.len() != 32 {
        return Err(AdminError::invalid(
            "the local challenge identity is malformed",
        ));
    }
    let mut key = [0_u8; 16];
    for (slot, pair) in key.iter_mut().zip(encoded.as_bytes().chunks_exact(2)) {
        let text = std::str::from_utf8(pair)
            .map_err(|_| AdminError::invalid("the local challenge identity is malformed"))?;
        *slot = u8::from_str_radix(text, 16)
            .map_err(|_| AdminError::invalid("the local challenge identity is malformed"))?;
    }
    Ok(key)
}

fn fingerprint(key: &[u8; 32]) -> String {
    format!("{}...", hex(&key[..8]))
}

fn hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        output.push(hex_digit(byte >> 4));
        output.push(hex_digit(byte & 0x0f));
    }
    output
}

const fn hex_digit(nibble: u8) -> char {
    match nibble {
        0 => '0',
        1 => '1',
        2 => '2',
        3 => '3',
        4 => '4',
        5 => '5',
        6 => '6',
        7 => '7',
        8 => '8',
        9 => '9',
        10 => 'a',
        11 => 'b',
        12 => 'c',
        13 => 'd',
        14 => 'e',
        15 => 'f',
        _ => '?',
    }
}

/// Safe local administration failure.
pub(crate) struct AdminError {
    message: &'static str,
}

impl AdminError {
    const fn invalid(message: &'static str) -> Self {
        Self { message }
    }

    const fn unavailable(message: &'static str) -> Self {
        Self { message }
    }

    const fn state() -> Self {
        Self {
            message: "Runtime integration authority could not be updated safely",
        }
    }
}

impl core::fmt::Display for AdminError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str(self.message)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use base64ct::{Base64UrlUnpadded, Encoding as _};
    use ed25519_dalek::{Signer as _, SigningKey};
    use runtrol_runtime_protocol::self_approval_signing_payload;
    use runtrol_store::EnrollmentRow;

    use super::*;

    static NEXT_SCRATCH: AtomicU64 = AtomicU64::new(0);

    struct Scratch {
        path: std::path::PathBuf,
    }

    impl Scratch {
        fn make() -> Self {
            let sequence = NEXT_SCRATCH.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "runtrol-integration-admin-{}-{sequence}",
                std::process::id()
            ));
            std::fs::create_dir_all(&path).expect("create integration scratch");
            Self { path }
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ignored = std::fs::remove_dir_all(&self.path);
        }
    }

    /// One pending enrollment for a caller-owned key, requesting one real project root.
    ///
    /// A real root because session authority without one is refused, which is the point of
    /// `session_authority_without_a_root_is_refused` below. Everything else here is about who may spend an
    /// enrollment, so the root is simply the scratch directory and never varies.
    fn pending(
        composed: &Composed,
        signing: &SigningKey,
        expires_at: WallMs,
        roots: Vec<Box<str>>,
    ) -> EnrollmentKey {
        let Ok(bytes) = random_key() else {
            panic!("the operating system supplies randomness for a test enrollment key");
        };
        let key = EnrollmentKey::from_bytes(bytes);
        let now = WallMs::now();
        let row = EnrollmentRow {
            public_key: signing.verifying_key().to_bytes(),
            client_instance_id: "instance".into(),
            client_name: "Test Consumer".into(),
            client_version: "0.0.0".into(),
            manifest_digest: [7; 32],
            scopes: vec![AppScope::SessionList.to_string().into()],
            roots,
            created_at: now,
            expires_at,
            state: EnrollmentState::Pending,
        };
        assert!(
            composed
                .store
                .create_enrollment(key, &row)
                .expect("create enrollment"),
            "the scratch store had no such enrollment yet"
        );
        key
    }

    fn sign_for(signing: &SigningKey, enrollment: EnrollmentKey) -> String {
        let payload =
            self_approval_signing_payload(&pending_id(enrollment)).expect("canonical payload");
        Base64UrlUnpadded::encode_string(&signing.sign(&payload).to_bytes())
    }

    /// Comfortably in the future, so a slow test machine cannot expire the enrollment mid-test.
    fn live() -> WallMs {
        WallMs::now().plus_millis(10 * 60_000)
    }

    /// Unambiguously in the past. `now` with a zero window can land in the same millisecond as the clock read
    /// inside the code under test, and a test that depends on which side of a tie it falls is a flaky test.
    fn already_expired() -> WallMs {
        WallMs::from_millis(1)
    }

    fn composed_for(scratch: &Scratch) -> Composed {
        let home = scratch.path.join("home");
        std::fs::create_dir_all(&home).expect("create runtrol home");
        Composed::for_tests(
            home.to_str().expect("UTF-8 home"),
            runtrol_drivers::builtin(),
        )
        .expect("compose")
    }

    /// One real directory to hold authority over.
    ///
    /// A sibling of the runtrol home rather than a child of it. The deny list refuses any root that would let an
    /// agent read runtrol's own state, so a project nested inside the home is rejected before the signature is
    /// ever the reason.
    fn one_root(scratch: &Scratch) -> Vec<Box<str>> {
        let project = scratch.path.join("project");
        std::fs::create_dir_all(&project).expect("create project root");
        vec![project.to_str().expect("UTF-8 root").into()]
    }

    #[test]
    fn the_enrolling_key_spends_its_own_enrollment() {
        let scratch = Scratch::make();
        let composed = composed_for(&scratch);
        let signing = SigningKey::from_bytes(&[3; 32]);
        let enrollment = pending(&composed, &signing, live(), one_root(&scratch));

        let Ok(granted) = IntegrationAdmin::self_approve(
            &composed,
            pending_id(enrollment).as_str(),
            &sign_for(&signing, enrollment),
        ) else {
            panic!("the enrolling key may spend its own enrollment");
        };

        assert!(granted.as_str().starts_with("int_"));
        let row = composed
            .store
            .get_enrollment(enrollment)
            .expect("read enrollment")
            .expect("the enrollment row survives its decision");
        assert_ne!(
            row.state,
            EnrollmentState::Pending,
            "an approved enrollment is no longer pending"
        );
    }

    #[test]
    fn another_key_cannot_spend_an_enrollment_it_did_not_request() {
        // The whole reason this path carries a signature. Reaching the private endpoint is not enough; the
        // caller has to be the enrollment.
        let scratch = Scratch::make();
        let composed = composed_for(&scratch);
        let enrolling = SigningKey::from_bytes(&[3; 32]);
        let impostor = SigningKey::from_bytes(&[9; 32]);
        let enrollment = pending(&composed, &enrolling, live(), one_root(&scratch));

        let refused = IntegrationAdmin::self_approve(
            &composed,
            pending_id(enrollment).as_str(),
            &sign_for(&impostor, enrollment),
        );

        assert!(refused.is_err(), "a foreign signature must not be accepted");
        let row = composed
            .store
            .get_enrollment(enrollment)
            .expect("read enrollment")
            .expect("the enrollment is still there");
        assert_eq!(
            row.state,
            EnrollmentState::Pending,
            "a refused attempt must not spend the enrollment"
        );
    }

    #[test]
    fn a_signature_for_one_enrollment_cannot_spend_another() {
        // The pending identity is inside the signed payload precisely so that one captured proof cannot be
        // pointed at a second enrollment the same key also owns.
        let scratch = Scratch::make();
        let composed = composed_for(&scratch);
        let signing = SigningKey::from_bytes(&[3; 32]);
        let first = pending(&composed, &signing, live(), one_root(&scratch));
        let second = pending(&composed, &signing, live(), one_root(&scratch));

        let refused = IntegrationAdmin::self_approve(
            &composed,
            pending_id(second).as_str(),
            &sign_for(&signing, first),
        );

        assert!(
            refused.is_err(),
            "a proof naming one enrollment must not spend a different one"
        );
        assert_eq!(
            composed
                .store
                .get_enrollment(second)
                .expect("read enrollment")
                .expect("still there")
                .state,
            EnrollmentState::Pending
        );
    }

    #[test]
    fn an_expired_enrollment_cannot_be_self_approved() {
        let scratch = Scratch::make();
        let composed = composed_for(&scratch);
        let signing = SigningKey::from_bytes(&[3; 32]);
        let enrollment = pending(&composed, &signing, already_expired(), one_root(&scratch));

        let refused = IntegrationAdmin::self_approve(
            &composed,
            pending_id(enrollment).as_str(),
            &sign_for(&signing, enrollment),
        );

        assert!(
            refused.is_err(),
            "an expired pending request is not a live decision"
        );
    }

    #[test]
    fn one_enrollment_is_spent_at_most_once() {
        // A replayed proof must not mint a second integration. The signature never expires on its own, so the
        // enrollment state is what has to stop it.
        let scratch = Scratch::make();
        let composed = composed_for(&scratch);
        let signing = SigningKey::from_bytes(&[3; 32]);
        let enrollment = pending(&composed, &signing, live(), one_root(&scratch));
        let proof = sign_for(&signing, enrollment);

        assert!(
            IntegrationAdmin::self_approve(&composed, pending_id(enrollment).as_str(), &proof)
                .is_ok(),
            "the first spend succeeds"
        );
        let replayed =
            IntegrationAdmin::self_approve(&composed, pending_id(enrollment).as_str(), &proof);

        assert!(
            replayed.is_err(),
            "replaying the same proof must not mint a second integration"
        );
    }

    #[test]
    fn session_authority_without_a_root_is_refused() {
        // Every other grant-writing path refuses this, and self-approval must too. A grant carrying session
        // authority with no root to exercise it in is not a smaller grant, it is a broken one: every later call
        // fails root-denied, the grant itself stays valid, and the client never learns to re-enroll.
        let scratch = Scratch::make();
        let composed = composed_for(&scratch);
        let signing = SigningKey::from_bytes(&[3; 32]);
        let enrollment = pending(&composed, &signing, live(), Vec::new());

        let refused = IntegrationAdmin::self_approve(
            &composed,
            pending_id(enrollment).as_str(),
            &sign_for(&signing, enrollment),
        );

        assert!(
            refused.is_err(),
            "session authority with no approved root must not be granted"
        );
        assert_eq!(
            composed
                .store
                .get_enrollment(enrollment)
                .expect("read enrollment")
                .expect("still there")
                .state,
            EnrollmentState::Pending
        );
    }

    #[test]
    fn a_malformed_pending_identity_or_signature_is_refused() {
        let scratch = Scratch::make();
        let composed = composed_for(&scratch);
        let signing = SigningKey::from_bytes(&[3; 32]);
        let enrollment = pending(&composed, &signing, live(), one_root(&scratch));

        assert!(
            IntegrationAdmin::self_approve(
                &composed,
                "not-a-pending-id",
                &sign_for(&signing, enrollment)
            )
            .is_err()
        );
        assert!(
            IntegrationAdmin::self_approve(
                &composed,
                pending_id(enrollment).as_str(),
                "not-a-signature"
            )
            .is_err()
        );
    }
}
