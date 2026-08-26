//! Public Runtime integration challenge, enrollment, and grant verification.

use std::collections::BTreeSet;

use base64ct::{Base64UrlUnpadded, Encoding as _};
use ed25519_dalek::{Signature, VerifyingKey};
use runtrol_provider::WallMs;
use runtrol_runtime_protocol::{
    AppScope, CHALLENGE_LIFETIME_MS, ClientCapabilities, ClientInfo, ENROLLMENT_LIFETIME_MS,
    EnrollmentDecision, EnrollmentReceipt, IDEMPOTENCY_WINDOW_MS, IntegrationAuthentication,
    IntegrationGrant, IntegrationId, MAX_PENDING_ENROLLMENTS, MUTATION_CLOCK_SKEW_MS,
    PendingEnrollmentId, ProtocolRevision, RequestEnrollmentParams, RotateIntegrationKeyParams,
    RuntimeErrorKind, ServerChallenge, enrollment_signing_payload, initialization_signing_payload,
    key_rotation_signing_payload, self_approval_signing_payload,
};
use runtrol_store::{
    EnrollmentKey, EnrollmentRow, EnrollmentState, IntegrationKey, IntegrationRootRow,
    IntegrationRow, Store,
};

const MAX_CLIENT_TEXT_CHARS: usize = 128;
const MAX_ROOTS: usize = 32;
const MAX_ROOT_BYTES: usize = 32 * 1024;

/// Client negotiation facts bound into enrollment proof after initialization.
#[derive(Clone)]
pub(crate) struct ClientContext {
    pub(crate) challenge: ServerChallenge,
    pub(crate) supported_revisions: Vec<ProtocolRevision>,
    pub(crate) selected_revision: ProtocolRevision,
    pub(crate) client: ClientInfo,
    pub(crate) capabilities: ClientCapabilities,
}

/// One verified current grant. The durable row remains the authority and is checked before every operation.
#[derive(Clone)]
pub(crate) struct AuthorizedIntegration {
    pub(crate) key: IntegrationKey,
    pub(crate) grant: IntegrationGrant,
    pub(crate) roots: Vec<IntegrationRootRow>,
}

/// Safe stable failure before it is placed into a JSON-RPC error envelope.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AuthorizationFailure {
    pub(crate) kind: RuntimeErrorKind,
    pub(crate) message: &'static str,
}

impl AuthorizationFailure {
    const fn invalid(message: &'static str) -> Self {
        Self {
            kind: RuntimeErrorKind::InvalidRequest,
            message,
        }
    }

    const fn unauthenticated(message: &'static str) -> Self {
        Self {
            kind: RuntimeErrorKind::Unauthenticated,
            message,
        }
    }

    const fn internal() -> Self {
        Self {
            kind: RuntimeErrorKind::Internal,
            message: "Runtime could not validate integration authority",
        }
    }
}

/// Mint one connection-bound, short-lived challenge.
pub(crate) fn challenge(instance_id: &str) -> Result<ServerChallenge, AuthorizationFailure> {
    let nonce = random_bytes::<32>()?;
    let nonce_id = random_bytes::<16>()?;
    Ok(ServerChallenge {
        instance_id: instance_id.to_owned(),
        nonce_id: format!("nonce_{}", hex(&nonce_id)),
        nonce: Base64UrlUnpadded::encode_string(&nonce),
        expires_at_ms: WallMs::now()
            .as_millis()
            .saturating_add(CHALLENGE_LIFETIME_MS),
    })
}

/// Verify an approved integration signature and return its current authority.
pub(crate) fn authenticate(
    store: &Store,
    context: &ClientContext,
    authentication: &IntegrationAuthentication,
) -> Result<AuthorizedIntegration, AuthorizationFailure> {
    ensure_challenge_fresh(&context.challenge)?;
    let key = integration_key(&authentication.integration_id)?;
    let Some(row) = store
        .get_integration(key)
        .map_err(|_| AuthorizationFailure::internal())?
    else {
        return Err(AuthorizationFailure::unauthenticated(
            "the integration is not enrolled",
        ));
    };
    authenticate_against_row(context, authentication, key, &row)
}

