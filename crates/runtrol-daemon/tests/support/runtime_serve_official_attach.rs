//! Public Runtime proof for a provider-owned official terminal attachment.

use std::io::BufRead as _;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use base64ct::{Base64, Encoding as _};
use tokio::sync::{mpsc, watch};

use super::*;

const PROVIDER: &str = "official-fixture";
const NATIVE: &str = "official-native-one";
const TARGET: &str = "official-target-one";
const READY_MARKER: &[u8] = b"official-attach-ready:official-target-one";
const SCRIPT_NAME: &str = "officialAttach.py";
const SCRIPT: &str = r#"from pathlib import Path
import os
import sys
import time

mode, target = sys.argv[1:]
owner = Path(f"{target}.owner")
stop = Path(f"{target}.stop")

if mode == "owner":
    owner.write_text(str(os.getpid()), encoding="utf-8")
    print("owner-ready", flush=True)
    while not stop.exists():
        time.sleep(0.01)
    owner.unlink(missing_ok=True)
    stop.unlink(missing_ok=True)
elif mode == "attach":
    if not owner.exists():
        raise SystemExit(2)
    os.write(sys.stdout.fileno(), b"official-attach-warmup")
    time.sleep(0.1)
    os.write(sys.stdout.fileno(), f"official-attach-ready:{target}".encode("utf-8"))
    for raw in sys.stdin.buffer:
        sys.stdout.buffer.write(b"echo:" + raw)
        sys.stdout.buffer.flush()
elif mode == "stop":
    Path("stopped-target.txt").write_text(target, encoding="utf-8")
    stop.write_text("stop", encoding="utf-8")
    deadline = time.monotonic() + 5
    while owner.exists() and time.monotonic() < deadline:
        time.sleep(0.01)
    if owner.exists():
        raise SystemExit(3)
else:
    raise SystemExit(4)
"#;

const MANIFEST: &str = r#"
schema = 1
id = "official-fixture"
display_name = "Official Attach Fixture"
kind = "official-fixture-kind"

[bin]
names = ["python", "python3"]

[probe]
version = { args = ["--version"], parse = "semver-anywhere" }

[transport]
argv = []
listen = "stdio"

[tui]
attach = ["-u", "officialAttach.py", "attach"]
stop = ["-u", "officialAttach.py", "stop"]
"#;

fn make_provider(context: &runtrol_drivers::DriverContext) -> Box<dyn runtrol_provider::Provider> {
    Box::new(OfficialProvider {
        provider: context.provider,
    })
}

const KINDS: &[runtrol_drivers::DriverKind] = &[runtrol_drivers::DriverKind {
    kind: "official-fixture-kind",
    make: Some(make_provider),
    flags: &[],
    consult: runtrol_drivers::ConsultSurface {
        registrar: None,
        server: None,
    },
    unavailable: None,
}];
const MANIFESTS: &[&str] = &[MANIFEST];

#[derive(Clone)]
struct LiveBinding {
    pid: u32,
    workspace: String,
}

static LIVE_BINDING: tokio::sync::Mutex<Option<LiveBinding>> = tokio::sync::Mutex::const_new(None);

struct BindingReset;

impl Drop for BindingReset {
    fn drop(&mut self) {
        if let Ok(mut binding) = LIVE_BINDING.try_lock() {
            *binding = None;
        }
    }
}

struct FixtureDirectory(Option<std::path::PathBuf>);

impl FixtureDirectory {
    fn remove(mut self) {
        let path = self.0.take().expect("the fixture directory is still owned");
        remove_fixture_directory(&path)
            .expect("remove the fixture Runtime home after every handle closes");
    }
}

impl Drop for FixtureDirectory {
    fn drop(&mut self) {
        if let Some(path) = self.0.take() {
            drop(remove_fixture_directory(&path));
        }
    }
}

