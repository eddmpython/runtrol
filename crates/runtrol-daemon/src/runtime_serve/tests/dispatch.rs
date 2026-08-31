use std::time::Duration;

use base64ct::Encoding as _;
use runtrol_runtime_protocol::{
    AppScope, ArchiveNativeSessionParams, JsonRpcNotification, ProviderCapabilityAvailability,
    ProviderCapabilityObservation, RuntimeModelCatalog, RuntimeSessionId,
};

use super::super::authority::required_scope;
use super::super::connection::serve_connection;
use super::super::provider_requests::{
    attachable_native_sessions, method_needs_provider_refresh, model_catalogue,
    provider_capabilities,
};
use super::super::session_control::{
    mode_within_manifest_vocabulary, mode_within_provider_vocabulary, reasoning_effort_is_current,
};
use super::super::session_requests::{
    NativeSessionMutation, forget_pointers_of, parse_native_session_mutation,
};
use super::super::watch_relay::event_notification_edges;

#[test]
fn native_archive_has_the_same_authority_and_exact_payload_boundary_as_delete() {
    let params = ArchiveNativeSessionParams {
        request_id: runtrol_runtime_protocol::MutationRequestId::now(),
        provider_id: runtrol_runtime_protocol::ProviderId::new("codex"),
        native_session_id: "thread-1".to_owned(),
        workspace: "C:\\work\\alpha".to_owned(),
    };
    let (mutation, parsed) = parse_native_session_mutation(
        RuntimeMethod::SessionsArchiveNative,
        serde_json::to_value(params).expect("archive parameters serialize"),
    )
    .expect("archive parameters stay inside their public DTO");
    assert!(matches!(mutation, NativeSessionMutation::Archive));
    assert_eq!(parsed.native_session_id, "thread-1");
    assert_eq!(
        required_scope(RuntimeMethod::SessionsArchiveNative),
        Some(AppScope::SessionDelete),
    );
}

#[test]
fn archive_capability_is_projected_without_guessing() {
    let provider_id = runtrol_runtime_protocol::ProviderId::new("provider-a");
    let projected = provider_capabilities(
        provider_id.clone(),
        runtrol_provider::ProviderCapabilities::unknown(),
    );
    assert_eq!(projected.provider_id, provider_id);
    assert!(matches!(
        projected.native_session_archive,
        Some(ProviderCapabilityObservation {
            availability: ProviderCapabilityAvailability::Unknown,
            ..
        })
    ));
}

/// After the provider deletes a conversation, every Runtrol pointer that named it goes too, and
/// nothing else does. Measured 2026-08-25 before this: two deleted Claude conversations lingered
/// as nameless rows that could be neither opened nor deleted again.
#[test]
fn deleting_a_native_conversation_forgets_only_its_own_pointers() {
    let scratch =
        std::env::temp_dir().join(format!("runtrol-forget-pointer-{}", std::process::id()));
    if scratch.exists() {
        std::fs::remove_dir_all(&scratch).expect("clear the previous run");
    }
    std::fs::create_dir(&scratch).expect("create the scratch home");
    let home = scratch.to_str().expect("UTF-8 scratch path");
    let composed = crate::Composed::for_tests(home, runtrol_drivers::builtin())
        .expect("a fresh home composes");
    let cwd =
        runtrol_provider::AbsPath::canonicalize(home).expect("the scratch home canonicalizes");
    let claude = runtrol_provider::ProviderId::parse("claude").expect("a builtin provider");
    let codex = runtrol_provider::ProviderId::parse("codex").expect("a builtin provider");
    let now = runtrol_provider::WallMs::now();
    let row = |provider, native: &str| runtrol_store::SessionRow {
        provider,
        native: runtrol_provider::NativeSessionId::new(native).expect("a valid native id"),
        cwd: cwd.clone(),
        label: None,
        created_at: now,
        last_seen_at: now,
        pinned: false,
        archived: false,
        forked_from: None,
        live: None,
    };
    let gone_a = runtrol_provider::SessionId::now();
    let gone_b = runtrol_provider::SessionId::now();
    let other_native = runtrol_provider::SessionId::now();
    let other_provider = runtrol_provider::SessionId::now();
    composed
        .store
        .put_session(gone_a, &row(claude, "deleted-one"))
        .expect("store");
    composed
        .store
        .put_session(gone_b, &row(claude, "deleted-one"))
        .expect("store");
    composed
        .store
        .put_session(other_native, &row(claude, "kept-one"))
        .expect("store");
    composed
        .store
        .put_session(other_provider, &row(codex, "deleted-one"))
        .expect("store");

    let deleted = runtrol_provider::NativeSessionId::new("deleted-one").expect("a valid native id");
    let forgotten =
        forget_pointers_of(&composed.store, claude, &deleted).expect("the store answers");
    assert_eq!(forgotten, 2, "both pointers to the deleted conversation go");
    let remaining: Vec<_> = composed
        .store
        .list_sessions()
        .expect("list")
        .sessions
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    assert_eq!(remaining, vec![other_native, other_provider]);
    assert_eq!(
        forget_pointers_of(&composed.store, claude, &deleted).expect("the store answers"),
        0,
        "a second deletion finds nothing to forget"
    );
    std::fs::remove_dir_all(&scratch).expect("clean the scratch home");
}