/// Verify initialization against a row supplied by the successor-owned draining-generation relay.
pub(crate) fn authenticate_against_row(
    context: &ClientContext,
    authentication: &IntegrationAuthentication,
    key: IntegrationKey,
    row: &IntegrationRow,
) -> Result<AuthorizedIntegration, AuthorizationFailure> {
    if row.revoked_at.is_some() {
        return Err(AuthorizationFailure {
            kind: RuntimeErrorKind::IntegrationRevoked,
            message: "the integration grant was revoked",
        });
    }
    if row.key_generation != authentication.key_generation
        || row.grant_generation < authentication.grant_generation
    {
        return Err(AuthorizationFailure::unauthenticated(
            "the integration generation is stale",
        ));
    }
    let payload = initialization_signing_payload(
        &context.challenge,
        &context.supported_revisions,
        &context.client,
        &context.capabilities,
        authentication,
    )
    .map_err(|_| AuthorizationFailure::internal())?;
    verify_signature(&row.public_key, &authentication.signature, &payload)?;
    Ok(AuthorizedIntegration {
        key,
        grant: grant(authentication.integration_id.clone(), row)?,
        roots: row.roots.clone(),
    })
}

/// Re-read and verify the current grant generation before an authorized method acts.
pub(crate) fn refresh(
    store: &Store,
    current: &AuthorizedIntegration,
) -> Result<AuthorizedIntegration, AuthorizationFailure> {
    let Some(row) = store
        .get_integration(current.key)
        .map_err(|_| AuthorizationFailure::internal())?
    else {
        return Err(AuthorizationFailure::unauthenticated(
            "the integration grant no longer exists",
        ));
    };
    refresh_against_row(current, &row)
}

/// Revalidate an existing connection against one exact authoritative row.
pub(crate) fn refresh_against_row(
    current: &AuthorizedIntegration,
    row: &IntegrationRow,
) -> Result<AuthorizedIntegration, AuthorizationFailure> {
    if row.revoked_at.is_some() {
        return Err(AuthorizationFailure {
            kind: RuntimeErrorKind::IntegrationRevoked,
            message: "the integration grant was revoked",
        });
    }
    if row.key_generation != current.grant.key_generation {
        return Err(AuthorizationFailure::unauthenticated(
            "the integration key changed; reconnect and authenticate again",
        ));
    }
    let next = grant(current.grant.integration_id.clone(), row)?;
    if row.grant_generation != current.grant.grant_generation {
        // A newer generation that only ADDED authority continues in place. The caller proved its identity
        // against this same key generation, the store row stays the authority for every request either way,
        // and this widening is something the same integration asked for a moment ago: forcing a reconnect
        // here bought no security and cost ~5 seconds between opening a folder and its conversations
        // arriving (measured 2026-08-20). Anything that removed or replaced authority, and any older
        // generation, still tears the connection down, so a shrink or a rollback can never ride this.
        if !widening_continues(&current.grant, &next) {
            return Err(AuthorizationFailure::unauthenticated(
                "the integration grant changed; reconnect and authenticate again",
            ));
        }
    }
    Ok(AuthorizedIntegration {
        key: current.key,
        grant: next,
        roots: row.roots.clone(),
    })
}

/// Whether a newer grant is a pure widening the live connection may continue across.
///
/// Strictly newer, and everything already held still held: any removed scope, any removed root, and any
/// older generation answers false, which keeps shrink and rollback on the reconnect-and-reauthenticate
/// path where they belong.
fn widening_continues(current: &IntegrationGrant, next: &IntegrationGrant) -> bool {
    next.grant_generation > current.grant_generation
        && current
            .scopes
            .iter()
            .all(|scope| next.scopes.contains(scope))
        && current.roots.iter().all(|root| next.roots.contains(root))
}

