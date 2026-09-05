//! Provider-faithful terminal operations over the public Runtime protocol.

use std::collections::VecDeque;

use base64ct::{Base64, Encoding as _};
use runtrol_runtime_protocol::{
    ErrorResponse, JsonRpcId, JsonRpcNotification, JsonRpcRequest, JsonRpcResponse,
    ListTerminalsParams, MutationRequestId, RuntimeError, RuntimeErrorKind, RuntimeMethod,
    SuccessResponse, TerminalAcquireControlParams, TerminalAttachParams, TerminalControlLease,
    TerminalControlParams, TerminalDetachParams, TerminalExitedNotification,
    TerminalIndexChangedNotification, TerminalIndexEndedNotification, TerminalIndexSnapshot,
    TerminalLaggedNotification, TerminalOpenParams, TerminalOutputNotification,
    TerminalResizeParams, TerminalSetDialogueParams, TerminalStopParams, TerminalViewOpened,
    TerminalWriteParams, WatchTerminalIndexParams, WatchTerminalIndexResult,
};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::{ClientError, ClientOptions, RuntimeClient, RuntimeLocator};

/// Typed provider-faithful terminal operations on one Runtime generation.
pub struct TerminalClient<'runtime> {
    runtime: &'runtime mut RuntimeClient,
}

impl<'runtime> TerminalClient<'runtime> {
    pub(crate) const fn new(runtime: &'runtime mut RuntimeClient) -> Self {
        Self { runtime }
    }

    /// List live terminal descriptors visible through the authenticated roots.
    ///
    /// # Errors
    ///
    /// Transport, protocol, scope, or Runtime failure.
    pub async fn list(&mut self) -> Result<TerminalIndexSnapshot, ClientError> {
        self.runtime
            .call(
                RuntimeMethod::TerminalsList,
                &ListTerminalsParams::default(),
            )
            .await
    }

    /// Convert this connection into one terminal-index subscription.
    ///
    /// # Errors
    ///
    /// Transport, protocol, scope, or Runtime failure.
    pub async fn watch_index(&mut self) -> Result<TerminalIndexSubscription<'_>, ClientError> {
        let started: WatchTerminalIndexResult = self
            .runtime
            .call(
                RuntimeMethod::TerminalsWatchIndex,
                &WatchTerminalIndexParams::default(),
            )
            .await?;
        Ok(TerminalIndexSubscription {
            runtime: self.runtime,
            subscription_id: started.subscription_id.clone(),
            started,
        })
    }

    /// Open or join a terminal and convert this connection into its dedicated view.
    ///
    /// # Errors
    ///
    /// Transport, protocol, provider, root, scope, admission, or Runtime failure.
    pub async fn open(
        &mut self,
        params: &TerminalOpenParams,
    ) -> Result<TerminalView<'_>, ClientError> {
        let opened: TerminalViewOpened = self
            .runtime
            .call_mutation(RuntimeMethod::TerminalsOpen, &params.request_id, params)
            .await?;
        TerminalView::new(self.runtime, opened)
    }

    /// Attach a new view to one exact terminal in this Runtime generation.
    ///
    /// # Errors
    ///
    /// Transport, protocol, root, scope, missing-terminal, or Runtime failure.
    pub async fn attach(
        &mut self,
        params: &TerminalAttachParams,
    ) -> Result<TerminalView<'_>, ClientError> {
        let opened: TerminalViewOpened = self
            .runtime
            .call(RuntimeMethod::TerminalsAttach, params)
            .await?;
        TerminalView::new(self.runtime, opened)
    }

    /// Validate and query every current and draining generation without hiding partial failures.
    ///
    /// # Errors
    ///
    /// The shared locator itself is missing, unsafe, malformed, or unreadable. Per-generation failures are returned
    /// inside their [`TerminalFleetEntry`].
    pub async fn list_all_generations(
        locator: &RuntimeLocator,
        options: ClientOptions,
    ) -> Result<Vec<TerminalFleetEntry>, ClientError> {
        let generations = locator.inspect_all()?;
        let mut entries = Vec::with_capacity(generations.len());
        for generation in generations {
            let digest = generation.digest().to_owned();
            let draining = generation.draining();
            let outcome = match RuntimeClient::connect_to(generation, options.clone()).await {
                Ok(runtime)
                    if !runtime
                        .initialization()
                        .server_capabilities
                        .terminal_surface =>
                {
                    TerminalFleetOutcome::Unsupported
                }
                Ok(mut runtime) => match runtime.terminals().list().await {
                    Ok(snapshot) => TerminalFleetOutcome::Listed(snapshot),
                    Err(error) => TerminalFleetOutcome::Failed(error),
                },
                Err(error) => TerminalFleetOutcome::Failed(error),
            };
            entries.push(TerminalFleetEntry {
                runtime_generation: digest,
                draining,
                outcome,
            });
        }
        Ok(entries)
    }
}

