//! Local one-action enablement for project-scoped Agent Tools.

use core::time::Duration;
use std::path::PathBuf;

use runtrol_ipc::wire::Request;
use runtrol_provider::AbsPath;
use runtrol_runtime_client::{
    ClientOptions, EnrollmentProposal, LocatorState, ReconnectPolicy, RuntimeLocator,
};
use runtrol_runtime_protocol::EnrollmentDecision;
use sha2::{Digest as _, Sha256};

use crate::AgentToolsError;
use crate::credentials::{ApprovedCredential, CredentialStore, PreparedCredential, SCOPES};

const PRODUCT_MANIFEST: &[u8] = b"runtrol-agent-tools/1\n\
tools=runtrol_providers,runtrol_models,runtrol_sessions,runtrol_start,runtrol_send,runtrol_next_event,runtrol_stop\n\
approvals=absent\ntranscript=provider-owned\naccess=exclusive\n";
const RUNTIME_STARTUP_WAIT: Duration = Duration::from_secs(10);
const RUNTIME_STARTUP_POLL: Duration = Duration::from_millis(25);

/// Process facts resolved once by the thin executable.
#[derive(Clone, Debug)]
pub struct CommandContext {
    /// Private local daemon endpoint.
    pub endpoint: String,
    /// Exact executable used to start or register this same build.
    pub executable: PathBuf,
}

/// Run one local `runtrol tools` command.
///
/// # Errors
///
/// The command shape is invalid, the project cannot be enrolled, local self-approval is refused, or no
/// provider can register the verified MCP server through its official command.
pub async fn run_command(
    words: &[String],
    context: &CommandContext,
) -> Result<Vec<String>, AgentToolsError> {
    match words.first().map(String::as_str) {
        Some("enable") => {
            let workspace = match words.get(1) {
                Some(workspace) => workspace.clone(),
                None => current_directory()?,
            };
            if let Some(extra) = words.get(2) {
                return Err(AgentToolsError::Mcp(format!(
                    "tools enable does not accept extra word {extra:?}"
                )));
            }
            enable(&workspace, context).await
        }
        Some("disable") => {
            let workspace = match words.get(1) {
                Some(workspace) => workspace.clone(),
                None => current_directory()?,
            };
            if let Some(extra) = words.get(2) {
                return Err(AgentToolsError::Mcp(format!(
                    "tools disable does not accept extra word {extra:?}"
                )));
            }
            disable(&workspace, context).await
        }
        Some("status") => {
            if let Some(extra) = words.get(1) {
                return Err(AgentToolsError::Mcp(format!(
                    "tools status does not accept extra word {extra:?}"
                )));
            }
            status().await
        }
        Some("list") => {
            if let Some(extra) = words.get(1) {
                return Err(AgentToolsError::Mcp(format!(
                    "tools list does not accept extra word {extra:?}"
                )));
            }
            list()
        }
        Some(other) => Err(AgentToolsError::Mcp(format!(
            "no tools command called {other:?}. try: tools enable [project], tools disable [project], tools status, tools list"
        ))),
        None => Err(AgentToolsError::Mcp(
            "tools needs a command. try: tools enable [project], tools disable [project], tools status, tools list"
                .to_owned(),
        )),
    }
}

async fn enable(workspace: &str, context: &CommandContext) -> Result<Vec<String>, AgentToolsError> {
    ensure_daemon(context).await?;
    let store = CredentialStore::open()?;
    let prepared = store.prepare(workspace)?;
    let (approved, newly_enrolled) = match CredentialStore::approved(&prepared)? {
        Some(approved) => {
            connect(&approved).await?;
            (approved, false)
        }
        None => (enroll(&prepared, context).await?, true),
    };
    if let Err(wiring) = ask_local(context, Request::AgentToolsWire).await {
        if newly_enrolled
            && let Err(rollback) = rollback_enrollment(&store, &approved, context).await
        {
            return Err(AgentToolsError::Refused(format!(
                "{wiring}; the new Runtime enrollment could not be rolled back: {rollback}"
            )));
        }
        return Err(wiring);
    }
    Ok(vec![
        format!("Agent Tools enabled for {}", approved.root.as_str()),
        "coding agents can now discover providers, start isolated sessions, delegate unchanged input, read bounded events, and stop work".to_owned(),
        "provider approvals still require a person in Runtrol".to_owned(),
    ])
}