/// Verify a replacement-key proof against the exact current or already-rotated grant row.
pub(crate) fn verify_key_rotation(
    authority: &AuthorizedIntegration,
    row: &IntegrationRow,
    params: &RotateIntegrationKeyParams,
) -> Result<[u8; 32], AuthorizationFailure> {
    let now = WallMs::now().as_millis();
    let Some(created_at) = params.request_id.unix_millis() else {
        return Err(AuthorizationFailure::invalid(
            "the key rotation request identity is malformed",
        ));
    };
    if created_at > now.saturating_add(MUTATION_CLOCK_SKEW_MS)
        || created_at.saturating_add(IDEMPOTENCY_WINDOW_MS) < now
    {
        return Err(AuthorizationFailure::invalid(
            "the key rotation request identity is outside its bounded lifetime",
        ));
    }
    if params.expected_key_generation == u64::MAX {
        return Err(AuthorizationFailure::invalid(
            "the expected integration key generation is exhausted",
        ));
    }
    let new_public_key = decode_exact::<32>(&params.new_public_key, "new public key")?;
    let first_attempt =
        row.key_generation == params.expected_key_generation && row.public_key != new_public_key;
    let completed_replay = row.key_generation == params.expected_key_generation + 1
        && row.public_key == new_public_key;
    if !first_attempt && !completed_replay {
        return Err(AuthorizationFailure {
            kind: RuntimeErrorKind::IdempotencyConflict,
            message: "the key rotation request no longer matches the integration generation",
        });
    }
    let payload = key_rotation_signing_payload(
        &authority.grant.integration_id,
        authority.grant.grant_generation,
        params,
    )
    .map_err(|_| AuthorizationFailure::internal())?;
    verify_signature(&new_public_key, &params.new_key_proof, &payload)?;
    Ok(new_public_key)
}

/// Prove possession, apply enrollment bounds, and persist one opaque pending decision.
#[expect(
    clippy::too_many_lines,
    reason = "one enrollment transaction keeps validation, replay binding, flood bounds, and durable creation together"
)]
pub(crate) fn request_enrollment(
    store: &Store,
    context: &ClientContext,
    params: &RequestEnrollmentParams,
) -> Result<(EnrollmentKey, EnrollmentReceipt), AuthorizationFailure> {
    ensure_challenge_fresh(&context.challenge)?;
    validate_client_text(&params.manifest.client_instance_id)?;
    validate_client_text(&context.client.name)?;
    validate_client_text(&context.client.version)?;
    validate_roots(&params.manifest.requested_roots)?;
    if params.manifest.requested_scopes.is_empty()
        || params.manifest.requested_scopes.len() > AppScope::ALL.len()
        || params
            .manifest
            .requested_scopes
            .iter()
            .collect::<BTreeSet<_>>()
            .len()
            != params.manifest.requested_scopes.len()
    {
        return Err(AuthorizationFailure::invalid(
            "requested scopes are empty, duplicated, or oversized",
        ));
    }
    let public_key = decode_exact::<32>(&params.manifest.public_key, "public key")?;
    let manifest_digest = decode_exact::<32>(&params.manifest.manifest_digest, "manifest digest")?;
    let payload = enrollment_signing_payload(
        &context.challenge,
        &context.supported_revisions,
        context.selected_revision,
        &context.client,
        &context.capabilities,
        &params.manifest,
    )
    .map_err(|_| AuthorizationFailure::internal())?;
    verify_signature(&public_key, &params.signature, &payload)?;

    let now = WallMs::now();
    store
        .purge_expired_enrollments(now)
        .map_err(|_| AuthorizationFailure::internal())?;
    let enrollments = store
        .list_enrollments()
        .map_err(|_| AuthorizationFailure::internal())?;
    if let Some((key, existing)) = enrollments.iter().find(|(_, row)| {
        row.public_key == public_key
            && row.client_instance_id.as_ref() == params.manifest.client_instance_id
            && row.expires_at >= now
    }) {
        if existing.manifest_digest == manifest_digest
            && existing.scopes.iter().map(AsRef::as_ref).eq(params
                .manifest
                .requested_scopes
                .iter()
                .map(|scope| scope.as_str()))
            && existing.roots.iter().map(AsRef::as_ref).eq(params
                .manifest
                .requested_roots
                .iter()
                .map(String::as_str))
        {
            return Ok((*key, receipt(*key, existing.expires_at)));
        }
        return Err(AuthorizationFailure {
            kind: RuntimeErrorKind::IdempotencyConflict,
            message: "the same integration key already has a different pending proposal",
        });
    }
    if enrollments
        .iter()
        .filter(|(_, row)| row.state == EnrollmentState::Pending && row.expires_at >= now)
        .count()
        >= usize::from(MAX_PENDING_ENROLLMENTS)
    {
        return Err(AuthorizationFailure {
            kind: RuntimeErrorKind::ResourceExhausted,
            message: "too many integration enrollments are pending local approval",
        });
    }

    let expires_at = now.plus_millis(ENROLLMENT_LIFETIME_MS);
    let row = EnrollmentRow {
        public_key,
        client_instance_id: params.manifest.client_instance_id.clone().into(),
        client_name: context.client.name.clone().into(),
        client_version: context.client.version.clone().into(),
        manifest_digest,
        scopes: params
            .manifest
            .requested_scopes
            .iter()
            .map(|scope| scope.as_str().into())
            .collect(),
        roots: params
            .manifest
            .requested_roots
            .iter()
            .cloned()
            .map(Into::into)
            .collect(),
        created_at: now,
        expires_at,
        state: EnrollmentState::Pending,
    };
    for _ in 0..4 {
        let bytes = random_bytes::<16>()?;
        let key = EnrollmentKey::from_bytes(bytes);
        if store
            .create_enrollment(key, &row)
            .map_err(|_| AuthorizationFailure::internal())?
        {
            return Ok((key, receipt(key, expires_at)));
        }
    }
    Err(AuthorizationFailure::internal())
}

