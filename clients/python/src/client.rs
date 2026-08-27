//! Actor-backed Python client over the typed public Rust client.

use std::sync::Arc;

use pyo3::prelude::*;
use runtrol_runtime_client::{
    ClientError, ClientOptions, EnrollmentProposal, IntegrationCredentials, IntegrationIdentity,
    ProviderNotification, RuntimeClient, RuntimeLocator, SessionIndexNotification,
    SessionNotification, TerminalIndexNotification,
};
use runtrol_runtime_protocol::{
    AcquireControlParams, AdoptNativeSessionParams, AppScope, ArchiveNativeSessionParams,
    ControlLeaseParams, CoolSessionParams, DeleteNativeSessionParams, ForgetSessionParams,
    GetProviderCapabilitiesParams, GetSessionParams, IntegrationGrant, ListModelsParams,
    ListNativeSessionsParams, ListPendingApprovalsParams, MutationRequestId, PendingEnrollmentId,
    RespondApprovalParams, ResumeSessionParams, SetModeParams, SetModelParams, StartSessionParams,
    SubmitBlocksParams, SubmitInputParams, WatchEventsParams,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::sync::{mpsc, oneshot};

use crate::{NativeError, native_error};

const COMMAND_CAPACITY: usize = 64;
const SUBSCRIPTION_CAPACITY: usize = 64;

#[derive(Clone)]
pub(crate) struct ConnectConfig {
    name: String,
    version: String,
    identity: Option<IntegrationIdentity>,
    grant: Option<IntegrationGrant>,
}

impl ConnectConfig {
    pub(crate) fn new(
        name: String,
        version: String,
        identity: Option<IntegrationIdentity>,
        grant: Option<IntegrationGrant>,
    ) -> PyResult<Self> {
        if grant.is_some() && identity.is_none() {
            return Err(NativeError::new_err(
                r#"{"code":"invalidRequest","message":"approved credentials require the consumer-owned identity","retryable":false,"action":null,"correlationId":"python-connect"}"#,
            ));
        }
        Ok(Self {
            name,
            version,
            identity,
            grant,
        })
    }

    fn options(&self) -> ClientOptions {
        let options = ClientOptions::new(self.name.clone(), self.version.clone());
        match (&self.identity, &self.grant) {
            (Some(identity), Some(grant)) => options
                .with_credentials(IntegrationCredentials::new(identity.clone(), grant.clone())),
            (Some(identity), None) => options.with_identity(identity.clone()),
            (None, None | Some(_)) => options,
        }
    }

    pub(crate) async fn connect(&self) -> Result<RuntimeClient, ClientError> {
        RuntimeLocator::system()?.connect(self.options()).await
    }
}

struct Command {
    operation: Box<str>,
    params: Box<str>,
    answer: oneshot::Sender<Result<String, String>>,
}

/// One initialized public Runtime connection serialized by a bounded Rust actor.
#[pyclass(module = "runtrol_runtime._native")]
pub(crate) struct PyRuntimeClient {
    sender: mpsc::Sender<Command>,
    config: ConnectConfig,
    initialization_json: String,
}

#[pymethods]
impl PyRuntimeClient {
    /// Initialization result selected by the connected Runtime.
    #[getter]
    fn initialization_json(&self) -> &str {
        &self.initialization_json
    }

    /// Run one closed typed operation. The Python layer supplies named methods over this private seam.
    fn call<'py>(
        &self,
        py: Python<'py>,
        operation: String,
        params_json: String,
    ) -> PyResult<Bound<'py, PyAny>> {
        let sender = self.sender.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let (answer, receive) = oneshot::channel();
            sender
                .send(Command {
                    operation: operation.into(),
                    params: params_json.into(),
                    answer,
                })
                .await
                .map_err(|_| native_error("runtimeUnavailable", "the Runtime client is closed"))?;
            receive
                .await
                .map_err(|_| native_error("runtimeUnavailable", "the Runtime client stopped"))?
                .map_err(NativeError::new_err)
        })
    }

    /// Open a dedicated read-only subscription connection.
    fn subscribe<'py>(
        &self,
        py: Python<'py>,
        kind: String,
        params_json: String,
    ) -> PyResult<Bound<'py, PyAny>> {
        let config = self.config.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            open_subscription(config, &kind, &params_json)
                .await
                .map_err(NativeError::new_err)
        })
    }

    /// Open or attach one provider-faithful terminal on its own authenticated connection.
    fn terminal<'py>(
        &self,
        py: Python<'py>,
        kind: String,
        params_json: String,
    ) -> PyResult<Bound<'py, PyAny>> {
        let config = self.config.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            crate::terminal::open_terminal(config, kind, params_json)
                .await
                .map_err(NativeError::new_err)
        })
    }

    /// Close this client actor after all earlier commands have drained.
    fn close<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let sender = self.sender.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let (answer, receive) = oneshot::channel();
            sender
                .send(Command {
                    operation: "close".into(),
                    params: "{}".into(),
                    answer,
                })
                .await
                .map_err(|_| {
                    native_error("runtimeUnavailable", "the Runtime client is already closed")
                })?;
            receive
                .await
                .map_err(|_| native_error("runtimeUnavailable", "the Runtime client stopped"))?
                .map_err(NativeError::new_err)
        })
    }
}

