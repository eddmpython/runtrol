//! Public Runtime proof that the terminal engine publishes only proved generic states (`STATE-01`).
//!
//! Every state transition a hosted terminal can make is replayed through the public protocol alone, and the
//! event sequence a client sees is held against the vocabulary the engine may publish
//! (`mainPlan/terminalTransportIntegrity`, observable state): `process_alive`, `owner_reachable`,
//! `view_count`, `lease_holder`, `output_flowing`, `checkpoint_available`, `lagged`, `message_pending` and
//! `process_exited`. Nothing else may appear: no working, stuck or turn state is derived from output, silence
//! or timing here, and a view's sequence is exact across an announced loss boundary.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use base64ct::{Base64, Encoding as _};
use runtrol_runtime_protocol::{AppScope, TerminalDescriptor, TerminalProcessState};
use tokio::sync::{mpsc, watch};

use crate::runtime_native_sessions::NativeCursorCodec;

use super::*;

const PROVIDER: &str = "replay-fixture";
const SCRIPT_NAME: &str = "stateReplay.py";
/// A small provider stand-in: it answers each line it is given, floods on request, and exits with the code it
/// is told. Nothing it prints is interpreted; the markers only let the test know where a stream is.
const SCRIPT: &str = r#"import sys

out = sys.stdout.buffer
out.write(b"replay-ready\n")
out.flush()
for raw in sys.stdin.buffer:
    line = raw.strip()
    if line.startswith(b"echo "):
        out.write(b"echo:" + line[5:] + b"\n")
        out.flush()
    elif line.startswith(b"flood "):
        count = int(line[6:])
        chunk = (b"x" * 4000) + b"\n"
        for _ in range(count):
            out.write(chunk)
        out.write(b"flood-done\n")
        out.flush()
    elif line.startswith(b"exit "):
        out.flush()
        sys.exit(int(line[5:]))
"#;

const MANIFEST: &str = r#"
schema = 1
id = "replay-fixture"
display_name = "State Replay Fixture"
kind = "replay-fixture-kind"

[bin]
names = ["python", "python3"]

[probe]
version = { args = ["--version"], parse = "semver-anywhere" }

[transport]
argv = []
listen = "stdio"

[tui]
new = ["-u", "stateReplay.py"]
"#;

/// The keys a public terminal descriptor may carry, each one a proved fact or an identity. A key outside this
/// set is a state the engine cannot prove, and the test names it.
const DESCRIPTOR_KEYS: &[&str] = &[
    "terminalId",
    "runtimeGeneration",
    "providerId",
    "workspace",
    "nativeSessionId",
    "processState",
    "openedAtMs",
    "terminalGeneration",
    "geometry",
    "controlGeneration",
    "controlHeld",
    "viewerCount",
    "origin",
    "ownerWindowSessionId",
    "ownerTerminalKey",
    "memoryBytes",
];

fn make_provider(context: &runtrol_drivers::DriverContext) -> Box<dyn runtrol_provider::Provider> {
    Box::new(ReplayProvider {
        provider: context.provider,
    })
}

const KINDS: &[runtrol_drivers::DriverKind] = &[runtrol_drivers::DriverKind {
    kind: "replay-fixture-kind",
    make: Some(make_provider),
    flags: &[],
    legacy_mcp: runtrol_drivers::LegacyMcpSurface::NONE,
    unavailable: None,
}];
const MANIFESTS: &[&str] = &[MANIFEST];

struct ReplayProvider {
    provider: runtrol_provider::ProviderId,
}

#[async_trait::async_trait]
impl runtrol_provider::Provider for ReplayProvider {
    fn id(&self) -> runtrol_provider::ProviderId {
        self.provider
    }

    fn enumerates_machine(&self) -> bool {
        true
    }

    async fn native_sessions(
        &self,
        _query: runtrol_provider::NativeSessionQuery,
    ) -> Result<runtrol_provider::NativeSessionCatalogue, runtrol_provider::ProviderError> {
        Ok(runtrol_provider::NativeSessionCatalogue {
            coverage: runtrol_provider::NativeCatalogueCoverage::Complete {
                source: runtrol_provider::NativeCatalogueSource::OfficialProtocol,
            },
            sessions: Vec::new(),
            next_cursor: None,
        })
    }

