use super::*;
use crate::runtime_auth::ClientContext;
use crate::runtime_serve::connection_state::PublicAuthority;
use crate::serve::structured_worktree_tests::Fixture;
use runtrol_provider::{AbsPath, WallMs};
use runtrol_runtime_protocol::{
    ClientCapabilities, ClientInfo, IntegrationGrant, IntegrationId, JsonRpcResponse,
    MutationRequestId, REVISION_2026_08_27,
};
use runtrol_store::{IntegrationKey, IntegrationRootRow, IntegrationRow};

fn state(fixture: &Fixture) -> PublicState {
    let roots: Vec<_> = [
        &fixture.scratch.project,
        &fixture.owned,
        &fixture.structured,
    ]
    .into_iter()
    .map(|path| IntegrationRootRow {
        path: path.as_str().into(),
        identity: runtrol_security::ProjectRootIdentity::read(path)
            .unwrap()
            .to_bytes(),
    })
    .collect();
    let scopes = vec![AppScope::SessionStart, AppScope::SessionResume];
    let key = IntegrationKey::from_bytes([41; 16]);
    let row = IntegrationRow {
        public_key: [42; 32],
        client_instance_id: "structured-ownership-fixture".into(),
        label: "Structured ownership".into(),
        manifest_digest: [43; 32],
        scopes: scopes.iter().map(|scope| scope.as_str().into()).collect(),
        roots: roots.clone(),
        key_generation: 1,
        grant_generation: 1,
        approved_at: WallMs::now(),
        revoked_at: None,
    };
    fixture
        .composed
        .integration_authority
        .publish_committed(key, row)
        .unwrap();
    PublicState::Ready {
        context: ClientContext {
            challenge: crate::runtime_auth::challenge("structured-ownership-fixture").unwrap(),
            supported_revisions: vec![REVISION_2026_08_27],
            selected_revision: REVISION_2026_08_27,
            client: ClientInfo {
                name: "Structured ownership fixture".to_owned(),
                version: "1".to_owned(),
            },
            capabilities: ClientCapabilities::default(),
        },
        authority: PublicAuthority::Authorized(AuthorizedIntegration {
            key,
            grant: IntegrationGrant {
                integration_id: IntegrationId::new("structured-ownership-fixture"),
                scopes,
                roots: roots.iter().map(|root| root.path.to_string()).collect(),
                key_generation: 1,
                grant_generation: 1,
            },
            roots,
        }),
        token: crate::window_registry::ConnectionToken::next(),
    }
}

async fn public_requests(fixture: &Fixture, paths: &[AbsPath]) -> (Vec<JsonRpcResponse>, usize) {
    let mut state = state(fixture);
    let discovering = crate::serve::DiscoveryGates::new(&fixture.composed.registry);
    let cursors = NativeCursorCodec::new().unwrap();
    let (asking, mut asked) = mpsc::channel::<Box<RuntimeAsked>>(1);
    let (returning, mut returned) = mpsc::unbounded_channel();
    // The owner rejects admitted requests without handing back a provider opening. Counting this
    // channel proves both native adoption inspection and provider open remain unreachable on denial.
    let owner = tokio::spawn(async move {
        let mut count = 0;
        while let Some(request) = asked.recv().await {
            assert!(matches!(
                request.request,
                RuntimeControlRequest::PrepareOpen(_)
            ));
            count += 1;
            request
                .answered
                .send(RuntimeControlReply::Failed(RuntimeControlFailure::new(
                    RuntimeErrorKind::ProviderUnavailable,
                    "fixture owner stops before provider preparation",
                )))
                .unwrap_or_else(|_| panic!("the opening caller still owns its reply"));
        }
        count
    });
    let mut responses = Vec::new();
    for path in paths {
        let sessions = RuntimeSessionCatalogue::one_for_tests(
            runtrol_provider::ProviderId::parse("fixture").unwrap(),
            "different-native",
            path,
        );
        let request_id = MutationRequestId::now();
        for (method, params) in [
            (
                RuntimeMethod::SessionsStart,
                serde_json::json!({
                    "requestId": request_id, "providerId": "fixture",
                    "workspace": path.as_str(), "access": "exclusive"
                }),
            ),
            (
                RuntimeMethod::SessionsAdoptNative,
                serde_json::json!({
                    "requestId": request_id, "providerId": "fixture",
                    "workspace": path.as_str(), "access": "exclusive",
                    "nativeSessionId": "different-native", "adoptionToken": "fixture-proof"
                }),
            ),
            (
                RuntimeMethod::SessionsResume,
                serde_json::json!({
                    "requestId": request_id,
                    "sessionId": sessions.first_session_id_for_tests().unwrap().to_string(),
                    "expectedLifecycle": "cold", "expectedSessionGeneration": 0,
                    "workspace": path.as_str(), "access": "exclusive"
                }),
            ),
        ] {
            let answer = open_session(
                &mut state,
                &fixture.composed,
                &discovering,
                &cursors,
                &sessions,
                &asking,
                &returning,
                method,
                JsonRpcId::Number(1),
                params,
            )
            .await;
            assert!(!answer.close);
            responses.push(answer.response);
        }
    }
    drop(asking);
    let count = owner.await.unwrap();
    assert!(
        returned.try_recv().is_err(),
        "no provider process or adoption was attempted"
    );
    (responses, count)
}

#[tokio::test]
async fn public_structured_open_refuses_retained_terminal_worktrees_before_reservation() {
    let fixture = Fixture::new().await;
    let (responses, count) = public_requests(
        &fixture,
        &[fixture.owned.clone(), Fixture::child(&fixture.owned)],
    )
    .await;
    assert_eq!(
        count, 0,
        "even a grant for the worktree cannot bypass its owner"
    );
    for response in responses {
        let JsonRpcResponse::Error(response) = response else {
            panic!("the public wire must retain its existing error envelope");
        };
        assert_eq!(response.error.code, RuntimeErrorKind::WorkspaceConflict);
    }
    assert!(fixture.owned.as_std_path().join("kept.txt").exists());
}

#[tokio::test]
async fn public_structured_open_keeps_original_and_session_owned_workspaces() {
    let fixture = Fixture::new().await;
    let paths = [
        fixture.scratch.project.clone(),
        Fixture::child(&fixture.scratch.project),
        fixture.structured.clone(),
        Fixture::child(&fixture.structured),
    ];
    let (responses, count) = public_requests(&fixture, &paths).await;
    assert_eq!(count, paths.len() * 3);
    for response in responses {
        let JsonRpcResponse::Error(response) = response else {
            panic!("the fixture owner refuses before provider preparation");
        };
        assert_eq!(response.error.code, RuntimeErrorKind::ProviderUnavailable);
        assert_eq!(
            response.error.message,
            "fixture owner stops before provider preparation"
        );
    }
}
