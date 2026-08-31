//! The public Runtime client a gate drives against two daemon generations.
//!
//! `tests/audit/generationHandover.py` starts one generation, lets this probe enroll and open a terminal in
//! it, starts a second generation so the first drains, and then asks this probe to attach to the first
//! generation's terminal through a brand new public connection. That attach is the product's promise that
//! a conversation kept alive across an update still opens (`docs/terminalSurface.md`, generation
//! continuity), and it is the request a draining generation refused for four days without any gate
//! noticing (measured 2026-08-29: "Runtime authorization audit storage is unavailable").
//!
//! Every phase is one process, so the connection it opens is a new connection, which is what a new window
//! makes. The identity and grant live in a small file between phases, as a real client keeps them.
//!
//! ```text
//! handoverProbe enroll <home> <runtrol exe> <identity file> <workspace>
//! handoverProbe open   <home> <identity file> <provider> <workspace>
//! handoverProbe open-native <home> <identity file> <provider> <native> <workspace>
//! handoverProbe find-native <home> <identity file> <provider> <native>
//! handoverProbe attach <home> <identity file> <digest> <terminal id>
//! handoverProbe stop   <home> <identity file> <digest> <terminal id>
//! ```
//!
//! Each phase prints one JSON object on stdout and exits non-zero with the failure on stderr.

use std::io::Write as _;
use std::path::Path;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use runtrol_runtime_client::protocol::{
    AppScope, EnrollmentDecision, InstallationState, IntegrationGrant, MutationRequestId,
    RuntimeTerminalId, TerminalAcquireControlParams, TerminalAttachParams, TerminalGeometry,
    TerminalOpenParams, TerminalOpenTarget, TerminalStopParams, TerminalWriteParams,
};
use runtrol_runtime_client::{
    ClientOptions, EnrollmentProposal, IntegrationCredentials, IntegrationIdentity, LocatorState,
    RuntimeClient, RuntimeLocator, ValidatedLocator,
};

const PROBE_NAME: &str = "runtrol-handover-probe";
const PROBE_VERSION: &str = "0.0.0";
/// How long an enrollment decision may take to settle after the self-approval was accepted.
const DECISION_SETTLE: Duration = Duration::from_secs(5);
/// How long a freshly started generation may take to observe a provider usable.
const PROVIDER_SETTLE: Duration = Duration::from_mins(1);
const TERMINAL_SETTLE: Duration = Duration::from_secs(15);
const GEOMETRY: TerminalGeometry = TerminalGeometry {
    columns: 100,
    rows: 30,
};

/// What one phase leaves for the next: the signing key and the approved grant.
struct Stored {
    secret: [u8; 32],
    grant: IntegrationGrant,
}

impl Stored {
    fn to_json(&self) -> Result<String, String> {
        let grant = serde_json::to_value(&self.grant).map_err(|error| error.to_string())?;
        Ok(serde_json::json!({ "secret": self.secret.to_vec(), "grant": grant }).to_string())
    }

    fn from_json(text: &str) -> Result<Self, String> {
        let value: serde_json::Value = serde_json::from_str(text)
            .map_err(|error| format!("parse the stored identity: {error}"))?;
        let bytes = value
            .get("secret")
            .and_then(serde_json::Value::as_array)
            .ok_or("the stored identity has no secret")?
            .iter()
            .map(|byte| match byte.as_u64().map(u8::try_from) {
                Some(Ok(byte)) => Some(byte),
                Some(Err(_)) | None => None,
            })
            .collect::<Option<Vec<u8>>>()
            .ok_or("the stored secret is not bytes")?;
        let secret: [u8; 32] = bytes
            .try_into()
            .map_err(|_| "the stored secret is not 32 bytes".to_owned())?;
        let grant: IntegrationGrant = serde_json::from_value(
            value
                .get("grant")
                .cloned()
                .ok_or("the stored identity has no grant")?,
        )
        .map_err(|error| format!("parse the stored grant: {error}"))?;
        Ok(Self { secret, grant })
    }
}