    async fn native_process_activity(
        &self,
    ) -> Result<runtrol_provider::NativeProcessActivity, runtrol_provider::ProviderError> {
        Ok(runtrol_provider::NativeProcessActivity::default())
    }

    async fn open(
        &self,
        _intent: runtrol_provider::OpenIntent,
    ) -> Result<Box<dyn runtrol_provider::Agent>, runtrol_provider::ProviderError> {
        Err(runtrol_provider::ProviderError::Unsupported {
            provider: self.provider,
            what: "structured fixture open".to_owned(),
            why: "this fixture proves only the hosted terminal state sequence",
        })
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

/// One public event as the replay records it, in order.
#[derive(Debug)]
enum Seen {
    Index {
        terminals: usize,
        state: Option<TerminalProcessState>,
        viewers: Option<u32>,
        control_held: Option<bool>,
        control_generation: Option<u64>,
    },
    Output {
        view: &'static str,
        sequence: u64,
    },
    Lagged {
        view: &'static str,
        lost: u64,
        next_sequence: u64,
    },
    Exited {
        view: &'static str,
        code: i32,
    },
}

fn descriptor_keys(descriptor: &TerminalDescriptor) -> BTreeSet<String> {
    let value = serde_json::to_value(descriptor).expect("a descriptor serialises");
    value
        .as_object()
        .expect("a descriptor is an object")
        .keys()
        .cloned()
        .collect()
}

/// Read the next output on a view until `needle` has been seen, recording every notification and holding the
/// view's sequence exact: each output is the one after the last, and a loss boundary announces the next one.
/// Across a loss boundary the replacement screen is the state so far: what was lost is on that screen or nowhere.
async fn drain_until(
    view: &mut runtrol_runtime_client::TerminalView<'_>,
    name: &'static str,
    needle: &[u8],
    expected_sequence: &mut u64,
    seen: &mut Vec<Seen>,
) -> Vec<u8> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let mut received: Vec<u8> = Vec::new();
    loop {
        let Ok(next) = tokio::time::timeout_at(deadline, view.next()).await else {
            let shown: Vec<u8> = received.iter().copied().take(600).collect();
            panic!(
                "{name}: {needle:?} did not arrive within the public latency bound; received {} bytes so far: {:?}",
                received.len(),
                String::from_utf8_lossy(&shown)
            );
        };
        match next.expect("read exact terminal notification") {
            runtrol_runtime_client::TerminalNotification::Output { sequence, bytes } => {
                assert_eq!(
                    sequence, *expected_sequence,
                    "{name}: output sequence is exact and gapless"
                );
                *expected_sequence += 1;
                seen.push(Seen::Output {
                    view: name,
                    sequence,
                });
                received.extend_from_slice(&bytes);
            }
            runtrol_runtime_client::TerminalNotification::Lagged {
                lost_chunks,
                next_sequence,
                screen,
            } => {
                assert!(
                    lost_chunks > 0,
                    "{name}: a loss boundary names what was lost"
                );
                assert!(
                    next_sequence >= *expected_sequence,
                    "{name}: the announced next sequence never runs backwards"
                );
                seen.push(Seen::Lagged {
                    view: name,
                    lost: lost_chunks,
                    next_sequence,
                });
                *expected_sequence = next_sequence;
                received.clear();
                received.extend_from_slice(&screen);
            }
            runtrol_runtime_client::TerminalNotification::Exited { exit_code } => {
                panic!("{name}: the fixture exited early with {exit_code}")
            }
        }
        if received
            .windows(needle.len())
            .any(|window| window == needle)
        {
            return received;
        }
    }
}

/// Read a view to its exit, holding the sequence exact on the way.
async fn drain_to_exit(
    view: &mut runtrol_runtime_client::TerminalView<'_>,
    name: &'static str,
    expected_sequence: &mut u64,
    seen: &mut Vec<Seen>,
) -> i32 {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let next = tokio::time::timeout_at(deadline, view.next())
            .await
            .expect("the exit arrives after the output drained")
            .expect("read exact terminal notification");
        match next {
            runtrol_runtime_client::TerminalNotification::Output { sequence, .. } => {
                assert_eq!(
                    sequence, *expected_sequence,
                    "{name}: exact sequence to the end"
                );
                *expected_sequence += 1;
                seen.push(Seen::Output {
                    view: name,
                    sequence,
                });
            }
            runtrol_runtime_client::TerminalNotification::Lagged {
                lost_chunks,
                next_sequence,
                ..
            } => {
                seen.push(Seen::Lagged {
                    view: name,
                    lost: lost_chunks,
                    next_sequence,
                });
                *expected_sequence = next_sequence;
            }
            runtrol_runtime_client::TerminalNotification::Exited { exit_code } => {
                seen.push(Seen::Exited {
                    view: name,
                    code: exit_code,
                });
                return exit_code;
            }
        }
    }
}

