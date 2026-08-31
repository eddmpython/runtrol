//! Runtime initialization, enrollment, and integration administration requests.

use std::collections::BTreeSet;

use runtrol_runtime_protocol::{
    FINALIZED_REVISIONS, InitializeParams, InitializeResult, JsonRpcId, MAX_REVISION_OFFERS,
    RequestEnrollmentParams, RotateIntegrationKeyParams, RuntimeCapabilities, RuntimeErrorKind,
    RuntimeInstance, RuntimeLimits, WatchEnrollmentParams, negotiate,
};
use runtrol_store::IntegrationKeyRotation;

use crate::Composed;
use crate::runtime_auth::{
    AuthorizationFailure, ClientContext, enrollment_decision, request_enrollment,
};

use super::authority::{authenticate_with_relay_patience, authorized};
use super::connection_state::{PublicAuthority, PublicState};
use super::response::{Answer, EmptyParams, confirmation_failure, not_ready, platform_name};

pub(super) async fn initialize(
    state: &mut PublicState,
    instance_id: &str,
    composed: &Composed,
    id: JsonRpcId,
    params: serde_json::Value,
) -> Answer {
    let PublicState::Fresh { challenge } = state else {
        return Answer::plain(
            id,
            RuntimeErrorKind::InvalidRequest,
            "Runtime initialization cannot be repeated on one connection",
        );
    };
    let Ok(params) = serde_json::from_value::<InitializeParams>(params) else {
        return Answer::plain(
            id,
            RuntimeErrorKind::InvalidRequest,
            "initialization parameters are invalid",
        );
    };
    if params.supported_revisions.is_empty()
        || params.supported_revisions.len() > usize::from(MAX_REVISION_OFFERS)
        || params
            .supported_revisions
            .iter()
            .collect::<BTreeSet<_>>()
            .len()
            != params.supported_revisions.len()
    {
        return Answer::plain(
            id,
            RuntimeErrorKind::InvalidRequest,
            "the finalized revision offer is empty, duplicated, or oversized",
        );
    }
    let Some(revision) = negotiate(&params.supported_revisions, &FINALIZED_REVISIONS) else {
        return Answer::plain(
            id,
            RuntimeErrorKind::ProtocolIncompatible,
            "no finalized Runtime revision is shared",
        );
    };
    let context = ClientContext {
        challenge: challenge.clone(),
        supported_revisions: params.supported_revisions,
        selected_revision: revision,
        client: params.client,
        capabilities: params.client_capabilities,
    };
    let authority = match params.authentication.as_ref() {
        Some(proof) => match authenticate_with_relay_patience(composed, &context, proof).await {
            Ok(authorized) => PublicAuthority::Authorized(authorized),
            Err(failure) => return Answer::failure(id, failure),
        },
        None => PublicAuthority::Anonymous,
    };
    let granted = match &authority {
        PublicAuthority::Authorized(authorized) => Some(authorized.grant.clone()),
        PublicAuthority::Anonymous | PublicAuthority::Pending(_) => None,
    };
    let result = InitializeResult {
        selected_revision: revision,
        runtime: RuntimeInstance {
            instance_id: instance_id.to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            platform: platform_name().to_owned(),
            build_digest: crate::build_identity::build_digest().map(str::to_owned),
        },
        server_capabilities: RuntimeCapabilities {
            integration_enrollment: true,
            provider_inventory: true,
            managed_session_list: true,
            model_discovery: true,
            native_session_catalogue: true,
            session_control: true,
            session_events: true,
            terminal_surface: true,
        },
        limits: RuntimeLimits::default(),
        grant: granted,
    };
    *state = PublicState::Negotiated { context, authority };
    Answer::success(id, &result)
}

pub(super) fn request_integration(
    state: &mut PublicState,
    composed: &Composed,
    id: JsonRpcId,
    params: serde_json::Value,
) -> Answer {
    let PublicState::Ready { context, authority } = state else {
        return not_ready(id);
    };
    if !matches!(authority, PublicAuthority::Anonymous) {
        return Answer::plain(
            id,
            RuntimeErrorKind::EnrollmentPending,
            "this connection already has integration authority or a pending decision",
        );
    }
    let Ok(params) = serde_json::from_value::<RequestEnrollmentParams>(params) else {
        return Answer::plain(
            id,
            RuntimeErrorKind::InvalidRequest,
            "integration enrollment parameters are invalid",
        );
    };
    match request_enrollment(&composed.store, context, &params) {
        Ok((pending, receipt)) => {
            *authority = PublicAuthority::Pending(pending);
            Answer::success(id, &receipt)
        }
        Err(failure) => Answer::failure(id, failure),
    }
}

pub(super) fn watch_integration(
    state: &mut PublicState,
    composed: &Composed,
    id: JsonRpcId,
    params: serde_json::Value,
) -> Answer {
    let PublicState::Ready { authority, .. } = state else {
        return not_ready(id);
    };
    let PublicAuthority::Pending(expected) = authority else {
        return Answer::plain(
            id,
            RuntimeErrorKind::Unauthenticated,
            "this connection has no proved pending enrollment",
        );
    };
    let Ok(params) = serde_json::from_value::<WatchEnrollmentParams>(params) else {
        return Answer::plain(
            id,
            RuntimeErrorKind::InvalidRequest,
            "enrollment watch parameters are invalid",
        );
    };
    match enrollment_decision(&composed.store, *expected, &params.pending_id) {
        Ok(decision) => Answer::success(id, &decision),
        Err(failure) => Answer::failure(id, failure),
    }
}