fn remove_fixture_directory(path: &std::path::Path) -> std::io::Result<()> {
    const ATTEMPTS: usize = 40;
    for attempt in 0..ATTEMPTS {
        match std::fs::remove_dir_all(path) {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(_) if attempt + 1 < ATTEMPTS => {
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(error) => return Err(error),
        }
    }
    Err(std::io::Error::other(
        "the bounded fixture cleanup loop exhausted without an OS result",
    ))
}

struct OfficialProvider {
    provider: runtrol_provider::ProviderId,
}

#[async_trait::async_trait]
impl runtrol_provider::Provider for OfficialProvider {
    fn id(&self) -> runtrol_provider::ProviderId {
        self.provider
    }

    fn enumerates_machine(&self) -> bool {
        true
    }

    async fn native_sessions(
        &self,
        query: runtrol_provider::NativeSessionQuery,
    ) -> Result<runtrol_provider::NativeSessionCatalogue, runtrol_provider::ProviderError> {
        let binding = LIVE_BINDING.lock().await.clone();
        let workspace = query.root.map_or_else(
            || binding.map_or_else(|| "C:/fixture".to_owned(), |live| live.workspace),
            |root| root.to_string(),
        );
        Ok(runtrol_provider::NativeSessionCatalogue {
            coverage: runtrol_provider::NativeCatalogueCoverage::Complete {
                source: runtrol_provider::NativeCatalogueSource::OfficialProtocol,
            },
            sessions: vec![runtrol_provider::NativeSessionEntry {
                native: runtrol_provider::NativeSessionId::new(NATIVE)
                    .expect("the fixture native identity"),
                cwd: workspace.into(),
                additional_directories: Vec::new(),
                title: Some("Official attachment fixture".into()),
                updated_at: Some("2026-08-30T00:00:00Z".into()),
                resume: runtrol_provider::NativeResumeCapability::Available,
            }],
            next_cursor: None,
        })
    }

    async fn native_process_activity(
        &self,
    ) -> Result<runtrol_provider::NativeProcessActivity, runtrol_provider::ProviderError> {
        let binding = LIVE_BINDING.lock().await.clone();
        let Some(binding) = binding else {
            return Ok(runtrol_provider::NativeProcessActivity::default());
        };
        let native =
            runtrol_provider::NativeSessionId::new(NATIVE).expect("the fixture native identity");
        Ok(runtrol_provider::NativeProcessActivity {
            live: vec![native.clone()],
            active: Vec::new(),
            processes: vec![runtrol_provider::NativeProcessBinding {
                pid: binding.pid,
                native,
                cwd: Some(binding.workspace),
                terminal_access: runtrol_provider::NativeTerminalAccess::Official {
                    target: runtrol_provider::NativeTerminalTarget::new(TARGET)
                        .expect("the fixture terminal target"),
                },
            }],
        })
    }

    async fn open(
        &self,
        _intent: runtrol_provider::OpenIntent,
    ) -> Result<Box<dyn runtrol_provider::Agent>, runtrol_provider::ProviderError> {
        Err(runtrol_provider::ProviderError::Unsupported {
            provider: self.provider,
            what: "structured fixture open".to_owned(),
            why: "this fixture proves only the provider terminal attachment path",
        })
    }
}

struct OwnerProcess {
    child: Child,
}

impl OwnerProcess {
    fn start(program: &runtrol_childproc::Program, workspace: &std::path::Path) -> Self {
        let mut command = Command::new(program.path().as_std_path());
        command
            .args(program.leading())
            .args(["-u", SCRIPT_NAME, "owner", TARGET])
            .current_dir(workspace)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        runtrol_childproc::hide_console_window(&mut command);
        let mut child = command.spawn().expect("start the exact fixture owner");
        let stdout = child
            .stdout
            .take()
            .expect("capture the fixture owner readiness");
        let mut line = String::new();
        std::io::BufReader::new(stdout)
            .read_line(&mut line)
            .expect("read the fixture owner readiness");
        assert_eq!(line.trim(), "owner-ready");
        Self { child }
    }

    fn pid(&self) -> u32 {
        self.child.id()
    }

    fn is_running(&mut self) -> bool {
        self.child
            .try_wait()
            .expect("inspect the exact fixture owner")
            .is_none()
    }

    async fn wait_for_exit(&mut self) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while self.is_running() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("the official stop command ends the fixture owner");
    }
}

impl Drop for OwnerProcess {
    fn drop(&mut self) {
        if self.is_running() {
            let _result = self.child.kill();
            let _result = self.child.wait();
        }
    }
}

