//! Dedicated actor for one provider-faithful terminal view.

use std::collections::VecDeque;

use pyo3::prelude::*;
use pyo3::types::PyBytes;
use runtrol_runtime_client::{ClientError, TerminalNotification};
use runtrol_runtime_protocol::{
    TerminalAcquireControlParams, TerminalAttachParams, TerminalControlParams,
    TerminalDetachParams, TerminalOpenParams, TerminalResizeParams, TerminalSetDialogueParams,
    TerminalStopParams, TerminalWriteParams,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::sync::{mpsc, oneshot};

use crate::client::ConnectConfig;
use crate::{NativeError, native_error};

const TERMINAL_COMMAND_CAPACITY: usize = 64;
const TERMINAL_EVENT_CAPACITY: usize = 64;

enum TerminalCommand {
    Call {
        operation: Box<str>,
        params: Box<str>,
        answer: oneshot::Sender<Result<String, String>>,
    },
    Next {
        answer: oneshot::Sender<Result<TerminalEventPayload, String>>,
    },
}

struct TerminalEventPayload {
    kind: &'static str,
    sequence: Option<u64>,
    bytes: Vec<u8>,
    lost_chunks: Option<u64>,
    next_sequence: Option<u64>,
    exit_code: Option<i32>,
}

impl From<TerminalNotification> for TerminalEventPayload {
    fn from(notification: TerminalNotification) -> Self {
        match notification {
            TerminalNotification::Output { sequence, bytes } => Self {
                kind: "output",
                sequence: Some(sequence),
                bytes,
                lost_chunks: None,
                next_sequence: None,
                exit_code: None,
            },
            TerminalNotification::Lagged {
                lost_chunks,
                screen,
                next_sequence,
            } => Self {
                kind: "lagged",
                sequence: None,
                bytes: screen,
                lost_chunks: Some(lost_chunks),
                next_sequence: Some(next_sequence),
                exit_code: None,
            },
            TerminalNotification::Exited { exit_code } => Self {
                kind: "exited",
                sequence: None,
                bytes: Vec::new(),
                lost_chunks: None,
                next_sequence: None,
                exit_code: Some(exit_code),
            },
        }
    }
}

/// One exact terminal event, with provider bytes kept as bytes.
#[pyclass(module = "runtrol_runtime._native")]
pub(crate) struct PyTerminalEvent {
    payload: TerminalEventPayload,
}

#[pymethods]
impl PyTerminalEvent {
    /// `output`, `lagged`, or `exited`.
    #[getter]
    fn kind(&self) -> &str {
        self.payload.kind
    }

    /// Output sequence when this is an output event.
    #[getter]
    const fn sequence(&self) -> Option<u64> {
        self.payload.sequence
    }

    /// Exact output bytes, or the replacement screen bytes on lag.
    fn bytes<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        PyBytes::new(py, &self.payload.bytes)
    }

    /// Chunks explicitly lost before a replacement screen.
    #[getter]
    const fn lost_chunks(&self) -> Option<u64> {
        self.payload.lost_chunks
    }

    /// Sequence assigned to output after a replacement screen.
    #[getter]
    const fn next_sequence(&self) -> Option<u64> {
        self.payload.next_sequence
    }

    /// Provider process exit code when this is the terminal event.
    #[getter]
    const fn exit_code(&self) -> Option<i32> {
        self.payload.exit_code
    }
}

/// One connection-bound terminal view running on a dedicated Rust actor.
#[pyclass(module = "runtrol_runtime._native")]
pub(crate) struct PyTerminalView {
    sender: mpsc::Sender<TerminalCommand>,
    opened_json: String,
    initial_screen: Vec<u8>,
}

#[pymethods]
impl PyTerminalView {
    /// Descriptor, view identity, and initial control lease.
    #[getter]
    fn opened_json(&self) -> &str {
        &self.opened_json
    }

    /// Current bounded screen snapshot delivered before live output.
    fn initial_screen<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        PyBytes::new(py, &self.initial_screen)
    }

    /// Wait for exact output, an explicit lag replacement, or process exit.
    fn next<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let sender = self.sender.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let (answer, receive) = oneshot::channel();
            sender
                .send(TerminalCommand::Next { answer })
                .await
                .map_err(|_| native_error("terminalGone", "the terminal view is closed"))?;
            receive
                .await
                .map_err(|_| native_error("terminalGone", "the terminal view ended"))?
                .map(|payload| PyTerminalEvent { payload })
                .map_err(NativeError::new_err)
        })
    }

    /// Run one typed terminal control operation while output keeps draining.
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
                .send(TerminalCommand::Call {
                    operation: operation.into(),
                    params: params_json.into(),
                    answer,
                })
                .await
                .map_err(|_| native_error("terminalGone", "the terminal view is closed"))?;
            receive
                .await
                .map_err(|_| native_error("terminalGone", "the terminal view ended"))?
                .map_err(NativeError::new_err)
        })
    }
}

pub(crate) async fn open_terminal(
    config: ConnectConfig,
    kind: String,
    params_json: String,
    runtime_generation: Option<String>,
) -> Result<PyTerminalView, String> {
    let (ready, opened) = oneshot::channel();
    let (sender, commands) = mpsc::channel(TERMINAL_COMMAND_CAPACITY);
    tokio::spawn(run_terminal(
        config,
        kind,
        params_json,
        runtime_generation,
        ready,
        commands,
    ));
    let (opened_json, initial_screen) = opened.await.map_err(|_| {
        serde_json::json!({
            "code": "runtimeUnavailable",
            "message": "the terminal actor did not start",
            "retryable": true,
            "action": null,
            "correlationId": "python-terminal",
        })
        .to_string()
    })??;
    Ok(PyTerminalView {
        sender,
        opened_json,
        initial_screen,
    })
}