async fn rollback_enrollment(
    store: &CredentialStore,
    approved: &ApprovedCredential,
    context: &CommandContext,
) -> Result<(), AgentToolsError> {
    ask_local(
        context,
        Request::IntegrationRevoke {
            integration_id: approved
                .credentials
                .grant()
                .integration_id
                .to_string()
                .into(),
        },
    )
    .await?;
    let stored =
        store
            .existing(approved.root.as_str())?
            .ok_or_else(|| AgentToolsError::Credential {
                path: approved.root.as_str().to_owned(),
                why: "the just-enrolled Agent Tools credential disappeared before rollback"
                    .to_owned(),
            })?;
    CredentialStore::remove(&stored)
}

async fn disable(
    workspace: &str,
    context: &CommandContext,
) -> Result<Vec<String>, AgentToolsError> {
    let store = CredentialStore::open()?;
    let requested = AbsPath::canonicalize(workspace)?;
    let Some(stored) = store.existing(requested.as_str())? else {
        return Ok(vec![format!(
            "Agent Tools is already disabled for {}",
            requested.as_str()
        )]);
    };
    ensure_daemon(context).await?;

    let mut warning = None;
    if !store.has_approved_other_than(&stored.root)?
        && let Err(error) = ask_local(context, Request::AgentToolsUnwire).await
    {
        warning = Some(format!(
            "warning: provider registration could not be removed; Runtime authority is absent and local credentials were deleted: {error}"
        ));
    }
    if let Some(grant) = &stored.grant {
        ask_local(
            context,
            Request::IntegrationRevoke {
                integration_id: grant.integration_id.to_string().into(),
            },
        )
        .await?;
    }
    CredentialStore::remove(&stored)?;

    let mut lines = vec![format!(
        "Agent Tools disabled and Runtime authority revoked for {}",
        stored.root.as_str()
    )];
    if let Some(warning) = warning {
        lines.push(warning);
    }
    Ok(lines)
}

async fn status() -> Result<Vec<String>, AgentToolsError> {
    let store = CredentialStore::open()?;
    let approved = store.select_for_current_directory()?;
    connect(&approved).await?;
    Ok(vec![format!(
        "Agent Tools is enabled and Runtime-authorized for {}",
        approved.root.as_str()
    )])
}

fn list() -> Result<Vec<String>, AgentToolsError> {
    let roots = CredentialStore::open()?.approved_roots()?;
    if roots.is_empty() {
        return Ok(vec!["no projects enabled".to_owned()]);
    }
    Ok(roots
        .into_iter()
        .map(|root| format!("enabled  {}", root.as_str()))
        .collect())
}

async fn enroll(
    prepared: &PreparedCredential,
    context: &CommandContext,
) -> Result<ApprovedCredential, AgentToolsError> {
    let locator = RuntimeLocator::system().map_err(runtrol_runtime_client::ClientError::Locator)?;
    let options = ClientOptions::new("runtrol-agent-tools", env!("CARGO_PKG_VERSION"))
        .with_identity(prepared.identity.clone());
    let mut runtime = locator
        .connect_with_retry(options, ReconnectPolicy::default())
        .await?;
    let digest: [u8; 32] = Sha256::digest(PRODUCT_MANIFEST).into();
    let receipt = runtime
        .integrations()
        .request(EnrollmentProposal::new(
            format!(
                "runtrol-agent-tools:{}",
                prepared.identity.public_key_base64()
            ),
            digest,
            SCOPES.to_vec(),
            vec![prepared.root.as_str().to_owned()],
        ))
        .await?;
    let signature = prepared
        .identity
        .self_approval_signature(&receipt.pending_id)?;
    ask_local(
        context,
        Request::IntegrationSelfApprove {
            pending_id: receipt.pending_id.as_str().into(),
            signature: signature.into(),
        },
    )
    .await?;
    match runtime
        .integrations()
        .watch(receipt.pending_id.clone())
        .await?
    {
        EnrollmentDecision::Approved { grant } => CredentialStore::persist(prepared, grant),
        EnrollmentDecision::Pending => Err(AgentToolsError::Refused(
            "the local Agent Tools enrollment is still pending after self-approval".to_owned(),
        )),
        EnrollmentDecision::Denied => Err(AgentToolsError::Refused(
            "the local Agent Tools enrollment was denied".to_owned(),
        )),
        EnrollmentDecision::Expired => Err(AgentToolsError::Refused(
            "the local Agent Tools enrollment expired before approval".to_owned(),
        )),
    }
}