/// One explicit generation result from a fleet-wide terminal listing.
pub struct TerminalFleetEntry {
    /// Exact executable digest that owns this generation.
    pub runtime_generation: String,
    /// Whether a successor owns new work.
    pub draining: bool,
    /// Explicit query result for this generation.
    pub outcome: TerminalFleetOutcome,
}

/// A generation is always represented, including unsupported and unreachable peers.
pub enum TerminalFleetOutcome {
    /// The generation supports the terminal contract and returned its visible descriptors.
    Listed(TerminalIndexSnapshot),
    /// The generation predates the public terminal contract.
    Unsupported,
    /// The generation could not authenticate, negotiate, or answer.
    Failed(ClientError),
}

#[derive(serde::Deserialize)]
pub(crate) struct EmptyResult {}

/// One terminal-index stream notification.
#[derive(Debug)]
pub enum TerminalIndexNotification {
    /// The complete caller-visible index changed.
    Changed(TerminalIndexChangedNotification),
    /// Runtime ended the subscription with a typed reason.
    Ended(TerminalIndexEndedNotification),
}

/// One dedicated terminal-index stream borrowed from a Runtime connection.
pub struct TerminalIndexSubscription<'runtime> {
    runtime: &'runtime mut RuntimeClient,
    subscription_id: String,
    started: WatchTerminalIndexResult,
}

impl TerminalIndexSubscription<'_> {
    /// Initial complete caller-visible terminal snapshot.
    #[must_use]
    pub const fn started(&self) -> &WatchTerminalIndexResult {
        &self.started
    }

    /// Wait for a changed index or typed end reason.
    ///
    /// # Errors
    ///
    /// Transport failure or a notification outside the selected protocol revision.
    pub async fn next(&mut self) -> Result<TerminalIndexNotification, ClientError> {
        let payload = self.runtime.connection.receive().await?;
        let notification = decode_notification(&payload, "terminal index")?;
        match parse_method(&notification, "terminal index")? {
            RuntimeMethod::TerminalsIndexChanged => {
                let changed: TerminalIndexChangedNotification =
                    decode_params(notification.params, "terminal index change")?;
                require_subscription(&self.subscription_id, &changed.subscription_id)?;
                Ok(TerminalIndexNotification::Changed(changed))
            }
            RuntimeMethod::TerminalsIndexEnded => {
                let ended: TerminalIndexEndedNotification =
                    decode_params(notification.params, "terminal index end")?;
                require_subscription(&self.subscription_id, &ended.subscription_id)?;
                Ok(TerminalIndexNotification::Ended(ended))
            }
            _ => Err(ClientError::Protocol(
                "the dedicated terminal index stream received a different method".to_owned(),
            )),
        }
    }
}

/// Exact terminal bytes or one explicit view lifecycle boundary.
#[derive(Debug, PartialEq, Eq)]
pub enum TerminalNotification {
    /// One exact bounded provider-output chunk.
    Output {
        /// Monotonic sequence within this view.
        sequence: u64,
        /// Decoded exact provider bytes.
        bytes: Vec<u8>,
    },
    /// Explicit output loss with an atomic replacement screen boundary.
    Lagged {
        /// Broadcast chunks skipped before replacement.
        lost_chunks: u64,
        /// Decoded replacement screen bytes.
        screen: Vec<u8>,
        /// Sequence assigned to the next output chunk.
        next_sequence: u64,
    },
    /// The hosted provider process ended after earlier output drained.
    Exited {
        /// Provider process exit code.
        exit_code: i32,
    },
}