pub(crate) async fn connect_client(config: ConnectConfig) -> Result<PyRuntimeClient, String> {
    let runtime = config
        .connect()
        .await
        .map_err(|error| crate::error_json(&error))?;
    let initialization_json =
        encode(runtime.initialization()).map_err(|error| crate::error_json(&error))?;
    let (sender, receiver) = mpsc::channel(COMMAND_CAPACITY);
    tokio::spawn(run_client(runtime, receiver));
    Ok(PyRuntimeClient {
        sender,
        config,
        initialization_json,
    })
}

async fn run_client(mut runtime: RuntimeClient, mut commands: mpsc::Receiver<Command>) {
    while let Some(command) = commands.recv().await {
        if command.operation.as_ref() == "close" {
            let _sent = command.answer.send(Ok("{}".to_owned()));
            return;
        }
        let outcome = execute(&mut runtime, &command.operation, &command.params)
            .await
            .map_err(|error| crate::error_json(&error));
        let _sent = command.answer.send(outcome);
    }
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EnrollmentInput {
    client_instance_id: String,
    manifest_digest: Vec<u8>,
    requested_scopes: Vec<AppScope>,
    requested_roots: Vec<String>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EnrollmentWatchInput {
    pending_id: PendingEnrollmentId,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RotationInput {
    request_id: MutationRequestId,
    expected_key_generation: u64,
    replacement_secret: Vec<u8>,
}

#[expect(
    clippy::too_many_lines,
    reason = "the closed Python operation table keeps every public method mapped to its typed Rust DTO in one auditable place"
)]
async fn execute(
    client: &mut RuntimeClient,
    operation: &str,
    params_json: &str,
) -> Result<String, ClientError> {
    macro_rules! result {
        ($future:expr) => {
            encode(&$future.await?)
        };
    }
    macro_rules! session_result {
        ($type:ty, $method:ident) => {{
            let params: $type = decode(params_json)?;
            let value = client.sessions().$method(&params).await?;
            encode(&value)
        }};
    }
    macro_rules! session_void {
        ($type:ty, $method:ident) => {{
            let params: $type = decode(params_json)?;
            client.sessions().$method(&params).await?;
            Ok("{}".to_owned())
        }};
    }

    match operation {
        "integrations.request" => {
            let params: EnrollmentInput = decode(params_json)?;
            let manifest_digest: [u8; 32] = params.manifest_digest.try_into().map_err(|_| {
                ClientError::Protocol("manifest digest must contain exactly 32 bytes".to_owned())
            })?;
            result!(client.integrations().request(EnrollmentProposal::new(
                params.client_instance_id,
                manifest_digest,
                params.requested_scopes,
                params.requested_roots,
            )))
        }
        "integrations.watch" => {
            let params: EnrollmentWatchInput = decode(params_json)?;
            result!(client.integrations().watch(params.pending_id))
        }
        "integrations.grant" => result!(client.integrations().grant()),
        "integrations.rotateKey" => {
            let params: RotationInput = decode(params_json)?;
            let secret: [u8; 32] = params.replacement_secret.try_into().map_err(|_| {
                ClientError::Protocol(
                    "replacement identity must contain exactly 32 bytes".to_owned(),
                )
            })?;
            let replacement = IntegrationIdentity::from_secret_bytes(secret);
            let credentials = client
                .integrations()
                .rotate_key(
                    params.request_id,
                    params.expected_key_generation,
                    &replacement,
                )
                .await?;
            encode(credentials.grant())
        }
        "providers.list" => result!(client.providers().list()),
        "providers.usage" => result!(client.providers().usage()),
        "providers.capabilities" => {
            let params: GetProviderCapabilitiesParams = decode(params_json)?;
            result!(client.providers().get_capabilities(params.provider_id))
        }
        "providers.models" => {
            let params: ListModelsParams = decode(params_json)?;
            result!(client.providers().list_models(params.provider_id))
        }
        "providers.nativeSessions" => {
            let params: ListNativeSessionsParams = decode(params_json)?;
            result!(client.providers().list_native_sessions(params))
        }
        "sessions.list" => result!(client.sessions().list()),
        "sessions.get" => {
            let params: GetSessionParams = decode(params_json)?;
            result!(client.sessions().get(params.session_id))
        }
        "sessions.start" => session_result!(StartSessionParams, start),
        "sessions.adoptNative" => session_result!(AdoptNativeSessionParams, adopt_native),
        "sessions.resume" => session_result!(ResumeSessionParams, resume),
        "sessions.acquireControl" => session_result!(AcquireControlParams, acquire_control),
        "sessions.renewControl" => session_result!(ControlLeaseParams, renew_control),
        "sessions.releaseControl" => {
            let params: ControlLeaseParams = decode(params_json)?;
            client.sessions().release_control(&params).await?;
            Ok("{}".to_owned())
        }
        "sessions.submitInput" => session_void!(SubmitInputParams, submit_input),
        "sessions.submitBlocks" => session_void!(SubmitBlocksParams, submit_blocks),
        "sessions.setModel" => session_void!(SetModelParams, set_model),
        "sessions.setMode" => session_void!(SetModeParams, set_mode),
        "sessions.interrupt" => session_void!(ControlLeaseParams, interrupt),
        "sessions.cool" => session_void!(CoolSessionParams, cool),
        "sessions.forget" => session_void!(ForgetSessionParams, forget),
        "sessions.deleteNative" => session_void!(DeleteNativeSessionParams, delete_native),
        "sessions.archiveNative" => session_void!(ArchiveNativeSessionParams, archive_native),
        "approvals.listPending" => {
            let params: ListPendingApprovalsParams = decode(params_json)?;
            result!(client.approvals().list_pending(&params))
        }
        "approvals.respond" => {
            let params: RespondApprovalParams = decode(params_json)?;
            client.approvals().respond(&params).await?;
            Ok("{}".to_owned())
        }
        "terminals.list" => result!(client.terminals().list()),
        "panicStop" => {
            client.panic_stop().await?;
            Ok("{}".to_owned())
        }
        _ => Err(ClientError::Protocol(format!(
            "the Python client has no operation named {operation}"
        ))),
    }
}

fn decode<T: DeserializeOwned>(value: &str) -> Result<T, ClientError> {
    serde_json::from_str(value).map_err(|error| {
        ClientError::Protocol(format!("Python parameters have the wrong shape: {error}"))
    })
}

fn encode<T: Serialize>(value: &T) -> Result<String, ClientError> {
    serde_json::to_string(value)
        .map_err(|error| ClientError::Protocol(format!("Python result cannot be encoded: {error}")))
}

/// One dedicated read-only Runtime subscription.
#[pyclass(module = "runtrol_runtime._native")]
pub(crate) struct PySubscription {
    started_json: String,
    receiver: Arc<tokio::sync::Mutex<mpsc::Receiver<Result<String, String>>>>,
}

#[pymethods]
impl PySubscription {
    /// Initial bounded snapshot and stream identity.
    #[getter]
    fn started_json(&self) -> &str {
        &self.started_json
    }

    /// Await one next typed stream item encoded for the Python model layer.
    fn next<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let receiver = Arc::clone(&self.receiver);
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let item = receiver.lock().await.recv().await.ok_or_else(|| {
                native_error("runtimeUnavailable", "the Runtime subscription ended")
            })?;
            item.map_err(NativeError::new_err)
        })
    }

    /// Stop this local subscriber without changing Runtime or provider process lifetime.
    fn close<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let receiver = Arc::clone(&self.receiver);
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            receiver.lock().await.close();
            Ok(())
        })
    }
}