async fn connect(approved: &ApprovedCredential) -> Result<(), AgentToolsError> {
    let locator = RuntimeLocator::system().map_err(runtrol_runtime_client::ClientError::Locator)?;
    let options = ClientOptions::new("runtrol-agent-tools", env!("CARGO_PKG_VERSION"))
        .with_credentials(approved.credentials.clone());
    drop(
        locator
            .connect_with_retry(options, ReconnectPolicy::default())
            .await?,
    );
    Ok(())
}

async fn ensure_daemon(context: &CommandContext) -> Result<(), AgentToolsError> {
    drop(
        runtrol_cli::reach(&context.endpoint, &context.executable)
            .await
            .map_err(runtrol_cli::Failed::Unreachable)?,
    );

    // The private listener is bound just before daemon assembly publishes the public Runtime locator. A command on
    // a loaded machine can reach that listener inside this short startup window. Since this command initiated or
    // joined that exact daemon startup, wait only for the missing locator state. Unsafe, malformed, and I/O states
    // remain immediate failures rather than being hidden by retries.
    let locator = RuntimeLocator::system().map_err(runtrol_runtime_client::ClientError::Locator)?;
    let deadline = tokio::time::Instant::now() + RUNTIME_STARTUP_WAIT;
    loop {
        match locator
            .inspect()
            .map_err(runtrol_runtime_client::ClientError::Locator)?
        {
            LocatorState::Running(_) => return Ok(()),
            LocatorState::NotInstalled if tokio::time::Instant::now() < deadline => {
                tokio::time::sleep(RUNTIME_STARTUP_POLL).await;
            }
            LocatorState::NotInstalled => {
                return Err(AgentToolsError::Refused(format!(
                    "the Runtrol daemon started but its public Runtime did not become ready within {} seconds",
                    RUNTIME_STARTUP_WAIT.as_secs()
                )));
            }
        }
    }
}

async fn ask_local(context: &CommandContext, request: Request) -> Result<(), AgentToolsError> {
    let mut lines = Vec::new();
    let outcome = runtrol_cli::ask(&context.endpoint, &context.executable, request, |line| {
        lines.push(line.to_owned());
    })
    .await?;
    match outcome {
        runtrol_cli::Outcome::Carried => Ok(()),
        runtrol_cli::Outcome::Refused => Err(AgentToolsError::Refused(lines.join(" "))),
    }
}

fn current_directory() -> Result<String, AgentToolsError> {
    let current = std::env::current_dir().map_err(|error| AgentToolsError::Io {
        doing: "reading the current project directory",
        path: ".".to_owned(),
        detail: error.to_string(),
    })?;
    current.to_str().map(str::to_owned).ok_or_else(|| {
        AgentToolsError::Authority("the current project directory is not UTF-8".to_owned())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_closed_manifest_names_every_tool_and_excludes_approvals() {
        let manifest = std::str::from_utf8(PRODUCT_MANIFEST).expect("manifest is UTF-8");
        for name in [
            "runtrol_providers",
            "runtrol_models",
            "runtrol_sessions",
            "runtrol_start",
            "runtrol_send",
            "runtrol_next_event",
            "runtrol_stop",
        ] {
            assert!(manifest.contains(name), "missing {name}");
        }
        assert!(manifest.contains("approvals=absent"));
        assert!(manifest.contains("access=exclusive"));
    }

    #[test]
    fn scopes_exclude_approval_delete_resume_and_native_discovery() {
        use runtrol_runtime_protocol::AppScope;

        let selected = SCOPES.to_vec();
        assert!(!selected.contains(&AppScope::ApprovalRespondLow));
        assert!(!selected.contains(&AppScope::ApprovalRespondHigh));
        assert!(!selected.contains(&AppScope::SessionDelete));
        assert!(!selected.contains(&AppScope::SessionResume));
        assert!(!selected.contains(&AppScope::SessionNativeDiscover));
    }
}