fn main() -> ExitCode {
    let words = std::env::args().skip(1).collect::<Vec<_>>();
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => return fail(&format!("tokio runtime: {error}")),
    };
    let outcome = runtime.block_on(async {
        match words
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .as_slice()
        {
            ["enroll", home, runtrol, identity, workspace] => {
                enroll(
                    Path::new(home),
                    Path::new(runtrol),
                    Path::new(identity),
                    workspace,
                )
                .await
            }
            ["open", home, identity, provider, workspace] => {
                open(Path::new(home), Path::new(identity), provider, workspace).await
            }
            ["open-native", home, identity, provider, native, workspace] => {
                open_native(
                    Path::new(home),
                    Path::new(identity),
                    provider,
                    native,
                    workspace,
                )
                .await
            }
            ["find-native", home, identity, provider, native] => {
                find_native(Path::new(home), Path::new(identity), provider, native).await
            }
            ["attach", home, identity, digest, terminal] => {
                attach(Path::new(home), Path::new(identity), digest, terminal).await
            }
            ["stop", home, identity, digest, terminal] => {
                stop(Path::new(home), Path::new(identity), digest, terminal).await
            }
            ["parity", home, identity, digest, terminal] => {
                parity(Path::new(home), Path::new(identity), digest, terminal, true).await
            }
            // Raw parity types the nonce without a carriage return: on a real coding CLI the bytes render
            // in its input line without submitting a prompt, so the journey costs no model turn.
            ["parity-raw", home, identity, digest, terminal] => {
                parity(
                    Path::new(home),
                    Path::new(identity),
                    digest,
                    terminal,
                    false,
                )
                .await
            }
            ["parity-navigation", home, identity, digest, terminal] => {
                parity_navigation(Path::new(home), Path::new(identity), digest, terminal).await
            }
            _ => Err(
                "usage: handoverProbe enroll|open|open-native|find-native|attach|stop|parity ..."
                    .to_owned(),
            ),
        }
    });
    match outcome {
        Ok(line) => {
            if writeln!(std::io::stdout().lock(), "{line}").is_err() {
                return ExitCode::FAILURE;
            }
            ExitCode::SUCCESS
        }
        Err(message) => fail(&message),
    }
}

fn fail(message: &str) -> ExitCode {
    // A failed write of the failure itself leaves only the exit code, which is still the answer.
    drop(writeln!(
        std::io::stderr().lock(),
        "handoverProbe: {message}"
    ));
    ExitCode::FAILURE
}

/// The locator of the home this probe was pointed at. `RUNTROL_HOME` is how every Runtime client finds it,
/// and the gate sets it; the `home` argument is the same folder, named again for the record it reads directly.
fn locator(home: &Path) -> Result<RuntimeLocator, String> {
    let expected = home.join("runtime.locator.json");
    if !expected.is_file() {
        return Err(format!("no locator at {}", expected.display()));
    }
    RuntimeLocator::system().map_err(|error| error.to_string())
}

/// The generation the locator names as current.
fn current(home: &Path) -> Result<ValidatedLocator, String> {
    match locator(home)?
        .inspect()
        .map_err(|error| error.to_string())?
    {
        LocatorState::Running(generation) => Ok(generation),
        LocatorState::NotInstalled => Err("no Runtime generation is listed".to_owned()),
    }
}

/// The exact generation running one digest, draining or not.
fn generation(home: &Path, digest: &str) -> Result<ValidatedLocator, String> {
    locator(home)?
        .inspect_all()
        .map_err(|error| error.to_string())?
        .into_iter()
        .find(|generation| generation.digest() == digest)
        .ok_or_else(|| format!("no listed Runtime generation runs digest {digest}"))
}

/// The private control endpoint of the generation running one digest, read from the locator record.
fn control_endpoint(home: &Path, digest: &str) -> Result<String, String> {
    let text = std::fs::read_to_string(home.join("runtime.locator.json"))
        .map_err(|error| format!("read the locator: {error}"))?;
    let record: serde_json::Value =
        serde_json::from_str(&text).map_err(|error| format!("parse the locator: {error}"))?;
    record
        .get("generations")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .find(|generation| {
            generation.get("digest").and_then(serde_json::Value::as_str) == Some(digest)
        })
        .and_then(|generation| generation.get("controlEndpoint"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("the locator lists no control endpoint for {digest}"))
}

fn read_stored(path: &Path) -> Result<Stored, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| format!("read {}: {error}", path.display()))?;
    Stored::from_json(&text)
}

fn credentials(stored: &Stored) -> IntegrationCredentials {
    IntegrationCredentials::new(
        IntegrationIdentity::from_secret_bytes(stored.secret),
        stored.grant.clone(),
    )
}

fn options_with(stored: &Stored) -> ClientOptions {
    ClientOptions::new(PROBE_NAME, PROBE_VERSION).with_credentials(credentials(stored))
}

