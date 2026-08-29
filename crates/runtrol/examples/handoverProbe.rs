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
    TerminalOpenParams, TerminalOpenTarget, TerminalStopParams,
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
            ["attach", home, identity, digest, terminal] => {
                attach(Path::new(home), Path::new(identity), digest, terminal).await
            }
            ["stop", home, identity, digest, terminal] => {
                stop(Path::new(home), Path::new(identity), digest, terminal).await
            }
            _ => Err("usage: handoverProbe enroll|open|attach|stop ...".to_owned()),
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
    Ok(format!(
        r#"{{"terminalId":"{}","generation":"{}","screenBytes":{}}}"#,
        opened.terminal.terminal_id.as_str(),
        opened.terminal.runtime_generation,
        view.initial_screen().len()
    ))
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