async fn run_terminal(
    config: ConnectConfig,
    kind: String,
    params_json: String,
    runtime_generation: Option<String>,
    ready: oneshot::Sender<Result<(String, Vec<u8>), String>>,
    mut commands: mpsc::Receiver<TerminalCommand>,
) {
    let mut runtime = match config.connect_terminal(runtime_generation.as_deref()).await {
        Ok(runtime) => runtime,
        Err(error) => {
            let _sent = ready.send(Err(crate::error_json(&error)));
            return;
        }
    };
    let mut terminals = runtime.terminals();
    let view = match kind.as_str() {
        "open" => match decode::<TerminalOpenParams>(&params_json) {
            Ok(params) => terminals.open(&params).await,
            Err(error) => Err(error),
        },
        "attach" => match decode::<TerminalAttachParams>(&params_json) {
            Ok(params) => terminals.attach(&params).await,
            Err(error) => Err(error),
        },
        _ => Err(ClientError::Protocol(
            "terminal view kind must be open or attach".to_owned(),
        )),
    };
    let mut view = match view {
        Ok(view) => view,
        Err(error) => {
            let _sent = ready.send(Err(crate::error_json(&error)));
            return;
        }
    };
    let opened_json = match encode(view.opened()) {
        Ok(value) => value,
        Err(error) => {
            let _sent = ready.send(Err(crate::error_json(&error)));
            return;
        }
    };
    if ready
        .send(Ok((opened_json, view.initial_screen().to_vec())))
        .is_err()
    {
        return;
    }

    let mut queued = VecDeque::new();
    let mut waiter: Option<oneshot::Sender<Result<TerminalEventPayload, String>>> = None;
    let mut ended = false;
    loop {
        tokio::select! {
            command = commands.recv() => {
                let Some(command) = command else { return; };
                match command {
                    TerminalCommand::Next { answer } => {
                        if let Some(event) = queued.pop_front() {
                            let _sent = answer.send(Ok(event));
                        } else if ended {
                            let _sent = answer.send(Err(terminal_error("terminalGone", "the terminal process has exited")));
                        } else if waiter.is_some() {
                            let _sent = answer.send(Err(terminal_error("invalidRequest", "only one terminal next call may wait at a time")));
                        } else {
                            waiter = Some(answer);
                        }
                    }
                    TerminalCommand::Call { operation, params, answer } => {
                        if operation.as_ref() == "detach" {
                            let result = match decode::<TerminalDetachParams>(&params) {
                                Ok(params) => view.detach(&params).await.map(|()| "{}".to_owned()),
                                Err(error) => Err(error),
                            };
                            let _sent = answer.send(result.map_err(|error| crate::error_json(&error)));
                            return;
                        }
                        let result = terminal_call(&mut view, &operation, &params)
                            .await
                            .map_err(|error| crate::error_json(&error));
                        let _sent = answer.send(result);
                    }
                }
            }
            event = view.next(), if !ended && (waiter.is_some() || queued.len() < TERMINAL_EVENT_CAPACITY) => {
                match event {
                    Ok(notification) => {
                        ended = matches!(notification, TerminalNotification::Exited { .. });
                        let payload = TerminalEventPayload::from(notification);
                        if let Some(answer) = waiter.take() {
                            let _sent = answer.send(Ok(payload));
                        } else {
                            queued.push_back(payload);
                        }
                    }
                    Err(error) => {
                        ended = true;
                        let failure = crate::error_json(&error);
                        if let Some(answer) = waiter.take() {
                            let _sent = answer.send(Err(failure));
                        }
                    }
                }
            }
        }
    }
}

async fn terminal_call(
    view: &mut runtrol_runtime_client::TerminalView<'_>,
    operation: &str,
    params_json: &str,
) -> Result<String, ClientError> {
    match operation {
        "acquireControl" => {
            let params = decode::<TerminalAcquireControlParams>(params_json)?;
            encode(&view.acquire_control(&params).await?)
        }
        "renewControl" => {
            let params = decode::<TerminalControlParams>(params_json)?;
            encode(&view.renew_control(&params).await?)
        }
        "releaseControl" => {
            let params = decode::<TerminalControlParams>(params_json)?;
            view.release_control(&params).await?;
            Ok("{}".to_owned())
        }
        "write" => {
            let params = decode::<TerminalWriteParams>(params_json)?;
            view.write(&params).await?;
            Ok("{}".to_owned())
        }
        "resize" => {
            let params = decode::<TerminalResizeParams>(params_json)?;
            view.resize(&params).await?;
            Ok("{}".to_owned())
        }
        "stop" => {
            let params = decode::<TerminalStopParams>(params_json)?;
            view.stop(&params).await?;
            Ok("{}".to_owned())
        }
        "setDialogue" => {
            let params = decode::<TerminalSetDialogueParams>(params_json)?;
            view.set_dialogue(&params).await?;
            Ok("{}".to_owned())
        }
        _ => Err(ClientError::Protocol(format!(
            "the Python terminal has no operation named {operation}"
        ))),
    }
}

fn decode<T: DeserializeOwned>(value: &str) -> Result<T, ClientError> {
    serde_json::from_str(value).map_err(|error| {
        ClientError::Protocol(format!(
            "Python terminal parameters have the wrong shape: {error}"
        ))
    })
}

fn encode<T: Serialize>(value: &T) -> Result<String, ClientError> {
    serde_json::to_string(value).map_err(|error| {
        ClientError::Protocol(format!("Python terminal result cannot be encoded: {error}"))
    })
}

fn terminal_error(code: &str, message: &str) -> String {
    serde_json::json!({
        "code": code,
        "message": message,
        "retryable": false,
        "action": null,
        "correlationId": "python-terminal",
    })
    .to_string()
}