/// One connection-bound terminal view. Dropping it detaches and never stops the provider process.
pub struct TerminalView<'runtime> {
    runtime: &'runtime mut RuntimeClient,
    opened: TerminalViewOpened,
    initial_screen: Vec<u8>,
    pending: VecDeque<TerminalNotification>,
    ended: bool,
}

impl<'runtime> TerminalView<'runtime> {
    fn new(
        runtime: &'runtime mut RuntimeClient,
        opened: TerminalViewOpened,
    ) -> Result<Self, ClientError> {
        let initial_screen = decode_bytes(&opened.screen_base64, "terminal screen snapshot")?;
        Ok(Self {
            runtime,
            opened,
            initial_screen,
            pending: VecDeque::new(),
            ended: false,
        })
    }

    /// Descriptor, view identity, and initial lease returned by Runtime.
    #[must_use]
    pub const fn opened(&self) -> &TerminalViewOpened {
        &self.opened
    }

    /// Initial decoded terminal screen snapshot.
    #[must_use]
    pub fn initial_screen(&self) -> &[u8] {
        &self.initial_screen
    }

    /// Wait for exact output, an explicit loss boundary, or process exit.
    ///
    /// # Errors
    ///
    /// Transport failure or a notification outside the selected protocol revision.
    pub async fn next(&mut self) -> Result<TerminalNotification, ClientError> {
        if let Some(notification) = self.pending.pop_front() {
            return Ok(notification);
        }
        if self.ended {
            return Err(ClientError::Protocol(
                "terminal view already ended".to_owned(),
            ));
        }
        let payload = self.runtime.connection.receive().await?;
        let notification = self.decode_terminal_notification(&payload)?;
        if matches!(notification, TerminalNotification::Exited { .. }) {
            self.ended = true;
        }
        Ok(notification)
    }

    /// Acquire the one renewable control lease.
    ///
    /// # Errors
    ///
    /// Transport, protocol, scope, lease-conflict, visibility, or Runtime failure.
    pub async fn acquire_control(
        &mut self,
        params: &TerminalAcquireControlParams,
    ) -> Result<TerminalControlLease, ClientError> {
        self.command(
            RuntimeMethod::TerminalsAcquireControl,
            params,
            Some(&params.request_id),
        )
        .await
    }

    /// Renew one exact lease generation.
    ///
    /// # Errors
    ///
    /// Transport, protocol, scope, stale-lease, visibility, or Runtime failure.
    pub async fn renew_control(
        &mut self,
        params: &TerminalControlParams,
    ) -> Result<TerminalControlLease, ClientError> {
        self.command(
            RuntimeMethod::TerminalsRenewControl,
            params,
            Some(&params.request_id),
        )
        .await
    }

    /// Release one exact lease generation.
    ///
    /// # Errors
    ///
    /// Transport, protocol, scope, stale-lease, visibility, or Runtime failure.
    pub async fn release_control(
        &mut self,
        params: &TerminalControlParams,
    ) -> Result<(), ClientError> {
        let _: EmptyResult = self
            .command(
                RuntimeMethod::TerminalsReleaseControl,
                params,
                Some(&params.request_id),
            )
            .await?;
        Ok(())
    }

    /// Write exact caller-owned bytes once under the current lease.
    ///
    /// # Errors
    ///
    /// Transport, protocol, scope, stale-lease, size, visibility, or Runtime failure.
    pub async fn write(&mut self, params: &TerminalWriteParams) -> Result<(), ClientError> {
        let _: EmptyResult = self
            .command(
                RuntimeMethod::TerminalsWrite,
                params,
                Some(&params.request_id),
            )
            .await?;
        Ok(())
    }

    /// Resize the shared PTY once under the current lease.
    ///
    /// # Errors
    ///
    /// Transport, protocol, scope, stale-lease, geometry, visibility, or Runtime failure.
    pub async fn resize(&mut self, params: &TerminalResizeParams) -> Result<(), ClientError> {
        let _: EmptyResult = self
            .command(
                RuntimeMethod::TerminalsResize,
                params,
                Some(&params.request_id),
            )
            .await?;
        Ok(())
    }