/// Read only the decision attached to the proved pending key.
pub(crate) fn enrollment_decision(
    store: &Store,
    expected: EnrollmentKey,
    supplied: &PendingEnrollmentId,
) -> Result<EnrollmentDecision, AuthorizationFailure> {
    if enrollment_key(supplied)? != expected {
        return Err(AuthorizationFailure::unauthenticated(
            "the pending enrollment does not belong to this connection",
        ));
    }
    let Some(row) = store
        .get_enrollment(expected)
        .map_err(|_| AuthorizationFailure::internal())?
    else {
        return Ok(EnrollmentDecision::Expired);
    };
    if row.expires_at < WallMs::now() {
        return Ok(EnrollmentDecision::Expired);
    }
    match row.state {
        EnrollmentState::Pending => Ok(EnrollmentDecision::Pending),
        EnrollmentState::Denied => Ok(EnrollmentDecision::Denied),
        EnrollmentState::Approved(key) => {
            let Some(grant_row) = store
                .get_integration(key)
                .map_err(|_| AuthorizationFailure::internal())?
            else {
                return Err(AuthorizationFailure::internal());
            };
            Ok(EnrollmentDecision::Approved {
                grant: grant(integration_id(key), &grant_row)?,
            })
        }
    }
}

pub(crate) fn grant(
    id: IntegrationId,
    row: &IntegrationRow,
) -> Result<IntegrationGrant, AuthorizationFailure> {
    let scopes = row
        .scopes
        .iter()
        .map(|scope| scope.parse().map_err(|_| AuthorizationFailure::internal()))
        .collect::<Result<_, _>>()?;
    Ok(IntegrationGrant {
        integration_id: id,
        scopes,
        roots: row.roots.iter().map(|root| root.path.to_string()).collect(),
        key_generation: row.key_generation,
        grant_generation: row.grant_generation,
    })
}

fn receipt(key: EnrollmentKey, expires_at: WallMs) -> EnrollmentReceipt {
    EnrollmentReceipt {
        pending_id: pending_id(key),
        expires_at_ms: expires_at.as_millis(),
    }
}