/// Enroll a fresh identity with the current generation and approve it the way Studio approves its own.
async fn enroll(
    home: &Path,
    runtrol: &Path,
    identity_file: &Path,
    workspace: &str,
) -> Result<String, String> {
    let identity = IntegrationIdentity::generate().map_err(|error| error.to_string())?;
    let generation = current(home)?;
    let digest = generation.digest().to_owned();
    let mut client = RuntimeClient::connect_to(
        generation,
        ClientOptions::new(PROBE_NAME, PROBE_VERSION).with_identity(identity.clone()),
    )
    .await
    .map_err(|error| format!("connect for enrollment: {error}"))?;
    let receipt = client
        .integrations()
        .request(EnrollmentProposal::new(
            "handover-probe-instance",
            [7; 32],
            AppScope::ALL.to_vec(),
            vec![workspace.to_owned()],
        ))
        .await
        .map_err(|error| format!("request enrollment: {error}"))?;
    let signature = identity
        .self_approval_signature(&receipt.pending_id)
        .map_err(|error| error.to_string())?;
    let control = control_endpoint(home, &digest)?;
    let approved = runtrol_cli::request(
        &control,
        runtrol,
        runtrol_ipc::wire::Request::IntegrationSelfApprove {
            pending_id: receipt.pending_id.to_string().into_boxed_str(),
            signature: signature.into_boxed_str(),
        },
    )
    .await
    .map_err(|error| format!("self-approve over the control endpoint: {error}"))?;
    if !matches!(
        approved,
        runtrol_ipc::wire::Response::IntegrationApproved { .. }
    ) {
        return Err(format!("self-approval answered {approved:?}"));
    }
    let deadline = Instant::now() + DECISION_SETTLE;
    let grant = loop {
        match client
            .integrations()
            .watch(receipt.pending_id.clone())
            .await
            .map_err(|error| format!("watch the decision: {error}"))?
        {
            EnrollmentDecision::Approved { grant } => break grant,
            EnrollmentDecision::Pending if Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            other => return Err(format!("enrollment ended as {other:?}")),
        }
    };
    let stored = Stored {
        secret: identity.secret_bytes(),
        grant,
    };
    std::fs::write(identity_file, stored.to_json()?)
        .map_err(|error| format!("write {}: {error}", identity_file.display()))?;
    Ok(format!(r#"{{"enrolled":true,"generation":"{digest}"}}"#))
}

/// A generation probes its providers after it starts serving, so a provider named a moment after the start is
/// not yet observed usable. Wait for the observation rather than for a clock.
async fn wait_until_usable(client: &mut RuntimeClient, provider: &str) -> Result<(), String> {
    let deadline = Instant::now() + PROVIDER_SETTLE;
    loop {
        let listed = client
            .providers()
            .list()
            .await
            .map_err(|error| format!("list providers: {error}"))?;
        let usable = listed.providers.iter().any(|descriptor| {
            descriptor.provider_id.as_str() == provider
                && descriptor.installation.state == InstallationState::Usable
        });
        if usable {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "provider {provider} was not observed usable within {}s",
                PROVIDER_SETTLE.as_secs()
            ));
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// Open a fresh terminal for one provider in the current generation and leave it running with no viewer.
async fn open(
    home: &Path,
    identity_file: &Path,
    provider: &str,
    workspace: &str,
) -> Result<String, String> {
    let stored = read_stored(identity_file)?;
    let generation = current(home)?;
    let mut client = RuntimeClient::connect_to(generation, options_with(&stored))
        .await
        .map_err(|error| format!("connect to open: {error}"))?;
    wait_until_usable(&mut client, provider).await?;
    let params = TerminalOpenParams {
        request_id: MutationRequestId::now(),
        provider_id: runtrol_runtime_client::protocol::ProviderId::new(provider),
        workspace: workspace.to_owned(),
        target: TerminalOpenTarget::Fresh,
        geometry: GEOMETRY,
    };
    let mut terminals = client.terminals();
    let view = terminals
        .open(&params)
        .await
        .map_err(|error| format!("open a terminal: {error}"))?;
    let opened = view.opened();
    Ok(serde_json::json!({
        "terminalId": opened.terminal.terminal_id.as_str(),
        "generation": opened.terminal.runtime_generation,
        "provider": opened.terminal.provider_id.as_str(),
        "workspace": opened.terminal.workspace,
        "processState": format!("{:?}", opened.terminal.process_state),
        "screenBytes": view.initial_screen().len(),
    })
    .to_string())
}

/// Open one exact provider-native conversation through a fresh catalogue proof.
///
/// If the provider reports that another live process owns it, Runtime must attach through that process's
/// official peer rather than run the manifest's resume command. The returned descriptor proves which path won
/// without reading or writing conversation content.
async fn open_native(
    home: &Path,
    identity_file: &Path,
    provider: &str,
    native: &str,
    workspace: &str,
) -> Result<String, String> {
    let stored = read_stored(identity_file)?;
    let generation = current(home)?;
    let mut client = RuntimeClient::connect_to(generation, options_with(&stored))
        .await
        .map_err(|error| format!("connect to open the native terminal: {error}"))?;
    wait_until_usable(&mut client, provider).await?;
    let catalogue = client
        .providers()
        .list_native_sessions(runtrol_runtime_client::protocol::ListNativeSessionsParams {
            provider_id: runtrol_runtime_client::protocol::ProviderId::new(provider),
            root: Some(workspace.to_owned()),
            cursor: None,
        })
        .await
        .map_err(|error| format!("list the provider-native sessions: {error}"))?;
    let listed = catalogue
        .sessions
        .into_iter()
        .find(|session| session.native_session_id == native)
        .ok_or_else(|| format!("the native catalogue did not name {provider}/{native}"))?;
    let adoption_token = listed.adoption_token.ok_or_else(|| {
        format!("the native catalogue gave {provider}/{native} no adoption proof")
    })?;
    let params = TerminalOpenParams {
        request_id: MutationRequestId::now(),
        provider_id: runtrol_runtime_client::protocol::ProviderId::new(provider),
        workspace: workspace.to_owned(),
        target: TerminalOpenTarget::Native {
            native_session_id: native.to_owned(),
            adoption_token,
        },
        geometry: GEOMETRY,
    };
    let mut terminals = client.terminals();
    let view = terminals
        .open(&params)
        .await
        .map_err(|error| format!("open the provider-native terminal: {error}"))?;
    let opened = view.opened();
    Ok(serde_json::json!({
        "terminalId": opened.terminal.terminal_id.as_str(),
        "generation": opened.terminal.runtime_generation,
        "provider": opened.terminal.provider_id.as_str(),
        "native": opened.terminal.native_session_id,
        "workspace": opened.terminal.workspace,
        "processState": format!("{:?}", opened.terminal.process_state),
        "screenBytes": view.initial_screen().len(),
    })
    .to_string())
}

/// Find a terminal that the Runtime discovered from a provider-owned native process.
///
/// The caller knows no Runtime terminal identity. That is the state of a new window looking at a CLI started
/// elsewhere: the provider roster supplies the native identity, the daemon binds or mirrors it, and the public
/// terminal index becomes the only attach target. Wait for that published target and prove it already has a
/// screen without opening or resuming another provider process.
async fn find_native(
    home: &Path,
    identity_file: &Path,
    provider: &str,
    native: &str,
) -> Result<String, String> {
    let stored = read_stored(identity_file)?;
    let generation = current(home)?;
    let digest = generation.digest().to_owned();
    let mut client = RuntimeClient::connect_to(generation, options_with(&stored))
        .await
        .map_err(|error| format!("connect to find the native terminal: {error}"))?;
    wait_until_usable(&mut client, provider).await?;
    let deadline = Instant::now() + TERMINAL_SETTLE;
    loop {
        let mut terminals = client.terminals();
        let listed = terminals.list().await.map_err(|error| {
            format!("list terminals while finding {provider}/{native}: {error}")
        })?;
        if let Some(descriptor) = listed.terminals.into_iter().find(|descriptor| {
            descriptor.provider_id.as_str() == provider
                && descriptor.native_session_id.as_deref() == Some(native)
        }) {
            let view = terminals
                .attach(&TerminalAttachParams {
                    terminal_id: descriptor.terminal_id.clone(),
                })
                .await
                .map_err(|error| format!("attach the discovered terminal: {error}"))?;
            if !view.initial_screen().is_empty() {
                return Ok(serde_json::json!({
                    "terminalId": descriptor.terminal_id.as_str(),
                    "generation": digest,
                    "provider": descriptor.provider_id.as_str(),
                    "native": descriptor.native_session_id,
                    "workspace": descriptor.workspace,
                    "processState": format!("{:?}", descriptor.process_state),
                    "screenBytes": view.initial_screen().len(),
                })
                .to_string());
            }
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "no mirrored terminal for {provider}/{native} acquired a screen within {}s",
                TERMINAL_SETTLE.as_secs()
            ));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Attach to one terminal in the exact generation that owns it, on a connection that did not exist before.
async fn attach(
    home: &Path,
    identity_file: &Path,
    digest: &str,
    terminal: &str,
) -> Result<String, String> {
    let stored = read_stored(identity_file)?;
    let generation = generation(home, digest)?;
    let draining = generation.draining();
    let mut client = RuntimeClient::connect_to(generation, options_with(&stored))
        .await
        .map_err(|error| format!("connect to generation {digest}: {error}"))?;
    let terminal_id = terminal
        .parse::<RuntimeTerminalId>()
        .map_err(|error| format!("terminal id: {error}"))?;
    let mut terminals = client.terminals();
    let listed = terminals
        .list()
        .await
        .map_err(|error| format!("list terminals in {digest}: {error}"))?;
    let known = listed
        .terminals
        .iter()
        .any(|descriptor| descriptor.terminal_id.as_str() == terminal);
    let view = terminals
        .attach(&TerminalAttachParams { terminal_id })
        .await
        .map_err(|error| format!("attach in generation {digest}: {error}"))?;
    let state = format!("{:?}", view.opened().terminal.process_state);
    Ok(format!(
        r#"{{"attached":true,"generation":"{digest}","draining":{draining},"listed":{known},"processState":"{state}","screenBytes":{}}}"#,
        view.initial_screen().len()
    ))
}

/// Two live views of one terminal see the same session, and the remaining view survives a writer handoff.
///
/// This is the product's central promise measured over the public wire (goal 1.1): viewer A and viewer B
/// attach on connections that share nothing and only read. A third connection W takes the control lease and
/// types a nonce without submitting it. If a startup modal intentionally ignores printable input, the probe
/// falls back to a reversible Down then Up navigation pair. It never interprets provider text or assumes a
/// provider's menu ordering. Both live streams must carry the first input, two fresh attaches must read one
/// byte-identical changed screen, viewer A is then closed, and a new writer must change the screen observed by
/// viewer B. A view whose `next()` timed out is discarded: cancelling that future tears down its connection
/// (measured 2026-08-30, "connection ended" on the next command).
async fn parity(
    home: &Path,
    identity_file: &Path,
    digest: &str,
    terminal: &str,
    submit_line: bool,
) -> Result<String, String> {
    let stored = read_stored(identity_file)?;
    let terminal_id = terminal
        .parse::<RuntimeTerminalId>()
        .map_err(|error| format!("terminal id: {error}"))?;
    // Attach after startup settles. Initial snapshots are then the current provider screen, rather than the
    // launch handshake followed by a queue of old drawing operations.
    tokio::time::sleep(Duration::from_secs(8)).await;
    let mut client_a = RuntimeClient::connect_to(generation(home, digest)?, options_with(&stored))
        .await
        .map_err(|error| format!("connect viewer A: {error}"))?;
    let mut client_b = RuntimeClient::connect_to(generation(home, digest)?, options_with(&stored))
        .await
        .map_err(|error| format!("connect viewer B: {error}"))?;
    let mut terminals_a = client_a.terminals();
    let mut view_a = terminals_a
        .attach(&TerminalAttachParams {
            terminal_id: terminal_id.clone(),
        })
        .await
        .map_err(|error| format!("attach viewer A: {error}"))?;
    let mut terminals_b = client_b.terminals();
    let mut view_b = terminals_b
        .attach(&TerminalAttachParams {
            terminal_id: terminal_id.clone(),
        })
        .await
        .map_err(|error| format!("attach viewer B: {error}"))?;
    if view_a.initial_screen() != view_b.initial_screen() {
        return Err("the two initial viewer snapshots differ".to_owned());
    }
    let nonce = format!(
        "parity-{}-{}",
        std::process::id(),
        Instant::now().elapsed().as_nanos()
    );
    let mut typed = nonce.clone();
    if submit_line {
        typed.push_str("\r\n");
    }
    write_through_a_writer(&stored, home, digest, terminal_id.clone(), typed.as_bytes()).await?;
    let first_byte = Instant::now();
    let drained_a = drain_until(&mut view_a, nonce.as_bytes(), first_byte).await;
    let drained_b = drain_until(&mut view_b, nonce.as_bytes(), first_byte).await;
    if let Some(code) = drained_a.exited.or(drained_b.exited) {
        return Err(format!(
            "the terminal process exited (code {code}) during the journey; last screen bytes: {:?}",
            drained_a.tail
        ));
    }
    let (echoed_first, waited_first_ms) = (drained_a.seen, drained_a.waited_ms);
    let (echoed_second, waited_second_ms) = (drained_b.seen, drained_b.waited_ms);
    if !(submit_line || echoed_first && echoed_second) {
        // Printable bytes are deliberately ignored by trust and approval modals. The timed-out views are no
        // longer safe to reuse, so start the reversible navigation measurement on two new connections.
        drop(view_a);
        drop(client_a);
        drop(view_b);
        drop(client_b);
        return navigation_parity(&stored, home, digest, terminal_id).await;
    }
    // Let the echo land in the shared screen model before the fresh looks read it.
    tokio::time::sleep(Duration::from_millis(400)).await;
    let fresh_a = fresh_screen(&stored, home, digest, terminal_id.clone()).await?;
    let fresh_b = fresh_screen(&stored, home, digest, terminal_id.clone()).await?;
    let equal = fresh_a == fresh_b;
    let nonce_on_screen = contains(&fresh_a, nonce.as_bytes());
    if submit_line {
        return Ok(submitted_parity_result(
            &fresh_a,
            equal,
            echoed_first && echoed_second,
            nonce_on_screen,
            waited_first_ms,
            waited_second_ms,
        ));
    }

    // Closing A must not disturb B or pin the first writer's lease. A new connection writes a second nonce,
    // and the still-open B has to receive it before fresh readers agree on the resulting screen.
    drop(view_a);
    drop(client_a);
    raw_text_handoff(
        &stored,
        home,
        digest,
        terminal_id,
        &nonce,
        &mut view_b,
        TextParityFirst {
            equal,
            nonce_on_screen,
            waited_first_ms,
            waited_second_ms,
        },
    )
    .await
}

/// Measure two viewers and writer handoff without placing text in a provider-owned startup surface.
async fn parity_navigation(
    home: &Path,
    identity_file: &Path,
    digest: &str,
    terminal: &str,
) -> Result<String, String> {
    let stored = read_stored(identity_file)?;
    let terminal_id = terminal
        .parse::<RuntimeTerminalId>()
        .map_err(|error| format!("terminal id: {error}"))?;
    tokio::time::sleep(Duration::from_secs(8)).await;
    navigation_parity(&stored, home, digest, terminal_id).await
}

fn submitted_parity_result(
    screen: &[u8],
    equal: bool,
    both_streams: bool,
    nonce_on_screen: bool,
    waited_first_ms: u128,
    waited_second_ms: u128,
) -> String {
    let tail = printable_tail(screen, 160);
    format!(
        r#"{{"mode":"submittedText","parity":{equal},"screenBytes":{},"nonceOnBothStreams":{both_streams},"nonceOnScreen":{nonce_on_screen},"firstEchoMsA":{waited_first_ms},"firstEchoMsB":{waited_second_ms},"screenTail":{tail:?}}}"#,
        screen.len(),
    )
}

struct TextParityFirst {
    equal: bool,
    nonce_on_screen: bool,
    waited_first_ms: u128,
    waited_second_ms: u128,
}

async fn raw_text_handoff(
    stored: &Stored,
    home: &Path,
    digest: &str,
    terminal_id: RuntimeTerminalId,
    nonce: &str,
    view_b: &mut runtrol_runtime_client::TerminalView<'_>,
    first: TextParityFirst,
) -> Result<String, String> {
    let handoff = "-handoff";
    write_through_a_writer(
        stored,
        home,
        digest,
        terminal_id.clone(),
        handoff.as_bytes(),
    )
    .await?;
    let handoff_byte = Instant::now();
    let handed = drain_until(view_b, handoff.as_bytes(), handoff_byte).await;
    if let Some(code) = handed.exited {
        return Err(format!(
            "the terminal process exited (code {code}) during writer handoff; last screen bytes: {:?}",
            handed.tail
        ));
    }
    tokio::time::sleep(Duration::from_millis(400)).await;
    let handed_a = fresh_screen(stored, home, digest, terminal_id.clone()).await?;
    let handed_b = fresh_screen(stored, home, digest, terminal_id).await?;
    let handoff_equal = handed_a == handed_b;
    let handoff_on_screen = contains(&handed_a, format!("{nonce}{handoff}").as_bytes());
    let tail = printable_tail(&handed_a, 160);
    Ok(format!(
        r#"{{"mode":"rawText","parity":{},"screenBytes":{},"nonceOnBothStreams":true,"nonceOnScreen":{},"firstEchoMsA":{},"firstEchoMsB":{},"viewerClosed":true,"writerHandoff":{},"handoffOnScreen":{handoff_on_screen},"handoffEchoMs":{},"screenTail":{tail:?}}}"#,
        first.equal && handoff_equal,
        handed_a.len(),
        first.nonce_on_screen,
        first.waited_first_ms,
        first.waited_second_ms,
        handed.seen,
        handed.waited_ms,
    ))
}

/// Measure the same topology while a provider-owned modal ignores printable text.
async fn navigation_parity(
    stored: &Stored,
    home: &Path,
    digest: &str,
    terminal_id: RuntimeTerminalId,
) -> Result<String, String> {
    let mut client_a = RuntimeClient::connect_to(generation(home, digest)?, options_with(stored))
        .await
        .map_err(|error| format!("connect navigation viewer A: {error}"))?;
    let mut client_b = RuntimeClient::connect_to(generation(home, digest)?, options_with(stored))
        .await
        .map_err(|error| format!("connect navigation viewer B: {error}"))?;
    let mut terminals_a = client_a.terminals();
    let mut view_a = terminals_a
        .attach(&TerminalAttachParams {
            terminal_id: terminal_id.clone(),
        })
        .await
        .map_err(|error| format!("attach navigation viewer A: {error}"))?;
    let mut terminals_b = client_b.terminals();
    let mut view_b = terminals_b
        .attach(&TerminalAttachParams {
            terminal_id: terminal_id.clone(),
        })
        .await
        .map_err(|error| format!("attach navigation viewer B: {error}"))?;
    let baseline = view_a.initial_screen().to_vec();
    if baseline != view_b.initial_screen() {
        return Err("the navigation viewers' initial snapshots differ".to_owned());
    }

    write_through_a_writer(stored, home, digest, terminal_id.clone(), b"\x1b[B").await?;
    let first_byte = Instant::now();
    let moved_a = drain_until_activity(&mut view_a, first_byte).await;
    let moved_b = drain_until_activity(&mut view_b, first_byte).await;
    fail_on_exit(&moved_a, "first navigation")?;
    fail_on_exit(&moved_b, "first navigation")?;
    tokio::time::sleep(Duration::from_millis(400)).await;
    let selected_a = fresh_screen(stored, home, digest, terminal_id.clone()).await?;
    let selected_b = fresh_screen(stored, home, digest, terminal_id.clone()).await?;
    let first_equal = selected_a == selected_b;
    let first_changed = selected_a != baseline;

    drop(view_a);
    drop(client_a);
    write_through_a_writer(stored, home, digest, terminal_id.clone(), b"\x1b[A").await?;
    let handoff_byte = Instant::now();
    let handed = drain_until_activity(&mut view_b, handoff_byte).await;
    fail_on_exit(&handed, "navigation writer handoff")?;
    tokio::time::sleep(Duration::from_millis(400)).await;
    let restored_a = fresh_screen(stored, home, digest, terminal_id.clone()).await?;
    let restored_b = fresh_screen(stored, home, digest, terminal_id).await?;
    let handoff_equal = restored_a == restored_b;
    let handoff_changed = restored_a != selected_a;
    let tail = printable_tail(&restored_a, 160);
    Ok(format!(
        r#"{{"mode":"reversibleNavigation","parity":{},"screenBytes":{},"firstInputOnBothStreams":{},"firstScreenChanged":{first_changed},"firstEchoMsA":{},"firstEchoMsB":{},"viewerClosed":true,"writerHandoff":{},"handoffScreenChanged":{handoff_changed},"handoffEchoMs":{},"screenTail":{tail:?}}}"#,
        first_equal && handoff_equal,
        restored_a.len(),
        moved_a.seen && moved_b.seen,
        moved_a.waited_ms,
        moved_b.waited_ms,
        handed.seen,
        handed.waited_ms,
    ))
}

fn printable_tail(bytes: &[u8], limit: usize) -> String {
    bytes
        .iter()
        .rev()
        .take(limit)
        .rev()
        .map(|byte| {
            if byte.is_ascii_graphic() || *byte == b' ' {
                char::from(*byte)
            } else {
                '.'
            }
        })
        .collect()
}

/// Take the control lease on a connection of its own and write exact bytes into the one session.
///
/// A connection of its own because a viewer being drained must never also be the writer: cancelling this
/// client's `next()` mid-frame tears its connection (measured 2026-08-30, "connection ended" on the next
/// command), and a writer whose connection is gone cannot write.
async fn write_through_a_writer(
    stored: &Stored,
    home: &Path,
    digest: &str,
    terminal_id: RuntimeTerminalId,
    bytes: &[u8],
) -> Result<(), String> {
    let mut client = RuntimeClient::connect_to(generation(home, digest)?, options_with(stored))
        .await
        .map_err(|error| format!("connect the writer: {error}"))?;
    let mut terminals = client.terminals();
    let mut view = terminals
        .attach(&TerminalAttachParams {
            terminal_id: terminal_id.clone(),
        })
        .await
        .map_err(|error| format!("attach the writer: {error}"))?;
    let lease = view
        .acquire_control(&TerminalAcquireControlParams {
            request_id: MutationRequestId::now(),
            terminal_id: terminal_id.clone(),
            expected_terminal_generation: view.opened().terminal.terminal_generation,
        })
        .await
        .map_err(|error| format!("acquire the control lease: {error}"))?;
    write_bytes(&mut view, &lease, terminal_id, bytes).await
}

/// One exact write under a held lease.
async fn write_bytes(
    view: &mut runtrol_runtime_client::TerminalView<'_>,
    lease: &runtrol_runtime_client::protocol::TerminalControlLease,
    terminal_id: RuntimeTerminalId,
    bytes: &[u8],
) -> Result<(), String> {
    view.write(&TerminalWriteParams {
        request_id: MutationRequestId::now(),
        terminal_id,
        lease_id: lease.lease_id.clone(),
        lease_generation: lease.lease_generation,
        bytes_base64: encode_base64(bytes),
    })
    .await
    .map_err(|error| format!("write through the writer: {error}"))
}

/// Read one fresh attach's initial screen on a connection that did not exist before.
async fn fresh_screen(
    stored: &Stored,
    home: &Path,
    digest: &str,
    terminal_id: RuntimeTerminalId,
) -> Result<Vec<u8>, String> {
    let generation = generation(home, digest)?;
    let mut client = RuntimeClient::connect_to(generation, options_with(stored))
        .await
        .map_err(|error| format!("connect a fresh look: {error}"))?;
    let mut terminals = client.terminals();
    let view = terminals
        .attach(&TerminalAttachParams { terminal_id })
        .await
        .map_err(|error| format!("attach a fresh look: {error}"))?;
    Ok(view.initial_screen().to_vec())
}

/// What draining one live view saw: whether the needle arrived and when, whether the process exited, and
/// the last bytes drawn, kept so a journey that dies mid-way can say what was on screen.
struct Drained {
    seen: bool,
    waited_ms: u128,
    exited: Option<i32>,
    tail: String,
}

/// Drain one view's live output until the needle appears, the process exits, or the view goes quiet.
async fn drain_until(
    view: &mut runtrol_runtime_client::TerminalView<'_>,
    needle: &[u8],
    since: Instant,
) -> Drained {
    let mut gathered: Vec<u8> = view.initial_screen().to_vec();
    let mut exited = None;
    let mut seen = contains(&gathered, needle);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !seen && exited.is_none() && Instant::now() < deadline {
        let next = tokio::time::timeout(Duration::from_millis(600), view.next()).await;
        match next {
            Ok(Ok(runtrol_runtime_client::TerminalNotification::Output { bytes, .. })) => {
                gathered.extend_from_slice(&bytes);
                seen = contains(&gathered, needle);
            }
            Ok(Ok(runtrol_runtime_client::TerminalNotification::Exited { exit_code, .. })) => {
                exited = Some(exit_code);
            }
            Ok(Ok(_)) => {}
            Ok(Err(_)) | Err(_) => break,
        }
    }
    let tail = gathered
        .iter()
        .rev()
        .take(220)
        .rev()
        .map(|byte| {
            if byte.is_ascii_graphic() || *byte == b' ' {
                char::from(*byte)
            } else {
                '.'
            }
        })
        .collect();
    Drained {
        seen,
        waited_ms: since.elapsed().as_millis(),
        exited,
        tail,
    }
}

/// Drain until one output notification proves that an input changed the live stream.
async fn drain_until_activity(
    view: &mut runtrol_runtime_client::TerminalView<'_>,
    since: Instant,
) -> Drained {
    let mut gathered = view.initial_screen().to_vec();
    let mut exited = None;
    let mut seen = false;
    let deadline = Instant::now() + Duration::from_secs(5);
    while !seen && exited.is_none() && Instant::now() < deadline {
        let next = tokio::time::timeout(Duration::from_millis(600), view.next()).await;
        match next {
            Ok(Ok(runtrol_runtime_client::TerminalNotification::Output { bytes, .. })) => {
                gathered.extend_from_slice(&bytes);
                seen = true;
            }
            Ok(Ok(runtrol_runtime_client::TerminalNotification::Exited { exit_code, .. })) => {
                exited = Some(exit_code);
            }
            Ok(Ok(_)) => {}
            Ok(Err(_)) | Err(_) => break,
        }
    }
    Drained {
        seen,
        waited_ms: since.elapsed().as_millis(),
        exited,
        tail: printable_tail(&gathered, 220),
    }
}

fn fail_on_exit(drained: &Drained, phase: &str) -> Result<(), String> {
    match drained.exited {
        Some(code) => Err(format!(
            "the terminal process exited (code {code}) during {phase}; last screen bytes: {:?}",
            drained.tail
        )),
        None => Ok(()),
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && haystack.len() >= needle.len()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
}

/// Standard base64, inline so the probe adds no dependency for one encode.
fn encode_base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            *chunk.first().unwrap_or(&0),
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        let keep = chunk.len() + 1;
        for (index, shift) in [18_u32, 12, 6, 0].into_iter().enumerate() {
            // A six-bit index into a 64-entry table cannot miss; the fallback keeps the lint honest.
            let quad = TABLE
                .get((n >> shift) as usize & 63)
                .copied()
                .unwrap_or(b'=');
            out.push(if index < keep { char::from(quad) } else { '=' });
        }
    }
    out
}

/// End one terminal's process in the exact generation that owns it.
async fn stop(
    home: &Path,
    identity_file: &Path,
    digest: &str,
    terminal: &str,
) -> Result<String, String> {
    let stored = read_stored(identity_file)?;
    let generation = generation(home, digest)?;
    let mut client = RuntimeClient::connect_to(generation, options_with(&stored))
        .await
        .map_err(|error| format!("connect to generation {digest}: {error}"))?;
    let terminal_id = terminal
        .parse::<RuntimeTerminalId>()
        .map_err(|error| format!("terminal id: {error}"))?;
    let mut terminals = client.terminals();
    let mut view = terminals
        .attach(&TerminalAttachParams {
            terminal_id: terminal_id.clone(),
        })
        .await
        .map_err(|error| format!("attach to stop in {digest}: {error}"))?;
    let expected_terminal_generation = view.opened().terminal.terminal_generation;
    let lease = view
        .acquire_control(&TerminalAcquireControlParams {
            request_id: MutationRequestId::now(),
            terminal_id: terminal_id.clone(),
            expected_terminal_generation,
        })
        .await
        .map_err(|error| format!("take control in {digest}: {error}"))?;
    view.stop(&TerminalStopParams {
        request_id: MutationRequestId::now(),
        terminal_id,
        lease_id: lease.lease_id,
        lease_generation: lease.lease_generation,
    })
    .await
    .map_err(|error| format!("stop in {digest}: {error}"))?;
    Ok(format!(r#"{{"stopped":true,"generation":"{digest}"}}"#))
}
