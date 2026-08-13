//! Local-only listing, physical-presence approval, denial, and revocation for public integrations.

use std::collections::{BTreeMap, BTreeSet};

use runtrol_ipc::wire::{IntegrationEnrollmentLine, IntegrationLine, RuntimeForgetLine};
use runtrol_provider::{AbsPath, WallMs};
use runtrol_runtime_protocol::{
    IDEMPOTENCY_WINDOW_MS, IntegrationId, MUTATION_CLOCK_SKEW_MS, MutationRequestId,
    PendingEnrollmentId,
};
use runtrol_security::{
    DenyList, GrantRequest, IntegrationProposal, LocalConsole, PresenceChallenge, WorkspaceRoot,
};
use runtrol_store::{
    EnrollmentKey, EnrollmentState, IntegrationAuditOutcome, IntegrationKey,
    IntegrationMutationKey, IntegrationRootRow, IntegrationRow,
};
use tokio::sync::Mutex;

use crate::Composed;
use crate::runtime_auth::{enrollment_key, integration_id, integration_key, pending_id};

const MAX_APPROVAL_CHALLENGES: usize = 16;
const APPROVAL_CHALLENGE_MS: u64 = runtrol_security::presence::CHALLENGE_WINDOW_MS;
const MAX_FORGET_CONFIRMATIONS: usize = 64;
const FORGET_CONFIRMATION_MS: u64 = 10 * 60_000;

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

/// Bounded one-use local integration approval state.
#[derive(Default)]
pub(crate) struct IntegrationAdmin {
    challenges: Mutex<BTreeMap<[u8; 16], PendingApproval>>,
    forget_confirmations: Mutex<BTreeMap<IntegrationMutationKey, PendingForgetConfirmation>>,
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

    #[expect(
        clippy::too_many_lines,
        reason = "one approval transaction keeps the one-use witness, exact proposal revalidation, filesystem identity binding, durable grant, and audit stages together"
    )]
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
                    "integrations/approve",
                    IntegrationAuditOutcome::Attempted,
                    "attempted",
                )
                .map_err(|_| AdminError::state())?;
                composed
                    .store
                    .approve_enrollment(pending.enrollment, key, &grant)
                    .map_err(|_| AdminError::state())?;
                crate::runtime_audit::local(
                    &composed.store,
                    Some(key),
                    Some(grant.key_generation),
                    "integrations/approve",
                    IntegrationAuditOutcome::Allowed,
                    "allowed",
                )
                .map_err(|_| AdminError::state())?;
                return Ok(integration_id(key));
            }
        }
        Err(AdminError::state())
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

fn approve_roots(roots: &[Box<str>], deny: &DenyList) -> Result<Vec<WorkspaceRoot>, AdminError> {
    roots
        .iter()
        .map(|root| {
            WorkspaceRoot::approve(root, deny).map_err(|_| {
                AdminError::invalid("a requested project root is unavailable or denied")
            })
        })
        .collect()
}

fn deny_list(composed: &Composed) -> Result<DenyList, AdminError> {
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