async fn next_output(
    view: &mut runtrol_runtime_client::TerminalView<'_>,
    needle: &[u8],
) -> Vec<u8> {
    tokio::time::timeout(Duration::from_secs(5), async {
        let mut received = Vec::new();
        loop {
            match view.next().await.expect("read exact terminal notification") {
                runtrol_runtime_client::TerminalNotification::Output { bytes, .. } => {
                    received.extend_from_slice(&bytes);
                    if received
                        .windows(needle.len())
                        .any(|window| window == needle)
                    {
                        return received;
                    }
                }
                runtrol_runtime_client::TerminalNotification::Lagged { .. } => {
                    panic!("the small fixture stream must not lag")
                }
                runtrol_runtime_client::TerminalNotification::Exited { exit_code } => {
                    panic!("the attachment renderer exited early with {exit_code}")
                }
            }
        }
    })
    .await
    .expect("terminal output arrives inside the public latency bound")
}

async fn next_index_count(
    index: &mut runtrol_runtime_client::TerminalIndexSubscription<'_>,
    expected: usize,
) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match index.next().await.expect("read the terminal index") {
                runtrol_runtime_client::TerminalIndexNotification::Changed(changed)
                    if changed.snapshot.terminals.len() == expected =>
                {
                    return;
                }
                runtrol_runtime_client::TerminalIndexNotification::Changed(_) => {}
                runtrol_runtime_client::TerminalIndexNotification::Ended(ended) => {
                    panic!("the terminal index ended early: {:?}", ended.reason)
                }
            }
        }
    })
    .await
    .expect("the terminal index change arrives without polling");
}

