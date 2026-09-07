use super::*;
use base64ct::{Base64UrlUnpadded, Encoding as _};
use ed25519_dalek::{Signer as _, SigningKey};
use runtrol_provider::{TerminalId, WallMs};
use runtrol_runtime_protocol::{
    AppScope, ClientCapabilities, ClientInfo, EnrollmentDecision, IntegrationAuthentication,
    IntegrationGrant, JsonRpcResponse, MutationRequestId, initialization_signing_payload,
    key_rotation_signing_payload,
};
use runtrol_store::{
    EnrollmentKey, EnrollmentRow, EnrollmentState, IntegrationKey, IntegrationRow,
};

fn result(answer: Answer) -> serde_json::Value {
    match answer.response {
        JsonRpcResponse::Success(success) => success.result,
        JsonRpcResponse::Error(error) => panic!("fixture request failed: {:?}", error.error.code),
    }
}

#[tokio::test]
async fn every_grant_response_projects_without_changing_current_authority() {
    let directory = std::env::temp_dir().join(format!("grant-projection-{}", TerminalId::now()));
    std::fs::create_dir(&directory).unwrap();
    let composed =
        Composed::for_tests(directory.to_str().unwrap(), runtrol_drivers::builtin()).unwrap();
    let signing = SigningKey::from_bytes(&[71; 32]);
    let key = IntegrationKey::from_bytes([72; 16]);
    let pending = EnrollmentKey::from_bytes([73; 16]);
    let now = WallMs::now();
    let scopes: Vec<Box<str>> = vec![
        AppScope::SessionInputWrite.as_str().into(),
        AppScope::SessionInputPriority.as_str().into(),
    ];
    let row = IntegrationRow {
        public_key: SigningKey::from_bytes(&[75; 32]).verifying_key().to_bytes(),
        client_instance_id: "projection-fixture".into(),
        label: "Projection fixture".into(),
        manifest_digest: [74; 32],
        scopes: scopes.clone(),
        roots: Vec::new(),
        key_generation: 1,
        grant_generation: 1,
        approved_at: now,
        revoked_at: None,
    };
    composed
        .store
        .create_enrollment(
            pending,
            &EnrollmentRow {
                public_key: row.public_key,
                client_instance_id: row.client_instance_id.clone(),
                client_name: row.label.clone(),
                client_version: "1".into(),
                manifest_digest: row.manifest_digest,
                scopes,
                roots: Vec::new(),
                created_at: now,
                expires_at: now.plus_millis(30_000),
                state: EnrollmentState::Pending,
            },
        )
        .unwrap();
    composed
        .store
        .approve_enrollment(pending, key, &row)
        .unwrap();
    let IntegrationKeyRotation::Rotated(row) = composed
        .store
        .rotate_integration_key(key, 1, signing.verifying_key().to_bytes())
        .unwrap()
    else {
        panic!("fixture key rotation")
    };
    composed
        .integration_authority
        .publish_committed(key, row.clone())
        .unwrap();
    for understands in [false, true] {
        exercise(&composed, &signing, key, pending, &row, understands).await;
    }
    assert_eq!(
        composed.store.get_integration(key).unwrap(),
        Some(row.clone())
    );
    assert_eq!(
        composed.integration_authority.row(key).unwrap().scopes,
        row.scopes
    );
    drop(composed);
    std::fs::remove_dir_all(directory).unwrap();
}