fn verify_signature(
    key: &[u8; 32],
    encoded_signature: &str,
    payload: &[u8],
) -> Result<(), AuthorizationFailure> {
    let verifying = VerifyingKey::from_bytes(key).map_err(|_| {
        AuthorizationFailure::invalid("the integration public key is not valid Ed25519")
    })?;
    let bytes = decode_base64(encoded_signature)
        .map_err(|()| AuthorizationFailure::invalid("the integration signature is malformed"))?;
    let signature = Signature::from_slice(&bytes)
        .map_err(|_| AuthorizationFailure::invalid("the integration signature is malformed"))?;
    verifying.verify_strict(payload, &signature).map_err(|_| {
        AuthorizationFailure::unauthenticated("the integration signature does not verify")
    })
}

/// Prove that whoever asks to approve this enrollment holds the key the enrollment was requested with.
///
/// Local administration already requires reaching the private endpoint, which is owner-only and therefore counts
/// as being at the machine. What that alone does not establish is *which* pending enrollment the caller is, so a
/// program at the machine could otherwise approve an enrollment some other program requested. This proof closes
/// that gap: only the enrolling key can spend its own pending decision.
pub(crate) fn verify_self_approval(
    public_key: &[u8; 32],
    encoded_signature: &str,
    pending: &PendingEnrollmentId,
) -> Result<(), AuthorizationFailure> {
    let payload =
        self_approval_signing_payload(pending).map_err(|_| AuthorizationFailure::internal())?;
    verify_signature(public_key, encoded_signature, &payload)
}

fn ensure_challenge_fresh(challenge: &ServerChallenge) -> Result<(), AuthorizationFailure> {
    if WallMs::now().as_millis() > challenge.expires_at_ms {
        return Err(AuthorizationFailure::unauthenticated(
            "the connection challenge expired",
        ));
    }
    Ok(())
}

fn validate_client_text(value: &str) -> Result<(), AuthorizationFailure> {
    if value.trim().is_empty()
        || value.chars().count() > MAX_CLIENT_TEXT_CHARS
        || value.chars().any(unsafe_text)
    {
        return Err(AuthorizationFailure::invalid(
            "integration identity text is blank, unsafe, or oversized",
        ));
    }
    Ok(())
}

fn validate_roots(roots: &[String]) -> Result<(), AuthorizationFailure> {
    if roots.len() > MAX_ROOTS {
        return Err(AuthorizationFailure::invalid(
            "too many project roots were requested",
        ));
    }
    for root in roots {
        let path = std::path::Path::new(root);
        if root.is_empty()
            || root.len() > MAX_ROOT_BYTES
            || root.chars().any(unsafe_text)
            || !path.is_absolute()
        {
            return Err(AuthorizationFailure::invalid(
                "a requested project root is unsafe, relative, or oversized",
            ));
        }
    }
    Ok(())
}

const fn unsafe_text(character: char) -> bool {
    character.is_control()
        || matches!(
            character,
            '\u{200b}'..='\u{200f}'
                | '\u{2028}'..='\u{202e}'
                | '\u{2060}'..='\u{206f}'
                | '\u{feff}'
        )
}

pub(crate) fn integration_key(id: &IntegrationId) -> Result<IntegrationKey, AuthorizationFailure> {
    parse_key(id.as_str(), "int_").map(IntegrationKey::from_bytes)
}

pub(crate) fn enrollment_key(
    id: &PendingEnrollmentId,
) -> Result<EnrollmentKey, AuthorizationFailure> {
    parse_key(id.as_str(), "enr_").map(EnrollmentKey::from_bytes)
}

pub(crate) fn integration_id(key: IntegrationKey) -> IntegrationId {
    IntegrationId::new(format!("int_{}", hex(&key.to_bytes())))
}

pub(crate) fn pending_id(key: EnrollmentKey) -> PendingEnrollmentId {
    PendingEnrollmentId::new(format!("enr_{}", hex(&key.to_bytes())))
}

