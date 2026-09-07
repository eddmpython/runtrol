//! One serial owner receiver over the typed public Rust SDK.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use pyo3::prelude::*;
use runtrol_runtime_client::{
    ClientError, RuntimeClient, WindowInputNotification, WindowInputSubscription,
};
use runtrol_runtime_protocol::{WatchWindowInputParams, WindowInputReceiptParams};
use serde::{Serialize, de::DeserializeOwned};
use tokio::sync::{mpsc, oneshot};

use crate::client::ConnectConfig;
use crate::{NativeError, native_error};

struct Command {
    operation: String,
    params: String,
    answer: oneshot::Sender<Result<String, String>>,
}

/// A single-consumer duplex. Cancelling a call or dropping the object closes its exact connection.
#[pyclass(module = "runtrol_runtime._native")]
pub(crate) struct PyWindowInput {
    started_json: String,
    sender: mpsc::Sender<Command>,
    busy: Arc<AtomicBool>,
    abort: tokio::task::AbortHandle,
}

#[pymethods]
impl PyWindowInput {
    /// Exact subscription identity and negotiated offer bound.
    #[getter]
    fn started_json(&self) -> &str {
        &self.started_json
    }

    /// Run one next, claim or receipt; concurrent operations are refused without queuing input.
    fn call<'py>(
        &self,
        py: Python<'py>,
        operation: String,
        params: String,
    ) -> PyResult<Bound<'py, PyAny>> {
        if self
            .busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(native_error(
                "invalidRequest",
                "the owner receiver already has an active operation",
            ));
        }
        let sender = self.sender.clone();
        let guard = Pending {
            busy: Arc::clone(&self.busy),
            abort: self.abort.clone(),
            completed: false,
        };
        pyo3_async_runtimes::tokio::future_into_py(py, invoke(sender, guard, operation, params))
    }

    /// Stop this exact receiver, including a blocked next or claim.
    fn close(&self) {
        self.abort.abort();
    }
}

impl Drop for PyWindowInput {
    fn drop(&mut self) {
        self.abort.abort();
    }
}

struct Pending {
    busy: Arc<AtomicBool>,
    abort: tokio::task::AbortHandle,
    completed: bool,
}

impl Pending {
    fn complete(&mut self) {
        self.completed = true;
    }
}

impl Drop for Pending {
    fn drop(&mut self) {
        if !self.completed {
            self.abort.abort();
        }
        self.busy.store(false, Ordering::Release);
    }
}

async fn invoke(
    sender: mpsc::Sender<Command>,
    mut guard: Pending,
    operation: String,
    params: String,
) -> PyResult<String> {
    let (answer, response) = oneshot::channel();
    sender
        .try_send(Command {
            operation,
            params,
            answer,
        })
        .map_err(|_| native_error("runtimeUnavailable", "the owner receiver is closed"))?;
    let result = response
        .await
        .map_err(|_| native_error("runtimeUnavailable", "the owner receiver ended"))?;
    guard.complete();
    result.map_err(NativeError::new_err)
}

pub(crate) async fn open(
    config: ConnectConfig,
    params_json: String,
) -> Result<PyWindowInput, String> {
    let params: WatchWindowInputParams =
        decode(&params_json).map_err(|error| crate::error_json(&error))?;
    let (ready, started) = oneshot::channel();
    // The API admits one active operation. This is a handoff slot, not a second input queue.
    let (sender, mut commands) = mpsc::channel::<Command>(1);
    let task = tokio::spawn(async move {
        let mut runtime = match config.connect().await {
            Ok(runtime) => runtime,
            Err(error) => {
                drop(ready.send(Err(crate::error_json(&error))));
                return;
            }
        };
        let mut windows = runtime.windows();
        let mut subscription = match windows.watch_input(&params).await {
            Ok(subscription) => subscription,
            Err(error) => {
                drop(ready.send(Err(crate::error_json(&error))));
                return;
            }
        };
        let encoded = encode(subscription.started()).map_err(|error| crate::error_json(&error));
        if ready.send(encoded).is_err() {
            return;
        }
        while let Some(command) = commands.recv().await {
            let result = input_call(&mut subscription, &command.operation, &command.params).await;
            if command
                .answer
                .send(result.map_err(|error| crate::error_json(&error)))
                .is_err()
            {
                return;
            }
        }
    });
    let abort = task.abort_handle();
    let mut guard = Pending {
        busy: Arc::new(AtomicBool::new(false)),
        abort: abort.clone(),
        completed: false,
    };
    let started_json = started.await.map_err(|_| {
        crate::error_json(&ClientError::Protocol(
            "owner receiver did not start".to_owned(),
        ))
    })??;
    guard.complete();
    Ok(PyWindowInput {
        started_json,
        sender,
        busy: Arc::clone(&guard.busy),
        abort,
    })
}