    /// Stop only the hosted provider process under the current lease.
    ///
    /// # Errors
    ///
    /// Transport, protocol, scope, stale-lease, visibility, or Runtime failure.
    pub async fn stop(&mut self, params: &TerminalStopParams) -> Result<(), ClientError> {
        let _: EmptyResult = self
            .command(
                RuntimeMethod::TerminalsStop,
                params,
                Some(&params.request_id),
            )
            .await?;
        Ok(())
    }

    /// Enable or disable dialogue for this live terminal under the current input lease.
    ///
    /// # Errors
    ///
    /// Transport, protocol, scope, stale-lease, visibility, or Runtime failure.
    pub async fn set_dialogue(
        &mut self,
        params: &TerminalSetDialogueParams,
    ) -> Result<(), ClientError> {
        let _: EmptyResult = self
            .command(
                RuntimeMethod::TerminalsSetDialogue,
                params,
                Some(&params.request_id),
            )
            .await?;
        Ok(())
    }

    /// End only this view. Dropping the view has the same process-lifetime effect.
    ///
    /// # Errors
    ///
    /// Transport, protocol, scope, view-identity, or Runtime failure.
    pub async fn detach(mut self, params: &TerminalDetachParams) -> Result<(), ClientError> {
        let _: EmptyResult = self
            .command::<_, EmptyResult>(RuntimeMethod::TerminalsDetach, params, None)
            .await?;
        self.ended = true;
        Ok(())
    }

    async fn command<P: Serialize, R: DeserializeOwned>(
        &mut self,
        method: RuntimeMethod,
        params: &P,
        mutation: Option<&MutationRequestId>,
    ) -> Result<R, ClientError> {
        let id = JsonRpcId::Number(self.runtime.next_id);
        self.runtime.next_id = self.runtime.next_id.checked_add(1).ok_or_else(|| {
            ClientError::Protocol("the connection exhausted its request identifiers".to_owned())
        })?;
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_owned(),
            id: id.clone(),
            method: method.to_string(),
            params: serde_json::to_value(params).map_err(|error| {
                ClientError::Protocol(format!(
                    "terminal request parameters cannot be encoded: {error}"
                ))
            })?,
        };
        let encoded = serde_json::to_vec(&request).map_err(|error| {
            ClientError::Protocol(format!("terminal request cannot be encoded: {error}"))
        })?;
        if let Err(error) = self.runtime.connection.send(&encoded).await {
            return Err(mutation_failure(error, mutation));
        }
        loop {
            let payload = match self.runtime.connection.receive().await {
                Ok(payload) => payload,
                Err(error) => return Err(mutation_failure(error, mutation)),
            };
            if let Ok(response) = serde_json::from_slice::<JsonRpcResponse>(&payload) {
                return decode_response(response, &id);
            }
            let notification = self.decode_terminal_notification(&payload)?;
            if matches!(notification, TerminalNotification::Exited { .. }) {
                self.ended = true;
            }
            self.pending.push_back(notification);
        }
    }

    fn decode_terminal_notification(
        &self,
        payload: &[u8],
    ) -> Result<TerminalNotification, ClientError> {
        let notification = decode_notification(payload, "terminal view")?;
        match parse_method(&notification, "terminal view")? {
            RuntimeMethod::TerminalsOutput => {
                let output: TerminalOutputNotification =
                    decode_params(notification.params, "terminal output")?;
                self.require_view(&output.view_id)?;
                Ok(TerminalNotification::Output {
                    sequence: output.sequence,
                    bytes: decode_bytes(&output.bytes_base64, "terminal output")?,
                })
            }
            RuntimeMethod::TerminalsLagged => {
                let lagged: TerminalLaggedNotification =
                    decode_params(notification.params, "terminal lag")?;
                self.require_view(&lagged.view_id)?;
                Ok(TerminalNotification::Lagged {
                    lost_chunks: lagged.lost_chunks,
                    screen: decode_bytes(&lagged.screen_base64, "terminal replacement screen")?,
                    next_sequence: lagged.next_sequence,
                })
            }
            RuntimeMethod::TerminalsExited => {
                let exited: TerminalExitedNotification =
                    decode_params(notification.params, "terminal exit")?;
                self.require_view(&exited.view_id)?;
                Ok(TerminalNotification::Exited {
                    exit_code: exited.exit_code,
                })
            }
            _ => Err(ClientError::Protocol(
                "the dedicated terminal view received a different method".to_owned(),
            )),
        }
    }

    fn require_view(
        &self,
        actual: &runtrol_runtime_protocol::RuntimeTerminalViewId,
    ) -> Result<(), ClientError> {
        if actual == &self.opened.view_id {
            Ok(())
        } else {
            Err(ClientError::Protocol(
                "terminal notification target does not match its view".to_owned(),
            ))
        }
    }
}

