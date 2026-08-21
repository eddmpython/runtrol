//! Direct adaptation from fixed Agent Tools calls to the typed public Runtime client.

use core::time::Duration;

use runtrol_provider::AbsPath;
use runtrol_runtime_client::{
    ClientOptions, ReconnectPolicy, RuntimeClient, RuntimeLocator, SessionNotification,
};
use runtrol_runtime_protocol::{
    AcquireControlParams, ControlLease, ControlLeaseParams, EventCursor, MutationRequestId,
    ProviderId, RuntimeSessionId, SessionDescriptor, SessionWorkspaceAccess, StartSessionParams,
    SubmitInputParams, WatchEventsParams,
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::AgentToolsError;
use crate::credentials::{ApprovedCredential, CredentialStore};

const EVENT_WAIT: Duration = Duration::from_secs(30);

pub(crate) async fn call(name: &str, arguments: Value) -> Result<Value, AgentToolsError> {
    let credential = CredentialStore::open()?.select_for_current_directory()?;
    match name {
        "runtrol_providers" => providers(&credential, empty(arguments)?).await,
        "runtrol_models" => models(&credential, parse(arguments)?).await,
        "runtrol_sessions" => sessions(&credential, empty(arguments)?).await,
        "runtrol_start" => start(&credential, parse(arguments)?).await,
        "runtrol_send" => send(&credential, parse(arguments)?).await,
        "runtrol_next_event" => next_event(&credential, parse(arguments)?).await,
        "runtrol_stop" => stop(&credential, parse(arguments)?).await,
        other => Err(AgentToolsError::Mcp(format!(
            "there is no Agent Tool called {other:?}"
        ))),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ModelsArgs {
    provider_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StartArgs {
    provider_id: String,
    workspace: String,
    input: String,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    reasoning_effort: Option<String>,
    #[serde(default)]
    permission: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SessionArgs {
    session_id: String,
    workspace: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SendArgs {
    session_id: String,
    workspace: String,
    input: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EventArgs {
    session_id: String,
    workspace: String,
    #[serde(default)]
    after: Option<EventCursor>,
}

async fn providers(credential: &ApprovedCredential, (): ()) -> Result<Value, AgentToolsError> {
    let mut runtime = connect(credential).await?;
    Ok(serde_json::to_value(runtime.providers().list().await?)?)
}

async fn models(
    credential: &ApprovedCredential,
    args: ModelsArgs,
) -> Result<Value, AgentToolsError> {
    let mut runtime = connect(credential).await?;
    Ok(serde_json::to_value(
        runtime
            .providers()
            .list_models(ProviderId::new(args.provider_id))
            .await?,
    )?)
}

async fn sessions(credential: &ApprovedCredential, (): ()) -> Result<Value, AgentToolsError> {
    let mut runtime = connect(credential).await?;
    Ok(serde_json::to_value(runtime.sessions().list().await?)?)
}

async fn start(credential: &ApprovedCredential, args: StartArgs) -> Result<Value, AgentToolsError> {
    let workspace = authorized_workspace(credential, &args.workspace)?;
    let mut runtime = connect(credential).await?;
    let opened = runtime
        .sessions()
        .start(&StartSessionParams {
            request_id: MutationRequestId::now(),
            provider_id: ProviderId::new(args.provider_id),
            workspace: workspace.as_str().to_owned(),
            access: SessionWorkspaceAccess::Exclusive,
            model: args.model,
            reasoning_effort: args.reasoning_effort,
            permission: args.permission,
        })
        .await?;
    if let Err(error) = submit(&mut runtime, &opened.control, args.input).await {
        return Err(release_after_failure(&mut runtime, &opened.control, error).await);
    }
    let warning = release(&mut runtime, &opened.control)
        .await
        .err()
        .map(|error| {
            format!(
                "input was submitted, but the control lease could not be released early: {error}"
            )
        });
    Ok(json!({
        "session": opened.session,
        "warning": warning,
    }))
}

async fn send(credential: &ApprovedCredential, args: SendArgs) -> Result<Value, AgentToolsError> {
    let mut runtime = connect(credential).await?;
    let session = exact_session(
        credential,
        &mut runtime,
        RuntimeSessionId::new(args.session_id),
        &args.workspace,
    )
    .await?;
    let lease = acquire(&mut runtime, &session).await?;
    if let Err(error) = submit(&mut runtime, &lease, args.input).await {
        return Err(release_after_failure(&mut runtime, &lease, error).await);
    }
    let warning = release(&mut runtime, &lease).await.err().map(|error| {
        format!("input was submitted, but the control lease could not be released early: {error}")
    });
    Ok(json!({
        "session": session,
        "warning": warning,
    }))
}

async fn stop(
    credential: &ApprovedCredential,
    args: SessionArgs,
) -> Result<Value, AgentToolsError> {
    let mut runtime = connect(credential).await?;
    let session = exact_session(
        credential,
        &mut runtime,
        RuntimeSessionId::new(args.session_id),
        &args.workspace,
    )
    .await?;
    let lease = acquire(&mut runtime, &session).await?;
    if let Err(error) = runtime.sessions().interrupt(&lease_params(&lease)).await {
        return Err(release_after_failure(&mut runtime, &lease, error.into()).await);
    }
    let warning = release(&mut runtime, &lease)
        .await
        .err()
        .map(|error| format!("the session was interrupted, but its control lease could not be released early: {error}"));
    Ok(json!({
        "session": session,
        "interrupted": true,
        "warning": warning,
    }))
}

async fn next_event(
    credential: &ApprovedCredential,
    args: EventArgs,
) -> Result<Value, AgentToolsError> {
    let mut runtime = connect(credential).await?;
    let session_id = RuntimeSessionId::new(args.session_id);
    let session = exact_session(
        credential,
        &mut runtime,
        session_id.clone(),
        &args.workspace,
    )
    .await?;
    let mut sessions = runtime.sessions();
    let mut subscription = sessions
        .watch_events(&WatchEventsParams {
            session_id,
            after: args.after,
        })
        .await?;
    let waiting_at = subscription.started().starts_at.clone();
    match tokio::time::timeout(EVENT_WAIT, subscription.next()).await {
        Ok(Ok(SessionNotification::Event(event))) => Ok(json!({
            "session": session,
            "event": event.event,
            "cursor": event.next_expected,
        })),
        Ok(Ok(SessionNotification::Lagged(lagged))) => Ok(json!({
            "session": session,
            "lagged": true,
            "cursor": lagged.next_expected,
        })),
        Ok(Err(error)) => Err(error.into()),
        Err(_) => Ok(json!({
            "session": session,
            "timeout": true,
            "cursor": waiting_at,
        })),
    }
}

async fn connect(credential: &ApprovedCredential) -> Result<RuntimeClient, AgentToolsError> {
    let locator = RuntimeLocator::system().map_err(runtrol_runtime_client::ClientError::Locator)?;
    let options = ClientOptions::new("runtrol-agent-tools", env!("CARGO_PKG_VERSION"))
        .with_credentials(credential.credentials.clone());
    Ok(locator
        .connect_with_retry(options, ReconnectPolicy::default())
        .await?)
}

async fn exact_session(
    credential: &ApprovedCredential,
    runtime: &mut RuntimeClient,
    session_id: RuntimeSessionId,
    workspace: &str,
) -> Result<SessionDescriptor, AgentToolsError> {
    let workspace = authorized_workspace(credential, workspace)?;
    let session = runtime.sessions().get(session_id).await?;
    if session.workspace != workspace.as_str() {
        return Err(AgentToolsError::Authority(format!(
            "session {} belongs to {}, not the supplied workspace {}",
            session.session_id, session.workspace, workspace
        )));
    }
    Ok(session)
}

async fn acquire(
    runtime: &mut RuntimeClient,
    session: &SessionDescriptor,
) -> Result<ControlLease, AgentToolsError> {
    Ok(runtime
        .sessions()
        .acquire_control(&AcquireControlParams {
            request_id: MutationRequestId::now(),
            session_id: session.session_id.clone(),
            expected_lifecycle: session.lifecycle,
            expected_session_generation: session.session_generation,
        })
        .await?)
}

async fn submit(
    runtime: &mut RuntimeClient,
    lease: &ControlLease,
    input: String,
) -> Result<(), AgentToolsError> {
    runtime
        .sessions()
        .submit_input(&SubmitInputParams {
            request_id: MutationRequestId::now(),
            session_id: lease.session_id.clone(),
            lease_id: lease.lease_id.clone(),
            lease_generation: lease.lease_generation,
            input,
        })
        .await?;
    Ok(())
}

async fn release(runtime: &mut RuntimeClient, lease: &ControlLease) -> Result<(), AgentToolsError> {
    runtime
        .sessions()
        .release_control(&lease_params(lease))
        .await?;
    Ok(())
}

async fn release_after_failure(
    runtime: &mut RuntimeClient,
    lease: &ControlLease,
    primary: AgentToolsError,
) -> AgentToolsError {
    match release(runtime, lease).await {
        Ok(()) => primary,
        Err(release_error) => AgentToolsError::Mcp(format!(
            "the Runtime action failed: {primary}; releasing its control lease also failed: {release_error}"
        )),
    }
}

fn lease_params(lease: &ControlLease) -> ControlLeaseParams {
    ControlLeaseParams {
        request_id: MutationRequestId::now(),
        session_id: lease.session_id.clone(),
        lease_id: lease.lease_id.clone(),
        lease_generation: lease.lease_generation,
    }
}

fn authorized_workspace(
    credential: &ApprovedCredential,
    workspace: &str,
) -> Result<AbsPath, AgentToolsError> {
    let workspace = AbsPath::canonicalize(workspace)?;
    if !workspace.is_under(&credential.root) {
        return Err(AgentToolsError::Authority(format!(
            "workspace {} is outside this MCP process's approved root {}",
            workspace.as_str(),
            credential.root.as_str()
        )));
    }
    Ok(workspace)
}

fn parse<T: for<'de> Deserialize<'de>>(value: Value) -> Result<T, AgentToolsError> {
    serde_json::from_value(value).map_err(|error| {
        AgentToolsError::Mcp(format!("tool arguments have the wrong shape: {error}"))
    })
}

fn empty(value: Value) -> Result<(), AgentToolsError> {
    let Value::Object(object) = value else {
        return Err(AgentToolsError::Mcp(
            "tool arguments must be an object".to_owned(),
        ));
    };
    if object.is_empty() {
        Ok(())
    } else {
        Err(AgentToolsError::Mcp(
            "this tool accepts no arguments".to_owned(),
        ))
    }
}