async fn open_subscription(
    config: ConnectConfig,
    kind: &str,
    params_json: &str,
) -> Result<PySubscription, String> {
    let (ready, started) = oneshot::channel();
    let (sender, receiver) = mpsc::channel(SUBSCRIPTION_CAPACITY);
    match kind {
        "providers" => {
            tokio::spawn(provider_subscription(config, ready, sender));
        }
        "sessions" => {
            tokio::spawn(session_index_subscription(config, ready, sender));
        }
        "events" => {
            let params: WatchEventsParams = serde_json::from_str(params_json)
                .map_err(|error| native_error_json("invalidRequest", &error.to_string()))?;
            tokio::spawn(event_subscription(config, params, ready, sender));
        }
        "terminals" => {
            tokio::spawn(terminal_index_subscription(config, ready, sender));
        }
        _ => {
            return Err(native_error_json(
                "invalidRequest",
                "the subscription kind is unknown",
            ));
        }
    }
    let started_json = started.await.map_err(|_| {
        native_error_json(
            "runtimeUnavailable",
            "the Runtime subscription did not start",
        )
    })??;
    Ok(PySubscription {
        started_json,
        receiver: Arc::new(tokio::sync::Mutex::new(receiver)),
    })
}

async fn provider_subscription(
    config: ConnectConfig,
    ready: oneshot::Sender<Result<String, String>>,
    sender: mpsc::Sender<Result<String, String>>,
) {
    let mut runtime = match config.connect().await {
        Ok(runtime) => runtime,
        Err(error) => {
            let _sent = ready.send(Err(crate::error_json(&error)));
            return;
        }
    };
    let mut providers = runtime.providers();
    let mut subscription = match providers.watch().await {
        Ok(subscription) => subscription,
        Err(error) => {
            let _sent = ready.send(Err(crate::error_json(&error)));
            return;
        }
    };
    let initial = encode(subscription.started()).map_err(|error| crate::error_json(&error));
    if ready.send(initial).is_err() {
        return;
    }
    loop {
        let (item, terminal) = match subscription.next().await {
            Ok(ProviderNotification::Changed(value)) => (envelope("changed", &value), false),
            Ok(ProviderNotification::UsageChanged(value)) => {
                (envelope("usageChanged", &value), false)
            }
            Ok(ProviderNotification::Ended(value)) => (envelope("ended", &value), true),
            Err(error) => (Err(error), true),
        };
        if sender
            .send(item.map_err(|error| crate::error_json(&error)))
            .await
            .is_err()
            || terminal
        {
            return;
        }
    }
}