fn parse_key(value: &str, prefix: &str) -> Result<[u8; 16], AuthorizationFailure> {
    let Some(hex) = value.strip_prefix(prefix) else {
        return Err(AuthorizationFailure::invalid(
            "an opaque integration identity is malformed",
        ));
    };
    if hex.len() != 32 {
        return Err(AuthorizationFailure::invalid(
            "an opaque integration identity is malformed",
        ));
    }
    let mut bytes = [0_u8; 16];
    for (slot, pair) in bytes.iter_mut().zip(hex.as_bytes().chunks_exact(2)) {
        let text = std::str::from_utf8(pair).map_err(|_| {
            AuthorizationFailure::invalid("an opaque integration identity is malformed")
        })?;
        *slot = u8::from_str_radix(text, 16).map_err(|_| {
            AuthorizationFailure::invalid("an opaque integration identity is malformed")
        })?;
    }
    Ok(bytes)
}

fn decode_exact<const N: usize>(
    encoded: &str,
    field: &'static str,
) -> Result<[u8; N], AuthorizationFailure> {
    let bytes = decode_base64(encoded).map_err(|()| malformed_fixed(field))?;
    bytes.try_into().map_err(|_| malformed_fixed(field))
}

fn decode_base64(encoded: &str) -> Result<Vec<u8>, ()> {
    match Base64UrlUnpadded::decode_vec(encoded) {
        Ok(bytes) => Ok(bytes),
        Err(_) => Err(()),
    }
}

fn malformed_fixed(field: &'static str) -> AuthorizationFailure {
    AuthorizationFailure::invalid(match field {
        "public key" => "the integration public key is malformed",
        "manifest digest" => "the integration manifest digest is malformed",
        _ => "a fixed public field is malformed",
    })
}

fn random_bytes<const N: usize>() -> Result<[u8; N], AuthorizationFailure> {
    let mut bytes = [0_u8; N];
    getrandom::fill(&mut bytes).map_err(|_| AuthorizationFailure::internal())?;
    Ok(bytes)
}

/// Lowercase hex, shared where a digest needs writing down (`build_identity` reuses this owner).
pub(crate) fn hex(bytes: &[u8]) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_pure_widening_continues_across_a_grant_change() {
        let grant_with =
            |generation: u64, scopes: Vec<AppScope>, roots: Vec<String>| IntegrationGrant {
                integration_id: IntegrationId::new("enr_ab"),
                scopes,
                roots,
                grant_generation: generation,
                key_generation: 1,
            };
        let current = grant_with(1, vec![AppScope::SessionList], vec!["C:/alpha".to_owned()]);

        let widened = grant_with(
            2,
            vec![AppScope::SessionList, AppScope::SessionStart],
            vec!["C:/alpha".to_owned(), "C:/beta".to_owned()],
        );
        assert!(
            widening_continues(&current, &widened),
            "added authority continues in place"
        );

        let shrunk_roots = grant_with(2, vec![AppScope::SessionList], Vec::new());
        assert!(
            !widening_continues(&current, &shrunk_roots),
            "a removed root must tear the connection down"
        );

        let shrunk_scopes = grant_with(2, Vec::new(), vec!["C:/alpha".to_owned()]);
        assert!(
            !widening_continues(&current, &shrunk_scopes),
            "a removed scope must tear the connection down"
        );

        let rolled_back = grant_with(
            0,
            vec![AppScope::SessionList, AppScope::SessionStart],
            vec!["C:/alpha".to_owned(), "C:/beta".to_owned()],
        );
        assert!(
            !widening_continues(&current, &rolled_back),
            "an older generation is never a widening, whatever it contains"
        );
    }

    #[test]
    fn opaque_ids_round_trip_without_accepting_another_prefix() {
        let integration = IntegrationKey::from_bytes([0xAB; 16]);
        assert_eq!(
            integration_key(&integration_id(integration)),
            Ok(integration)
        );
        assert!(integration_key(&IntegrationId::new("enr_00")).is_err());
    }
}