fn mutation_failure(error: ClientError, mutation: Option<&MutationRequestId>) -> ClientError {
    if matches!(error, ClientError::Transport { .. })
        && let Some(request_id) = mutation
    {
        return ClientError::Runtime(RuntimeError::plain(
            RuntimeErrorKind::OutcomeUnknown,
            "Runtime connection ended while the terminal mutation outcome was unresolved",
            request_id.as_str(),
        ));
    }
    error
}

fn decode_response<R: DeserializeOwned>(
    response: JsonRpcResponse,
    expected: &JsonRpcId,
) -> Result<R, ClientError> {
    match response {
        JsonRpcResponse::Success(SuccessResponse {
            jsonrpc,
            id,
            result,
        }) => {
            require_response_id(&jsonrpc, expected, &id)?;
            serde_json::from_value(result).map_err(|error| {
                ClientError::Protocol(format!("terminal result has the wrong shape: {error}"))
            })
        }
        JsonRpcResponse::Error(ErrorResponse { jsonrpc, id, error }) => {
            require_response_id(&jsonrpc, expected, &id)?;
            Err(ClientError::Runtime(error))
        }
    }
}

fn require_response_id(
    jsonrpc: &str,
    expected: &JsonRpcId,
    actual: &JsonRpcId,
) -> Result<(), ClientError> {
    if jsonrpc != "2.0" || expected != actual {
        return Err(ClientError::Protocol(
            "terminal response envelope does not match its request".to_owned(),
        ));
    }
    Ok(())
}

pub(crate) fn decode_notification(
    payload: &[u8],
    surface: &str,
) -> Result<JsonRpcNotification, ClientError> {
    let notification: JsonRpcNotification = serde_json::from_slice(payload).map_err(|error| {
        ClientError::Protocol(format!(
            "{surface} notification is not valid JSON-RPC: {error}"
        ))
    })?;
    if notification.jsonrpc != "2.0" {
        return Err(ClientError::Protocol(format!(
            "{surface} notification JSON-RPC version is not 2.0"
        )));
    }
    Ok(notification)
}

pub(crate) fn parse_method(
    notification: &JsonRpcNotification,
    surface: &str,
) -> Result<RuntimeMethod, ClientError> {
    notification
        .method
        .parse()
        .map_err(|_| ClientError::Protocol(format!("{surface} notification method is unknown")))
}

pub(crate) fn decode_params<T: DeserializeOwned>(
    params: serde_json::Value,
    surface: &str,
) -> Result<T, ClientError> {
    serde_json::from_value(params).map_err(|error| {
        ClientError::Protocol(format!(
            "{surface} notification has the wrong shape: {error}"
        ))
    })
}

pub(crate) fn require_subscription(expected: &str, actual: &str) -> Result<(), ClientError> {
    if expected == actual {
        Ok(())
    } else {
        Err(ClientError::Protocol(
            "terminal index notification target does not match its subscription".to_owned(),
        ))
    }
}

fn decode_bytes(encoded: &str, surface: &str) -> Result<Vec<u8>, ClientError> {
    Base64::decode_vec(encoded)
        .map_err(|error| ClientError::Protocol(format!("{surface} is not valid base64: {error}")))
}