/// Wait for an index snapshot the predicate accepts, recording every snapshot on the way and holding each
/// descriptor to the published vocabulary.
async fn next_index_where(
    index: &mut runtrol_runtime_client::TerminalIndexSubscription<'_>,
    seen: &mut Vec<Seen>,
    accept: impl Fn(&[TerminalDescriptor]) -> bool,
) -> Vec<TerminalDescriptor> {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            match index.next().await.expect("read the terminal index") {
                runtrol_runtime_client::TerminalIndexNotification::Changed(changed) => {
                    let terminals = changed.snapshot.terminals;
                    for descriptor in &terminals {
                        let unknown: Vec<String> = descriptor_keys(descriptor)
                            .into_iter()
                            .filter(|key| !DESCRIPTOR_KEYS.contains(&key.as_str()))
                            .collect();
                        assert!(
                            unknown.is_empty(),
                            "the descriptor publishes a state outside the proved vocabulary: {unknown:?}"
                        );
                    }
                    let first = terminals.first();
                    seen.push(Seen::Index {
                        terminals: terminals.len(),
                        state: first.map(|t| t.process_state),
                        viewers: first.map(|t| t.viewer_count),
                        control_held: first.map(|t| t.control_held),
                        control_generation: first.map(|t| t.control_generation),
                    });
                    if accept(&terminals) {
                        return terminals;
                    }
                }
                runtrol_runtime_client::TerminalIndexNotification::Ended(ended) => {
                    panic!("the terminal index ended early: {:?}", ended.reason)
                }
            }
        }
    })
    .await
    .expect("the terminal index change arrives without polling")
}