fn detach_params(
    opened: &runtrol_runtime_protocol::TerminalViewOpened,
) -> runtrol_runtime_protocol::TerminalDetachParams {
    runtrol_runtime_protocol::TerminalDetachParams {
        terminal_id: opened.terminal.terminal_id.clone(),
        view_id: opened.view_id.clone(),
    }
}

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "one public journey must keep catalogue proof, live binding, three views, drain, reopen and exact stop in sequence"
)]
async fn public_runtime_keeps_one_owner_behind_an_official_attachment() {
    let directory = std::env::temp_dir().join(format!(
        "runtrol-official-attach-{}-{}",
        std::process::id(),
        runtrol_provider::WallMs::now().as_millis()
    ));
    std::fs::create_dir_all(&directory).expect("create the fixture Runtime home");
    let directory_cleanup = FixtureDirectory(Some(directory.clone()));
    let workspace_path = directory.join("workspace");
    std::fs::create_dir(&workspace_path).expect("create the approved workspace");
    std::fs::write(workspace_path.join(SCRIPT_NAME), SCRIPT)
        .expect("write the fixture provider script");
    let workspace = runtrol_provider::AbsPath::canonicalize(
        workspace_path
            .to_str()
            .expect("the fixture workspace is UTF-8"),
    )
    .expect("canonicalize the fixture workspace");
    let workspace_identity = runtrol_security::ProjectRootIdentity::read(&workspace)
        .expect("read the fixture workspace identity")
        .to_bytes();
    let python = runtrol_childproc::resolve("python")
        .or_else(|_| runtrol_childproc::resolve("python3"))
        .expect("Python is installed for the repository test runner");
    let mut owner = OwnerProcess::start(&python, &workspace_path);
    *LIVE_BINDING.lock().await = Some(LiveBinding {
        pid: owner.pid(),
        workspace: workspace.to_string(),
    });
    let _binding_reset = BindingReset;

    let composed = Arc::new(
        crate::Composed::for_tests(
            directory.to_str().expect("the fixture home is UTF-8"),
            runtrol_drivers::Builtin {
                manifests: MANIFESTS,
                kinds: KINDS,
            },
        )
        .expect("compose the fixture Runtime"),
    );
    let provider = runtrol_provider::ProviderId::parse(PROVIDER).expect("the fixture provider id");
    if let Err(error) = crate::provider_prepare::prepared_terminal_driver(&composed, provider).await
    {
        panic!(
            "probe the fixture provider before publishing inventory: {}",
            error.message()
        );
    }

    let identity = crate::generations::GenerationIdentity::of_this_executable()
        .expect("the test runner measures itself");
    let endpoint = composed
        .home
        .paths()
        .generation_runtime_endpoint(identity.tag())
        .expect("the fixture generation endpoint")
        .address()
        .to_owned();
    let mut listener = runtrol_ipc::transport::Listener::bind_owner_only(&endpoint)
        .await
        .expect("bind the owner-only Runtime endpoint");
    let instance = "rtm_1123456789abcdef0123456789abcdef";
    let locator_path = directory.join("runtime.locator.json");
    let published = crate::generations::PublishedGeneration::publish(
        composed.home.paths(),
        instance,
        &identity,
        &endpoint,
        "control-endpoint-of-official-attach-test",
    )
    .await
    .expect("publish the fixture Runtime locator");

    let sessions = Arc::new(crate::runtime_inventory::RuntimeSessionCatalogue::empty_for_tests());
    let (session_publishing, session_watching) = watch::channel(sessions);
    let providers = Arc::new(crate::runtime_inventory::providers(&composed));
    assert!(providers.providers.iter().any(|entry| {
        entry.provider_id.as_str() == PROVIDER
            && entry.installation.state == runtrol_runtime_protocol::InstallationState::Usable
    }));
    let (provider_publishing, _provider_watching) = watch::channel(providers);
    let (usage_publishing, usage_watching) = watch::channel(Arc::new(
        runtrol_runtime_protocol::ProviderUsageList::default(),
    ));
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
        let runtime_asking = runtime_asking.clone();
        async move {
            let mut connections = tokio::task::JoinSet::new();
            for _ in 0..5 {
                let connection = listener.accept().await.expect("accept a public client");
                connections.spawn(serve_connection(
                    connection,
                    instance.to_owned(),
                    Arc::clone(&composed),
                    Arc::clone(&discovering),
                    Arc::clone(&native_cursors),
                    provider_publishing.clone(),
                    session_watching.clone(),
                    usage_watching.clone(),
                    runtime_asking.clone(),
                    runtime_returning.clone(),
                ));
            }
            while let Some(joined) = connections.join_next().await {
                joined.expect("the public connection task");
            }
        }
    });
    drop(runtime_asking);

    let locator = runtrol_runtime_client::RuntimeLocator::for_testing(&locator_path);
    let client_identity = runtrol_runtime_client::IntegrationIdentity::from_secret_bytes([7; 32]);
    let mut enrolling = locator
        .connect(
            runtrol_runtime_client::ClientOptions::new("official attach fixture", "1.0.0")
                .with_identity(client_identity.clone()),
        )
        .await
        .expect("initialize the enrollment client");
    let scopes = vec![
        AppScope::ProviderRead,
        AppScope::SessionList,
        AppScope::SessionNativeDiscover,
        AppScope::SessionOutputRead,
        AppScope::SessionResume,
        AppScope::SessionInputWrite,
        AppScope::SessionStop,
    ];
    let receipt = enrolling
        .integrations()
        .request(runtrol_runtime_client::EnrollmentProposal::new(
            "official-fixture-instance",
            [4; 32],
            scopes.clone(),
            vec![workspace.to_string()],
        ))
        .await
        .expect("request the fixture integration enrollment");
    let pending = crate::runtime_auth::enrollment_key(&receipt.pending_id)
        .expect("parse the fixture pending enrollment");
    let public_key =
        match base64ct::Base64UrlUnpadded::decode_vec(&client_identity.public_key_base64()) {
            Ok(bytes) => {
                <[u8; 32]>::try_from(bytes).expect("the integration public key is 32 bytes")
            }
            Err(error) => panic!("the integration public key must decode: {error}"),
        };
    let integration = runtrol_store::IntegrationKey::from_bytes([9; 16]);
    composed
        .store
        .approve_enrollment(
            pending,
            integration,
            &runtrol_store::IntegrationRow {
                public_key,
                client_instance_id: "official-fixture-instance".into(),
                label: "official attach fixture".into(),
                manifest_digest: [4; 32],
                scopes: scopes
                    .iter()
                    .map(|scope| Box::<str>::from(scope.as_str()))
                    .collect(),
                roots: vec![runtrol_store::IntegrationRootRow {
                    path: workspace.as_str().into(),
                    identity: workspace_identity,
                }],
                key_generation: 1,
                grant_generation: 1,
                approved_at: runtrol_provider::WallMs::now(),
                revoked_at: None,
            },
        )
        .expect("approve the exact fixture enrollment");
    let decision = enrolling
        .integrations()
        .watch(receipt.pending_id)
        .await
        .expect("read the approved fixture enrollment");
    let runtrol_runtime_protocol::EnrollmentDecision::Approved { grant } = decision else {
        panic!("the fixture enrollment must be approved")
    };
    let credentials = enrolling
        .credentials(grant)
        .expect("bind the fixture grant to its identity");
    drop(enrolling);

    let connect = || {
        locator.connect(
            runtrol_runtime_client::ClientOptions::new("official attach fixture", "1.0.0")
                .with_credentials(credentials.clone()),
        )
    };
    let (watching_result, first_result, second_result, third_result) =
        tokio::join!(connect(), connect(), connect(), connect());
    let mut watching_client = watching_result.expect("connect the terminal index watcher");
    let mut first_client = first_result.expect("connect the first terminal viewer");
    let mut second_client = second_result.expect("connect the second terminal viewer");
    let mut third_client = third_result.expect("connect the third terminal viewer");
    let mut watching_terminals = watching_client.terminals();
    let mut index = watching_terminals
        .watch_index()
        .await
        .expect("watch the public terminal index");
    assert!(index.started().snapshot.terminals.is_empty());

    let catalogue = first_client
        .providers()
        .list_native_sessions(runtrol_runtime_protocol::ListNativeSessionsParams {
            provider_id: runtrol_runtime_protocol::ProviderId::new(PROVIDER),
            root: Some(workspace.to_string()),
            cursor: None,
        })
        .await
        .expect("list the provider-owned fixture conversation");
    let listed = catalogue
        .sessions
        .first()
        .expect("one fixture conversation");
    assert_eq!(listed.native_session_id, NATIVE);
    let adoption_token = listed
        .adoption_token
        .clone()
        .expect("the native catalogue gives an adoption proof");
    assert_eq!(
        composed
            .open_terminals
            .load(std::sync::atomic::Ordering::Acquire),
        0,
        "an unviewed official conversation allocates no renderer"
    );

    let open_params = |request_id| runtrol_runtime_protocol::TerminalOpenParams {
        request_id,
        provider_id: runtrol_runtime_protocol::ProviderId::new(PROVIDER),
        workspace: workspace.to_string(),
        target: runtrol_runtime_protocol::TerminalOpenTarget::Native {
            native_session_id: NATIVE.to_owned(),
            adoption_token: adoption_token.clone(),
        },
        geometry: runtrol_runtime_protocol::TerminalGeometry {
            columns: 100,
            rows: 30,
        },
    };
    let mut first_terminals = first_client.terminals();
    let mut second_terminals = second_client.terminals();
    let mut third_terminals = third_client.terminals();
    let mut first_view = first_terminals
        .open(&open_params(
            runtrol_runtime_protocol::MutationRequestId::now(),
        ))
        .await
        .expect("open the official attachment through public Runtime");
    next_index_count(&mut index, 1).await;
    next_output(&mut first_view, READY_MARKER).await;
    let terminal_id = first_view.opened().terminal.terminal_id.clone();
    let renderer_claims = composed.native_claims.snapshot_except(None);
    assert_eq!(
        renderer_claims.len(),
        1,
        "one renderer exports one live claim"
    );
    let renderer_claim = renderer_claims.first().expect("the renderer claim");
    assert_eq!(renderer_claim.provider_id.as_ref(), PROVIDER);
    assert_eq!(renderer_claim.native_session_id.as_deref(), Some(NATIVE));
    assert_eq!(renderer_claim.workspace.as_ref(), workspace.as_str());
    assert_eq!(
        renderer_claim.surface,
        runtrol_ipc::GenerationLiveClaimSurface::Terminal
    );
    assert_eq!(renderer_claim.owner_id.as_ref(), terminal_id.as_str());
    let peer_claims = crate::native_claims::NativeLiveClaimRegistry::default();
    peer_claims.replace_remote("fixture-owner-generation", renderer_claims);
    assert!(matches!(
        peer_claims.reserve_terminal(
            runtrol_provider::TerminalId::now(),
            PROVIDER,
            Some(NATIVE),
            workspace.as_str(),
            false,
        ),
        Err(crate::native_claims::TerminalClaimError::TerminalAlreadyLive)
    ));
    let first_lease = first_view
        .opened()
        .control_lease
        .clone()
        .expect("the opening viewer receives input authority");
    let mut second_view = second_terminals
        .attach(&runtrol_runtime_protocol::TerminalAttachParams {
            terminal_id: terminal_id.clone(),
        })
        .await
        .expect("attach a second public viewer");
    let mut third_view = third_terminals
        .attach(&runtrol_runtime_protocol::TerminalAttachParams {
            terminal_id: terminal_id.clone(),
        })
        .await
        .expect("attach a third public viewer");
    assert_eq!(second_view.initial_screen(), third_view.initial_screen());
    assert!(
        second_view
            .initial_screen()
            .windows(b"official-attach-ready".len())
            .any(|window| window == b"official-attach-ready")
    );

    let nonce = b"public-parity-nonce\r";
    first_view
        .write(&runtrol_runtime_protocol::TerminalWriteParams {
            request_id: runtrol_runtime_protocol::MutationRequestId::now(),
            terminal_id: terminal_id.clone(),
            lease_id: first_lease.lease_id.clone(),
            lease_generation: first_lease.lease_generation,
            bytes_base64: Base64::encode_string(nonce),
        })
        .await
        .expect("write through the public terminal lease");
    let (first_bytes, second_bytes, third_bytes) = tokio::join!(
        next_output(&mut first_view, b"public-parity-nonce"),
        next_output(&mut second_view, b"public-parity-nonce"),
        next_output(&mut third_view, b"public-parity-nonce"),
    );
    assert_eq!(first_bytes, second_bytes);
    assert_eq!(second_bytes, third_bytes);

    let first_detach = detach_params(first_view.opened());
    let second_detach = detach_params(second_view.opened());
    let third_detach = detach_params(third_view.opened());
    first_view
        .detach(&first_detach)
        .await
        .expect("detach the first viewer without stopping the owner");
    second_view
        .detach(&second_detach)
        .await
        .expect("detach the second viewer without stopping the owner");
    third_view
        .detach(&third_detach)
        .await
        .expect("detach the third viewer without stopping the owner");
    crate::terminal_surface::close_idle_now_for_tests(&composed).await;
    next_index_count(&mut index, 0).await;
    assert!(
        composed.native_claims.snapshot_except(None).is_empty(),
        "draining the renderer retires its exact cross-generation claim"
    );
    assert!(
        owner.is_running(),
        "renderer drain must leave the external owner alive"
    );
    assert!(workspace_path.join(format!("{TARGET}.owner")).is_file());

    let mut reopened = first_terminals
        .open(&open_params(
            runtrol_runtime_protocol::MutationRequestId::now(),
        ))
        .await
        .expect("reopen a renderer for the same still-live owner");
    next_index_count(&mut index, 1).await;
    next_output(&mut reopened, READY_MARKER).await;
    assert_eq!(
        composed.native_claims.snapshot_except(None).len(),
        1,
        "reopening the renderer takes exactly one fresh claim"
    );
    let reopened_lease = reopened
        .opened()
        .control_lease
        .clone()
        .expect("the reopened renderer receives input authority");
    let reopened_terminal_id = reopened.opened().terminal.terminal_id.clone();
    reopened
        .stop(&runtrol_runtime_protocol::TerminalStopParams {
            request_id: runtrol_runtime_protocol::MutationRequestId::now(),
            terminal_id: reopened_terminal_id,
            lease_id: reopened_lease.lease_id,
            lease_generation: reopened_lease.lease_generation,
        })
        .await
        .expect("stop the owner through the exact official target");
    owner.wait_for_exit().await;
    assert_eq!(
        std::fs::read_to_string(workspace_path.join("stopped-target.txt"))
            .expect("read the exact stop target"),
        TARGET
    );
    drop(reopened);
    next_index_count(&mut index, 0).await;
    assert!(
        composed.native_claims.snapshot_except(None).is_empty(),
        "stopping the owner retires the renderer claim"
    );

    drop(index);
    drop(watching_client);
    drop(first_client);
    drop(second_client);
    drop(third_client);
    drop(session_publishing);
    drop(usage_publishing);
    serving.await.expect("the public server task finishes");
    owning.await.expect("the fixture Runtime owner finishes");
    drop(published);
    drop(discovering);
    drop(native_cursors);
    drop(composed);
    directory_cleanup.remove();
}