async fn session_index_subscription(
    config: ConnectConfig,
    ready: oneshot::Sender<Result<String, String>>,
    sender: mpsc::Sender<Result<String, String>>,
) {
    let mut runtime = match config.connect().await {
        Ok(runtime) => runtime,
        Err(error) => {
            let _sent = ready.send(Err(crate::error_json(&error)));
            return;
        }
    };
    let mut sessions = runtime.sessions();
    let mut subscription = match sessions.watch_index().await {
        Ok(subscription) => subscription,
        Err(error) => {
            let _sent = ready.send(Err(crate::error_json(&error)));
            return;
        }
    };
    let initial = encode(subscription.started()).map_err(|error| crate::error_json(&error));
    if ready.send(initial).is_err() {
        return;
    }
    loop {
        let (item, terminal) = match subscription.next().await {
            Ok(SessionIndexNotification::Changed(value)) => (envelope("changed", &value), false),
            Ok(SessionIndexNotification::Ended(value)) => (envelope("ended", &value), true),
            Err(error) => (Err(error), true),
        };
        if sender
            .send(item.map_err(|error| crate::error_json(&error)))
            .await
            .is_err()
            || terminal
        {
            return;
        }
    }
}

async fn event_subscription(
    config: ConnectConfig,
    params: WatchEventsParams,
    ready: oneshot::Sender<Result<String, String>>,
    sender: mpsc::Sender<Result<String, String>>,
) {
    let mut runtime = match config.connect().await {
        Ok(runtime) => runtime,
        Err(error) => {
            let _sent = ready.send(Err(crate::error_json(&error)));
            return;
        }
    };
    let mut sessions = runtime.sessions();
    let mut subscription = match sessions.watch_events(&params).await {
        Ok(subscription) => subscription,
        Err(error) => {
            let _sent = ready.send(Err(crate::error_json(&error)));
            return;
        }
    };
    let initial = encode(subscription.started()).map_err(|error| crate::error_json(&error));
    if ready.send(initial).is_err() {
        return;
    }
    loop {
        let item = subscription
            .next()
            .await
            .and_then(|notification| match notification {
                SessionNotification::Event(value) => envelope("event", &value),
                SessionNotification::Lagged(value) => envelope("lagged", &value),
            });
        if sender
            .send(item.map_err(|error| crate::error_json(&error)))
            .await
            .is_err()
        {
            return;
        }
    }
}