#[test]
fn a_mode_travels_only_within_the_provider_vocabulary() {
    // claude's manifest lists default, plan, and acceptEdits, and deliberately omits the modes that
    // remove safety prompts. A provider installed only through a local manifest has no mode list,
    // which defers the gate to its driver's session-announcement check. Both facts are what this test pins.
    let scratch = std::env::temp_dir().join(format!("runtrol-mode-gate-{}", std::process::id()));
    if scratch.exists() {
        std::fs::remove_dir_all(&scratch).expect("clear the previous run");
    }
    std::fs::create_dir(&scratch).expect("create the scratch home");
    let providers = scratch.join("providers");
    std::fs::create_dir(&providers).expect("create the local provider directory");
    std::fs::write(
        providers.join("fixture-acp-mode.toml"),
        r#"
schema = 1
id = "fixture-acp-mode"
display_name = "ACP Mode Fixture"
kind = "acp"

[bin]
names = ["fixture-acp-mode"]

[probe]
version = { args = ["--version"], parse = "semver-anywhere" }

[transport]
argv = []
listen = "stdio"
"#,
    )
    .expect("write the local provider manifest");
    let home = scratch.to_str().expect("UTF-8 scratch path");
    let composed = crate::Composed::for_tests(home, runtrol_drivers::builtin())
        .expect("a fresh home composes");
    let workspace =
        runtrol_provider::AbsPath::canonicalize(home).expect("the scratch home canonicalizes");

    let claude = runtrol_provider::ProviderId::parse("claude").expect("a builtin provider");
    let catalogue = crate::runtime_inventory::RuntimeSessionCatalogue::one_for_tests(
        claude,
        "native-mode-gate",
        &workspace,
    );
    let session = catalogue
        .first_session_id_for_tests()
        .expect("the fixture holds one session");
    assert!(
        mode_within_provider_vocabulary(&composed, &catalogue, session, "acceptEdits").is_ok(),
        "a manifest-listed mode travels"
    );
    assert!(
        mode_within_provider_vocabulary(&composed, &catalogue, session, "bypassPermissions")
            .is_err(),
        "the mode that removes every question must be unreachable through runtrol"
    );
    assert!(
        mode_within_provider_vocabulary(
            &composed,
            &catalogue,
            runtrol_provider::SessionId::now(),
            "default",
        )
        .is_err(),
        "an unidentifiable session fails closed"
    );

    let external =
        runtrol_provider::ProviderId::parse("fixture-acp-mode").expect("a local manifest provider");
    let announced = crate::runtime_inventory::RuntimeSessionCatalogue::one_for_tests(
        external,
        "native-mode-gate-acp",
        &workspace,
    );
    let acp_session = announced
        .first_session_id_for_tests()
        .expect("the fixture holds one session");
    assert!(
        mode_within_provider_vocabulary(&composed, &announced, acp_session, "anything").is_ok(),
        "an empty manifest list defers to the driver's session-announcement gate"
    );

    drop(composed);
    std::fs::remove_dir_all(&scratch).expect("remove the scratch home");
}

#[test]
fn starting_at_a_permission_mode_passes_the_exact_switch_boundary() {
    // sessions/start validates its permission through the same manifest function the mid-session
    // switch uses, so starting a session can never reach a mode that switching one could not.
    // plan is in claude's switchable list; the modes that remove safety prompts are not.
    let scratch =
        std::env::temp_dir().join(format!("runtrol-start-mode-gate-{}", std::process::id()));
    if scratch.exists() {
        std::fs::remove_dir_all(&scratch).expect("clear the previous run");
    }
    std::fs::create_dir(&scratch).expect("create the scratch home");
    let home = scratch.to_str().expect("UTF-8 scratch path");
    let composed = crate::Composed::for_tests(home, runtrol_drivers::builtin())
        .expect("a fresh home composes");
    let claude = runtrol_provider::ProviderId::parse("claude").expect("a builtin provider");
    assert!(
        mode_within_manifest_vocabulary(&composed, claude, "plan").is_ok(),
        "a session can start in plan mode"
    );
    for dangerous in ["bypassPermissions", "dontAsk", "auto"] {
        assert!(
            mode_within_manifest_vocabulary(&composed, claude, dangerous).is_err(),
            "{dangerous} must be unreachable at start exactly as it is at switch"
        );
    }
    drop(composed);
    std::fs::remove_dir_all(&scratch).expect("remove the scratch home");
}

use super::*;

const NATIVE_PROVIDER_MANIFEST: &str = r#"
schema = 1
id = "native-fixture"
display_name = "Native Fixture"
kind = "native-fixture-kind"

[bin]
names = ["rustc"]

[probe]
version = { args = ["--version"], parse = "semver-anywhere" }

[transport]
argv = []
listen = "stdio"
"#;

fn make_native_fixture(
    context: &runtrol_drivers::DriverContext,
) -> Box<dyn runtrol_provider::Provider> {
    Box::new(NativeFixtureProvider {
        provider: context.provider,
    })
}

const NATIVE_PROVIDER_KINDS: &[runtrol_drivers::DriverKind] = &[runtrol_drivers::DriverKind {
    kind: "native-fixture-kind",
    make: Some(make_native_fixture),
    flags: &[],
    consult: runtrol_drivers::ConsultSurface {
        registrar: None,
        server: None,
    },
    unavailable: None,
}];
const NATIVE_PROVIDER_MANIFESTS: &[&str] = &[NATIVE_PROVIDER_MANIFEST];

struct NativeFixtureProvider {
    provider: runtrol_provider::ProviderId,
}

struct NativeFixtureAgent {
    session: runtrol_provider::SessionId,
    native: String,
}

#[async_trait::async_trait]
impl runtrol_provider::Agent for NativeFixtureAgent {
    fn session(&self) -> runtrol_provider::SessionId {
        self.session
    }

    fn native(&self) -> Option<&str> {
        Some(&self.native)
    }

    async fn send(
        &mut self,
        _command: runtrol_provider::AgentCommand,
    ) -> Result<(), runtrol_provider::ProviderError> {
        Ok(())
    }

    async fn next(
        &mut self,
    ) -> Option<Result<runtrol_provider::Produced, runtrol_provider::ProviderError>> {
        core::future::pending().await
    }

    async fn close(
        self: Box<Self>,
        _how: runtrol_provider::CloseMode,
    ) -> Result<(), runtrol_provider::ProviderError> {
        Ok(())
    }
}

#[async_trait::async_trait]
impl runtrol_provider::Provider for NativeFixtureProvider {
    fn id(&self) -> runtrol_provider::ProviderId {
        self.provider
    }

    /// Stands in for the four measured providers that answer without a folder filter.
    fn enumerates_machine(&self) -> bool {
        true
    }