// The daemon serves on a multi-thread runtime; a flood against an undrained view is replayed the same way.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[expect(
    clippy::too_many_lines,
    reason = "one public replay must walk every terminal state transition in sequence to hold the vocabulary"
)]
async fn the_terminal_engine_publishes_only_proved_generic_states() {
    let directory = std::env::temp_dir().join(format!(
        "runtrol-state-replay-{}-{}",
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
    let instance = "rtm_2123456789abcdef0123456789abcdef";
    let locator_path = directory.join("runtime.locator.json");
    let published = crate::generations::PublishedGeneration::publish(
        composed.home.paths(),
        instance,
        &identity,
        &endpoint,
        "control-endpoint-of-state-replay-test",
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
            let (audit, audit_writer) = crate::runtime_audit::journal(Arc::clone(&composed));
            connections.spawn(async move {
                audit_writer.await.expect("audit writer remained healthy");
            });
            // The enrolling client, the index watcher, two viewers and the stop-path viewer.
            for _ in 0..5 {
                let connection = listener.accept().await.expect("accept a public client");
                connections.spawn(serve_connection(
                    connection,
                    instance.to_owned(),
                    Arc::clone(&composed),
                    audit.clone(),
                    Arc::clone(&discovering),
                    Arc::clone(&native_cursors),
                    provider_publishing.clone(),
                    session_watching.clone(),
                    usage_watching.clone(),
                    runtime_asking.clone(),
                    runtime_returning.clone(),
                ));
            }
            drop(audit);
            while let Some(joined) = connections.join_next().await {
                joined.expect("the public connection task");
            }
        }
    });
    drop(runtime_asking);

    let locator = runtrol_runtime_client::RuntimeLocator::for_testing(&locator_path);
    let client_identity = runtrol_runtime_client::IntegrationIdentity::from_secret_bytes([8; 32]);
    let mut enrolling = locator
        .connect(
            runtrol_runtime_client::ClientOptions::new("state replay fixture", "1.0.0")
                .with_identity(client_identity.clone()),
        )
        .await
        .expect("initialize the enrollment client");
    let scopes = vec![
        AppScope::ProviderRead,
        AppScope::SessionList,
        AppScope::SessionNativeDiscover,
        AppScope::SessionOutputRead,
        AppScope::SessionStart,
        AppScope::SessionResume,
        AppScope::SessionInputWrite,
        AppScope::SessionStop,
    ];
    let receipt = enrolling
        .integrations()
        .request(runtrol_runtime_client::EnrollmentProposal::new(
            "state-replay-instance",
            [5; 32],
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
    let integration = runtrol_store::IntegrationKey::from_bytes([10; 16]);
    let approved_row = runtrol_store::IntegrationRow {
        public_key,
        client_instance_id: "state-replay-instance".into(),
        label: "state replay fixture".into(),
        manifest_digest: [5; 32],
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
    };
    composed
        .store
        .approve_enrollment(pending, integration, &approved_row)
        .expect("approve the exact fixture enrollment");
    composed
        .integration_authority
        .publish_committed(integration, approved_row)
        .expect("publish the committed fixture enrollment");
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
            runtrol_runtime_client::ClientOptions::new("state replay fixture", "1.0.0")
                .with_credentials(credentials.clone()),
        )
    };
    let (watching_result, first_result, second_result, third_result) =
        tokio::join!(connect(), connect(), connect(), connect());
    let mut watching_client = watching_result.expect("connect the terminal index watcher");
    let mut first_client = first_result.expect("connect the first terminal viewer");
    let mut second_client = second_result.expect("connect the second terminal viewer");
    // A view that saw its process exit ends with its connection, so the stop path opens on a connection of its own.
    let mut third_client = third_result.expect("connect the stop-path viewer");
    let mut watching_terminals = watching_client.terminals();
    let mut index = watching_terminals
        .watch_index()
        .await
        .expect("watch the public terminal index");
    assert!(index.started().snapshot.terminals.is_empty());
    let mut seen: Vec<Seen> = Vec::new();

    // process_alive, owner_reachable, view_count, lease_holder: a fresh open is one running Runtrol-owned terminal
    // with one view, whose opener holds control.
    let open_params = |request_id| runtrol_runtime_protocol::TerminalOpenParams {
        request_id,
        provider_id: runtrol_runtime_protocol::ProviderId::new(PROVIDER),
        workspace: workspace.to_string(),
        target: runtrol_runtime_protocol::TerminalOpenTarget::Fresh,
        geometry: runtrol_runtime_protocol::TerminalGeometry {
            columns: 100,
            rows: 30,
        },
    };
    let mut first_terminals = first_client.terminals();
    let mut second_terminals = second_client.terminals();
    let mut first_view = first_terminals
        .open(&open_params(
            runtrol_runtime_protocol::MutationRequestId::now(),
        ))
        .await
        .expect("open a fresh hosted terminal through public Runtime");
    let opened = first_view.opened().clone();
    assert!(
        opened.checkpoint_available,
        "a fresh view opens on a current checkpoint"
    );
    assert_eq!(opened.terminal.process_state, TerminalProcessState::Running);
    assert_eq!(
        opened.terminal.origin,
        runtrol_runtime_protocol::TerminalOrigin::Owned
    );
    assert!(opened.terminal.owner_window_session_id.is_none());
    let first_lease = opened
        .control_lease
        .clone()
        .expect("the opening viewer receives input authority");
    let terminal_id = opened.terminal.terminal_id.clone();
    let listed = next_index_where(&mut index, &mut seen, |terminals| {
        terminals.len() == 1
            && terminals
                .first()
                .is_some_and(|only| only.viewer_count == 1 && only.control_held)
    })
    .await;
    let listed_control_generation = listed
        .first()
        .expect("the opened terminal is listed")
        .control_generation;
    assert_eq!(
        listed_control_generation, 1,
        "the first lease is control generation one"
    );
    let mut first_sequence = 1_u64;
    drain_until(
        &mut first_view,
        "first",
        b"replay-ready",
        &mut first_sequence,
        &mut seen,
    )
    .await;

    // view_count and checkpoint_available: a second view raises the count and starts on the screen so far.
    let mut second_view = second_terminals
        .attach(&runtrol_runtime_protocol::TerminalAttachParams {
            terminal_id: terminal_id.clone(),
        })
        .await
        .expect("attach a second public viewer");
    assert!(second_view.opened().checkpoint_available);
    assert!(
        second_view
            .initial_screen()
            .windows(b"replay-ready".len())
            .any(|window| window == b"replay-ready"),
        "the late view's checkpoint is the screen so far"
    );
    next_index_where(&mut index, &mut seen, |terminals| {
        terminals.len() == 1 && terminals.first().is_some_and(|only| only.viewer_count == 2)
    })
    .await;
    let mut second_sequence = 1_u64;

    // output_flowing and message_pending: one write is one output, and a repeat of the same request identity
    // is answered from its record, never written a second time.
    let echo_request = runtrol_runtime_protocol::MutationRequestId::now();
    let write_echo = |request_id: runtrol_runtime_protocol::MutationRequestId| {
        runtrol_runtime_protocol::TerminalWriteParams {
            request_id,
            terminal_id: terminal_id.clone(),
            lease_id: first_lease.lease_id.clone(),
            lease_generation: first_lease.lease_generation,
            bytes_base64: Base64::encode_string(b"echo one\r"),
        }
    };
    first_view
        .write(&write_echo(echo_request.clone()))
        .await
        .expect("write through the public terminal lease");
    let first_bytes = drain_until(
        &mut first_view,
        "first",
        b"echo:one",
        &mut first_sequence,
        &mut seen,
    )
    .await;
    first_view
        .write(&write_echo(echo_request))
        .await
        .expect("a repeated request identity is answered from its record");
    drain_until(
        &mut second_view,
        "second",
        b"echo:one",
        &mut second_sequence,
        &mut seen,
    )
    .await;
    // A second `echo:one` would be the repeat written twice; after a settle nothing more has arrived on the
    // first view than the one answer, which the next distinct write proves by arriving first.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        first_bytes
            .windows(b"echo:one".len())
            .filter(|window| *window == b"echo:one")
            .count(),
        1,
        "one write is one output"
    );

    // lease_holder: releasing control publishes a held-less terminal; acquiring from another view transfers it
    // under a higher control generation, and the old lease no longer writes.
    first_view
        .release_control(&runtrol_runtime_protocol::TerminalControlParams {
            request_id: runtrol_runtime_protocol::MutationRequestId::now(),
            terminal_id: terminal_id.clone(),
            lease_id: first_lease.lease_id.clone(),
            lease_generation: first_lease.lease_generation,
        })
        .await
        .expect("release the first lease");
    next_index_where(&mut index, &mut seen, |terminals| {
        terminals.len() == 1 && terminals.first().is_some_and(|only| !only.control_held)
    })
    .await;
    let second_lease = second_view
        .acquire_control(&runtrol_runtime_protocol::TerminalAcquireControlParams {
            request_id: runtrol_runtime_protocol::MutationRequestId::now(),
            terminal_id: terminal_id.clone(),
            expected_terminal_generation: opened.terminal.terminal_generation,
        })
        .await
        .expect("the second view takes control");
    let held = next_index_where(&mut index, &mut seen, |terminals| {
        terminals.len() == 1 && terminals.first().is_some_and(|only| only.control_held)
    })
    .await;
    let held_control_generation = held
        .first()
        .expect("the held terminal is listed")
        .control_generation;
    assert!(
        held_control_generation > listed_control_generation,
        "a transfer is a higher control generation"
    );
    let stale = first_view
        .write(&write_echo(
            runtrol_runtime_protocol::MutationRequestId::now(),
        ))
        .await;
    assert!(stale.is_err(), "a released lease writes nothing");

    // output_flowing and lagged: the first view drains as it goes. The second stops taking output for a few
    // seconds inside the flood, well under the deadline that would end a stalled connection, so it falls a whole
    // ring behind and is told exactly what it lost and where live output resumes.
    second_view
        .write(&runtrol_runtime_protocol::TerminalWriteParams {
            request_id: runtrol_runtime_protocol::MutationRequestId::now(),
            terminal_id: terminal_id.clone(),
            lease_id: second_lease.lease_id.clone(),
            lease_generation: second_lease.lease_generation,
            bytes_base64: Base64::encode_string(b"flood 400\r"),
        })
        .await
        .expect("ask the fixture to flood");
    let flood_started = std::time::Instant::now();
    let mut first_seen = Vec::new();
    let mut second_seen = Vec::new();
    tokio::join!(
        drain_until(
            &mut first_view,
            "first",
            b"flood-done",
            &mut first_sequence,
            &mut first_seen
        ),
        async {
            tokio::time::sleep(Duration::from_secs(4)).await;
            drain_until(
                &mut second_view,
                "second",
                b"flood-done",
                &mut second_sequence,
                &mut second_seen,
            )
            .await;
        },
    );
    let flood_ms = flood_started.elapsed().as_millis();
    let second_lagged = second_seen
        .iter()
        .filter(|event| matches!(event, Seen::Lagged { view: "second", .. }))
        .count();
    assert!(
        second_lagged > 0,
        "a view that stopped taking output inside a flood crosses an announced loss boundary (flood took {flood_ms} ms)"
    );
    seen.extend(first_seen);
    seen.extend(second_seen);

    // view_count again: a detached view leaves the count, and nothing else, behind.
    let second_detach = runtrol_runtime_protocol::TerminalDetachParams {
        terminal_id: terminal_id.clone(),
        view_id: second_view.opened().view_id.clone(),
    };
    second_view
        .detach(&second_detach)
        .await
        .expect("detach the second viewer without stopping the provider");
    next_index_where(&mut index, &mut seen, |terminals| {
        terminals.len() == 1 && terminals.first().is_some_and(|only| only.viewer_count == 1)
    })
    .await;

    // process_exited: the provider ends on its own; the exit arrives after its output, then the index drops it.
    let first_lease_again = first_view
        .acquire_control(&runtrol_runtime_protocol::TerminalAcquireControlParams {
            request_id: runtrol_runtime_protocol::MutationRequestId::now(),
            terminal_id: terminal_id.clone(),
            expected_terminal_generation: opened.terminal.terminal_generation,
        })
        .await
        .expect("the first view takes control back");
    first_view
        .write(&runtrol_runtime_protocol::TerminalWriteParams {
            request_id: runtrol_runtime_protocol::MutationRequestId::now(),
            terminal_id: terminal_id.clone(),
            lease_id: first_lease_again.lease_id.clone(),
            lease_generation: first_lease_again.lease_generation,
            bytes_base64: Base64::encode_string(b"exit 3\r"),
        })
        .await
        .expect("ask the fixture to exit");
    let code = drain_to_exit(&mut first_view, "first", &mut first_sequence, &mut seen).await;
    assert_eq!(code, 3, "the exit code is the process's own");
    next_index_where(&mut index, &mut seen, <[TerminalDescriptor]>::is_empty).await;
    drop(first_view);

    // process_alive to stopping to exited: a stop asked under the lease is published as the stopping state on
    // its way out, and the view learns the exit.
    let mut third_terminals = third_client.terminals();
    let mut stopped_view = third_terminals
        .open(&open_params(
            runtrol_runtime_protocol::MutationRequestId::now(),
        ))
        .await
        .expect("open a second fresh terminal to stop");
    let stop_lease = stopped_view
        .opened()
        .control_lease
        .clone()
        .expect("the opener holds control");
    let stopped_id = stopped_view.opened().terminal.terminal_id.clone();
    next_index_where(&mut index, &mut seen, |terminals| {
        terminals.len() == 1
            && terminals
                .first()
                .is_some_and(|only| only.process_state == TerminalProcessState::Running)
    })
    .await;
    let mut stopped_sequence = 1_u64;
    drain_until(
        &mut stopped_view,
        "stopped",
        b"replay-ready",
        &mut stopped_sequence,
        &mut seen,
    )
    .await;
    stopped_view
        .stop(&runtrol_runtime_protocol::TerminalStopParams {
            request_id: runtrol_runtime_protocol::MutationRequestId::now(),
            terminal_id: stopped_id,
            lease_id: stop_lease.lease_id,
            lease_generation: stop_lease.lease_generation,
        })
        .await
        .expect("stop the hosted provider under the exact lease");
    drain_to_exit(
        &mut stopped_view,
        "stopped",
        &mut stopped_sequence,
        &mut seen,
    )
    .await;
    next_index_where(&mut index, &mut seen, <[TerminalDescriptor]>::is_empty).await;
    drop(stopped_view);

    // The whole sequence, held against the vocabulary: every event is one of the proved states, exits come after
    // the outputs of their view, and a stop was seen on its way out or already gone.
    let mut kinds = BTreeSet::new();
    for event in &seen {
        kinds.insert(match event {
            Seen::Index { .. } => "index",
            Seen::Output { .. } => "output",
            Seen::Lagged { .. } => "lagged",
            Seen::Exited { .. } => "exited",
        });
    }
    assert_eq!(
        kinds.into_iter().collect::<Vec<_>>(),
        vec!["exited", "index", "lagged", "output"],
        "only the proved kinds were published"
    );
    let stopping_seen = seen.iter().any(|event| {
        matches!(
            event,
            Seen::Index {
                state: Some(TerminalProcessState::Stopping),
                ..
            }
        )
    });
    // The recorded facts, read back as the sidebar would read them: never more than one terminal listed, the
    // view count reached two and fell back, control was held and released and re-held under a rising
    // generation, the loss boundary named what was lost, and the exits carried the process's own codes.
    let mut terminals_max = 0;
    let mut viewers_max = 0;
    let mut held_states = BTreeSet::new();
    let mut control_generation_max = 0;
    let mut lost_total = 0;
    let mut next_sequences = Vec::new();
    let mut exit_codes = Vec::new();
    let mut output_sequence_max = 0_u64;
    for event in &seen {
        match event {
            Seen::Index {
                terminals,
                viewers,
                control_held,
                control_generation,
                ..
            } => {
                terminals_max = terminals_max.max(*terminals);
                viewers_max = viewers_max.max(viewers.unwrap_or(0));
                if let Some(held) = control_held {
                    held_states.insert(*held);
                }
                control_generation_max =
                    control_generation_max.max(control_generation.unwrap_or(0));
            }
            Seen::Lagged {
                lost,
                next_sequence,
                ..
            } => {
                lost_total += lost;
                next_sequences.push(*next_sequence);
            }
            Seen::Exited { code, .. } => exit_codes.push(*code),
            Seen::Output { sequence, .. } => {
                output_sequence_max = output_sequence_max.max(*sequence);
            }
        }
    }
    assert!(
        output_sequence_max > 1,
        "more than one output chunk was published under exact sequences"
    );
    assert_eq!(terminals_max, 1, "one hosted terminal at a time was listed");
    assert_eq!(viewers_max, 2, "the view count reached two");
    assert_eq!(
        held_states.len(),
        2,
        "control was published both held and not held"
    );
    assert!(
        control_generation_max >= 3,
        "control moved under a rising generation"
    );
    assert!(lost_total > 0, "the loss boundary named what was lost");
    assert!(
        next_sequences.iter().all(|next| *next > 1),
        "a loss boundary resumes after the first chunk"
    );
    assert!(
        exit_codes.contains(&3),
        "the fixture's own exit code was published"
    );
    eprintln!(
        "state replay: {} events, stopping state observed: {stopping_seen}, lost {lost_total} chunks across {} boundaries, exits {exit_codes:?}",
        seen.len(),
        next_sequences.len()
    );
    for (position, event) in seen.iter().enumerate() {
        if let Seen::Exited { view, .. } = event {
            let later_output = seen
                .iter()
                .skip(position)
                .any(|later| matches!(later, Seen::Output { view: v, .. } if v == view));
            assert!(
                !later_output,
                "{view}: nothing is published on a view after its exit"
            );
        }
    }

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