async fn input_call(
    subscription: &mut WindowInputSubscription<'_>,
    operation: &str,
    params: &str,
) -> Result<String, ClientError> {
    match operation {
        "next" => match subscription.next().await? {
            WindowInputNotification::Offered(offer) => {
                encode(&serde_json::json!({ "kind": "offered", "offered": offer }))
            }
            WindowInputNotification::Ended(ended) => {
                encode(&serde_json::json!({ "kind": "ended", "ended": ended }))
            }
        },
        "claimInput" => encode(&subscription.claim_input(decode::<u64>(params)?).await?),
        "inputReceipt" => {
            let params: WindowInputReceiptParams = decode(params)?;
            subscription.input_receipt(&params).await?;
            Ok("{}".to_owned())
        }
        _ => Err(ClientError::Protocol(
            "owner input accepts only next, claim and receipt".to_owned(),
        )),
    }
}

pub(crate) async fn window_call(
    runtime: &mut RuntimeClient,
    operation: &str,
    params: &str,
) -> Result<String, ClientError> {
    let mut windows = runtime.windows();
    match operation {
        "windows.register" => encode(&windows.register(&decode(params)?).await?),
        "windows.update" => {
            windows.update(&decode(params)?).await?;
            Ok("{}".to_owned())
        }
        "windows.mirrorOpen" => encode(&windows.mirror_open(&decode(params)?).await?),
        "windows.mirrorOutput" => {
            windows.mirror_output(&decode(params)?).await?;
            Ok("{}".to_owned())
        }
        "windows.mirrorEnd" => {
            windows.mirror_end(&decode(params)?).await?;
            Ok("{}".to_owned())
        }
        "windows.list" => encode(&windows.list().await?),
        "windows.reveal" => encode(&windows.reveal(&decode(params)?).await?),
        _ => Err(ClientError::Protocol(
            "the Python window operation is unknown".to_owned(),
        )),
    }
}

fn decode<T: DeserializeOwned>(value: &str) -> Result<T, ClientError> {
    serde_json::from_str(value).map_err(|error| {
        ClientError::Protocol(format!(
            "owner input parameters have the wrong shape: {error}"
        ))
    })
}

fn encode<T: Serialize>(value: &T) -> Result<String, ClientError> {
    serde_json::to_string(value).map_err(|error| {
        ClientError::Protocol(format!("owner input result cannot be encoded: {error}"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cancelled_owner_call_stops_its_actor_and_releases_the_single_slot() {
        let (started, ready) = oneshot::channel();
        let actor = tokio::spawn(async move {
            started.send(()).expect("signal actor start");
            std::future::pending::<()>().await;
        });
        ready.await.expect("actor started");
        let busy = Arc::new(AtomicBool::new(true));
        let operation = Pending {
            busy: Arc::clone(&busy),
            abort: actor.abort_handle(),
            completed: false,
        };
        let (sender, mut commands) = mpsc::channel(1);
        let call = tokio::spawn(invoke(
            sender,
            operation,
            "next".to_owned(),
            "{}".to_owned(),
        ));
        let command = commands
            .recv()
            .await
            .expect("one command reached its actor");
        assert!(busy.load(Ordering::Acquire));
        assert!(!actor.is_finished());
        call.abort();
        assert!(call.await.expect_err("cancelled call").is_cancelled());
        assert!(command.answer.is_closed());
        assert!(!busy.load(Ordering::Acquire));
        assert!(
            actor
                .await
                .expect_err("cancelled actor must end")
                .is_cancelled()
        );
    }

    #[tokio::test]
    async fn completed_owner_call_releases_the_slot_without_retiring_the_receiver() {
        let actor = tokio::spawn(std::future::pending::<()>());
        let busy = Arc::new(AtomicBool::new(true));
        let operation = Pending {
            busy: Arc::clone(&busy),
            abort: actor.abort_handle(),
            completed: false,
        };
        let (sender, mut commands) = mpsc::channel(1);
        let call = tokio::spawn(invoke(
            sender,
            operation,
            "next".to_owned(),
            "{}".to_owned(),
        ));
        let command = commands
            .recv()
            .await
            .expect("one command reached its actor");
        assert!(busy.load(Ordering::Acquire));
        command
            .answer
            .send(Ok("receipt".to_owned()))
            .expect("deliver result");
        assert_eq!(
            call.await.expect("call joined").expect("call succeeded"),
            "receipt"
        );
        assert!(!busy.load(Ordering::Acquire));
        assert!(!actor.is_finished());
        actor.abort();
        assert!(
            actor
                .await
                .expect_err("fixture actor explicitly closed")
                .is_cancelled()
        );
    }
}