async fn terminal_index_subscription(
    config: ConnectConfig,
    ready: oneshot::Sender<Result<String, String>>,
    sender: mpsc::Sender<Result<String, String>>,
) {
    let mut runtime = match config.connect().await {
        Ok(runtime) => runtime,
        Err(error) => {
            let _sent = ready.send(Err(crate::error_json(&error)));
            return;
        }
    };
    let mut terminals = runtime.terminals();
    let mut subscription = match terminals.watch_index().await {
        Ok(subscription) => subscription,
        Err(error) => {
            let _sent = ready.send(Err(crate::error_json(&error)));
            return;
        }
    };
    let initial = encode(subscription.started()).map_err(|error| crate::error_json(&error));
    if ready.send(initial).is_err() {
        return;
    }
    loop {
        let (item, terminal) = match subscription.next().await {
            Ok(TerminalIndexNotification::Changed(value)) => (envelope("changed", &value), false),
            Ok(TerminalIndexNotification::Ended(value)) => (envelope("ended", &value), true),
            Err(error) => (Err(error), true),
        };
        if sender
            .send(item.map_err(|error| crate::error_json(&error)))
            .await
            .is_err()
            || terminal
        {
            return;
        }
    }
}

fn envelope<T: Serialize>(kind: &str, value: &T) -> Result<String, ClientError> {
    serde_json::to_string(&serde_json::json!({ "kind": kind, "value": value })).map_err(|error| {
        ClientError::Protocol(format!("subscription item cannot be encoded: {error}"))
    })
}

fn native_error_json(code: &str, message: &str) -> String {
    serde_json::json!({
        "code": code,
        "message": message,
        "retryable": false,
        "action": null,
        "correlationId": "python-client",
    })
    .to_string()
}