    async fn native_sessions(
        &self,
        query: runtrol_provider::NativeSessionQuery,
    ) -> Result<runtrol_provider::NativeSessionCatalogue, runtrol_provider::ProviderError> {
        let (native, next_cursor) = match query.cursor.as_deref() {
            None => ("fixture-native-one", Some("fixture-page-two".into())),
            Some("fixture-page-two") => ("fixture-native-two", None),
            Some(_) => {
                return Err(runtrol_provider::ProviderError::Protocol {
                    provider: self.provider,
                    doing: "listing fixture sessions",
                    detail: "the cursor is unknown".to_owned(),
                });
            }
        };
        Ok(runtrol_provider::NativeSessionCatalogue {
            coverage: runtrol_provider::NativeCatalogueCoverage::Complete {
                source: runtrol_provider::NativeCatalogueSource::OfficialProtocol,
            },
            sessions: vec![runtrol_provider::NativeSessionEntry {
                native: runtrol_provider::NativeSessionId::new(native)
                    .expect("valid fixture native identity"),
                // The fixture answers inside whatever folder it was asked about, and names
                // its own when asked about the machine.
                cwd: query
                    .root
                    .as_ref()
                    .map_or_else(|| "C:/fixture".to_owned(), |root| root.as_str().to_owned())
                    .into(),
                additional_directories: Vec::new(),
                title: Some("Provider-owned fixture title".into()),
                updated_at: Some("2026-08-13T00:00:00Z".into()),
                resume: runtrol_provider::NativeResumeCapability::Available,
            }],
            next_cursor,
        })
    }

    async fn open(
        &self,
        intent: runtrol_provider::OpenIntent,
    ) -> Result<Box<dyn runtrol_provider::Agent>, runtrol_provider::ProviderError> {
        let native = match &intent.disposition {
            runtrol_provider::Disposition::Fresh => intent.session.to_string(),
            runtrol_provider::Disposition::Resume { native } => native.to_string(),
            other => {
                return Err(runtrol_provider::ProviderError::Unsupported {
                    provider: self.provider,
                    what: format!("opening with {other:?}"),
                    why: "the fixture supports only fresh and resumed sessions",
                });
            }
        };
        Ok(Box::new(NativeFixtureAgent {
            session: intent.session,
            native,
        }))
    }
}

#[test]
fn public_event_wrapper_preserves_the_existing_event_bytes() {
    let session = RuntimeSessionId::new("session_fixture");
    let next = runtrol_runtime_protocol::EventCursor {
        stream: "019c0000-0000-7000-8000-000000000001".to_owned(),
        epoch: 2,
        seq: 9,
    };
    let event = br#"{"body":{"text":"exact caller and provider bytes"}}"#;
    let (prefix, suffix) =
        event_notification_edges("sub_fixture", &session, &next).expect("event edges");
    let mut frame = prefix;
    frame.extend_from_slice(event);
    frame.extend_from_slice(&suffix);
    let notification: JsonRpcNotification =
        serde_json::from_slice(&frame).expect("valid notification");
    assert_eq!(notification.method, RuntimeMethod::SessionsEvent.as_str());
    assert_eq!(
        notification.params.get("event"),
        Some(&serde_json::json!({
            "body": {"text": "exact caller and provider bytes"}
        }))
    );
    assert!(frame.windows(event.len()).any(|window| window == event));
}

#[test]
fn private_control_names_never_enter_the_public_method_table() {
    for private in [
        "hello",
        "list",
        "start",
        "providerUpdate",
        "private/control",
    ] {
        assert!(
            private.parse::<RuntimeMethod>().is_err(),
            "admitted {private:?}"
        );
    }
}

#[test]
fn provider_refresh_is_absent_from_authenticated_session_streams() {
    for method in [
        RuntimeMethod::ProvidersList,
        RuntimeMethod::ProvidersWatch,
        RuntimeMethod::ProvidersGetCapabilities,
        RuntimeMethod::ProvidersListModels,
        RuntimeMethod::ProvidersListNativeSessions,
        RuntimeMethod::SessionsStart,
        RuntimeMethod::SessionsAdoptNative,
        RuntimeMethod::SessionsResume,
    ] {
        assert!(
            method_needs_provider_refresh(method),
            "missing refresh for {method:?}"
        );
    }
    for method in [
        RuntimeMethod::Initialize,
        RuntimeMethod::IntegrationsWatchEnrollment,
        RuntimeMethod::ProvidersNativeActivity,
        RuntimeMethod::SessionsList,
        RuntimeMethod::SessionsWatchIndex,
        RuntimeMethod::SessionsWatchEvents,
    ] {
        assert!(
            !method_needs_provider_refresh(method),
            "unrelated request refreshes providers: {method:?}"
        );
    }
}

#[test]
fn provider_model_catalogue_preserves_opaque_choices_and_coverage() {
    let catalogue = model_catalogue(runtrol_provider::ModelCatalog::Partial {
        aliases: vec!["provider-alias".into()],
        models: vec![runtrol_provider::ModelChoice {
            id: "provider-model".into(),
            display_name: "Provider Model".into(),
            description: "Provider description".into(),
            is_default: true,
            reasoning_efforts: vec![runtrol_provider::ReasoningChoice {
                id: "provider-effort".into(),
                description: "Provider effort".into(),
            }],
        }],
        reasoning_efforts: vec![runtrol_provider::ReasoningChoice {
            id: "provider-global-effort".into(),
            description: "Provider global effort".into(),
        }],
        why: "the provider reports a partial list".into(),
    });
    let RuntimeModelCatalog::Partial {
        aliases,
        models,
        reasoning_efforts,
        why,
    } = catalogue
    else {
        panic!("coverage must remain partial");
    };
    assert_eq!(aliases, ["provider-alias"]);
    assert_eq!(
        reasoning_efforts
            .first()
            .expect("one mapped global reasoning effort")
            .id,
        "provider-global-effort"
    );
    let model = models.first().expect("one mapped model");
    assert_eq!(model.id, "provider-model");
    assert_eq!(
        model
            .reasoning_efforts
            .first()
            .expect("one mapped reasoning effort")
            .id,
        "provider-effort"
    );
    assert_eq!(why, "the provider reports a partial list");

    assert!(matches!(
        model_catalogue(runtrol_provider::ModelCatalog::unsupported(
            "no official discovery surface"
        )),
        RuntimeModelCatalog::Unsupported { why }
            if why == "no official discovery surface"
    ));
}

