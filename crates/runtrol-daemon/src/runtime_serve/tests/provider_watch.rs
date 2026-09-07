use super::*;
use runtrol_ipc::transport::{Listener, connect};
use runtrol_provider::{TerminalId, WallMs};
use runtrol_runtime_protocol::{IntegrationGrant, IntegrationId};
use runtrol_store::{IntegrationKey, IntegrationRevocation, IntegrationRow};
use std::time::Duration;

struct Fixture {
    composed: Arc<Composed>,
    directory: std::path::PathBuf,
    row: IntegrationRow,
    authority: AuthorizedIntegration,
}

impl Fixture {
    fn new() -> Self {
        let directory = std::env::temp_dir().join(format!("provider-watch-{}", TerminalId::now()));
        std::fs::create_dir(&directory).unwrap();
        let composed = Arc::new(
            Composed::for_tests(directory.to_str().unwrap(), runtrol_drivers::builtin()).unwrap(),
        );
        let key = IntegrationKey::from_bytes([61; 16]);
        let scopes = vec![AppScope::ProviderRead];
        let row = IntegrationRow {
            public_key: [62; 32],
            client_instance_id: "provider-watch-fixture".into(),
            label: "Provider watch fixture".into(),
            manifest_digest: [63; 32],
            scopes: scopes.iter().map(|scope| scope.as_str().into()).collect(),
            roots: Vec::new(),
            key_generation: 1,
            grant_generation: 1,
            approved_at: WallMs::now(),
            revoked_at: None,
        };
        composed
            .integration_authority
            .publish_committed(key, row.clone())
            .unwrap();
        Self {
            composed,
            directory,
            row,
            authority: AuthorizedIntegration {
                key,
                grant: IntegrationGrant {
                    integration_id: IntegrationId::new("provider-watch-fixture"),
                    scopes,
                    roots: Vec::new(),
                    key_generation: 1,
                    grant_generation: 1,
                },
                roots: Vec::new(),
            },
        }
    }

    fn revoke(&self) {
        self.composed
            .integration_authority
            .publish_revocation(
                self.authority.key,
                IntegrationRevocation {
                    key_generation: 1,
                    grant_generation: self.row.grant_generation + 1,
                    revoked_at: WallMs::now(),
                    order: 1,
                },
            )
            .unwrap();
    }

    async fn pair(&self) -> (Connection, Connection) {
        #[cfg(windows)]
        let address = format!(r"\\.\pipe\provider-watch-{}", TerminalId::now());
        #[cfg(not(windows))]
        let address = self
            .directory
            .join("watch.sock")
            .to_str()
            .unwrap()
            .to_owned();
        let mut listener = Listener::bind_owner_only(&address).await.unwrap();
        let (client, server) = tokio::join!(connect(&address), listener.accept());
        (server.unwrap(), client.unwrap())
    }

    fn close(self) {
        drop(self.composed);
        std::fs::remove_dir_all(self.directory).unwrap();
    }
}