async fn exercise(
    composed: &Composed,
    signing: &SigningKey,
    key: IntegrationKey,
    pending: EnrollmentKey,
    row: &IntegrationRow,
    understands: bool,
) {
    let capabilities = ClientCapabilities {
        terminal_input_priority: understands,
        ..ClientCapabilities::default()
    };
    let challenge = crate::runtime_auth::challenge("projection-fixture").unwrap();
    let mut params = InitializeParams {
        supported_revisions: FINALIZED_REVISIONS.to_vec(),
        client: ClientInfo {
            name: "Projection fixture".into(),
            version: "1".into(),
        },
        client_capabilities: capabilities.clone(),
        authentication: None,
    };
    let mut proof = IntegrationAuthentication {
        integration_id: crate::runtime_auth::integration_id(key),
        key_generation: row.key_generation,
        grant_generation: row.grant_generation,
        signature: String::new(),
    };
    let payload = initialization_signing_payload(
        &challenge,
        &params.supported_revisions,
        &params.client,
        &capabilities,
        &proof,
    )
    .unwrap();
    proof.signature = Base64UrlUnpadded::encode_string(&signing.sign(&payload).to_bytes());
    params.authentication = Some(proof);
    let mut state = PublicState::Fresh {
        challenge,
        token: crate::window_registry::ConnectionToken::next(),
    };
    let hello: InitializeResult = serde_json::from_value(result(
        initialize(
            &mut state,
            "projection-fixture",
            composed,
            JsonRpcId::Number(1),
            serde_json::to_value(params).unwrap(),
        )
        .await,
    ))
    .unwrap();
    let full = crate::runtime_auth::grant(crate::runtime_auth::integration_id(key), row).unwrap();
    let expected = capabilities.project_grant(full.clone());
    assert_eq!(hello.grant, Some(expected.clone()));
    let PublicState::Negotiated {
        context,
        authority,
        token,
    } = state
    else {
        panic!("negotiated state")
    };
    state = PublicState::Ready {
        context: context.clone(),
        authority,
        token,
    };
    let granted: IntegrationGrant = serde_json::from_value(result(grant(
        &mut state,
        composed,
        JsonRpcId::Number(2),
        serde_json::json!({}),
    )))
    .unwrap();
    assert_eq!(granted, expected);
    assert_eq!(
        authorized(&mut state, composed, Some(AppScope::SessionInputPriority))
            .unwrap()
            .grant,
        full
    );

    check_enrollment(composed, context, pending, token, &expected);
    exercise_rotation(
        composed,
        signing,
        &mut state,
        row,
        &full,
        &expected,
        &capabilities,
    )
    .await;
}

async fn exercise_rotation(
    composed: &Composed,
    signing: &SigningKey,
    state: &mut PublicState,
    row: &IntegrationRow,
    full: &IntegrationGrant,
    expected: &IntegrationGrant,
    capabilities: &ClientCapabilities,
) {
    let mut rotation = RotateIntegrationKeyParams {
        request_id: MutationRequestId::now(),
        expected_key_generation: 1,
        new_public_key: Base64UrlUnpadded::encode_string(&row.public_key),
        new_key_proof: String::new(),
    };
    let payload =
        key_rotation_signing_payload(&full.integration_id, full.grant_generation, &rotation)
            .unwrap();
    rotation.new_key_proof = Base64UrlUnpadded::encode_string(&signing.sign(&payload).to_bytes());
    let replayed: IntegrationGrant = serde_json::from_value(result(
        rotate_integration_key(
            state,
            composed,
            JsonRpcId::Number(4),
            serde_json::to_value(rotation).unwrap(),
        )
        .await,
    ))
    .unwrap();
    assert_eq!(&replayed, expected);
    for outcome in [
        IntegrationKeyRotation::Rotated(row.clone()),
        IntegrationKeyRotation::Replayed(row.clone()),
    ] {
        let answer = key_rotation_answer(
            JsonRpcId::Number(5),
            full.integration_id.clone(),
            Ok(outcome),
            capabilities,
        );
        let returned: IntegrationGrant = serde_json::from_value(result(answer)).unwrap();
        assert_eq!(&returned, expected);
    }
}

fn check_enrollment(
    composed: &Composed,
    context: ClientContext,
    pending: EnrollmentKey,
    token: crate::window_registry::ConnectionToken,
    expected: &IntegrationGrant,
) {
    let mut enrollment = PublicState::Ready {
        context,
        authority: PublicAuthority::Pending(pending),
        token,
    };
    let decision: EnrollmentDecision = serde_json::from_value(result(watch_integration(
        &mut enrollment,
        composed,
        JsonRpcId::Number(3),
        serde_json::to_value(WatchEnrollmentParams {
            pending_id: crate::runtime_auth::pending_id(pending),
        })
        .unwrap(),
    )))
    .unwrap();
    assert_eq!(
        decision,
        EnrollmentDecision::Approved {
            grant: expected.clone()
        }
    );
}