#[test]
fn reasoning_effort_validation_uses_the_current_provider_catalogue() {
    let catalogue = runtrol_provider::ModelCatalog::Partial {
        aliases: vec!["provider-alias".into()],
        models: vec![runtrol_provider::ModelChoice {
            id: "provider-model".into(),
            display_name: "Provider Model".into(),
            description: "Provider description".into(),
            is_default: true,
            reasoning_efforts: vec![runtrol_provider::ReasoningChoice {
                id: "model-effort".into(),
                description: "Model effort".into(),
            }],
        }],
        reasoning_efforts: vec![runtrol_provider::ReasoningChoice {
            id: "global-effort".into(),
            description: "Global effort".into(),
        }],
        why: "the provider reports a partial list".into(),
    };

    assert!(reasoning_effort_is_current(
        &catalogue,
        Some("provider-model"),
        "model-effort"
    ));
    assert!(!reasoning_effort_is_current(
        &catalogue,
        Some("provider-model"),
        "global-effort"
    ));
    assert!(reasoning_effort_is_current(
        &catalogue,
        Some("provider-alias"),
        "global-effort"
    ));
    assert!(!reasoning_effort_is_current(
        &catalogue,
        Some("provider-alias"),
        "missing-effort"
    ));
}

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "one real endpoint journey proves anonymous refusal, enrollment, approval, authenticated reconnect, inventory, and live revocation in sequence"
)]
async fn real_owner_only_runtime_initializes_but_reveals_nothing_before_enrollment() {
    let directory =
        std::env::temp_dir().join(format!("runtrol-runtime-public-{}", std::process::id()));
    drop(std::fs::remove_dir_all(&directory));
    std::fs::create_dir_all(&directory).expect("create Runtime test directory");
    let project_path = directory.join("project");
    std::fs::create_dir(&project_path).expect("create approved project");
    let project = runtrol_provider::AbsPath::canonicalize(
        project_path.to_str().expect("UTF-8 approved project"),
    )
    .expect("canonical approved project");
    let project_identity = runtrol_security::ProjectRootIdentity::read(&project)
        .expect("read approved project identity")
        .to_bytes();
    let start_project_path = directory.join("start-project");
    std::fs::create_dir(&start_project_path).expect("create session start project");
    let start_project = runtrol_provider::AbsPath::canonicalize(
        start_project_path
            .to_str()
            .expect("UTF-8 session start project"),
    )
    .expect("canonical session start project");
    let start_project_identity = runtrol_security::ProjectRootIdentity::read(&start_project)
        .expect("read session start project identity")
        .to_bytes();
    let resume_project_path = directory.join("resume-project");
    std::fs::create_dir(&resume_project_path).expect("create session resume project");
    let resume_project = runtrol_provider::AbsPath::canonicalize(
        resume_project_path
            .to_str()
            .expect("UTF-8 session resume project"),
    )
    .expect("canonical session resume project");
    let resume_project_identity = runtrol_security::ProjectRootIdentity::read(&resume_project)
        .expect("read session resume project identity")
        .to_bytes();
    let locator_path = directory.join("runtime.locator.json");
    let instance = "rtm_0123456789abcdef0123456789abcdef";
    let composed = Arc::new(
        crate::Composed::for_tests(
            directory.to_str().expect("UTF-8 Runtime test home"),
            runtrol_drivers::Builtin {
                manifests: NATIVE_PROVIDER_MANIFESTS,
                kinds: NATIVE_PROVIDER_KINDS,
            },
        )
        .expect("compose test Runtime"),
    );
    let identity = crate::generations::GenerationIdentity::of_this_executable()
        .expect("the test runner measures itself");
    let endpoint = composed
        .home
        .paths()
        .generation_runtime_endpoint(identity.tag())
        .expect("generation Runtime endpoint")
        .address()
        .to_owned();
    let mut listener = runtrol_ipc::transport::Listener::bind_owner_only(&endpoint)
        .await
        .expect("bind owner-only Runtime endpoint");
    let published = crate::generations::PublishedGeneration::publish(
        composed.home.paths(),
        instance,
        &identity,
        &endpoint,
        "control-endpoint-of-this-test",
    )
    .await
    .expect("publish owner-only locator");
    let fixture_provider =
        runtrol_provider::ProviderId::parse("native-fixture").expect("valid provider");
    let sessions = Arc::new(
        crate::runtime_inventory::RuntimeSessionCatalogue::one_for_tests(
            fixture_provider,
            "fixture-native-one",
            &resume_project,
        ),
    );
    let (provider_updates, _provider_updates_receiver) =
        watch::channel(Arc::new(crate::runtime_inventory::providers(&composed)));
    let (publishing, watching) = watch::channel(sessions.clone());
    let (usage_publishing, usage_watching) = watch::channel(Arc::new(ProviderUsageList::default()));
    let (runtime_asking, runtime_asked) = mpsc::channel(1);
    let (runtime_returning, runtime_returned) = mpsc::unbounded_channel();
    let owning = tokio::spawn(crate::runtime_control::fixture_runtime_owner(
        Arc::clone(&composed),
        runtime_asked,
        runtime_returned,
    ));
    let discovering = Arc::new(crate::serve::DiscoveryGates::new(&composed.registry));
    let native_cursors =
        Arc::new(NativeCursorCodec::new().expect("create native catalogue cursor authority"));
    let serving = tokio::spawn({
        let composed = Arc::clone(&composed);
        let discovering = Arc::clone(&discovering);
        let native_cursors = Arc::clone(&native_cursors);
        let provider_updates = provider_updates.clone();
        async move {
            let mut connections = tokio::task::JoinSet::new();
            let (audit, audit_writer) = crate::runtime_audit::journal(Arc::clone(&composed));
            connections.spawn(async move {
                audit_writer.await.expect("audit writer remained healthy");
            });
            for _ in 0..6 {
                let connection = listener.accept().await.expect("accept public client");
                connections.spawn(serve_connection(
                    connection,
                    instance.to_owned(),
                    Arc::clone(&composed),
                    audit.clone(),
                    Arc::clone(&discovering),
                    Arc::clone(&native_cursors),
                    provider_updates.clone(),
                    watching.clone(),
                    usage_watching.clone(),
                    runtime_asking.clone(),
                    runtime_returning.clone(),
                ));
            }
            drop(audit);
            while let Some(joined) = connections.join_next().await {
                joined.expect("public connection task");
            }
        }
    });

    let locator = runtrol_runtime_client::RuntimeLocator::for_testing(&locator_path);
    let identity = runtrol_runtime_client::IntegrationIdentity::from_secret_bytes([7; 32]);
    let mut client = locator
        .connect(
            runtrol_runtime_client::ClientOptions::new("contract fixture", "1.0.0")
                .with_identity(identity.clone()),
        )
        .await
        .expect("initialize public client");
    let refused = client
        .providers()
        .list()
        .await
        .expect_err("inventory requires enrollment");
    assert!(matches!(
        refused,
        runtrol_runtime_client::ClientError::Runtime(error)
            if error.code == RuntimeErrorKind::Unauthenticated
    ));

    let receipt = client
        .integrations()
        .request(runtrol_runtime_client::EnrollmentProposal::new(
            "fixture-instance",
            [3; 32],
            vec![
                AppScope::ProviderRead,
                AppScope::ModelRead,
                AppScope::SessionList,
                AppScope::SessionNativeDiscover,
                AppScope::SessionStart,
                AppScope::SessionResume,
                AppScope::SessionStop,
                AppScope::SessionDelete,
            ],
            vec![
                project.to_string(),
                start_project.to_string(),
                resume_project.to_string(),
            ],
        ))
        .await
        .expect("request enrollment");
    let pending =
        crate::runtime_auth::enrollment_key(&receipt.pending_id).expect("valid pending identity");
    let public_key = match base64ct::Base64UrlUnpadded::decode_vec(&identity.public_key_base64()) {
        Ok(bytes) => <[u8; 32]>::try_from(bytes).expect("32-byte public key"),
        Err(error) => panic!("identity public key must decode: {error}"),
    };
    let integration = runtrol_store::IntegrationKey::from_bytes([9; 16]);
    let approved_row = runtrol_store::IntegrationRow {
        public_key,
        client_instance_id: "fixture-instance".into(),
        label: "contract fixture".into(),
        manifest_digest: [3; 32],
        scopes: vec![
            AppScope::ProviderRead.as_str().into(),
            AppScope::ModelRead.as_str().into(),
            AppScope::SessionList.as_str().into(),
            AppScope::SessionNativeDiscover.as_str().into(),
            AppScope::SessionStart.as_str().into(),
            AppScope::SessionResume.as_str().into(),
            AppScope::SessionStop.as_str().into(),
            AppScope::SessionDelete.as_str().into(),
        ],
        roots: vec![
            runtrol_store::IntegrationRootRow {
                path: project.as_str().into(),
                identity: project_identity,
            },
            runtrol_store::IntegrationRootRow {
                path: start_project.as_str().into(),
                identity: start_project_identity,
            },
            runtrol_store::IntegrationRootRow {
                path: resume_project.as_str().into(),
                identity: resume_project_identity,
            },
        ],
        key_generation: 1,
        grant_generation: 1,
        approved_at: runtrol_provider::WallMs::now(),
        revoked_at: None,
    };
    composed
        .store
        .approve_enrollment(pending, integration, &approved_row)
        .expect("approve exact enrollment");
    composed
        .integration_authority
        .publish_committed(integration, approved_row)
        .expect("publish the committed enrollment");
    let decision = client
        .integrations()
        .watch(receipt.pending_id)
        .await
        .expect("watch approved enrollment");
    let runtrol_runtime_protocol::EnrollmentDecision::Approved { grant } = decision else {
        panic!("the exact enrollment should be approved");
    };
    let credentials = client
        .credentials(grant.clone())
        .expect("bind returned grant to identity");

    drop(client);
    let mut approved = locator
        .connect(
            runtrol_runtime_client::ClientOptions::new("contract fixture", "1.0.0")
                .with_credentials(credentials.clone()),
        )
        .await
        .expect("authenticate approved client");
    assert_eq!(
        approved
            .integrations()
            .grant()
            .await
            .expect("current grant"),
        grant
    );
    approved
        .providers()
        .list()
        .await
        .expect("approved provider inventory");
    let capabilities = approved
        .providers()
        .get_capabilities(runtrol_runtime_protocol::ProviderId::new("native-fixture"))
        .await
        .expect("approved provider capability discovery");
    assert_eq!(
        capabilities.fresh_session.availability,
        runtrol_runtime_protocol::ProviderCapabilityAvailability::Unknown
    );
    assert_eq!(
        capabilities.freshness,
        runtrol_runtime_protocol::CapabilityFreshness::Current
    );
    let first = approved
        .providers()
        .list_native_sessions(runtrol_runtime_protocol::ListNativeSessionsParams {
            provider_id: runtrol_runtime_protocol::ProviderId::new("native-fixture"),
            root: Some(project.to_string()),
            cursor: None,
        })
        .await
        .expect("first native catalogue page");
    assert_eq!(first.sessions.len(), 1);
    assert!(
        first
            .sessions
            .first()
            .is_some_and(|session| session.already_managed_as.is_some())
    );
    let managed_session = first
        .sessions
        .first()
        .and_then(|session| session.already_managed_as.clone())
        .expect("managed native fixture identity");
    let stored_session = managed_session
        .as_str()
        .parse::<runtrol_provider::SessionId>()
        .expect("managed Runtime session identity");
    let descriptor = approved
        .sessions()
        .get(managed_session.clone())
        .await
        .expect("read one exact managed session");
    assert_eq!(descriptor.session_id, managed_session);
    assert_eq!(
        descriptor.lifecycle,
        runtrol_runtime_protocol::LifecycleState::Cold
    );
    let now = runtrol_provider::WallMs::now();
    composed
        .store
        .put_session(
            stored_session,
            &runtrol_store::SessionRow {
                provider: fixture_provider,
                native: runtrol_provider::NativeSessionId::new("fixture-native-one")
                    .expect("fixture native identity"),
                cwd: resume_project.clone(),
                label: None,
                created_at: now,
                last_seen_at: now,
                pinned: false,
                archived: false,
                forked_from: None,
                live: None,
            },
        )
        .expect("store managed resume pointer");
    let resume_params = runtrol_runtime_protocol::ResumeSessionParams {
        request_id: runtrol_runtime_protocol::MutationRequestId::now(),
        session_id: managed_session.clone(),
        expected_lifecycle: runtrol_runtime_protocol::LifecycleState::Cold,
        expected_session_generation: 0,
        workspace: resume_project.to_string(),
        access: runtrol_runtime_protocol::SessionWorkspaceAccess::Exclusive,
        model: None,
        reasoning_effort: None,
    };
    let resumed = approved
        .sessions()
        .resume(&resume_params)
        .await
        .expect("resume an exact managed cold session");
    assert_eq!(resumed.session.session_id, managed_session);
    assert_eq!(
        approved
            .sessions()
            .resume(&resume_params)
            .await
            .expect("replay exact session resume"),
        resumed
    );
    let stale_resume = approved
        .sessions()
        .resume(&runtrol_runtime_protocol::ResumeSessionParams {
            request_id: runtrol_runtime_protocol::MutationRequestId::now(),
            session_id: managed_session,
            expected_lifecycle: runtrol_runtime_protocol::LifecycleState::Cold,
            expected_session_generation: 1,
            workspace: resume_project.to_string(),
            access: runtrol_runtime_protocol::SessionWorkspaceAccess::Exclusive,
            model: None,
            reasoning_effort: None,
        })
        .await
        .expect_err("stale resume generation is rejected");
    assert!(matches!(
        stale_resume,
        runtrol_runtime_client::ClientError::Runtime(error)
            if error.code == RuntimeErrorKind::SessionConflict
    ));
    let cool = runtrol_runtime_protocol::CoolSessionParams {
        request_id: runtrol_runtime_protocol::MutationRequestId::now(),
        session_id: resumed.session.session_id.clone(),
        expected_session_generation: resumed.session.session_generation,
        lease_id: resumed.control.lease_id.clone(),
        lease_generation: resumed.control.lease_generation,
    };
    approved
        .sessions()
        .cool(&cool)
        .await
        .expect("cool the exact idle resumed session");
    approved
        .sessions()
        .cool(&cool)
        .await
        .expect("replay the completed cool mutation");
    let cooled = approved
        .sessions()
        .get(resumed.session.session_id.clone())
        .await
        .expect("observe the cold pointer before requesting removal");
    let forget = runtrol_runtime_protocol::ForgetSessionParams {
        request_id: runtrol_runtime_protocol::MutationRequestId::now(),
        session_id: resumed.session.session_id.clone(),
        expected_session_generation: cooled.session_generation,
    };
    let presence = approved
        .sessions()
        .forget(&forget)
        .await
        .expect_err("forget requires the exact local approval action");
    assert!(
        matches!(
            &presence,
            runtrol_runtime_client::ClientError::Runtime(error)
                if error.code == RuntimeErrorKind::PresenceRequired
                && error.operator_action.as_deref()
                    == Some("reviewRuntimeRequestsInRuntrolStudio")
                    && error.correlation_id.starts_with("fgt_")
        ),
        "unexpected forget admission: {presence:?}"
    );
    let Ok(pending_forgets) = composed.integration_admin.forget_requests(&composed).await else {
        panic!("list exact forget for local presentation");
    };
    let pending_forget = pending_forgets.first().expect("one pending forget");
    assert_eq!(
        pending_forget.session_id.as_ref(),
        stored_session.to_string()
    );
    assert_eq!(
        pending_forget.integration_id.as_ref(),
        "int_09090909090909090909090909090909"
    );
    assert!(
        composed
            .integration_admin
            .confirm_forget(&pending_forget.confirmation_id)
            .await
            .is_ok(),
        "confirm exact forget through local administration"
    );
    let Ok(remaining_forgets) = composed.integration_admin.forget_requests(&composed).await else {
        panic!("list confirmed forget state");
    };
    assert!(remaining_forgets.is_empty());
    approved
        .sessions()
        .forget(&forget)
        .await
        .expect("retry exact forget after local close confirmation");
    approved
        .sessions()
        .forget(&forget)
        .await
        .expect("replay completed forget mutation");
    let denied_root = approved
        .providers()
        .list_native_sessions(runtrol_runtime_protocol::ListNativeSessionsParams {
            provider_id: runtrol_runtime_protocol::ProviderId::new("native-fixture"),
            root: Some(directory.to_string_lossy().into_owned()),
            cursor: None,
        })
        .await
        .expect_err("an unapproved root cannot reach provider discovery");
    assert!(matches!(
        denied_root,
        runtrol_runtime_client::ClientError::Runtime(error)
            if error.code == RuntimeErrorKind::RootDenied
    ));
    let mut tampered = first
        .next_cursor
        .clone()
        .expect("first page carries a cursor");
    tampered.push('x');
    let denied_cursor = approved
        .providers()
        .list_native_sessions(runtrol_runtime_protocol::ListNativeSessionsParams {
            provider_id: runtrol_runtime_protocol::ProviderId::new("native-fixture"),
            root: Some(project.to_string()),
            cursor: Some(tampered),
        })
        .await
        .expect_err("a modified cursor is rejected before provider discovery");
    assert!(matches!(
        denied_cursor,
        runtrol_runtime_client::ClientError::Runtime(error)
            if error.code == RuntimeErrorKind::InvalidRequest
    ));
    let second = approved
        .providers()
        .list_native_sessions(runtrol_runtime_protocol::ListNativeSessionsParams {
            provider_id: runtrol_runtime_protocol::ProviderId::new("native-fixture"),
            root: Some(project.to_string()),
            cursor: first.next_cursor,
        })
        .await
        .expect("second native catalogue page");
    assert_eq!(second.sessions.len(), 1);
    assert!(second.next_cursor.is_none());
    let native = second.sessions.first().expect("second native session");
    let adoption_token = native
        .adoption_token
        .clone()
        .expect("unmanaged resumable session has an adoption proof");

    let start_params = runtrol_runtime_protocol::StartSessionParams {
        request_id: runtrol_runtime_protocol::MutationRequestId::now(),
        provider_id: runtrol_runtime_protocol::ProviderId::new("native-fixture"),
        workspace: start_project.to_string(),
        access: runtrol_runtime_protocol::SessionWorkspaceAccess::Exclusive,
        model: None,
        reasoning_effort: None,
        permission: None,
    };
    let started = approved
        .sessions()
        .start(&start_params)
        .await
        .expect("start an authorized fresh session");
    let repeated = approved
        .sessions()
        .start(&start_params)
        .await
        .expect("replay the exact session start");
    assert_eq!(repeated, started);
    let mut changed_start = start_params.clone();
    changed_start.model = Some("changed-model".to_owned());
    let conflict = approved
        .sessions()
        .start(&changed_start)
        .await
        .expect_err("changed start parameters cannot reuse a mutation identity");
    assert!(matches!(
        conflict,
        runtrol_runtime_client::ClientError::Runtime(error)
            if error.code == RuntimeErrorKind::IdempotencyConflict
    ));
    let shared_start = runtrol_runtime_protocol::StartSessionParams {
        request_id: runtrol_runtime_protocol::MutationRequestId::now(),
        provider_id: runtrol_runtime_protocol::ProviderId::new("native-fixture"),
        workspace: start_project.to_string(),
        access: runtrol_runtime_protocol::SessionWorkspaceAccess::Shared,
        model: None,
        reasoning_effort: None,
        permission: None,
    };
    let shared = approved
        .sessions()
        .start(&shared_start)
        .await
        .expect_err("public shared writer admission requires local presence");
    assert!(
        matches!(
            &shared,
            runtrol_runtime_client::ClientError::Runtime(error)
                if error.code == RuntimeErrorKind::PresenceRequired
                && error.operator_action.as_deref()
                    == Some("reviewRuntimeRequestsInRuntrolStudio")
                && error.correlation_id.starts_with("sho_")
        ),
        "unexpected shared open admission: {shared:?}"
    );
    let Ok(pending_opens) = composed
        .integration_admin
        .shared_open_requests(&composed)
        .await
    else {
        panic!("list exact shared open for local presentation");
    };
    let pending_open = pending_opens.first().expect("one pending shared open");
    assert_eq!(pending_open.workspace.as_ref(), start_project.to_string());
    assert_eq!(pending_open.provider_id.as_ref(), "native-fixture");
    assert_eq!(pending_open.operation.as_ref(), "sessions/start");
    assert!(
        composed
            .integration_admin
            .confirm_shared_open(&pending_open.confirmation_id)
            .await
            .is_ok(),
        "confirm exact shared open through local administration"
    );
    let shared_opened = approved
        .sessions()
        .start(&shared_start)
        .await
        .expect("retry exact shared start after local confirmation");
    assert_ne!(shared_opened.session.session_id, started.session.session_id);

    let mut invalid_token = adoption_token.clone();
    invalid_token.push('x');
    let invalid_adoption = approved
        .sessions()
        .adopt_native(&runtrol_runtime_protocol::AdoptNativeSessionParams {
            request_id: runtrol_runtime_protocol::MutationRequestId::now(),
            provider_id: runtrol_runtime_protocol::ProviderId::new("native-fixture"),
            native_session_id: native.native_session_id.clone(),
            workspace: project.to_string(),
            access: runtrol_runtime_protocol::SessionWorkspaceAccess::Exclusive,
            adoption_token: invalid_token,
        })
        .await
        .expect_err("modified adoption proof is rejected");
    assert!(matches!(
        invalid_adoption,
        runtrol_runtime_client::ClientError::Runtime(error)
            if error.code == RuntimeErrorKind::CapabilityUnavailable
    ));
    let adopted = approved
        .sessions()
        .adopt_native(&runtrol_runtime_protocol::AdoptNativeSessionParams {
            request_id: runtrol_runtime_protocol::MutationRequestId::now(),
            provider_id: runtrol_runtime_protocol::ProviderId::new("native-fixture"),
            native_session_id: native.native_session_id.clone(),
            workspace: project.to_string(),
            access: runtrol_runtime_protocol::SessionWorkspaceAccess::Exclusive,
            adoption_token,
        })
        .await
        .expect("adopt an exact native catalogue observation");
    assert_eq!(adopted.session.provider_id.as_str(), "native-fixture");
    let unavailable = approved
        .providers()
        .list_models(runtrol_runtime_protocol::ProviderId::new("not-registered"))
        .await
        .expect_err("an unknown provider cannot supply a model catalogue");
    assert!(matches!(
        unavailable,
        runtrol_runtime_client::ClientError::Runtime(error)
            if error.code == RuntimeErrorKind::ProviderUnavailable
    ));

    let replacement = runtrol_runtime_client::IntegrationIdentity::from_secret_bytes([8; 32]);
    let rotation_request = runtrol_runtime_protocol::MutationRequestId::now();
    let rotation_presence = approved
        .integrations()
        .rotate_key(rotation_request.clone(), grant.key_generation, &replacement)
        .await
        .expect_err("integration key rotation requires exact local confirmation");
    assert!(matches!(
        rotation_presence,
        runtrol_runtime_client::ClientError::Runtime(error)
            if error.code == RuntimeErrorKind::PresenceRequired
            && error.operator_action.as_deref()
                == Some("reviewRuntimeRequestsInRuntrolStudio")
            && error.correlation_id.starts_with("rot_")
    ));
    assert_eq!(
        composed
            .store
            .get_integration(integration)
            .expect("read integration before local key confirmation")
            .expect("integration exists")
            .key_generation,
        1
    );
    let Ok(pending_rotations) = composed
        .integration_admin
        .key_rotation_requests(&composed)
        .await
    else {
        panic!("list exact key rotation for local presentation");
    };
    let pending_rotation = pending_rotations.first().expect("one pending key rotation");
    assert_eq!(pending_rotation.current_key_generation, 1);
    assert_eq!(
        pending_rotation.integration_id.as_ref(),
        "int_09090909090909090909090909090909"
    );
    assert!(
        composed
            .integration_admin
            .confirm_key_rotation(&pending_rotation.confirmation_id)
            .await
            .is_ok(),
        "confirm exact key rotation through local administration"
    );
    let rotated_credentials = approved
        .integrations()
        .rotate_key(rotation_request.clone(), grant.key_generation, &replacement)
        .await
        .expect("retry exact key rotation after local confirmation");
    assert_eq!(rotated_credentials.grant().key_generation, 2);
    drop(approved);
    let old_key = locator
        .connect(
            runtrol_runtime_client::ClientOptions::new("contract fixture", "1.0.0")
                .with_credentials(credentials.clone()),
        )
        .await;
    assert!(matches!(
        old_key,
        Err(runtrol_runtime_client::ClientError::Runtime(error))
            if error.code == RuntimeErrorKind::Unauthenticated
    ));
    let mut rotated = locator
        .connect(
            runtrol_runtime_client::ClientOptions::new("contract fixture", "1.0.0")
                .with_credentials(rotated_credentials.clone()),
        )
        .await
        .expect("the replacement key authenticates at its new generation");
    let replayed_credentials = rotated
        .integrations()
        .rotate_key(rotation_request, grant.key_generation, &replacement)
        .await
        .expect("the replacement key can replay the completed rotation");
    assert_eq!(replayed_credentials.grant(), rotated_credentials.grant());
    drop(rotated);
    let mut watching_client = locator
        .connect(
            runtrol_runtime_client::ClientOptions::new("contract fixture", "1.0.0")
                .with_credentials(rotated_credentials.clone()),
        )
        .await
        .expect("connect a dedicated index watcher");
    {
        let mut provider_watching_client = locator
            .connect(
                runtrol_runtime_client::ClientOptions::new("contract fixture", "1.0.0")
                    .with_credentials(rotated_credentials),
            )
            .await
            .expect("connect a dedicated provider watcher");
        let mut provider_client = provider_watching_client.providers();
        let mut provider_watch = provider_client
            .watch()
            .await
            .expect("watch the structural provider inventory");
        assert_eq!(provider_watch.started().snapshot.providers.len(), 1);
        // The account usage rides the same subscription: once at the start, then on every change,
        // so a surface never asks `providers/usage` on a clock.
        let first = tokio::time::timeout(Duration::from_secs(2), provider_watch.next())
            .await
            .expect("the initial usage snapshot arrives without polling")
            .expect("typed usage notification");
        assert!(matches!(
            first,
            runtrol_runtime_client::ProviderNotification::UsageChanged(
                runtrol_runtime_protocol::ProvidersUsageChangedNotification { .. }
            )
        ));
        let initial_providers = provider_watch.started().snapshot.clone();
        let mut changed_providers = initial_providers.clone();
        changed_providers
            .providers
            .first_mut()
            .expect("one fixture provider")
            .display_name = "Changed fixture provider".to_owned();
        provider_updates.send_replace(Arc::new(changed_providers));
        let changed = tokio::time::timeout(Duration::from_secs(2), provider_watch.next())
            .await
            .expect("provider changes arrive without polling")
            .expect("typed provider change notification");
        assert!(matches!(
            changed,
            runtrol_runtime_client::ProviderNotification::Changed(
                runtrol_runtime_protocol::ProvidersChangedNotification { .. }
            )
        ));
        usage_publishing.send_replace(Arc::new(ProviderUsageList {
            providers: vec![runtrol_runtime_protocol::ProviderUsageGauge {
                provider_id: runtrol_runtime_protocol::ProviderId::new("native-fixture"),
                reached: false,
                windows: Vec::new(),
                cost: None,
                tokens_today: Some(1234),
                at_ms: 1,
            }],
        }));
        let moved = tokio::time::timeout(Duration::from_secs(2), provider_watch.next())
            .await
            .expect("a usage change arrives without polling")
            .expect("typed usage notification");
        match moved {
            runtrol_runtime_client::ProviderNotification::UsageChanged(notification) => {
                assert_eq!(
                    notification
                        .snapshot
                        .providers
                        .first()
                        .and_then(|g| g.tokens_today),
                    Some(1234)
                );
            }
            other => panic!("expected the usage change, got {other:?}"),
        }

        {
            let mut session_client = watching_client.sessions();
            let mut index = session_client
                .watch_index()
                .await
                .expect("watch the authorized session index");
            assert_eq!(index.started().snapshot.sessions.len(), 1);
            let revoked = composed
                .store
                .revoke_integration(integration, runtrol_provider::WallMs::now())
                .expect("revoke integration")
                .expect("integration exists");
            composed
                .integration_authority
                .publish_revocation(integration, revoked)
                .expect("publish the committed revocation");
            publishing.send_replace(sessions);
            provider_updates.send_replace(Arc::new(initial_providers));
            let ended = tokio::time::timeout(Duration::from_secs(2), index.next())
                .await
                .expect("revocation retires the index watch without polling")
                .expect("typed index end notification");
            assert!(matches!(
                ended,
                runtrol_runtime_client::SessionIndexNotification::Ended(
                    runtrol_runtime_protocol::SessionIndexEndedNotification {
                        reason: runtrol_runtime_protocol::SessionIndexEndReason::IntegrationRevoked,
                        ..
                    }
                )
            ));
        }

        let provider_ended = async {
            for _ in 0..3 {
                let notification = provider_watch
                    .next()
                    .await
                    .expect("typed provider watch notification");
                if let runtrol_runtime_client::ProviderNotification::Ended(ended) = notification {
                    return ended;
                }
            }
            panic!("provider watch did not end after revocation");
        };
        let provider_ended = tokio::time::timeout(Duration::from_secs(2), provider_ended)
            .await
            .expect("revocation retires the provider watch without polling");
        assert_eq!(
            provider_ended.reason,
            runtrol_runtime_protocol::ProviderWatchEndReason::IntegrationRevoked
        );
    }
    drop(watching_client);
    drop(publishing);
    serving.await.expect("public server task finishes");
    owning.await.expect("Runtime owner task finishes");
    drop(published);
    drop(composed);
    drop(std::fs::remove_dir_all(directory));
}

#[test]
fn only_live_processes_with_a_safe_route_are_publicly_attachable() {
    use runtrol_provider::{
        NativeProcessActivity, NativeProcessBinding, NativeSessionId, NativeTerminalAccess,
        NativeTerminalTarget,
    };

    let native = |value| NativeSessionId::new(value).expect("a valid native identity");
    let process = |identity: &'static str, terminal_access| NativeProcessBinding {
        pid: 1,
        native: native(identity),
        cwd: Some("C:\\work".to_owned()),
        terminal_access,
    };
    let activity = NativeProcessActivity {
        live: vec![native("console"), native("official"), native("unavailable")],
        active: Vec::new(),
        processes: vec![
            process("console", NativeTerminalAccess::Console),
            process(
                "official",
                NativeTerminalAccess::Official {
                    target: NativeTerminalTarget::new("job-opaque-1")
                        .expect("a valid opaque target"),
                },
            ),
            process("unavailable", NativeTerminalAccess::Unavailable),
            process("stale", NativeTerminalAccess::Console),
        ],
    };

    assert_eq!(
        attachable_native_sessions(&activity),
        vec!["console".to_owned(), "official".to_owned()]
    );
}