async fn next(client: &mut Connection) -> JsonRpcNotification {
    let bytes = tokio::time::timeout(Duration::from_secs(2), client.recv())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

fn assert_revoked(notification: JsonRpcNotification) {
    assert_eq!(
        notification.method,
        RuntimeMethod::ProvidersWatchEnded.to_string()
    );
    let ended: ProviderWatchEndedNotification =
        serde_json::from_value(notification.params).unwrap();
    assert_eq!(ended.reason, ProviderWatchEndReason::IntegrationRevoked);
}

#[tokio::test]
async fn provider_watch_checks_authority_before_its_initial_usage() {
    let fixture = Fixture::new();
    let (mut server, mut client) = fixture.pair().await;
    let (_, updates) = watch::channel(Arc::new(ProviderList {
        providers: Vec::new(),
    }));
    let (_, usage) = watch::channel(Arc::new(ProviderUsageList::default()));
    fixture.revoke();
    let relay = relay_providers(
        &mut server,
        &fixture.composed,
        "watch".into(),
        ProviderList {
            providers: Vec::new(),
        },
        updates,
        usage,
        fixture.authority.clone(),
    );
    let ((), notification) = tokio::join!(relay, next(&mut client));
    assert_revoked(notification);
    drop((server, client));
    fixture.close();
}

#[tokio::test]
async fn provider_watch_revocation_wakes_without_inventory_or_usage_changes() {
    revoked_watch(None).await;
}

#[tokio::test]
async fn provider_watch_revocation_wins_over_ready_usage() {
    revoked_watch(Some(true)).await;
}

#[tokio::test]
async fn provider_watch_revocation_wins_over_ready_inventory() {
    revoked_watch(Some(false)).await;
}

async fn revoked_watch(publish_usage: Option<bool>) {
    let fixture = Fixture::new();
    let (mut server, mut client) = fixture.pair().await;
    let (publishing, updates) = watch::channel(Arc::new(ProviderList {
        providers: Vec::new(),
    }));
    let (usage_publishing, usage) = watch::channel(Arc::new(ProviderUsageList::default()));
    let composed = Arc::clone(&fixture.composed);
    let authority = fixture.authority.clone();
    let relay = tokio::spawn(async move {
        relay_providers(
            &mut server,
            &composed,
            "watch".into(),
            ProviderList {
                providers: Vec::new(),
            },
            updates,
            usage,
            authority,
        )
        .await;
    });
    assert_eq!(
        next(&mut client).await.method,
        RuntimeMethod::ProvidersUsageChanged.to_string()
    );
    // No await separates revocation and data publication on this current-thread executor.
    fixture.revoke();
    if publish_usage == Some(false) {
        publishing.send_replace(Arc::new(ProviderList {
            providers: Vec::new(),
        }));
    }
    if publish_usage == Some(true) {
        usage_publishing.send_replace(Arc::new(ProviderUsageList {
            providers: vec![runtrol_runtime_protocol::ProviderUsageGauge {
                provider_id: runtrol_runtime_protocol::ProviderId::new("fixture"),
                reached: false,
                windows: Vec::new(),
                cost: None,
                tokens_today: Some(1234),
                at_ms: 1,
            }],
        }));
    }
    assert_revoked(next(&mut client).await);
    relay.await.unwrap();
    drop(client);
    fixture.close();
}

#[test]
fn provider_watch_retains_adjacent_authority_widening_witnesses() {
    let mut fixture = Fixture::new();
    for scope in [AppScope::ModelRead, AppScope::SessionList] {
        fixture.row.grant_generation += 1;
        fixture.row.scopes.push(scope.as_str().into());
        fixture
            .composed
            .integration_authority
            .publish_committed(fixture.authority.key, fixture.row.clone())
            .unwrap();
        refresh_provider_authority(&fixture.composed, &mut fixture.authority).unwrap();
        assert_eq!(
            fixture.authority.grant.grant_generation,
            fixture.row.grant_generation
        );
    }
    fixture.row.grant_generation += 1;
    fixture
        .row
        .scopes
        .retain(|scope| scope.as_ref() != AppScope::ProviderRead.as_str());
    fixture
        .composed
        .integration_authority
        .publish_committed(fixture.authority.key, fixture.row.clone())
        .unwrap();
    assert_eq!(
        refresh_provider_authority(&fixture.composed, &mut fixture.authority),
        Err(ProviderWatchEndReason::AuthorityChanged)
    );
    fixture.close();
}

#[tokio::test]
async fn native_activity_rechecks_authority_after_observation_waits() {
    use std::future::{Future as _, poll_fn};
    use std::task::Poll;

    use super::super::connection_state::{PublicAuthority, PublicState};
    use runtrol_runtime_protocol::{ClientCapabilities, ClientInfo, JsonRpcId, JsonRpcResponse};

    let mut fixture = Fixture::new();
    fixture.row.grant_generation += 1;
    fixture
        .row
        .scopes
        .push(AppScope::SessionNativeDiscover.as_str().into());
    fixture
        .composed
        .integration_authority
        .publish_committed(fixture.authority.key, fixture.row.clone())
        .unwrap();
    refresh_provider_authority(&fixture.composed, &mut fixture.authority).unwrap();
    let mut state = PublicState::Ready {
        context: crate::runtime_auth::ClientContext {
            challenge: crate::runtime_auth::challenge("native-activity-fixture").unwrap(),
            supported_revisions: vec![runtrol_runtime_protocol::REVISION_2026_08_27],
            selected_revision: runtrol_runtime_protocol::REVISION_2026_08_27,
            client: ClientInfo {
                name: "Native activity fixture".into(),
                version: "1".into(),
            },
            capabilities: ClientCapabilities::default(),
        },
        authority: PublicAuthority::Authorized(fixture.authority.clone()),
        token: crate::window_registry::ConnectionToken::next(),
    };
    let provider = runtrol_provider::ProviderId::parse("native-read-fixture").unwrap();
    let discovering = crate::serve::DiscoveryGates::new(&fixture.composed.registry);
    let lane = discovering.lane(provider).await.lock_owned().await;
    let mut request = Box::pin(super::super::provider_requests::native_activity(
        &mut state,
        &fixture.composed,
        &discovering,
        JsonRpcId::Number(1),
        serde_json::json!({"providerId": provider.as_str()}),
    ));
    poll_fn(|context| {
        assert!(
            request.as_mut().poll(context).is_pending(),
            "the authorized read waits on its provider lane"
        );
        Poll::Ready(())
    })
    .await;
    fixture.revoke();
    // Supply the bounded observer cache while the request is parked. This exercises the real read path
    // without starting or inspecting an installed provider process.
    assert!(
        !discovering
            .remember_native_activity(provider, runtrol_provider::NativeProcessActivity::default())
            .await
    );
    drop(lane);
    let answered = request.await;
    assert!(
        matches!(answered.response, JsonRpcResponse::Error(error) if error.error.code == RuntimeErrorKind::IntegrationRevoked)
    );
    drop(discovering);
    fixture.close();
}