pub(super) fn grant(
    state: &mut PublicState,
    composed: &Composed,
    id: JsonRpcId,
    params: serde_json::Value,
) -> Answer {
    if serde_json::from_value::<EmptyParams>(params).is_err() {
        return Answer::plain(
            id,
            RuntimeErrorKind::InvalidRequest,
            "integration grant parameters are invalid",
        );
    }
    match authorized(state, composed, None) {
        Ok(authority) => Answer::success(id, &authority.grant),
        Err(failure) => Answer::failure(id, failure),
    }
}

pub(super) async fn rotate_integration_key(
    state: &mut PublicState,
    composed: &Composed,
    id: JsonRpcId,
    params: serde_json::Value,
) -> Answer {
    let Ok(params) = serde_json::from_value::<RotateIntegrationKeyParams>(params) else {
        return Answer::plain(
            id,
            RuntimeErrorKind::InvalidRequest,
            "integration key rotation parameters are invalid",
        );
    };
    let authority = match authorized(state, composed, None) {
        Ok(authority) => authority.clone(),
        Err(failure) => return Answer::failure(id, failure),
    };
    let row = match composed.store.get_integration(authority.key) {
        Ok(Some(row)) => row,
        Ok(None) => {
            return Answer::plain(
                id,
                RuntimeErrorKind::Unauthenticated,
                "the integration grant no longer exists",
            );
        }
        Err(_) => {
            return Answer::plain(
                id,
                RuntimeErrorKind::Internal,
                "Runtime could not read integration authority",
            );
        }
    };
    let new_public_key = match crate::runtime_auth::verify_key_rotation(&authority, &row, &params) {
        Ok(key) => key,
        Err(failure) => return Answer::failure(id, failure),
    };
    if row.key_generation == params.expected_key_generation + 1 && row.public_key == new_public_key
    {
        return match crate::runtime_auth::grant(authority.grant.integration_id, &row) {
            Ok(grant) => Answer::success(id, &grant),
            Err(failure) => Answer::failure(id, failure),
        };
    }
    let confirmation = composed
        .integration_admin
        .request_key_rotation_confirmation(
            authority.key,
            &params.request_id,
            params.expected_key_generation,
            new_public_key,
        )
        .await;
    match confirmation {
        Ok(crate::integration_admin::Confirmation::Awaiting { confirmation_id }) => {
            return Answer::operator_action(
                id,
                RuntimeErrorKind::PresenceRequired,
                "approve the exact integration key replacement in Runtrol Studio, then retry this request",
                "reviewRuntimeRequestsInRuntrolStudio",
                confirmation_id.into(),
            );
        }
        Ok(crate::integration_admin::Confirmation::Confirmed) => {}
        Err(failure) => return confirmation_failure(id, failure, "key rotation"),
    }
    let outcome = composed.store.rotate_integration_key(
        authority.key,
        params.expected_key_generation,
        new_public_key,
    );
    if let Ok(IntegrationKeyRotation::Rotated(row) | IntegrationKeyRotation::Replayed(row)) =
        &outcome
        && composed
            .integration_authority
            .publish_committed(authority.key, row.clone())
            .is_err()
    {
        return Answer::plain(
            id,
            RuntimeErrorKind::Internal,
            "Runtime could not publish committed integration authority",
        );
    }
    key_rotation_answer(id, authority.grant.integration_id, outcome)
}

fn key_rotation_answer(
    id: JsonRpcId,
    integration_id: runtrol_runtime_protocol::IntegrationId,
    outcome: Result<IntegrationKeyRotation, runtrol_store::StoreError>,
) -> Answer {
    match outcome {
        Ok(IntegrationKeyRotation::Rotated(row)) => {
            match crate::runtime_auth::grant(integration_id, &row) {
                Ok(grant) => Answer::success_and_close(id, &grant),
                Err(failure) => Answer::failure(id, failure),
            }
        }
        Ok(IntegrationKeyRotation::Replayed(row)) => {
            match crate::runtime_auth::grant(integration_id, &row) {
                Ok(grant) => Answer::success(id, &grant),
                Err(failure) => Answer::failure(id, failure),
            }
        }
        Ok(IntegrationKeyRotation::Conflict) => Answer::plain(
            id,
            RuntimeErrorKind::IdempotencyConflict,
            "the integration key generation changed before this rotation committed",
        ),
        Ok(IntegrationKeyRotation::Missing) => Answer::plain(
            id,
            RuntimeErrorKind::Unauthenticated,
            "the integration grant no longer exists",
        ),
        Ok(IntegrationKeyRotation::Revoked) => Answer::failure(
            id,
            AuthorizationFailure {
                kind: RuntimeErrorKind::IntegrationRevoked,
                message: "the integration grant was revoked",
            },
        ),
        Err(_) => Answer::plain(
            id,
            RuntimeErrorKind::Internal,
            "Runtime could not rotate the integration key",
        ),
    }
}
