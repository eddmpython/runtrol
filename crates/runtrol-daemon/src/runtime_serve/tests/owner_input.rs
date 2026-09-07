//! The private owner receiver is admitted only by its exact registering integration and capability.

use super::super::connection_state::{PublicAuthority, PublicState};
use super::super::window_requests::window_operation;
use crate::runtime_auth::ClientContext;
use crate::runtime_terminal::view_control_tests::Fixture;
use crate::window_registry::ConnectionToken;
use runtrol_runtime_protocol::{
    AppScope, ClientCapabilities, ClientInfo, JsonRpcId, JsonRpcResponse, REVISION_2026_08_27,
    RuntimeMethod, WatchWindowInputParams, WindowRegisterParams, WindowRegistration,
};

fn caller(fixture: &Fixture) -> PublicState {
    let mut authority = fixture.view.authority.clone();
    authority.grant.scopes.push(AppScope::SessionList);
    let mut row = fixture
        .composed
        .integration_authority
        .row(authority.key)
        .unwrap()
        .as_ref()
        .clone();
    if !row
        .scopes
        .iter()
        .any(|scope| scope.as_ref() == "session.list")
    {
        row.scopes.push("session.list".into());
        row.grant_generation += 1;
        fixture
            .composed
            .integration_authority
            .publish_committed(authority.key, row.clone())
            .unwrap();
    }
    authority.grant.grant_generation = row.grant_generation;
    PublicState::Ready {
        context: ClientContext {
            challenge: crate::runtime_auth::challenge("owner-input-fixture").unwrap(),
            supported_revisions: vec![REVISION_2026_08_27],
            selected_revision: REVISION_2026_08_27,
            client: ClientInfo {
                name: "Owner input fixture".into(),
                version: "1".into(),
            },
            capabilities: ClientCapabilities::default(),
        },
        authority: PublicAuthority::Authorized(authority),
        token: ConnectionToken::next(),
    }
}

#[tokio::test]
async fn owner_input_watch_requires_the_exact_private_registration() {
    let fixture = Fixture::new().await;
    let mut registration_connection = caller(&fixture);
    let registration = window_operation(
        &mut registration_connection,
        &fixture.composed,
        RuntimeMethod::WindowsRegister,
        JsonRpcId::Number(1),
        serde_json::to_value(WindowRegisterParams {
            window_session_id: "owner-input-window".into(),
            host_generation: "owner-host".into(),
            vscode_version: "fixture".into(),
            workspace_folders: vec![fixture.view.hosted.workspace.to_string()],
            host_pid: None,
        })
        .unwrap(),
    )
    .await;
    let JsonRpcResponse::Success(registration) = registration.response else {
        panic!("register the owner");
    };
    let registration: WindowRegistration = serde_json::from_value(registration.result).unwrap();
    let snapshot = serde_json::to_string(&fixture.composed.windows.snapshot().await).unwrap();
    let mut owner = caller(&fixture);
    let mut params = WatchWindowInputParams {
        window_session_id: "owner-input-window".into(),
        registration_generation: registration.registration_generation,
        owner_token: "forged".into(),
    };
    let denied = window_operation(
        &mut owner,
        &fixture.composed,
        RuntimeMethod::WindowsWatchInput,
        JsonRpcId::Number(2),
        serde_json::to_value(&params).unwrap(),
    )
    .await;
    params.owner_token = registration.owner_token.clone();
    let admitted = window_operation(
        &mut owner,
        &fixture.composed,
        RuntimeMethod::WindowsWatchInput,
        JsonRpcId::Number(3),
        serde_json::to_value(params).unwrap(),
    )
    .await;
    let accepted = admitted.watching.is_some();
    drop(admitted);
    fixture
        .composed
        .windows
        .forget_connection(registration_connection.token())
        .await;
    fixture.close().await;
    assert!(
        !snapshot.contains(&registration.owner_token),
        "the public window index never includes the owner capability"
    );
    assert!(
        matches!(denied.response, JsonRpcResponse::Error(_)),
        "a window id alone cannot claim the receiver"
    );
    assert!(
        accepted,
        "the exact registration proof must admit one dedicated owner receiver"
    );
}
