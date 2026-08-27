//! Python stable-ABI binding over the public Rust Runtime client.

mod client;
mod terminal;

use pyo3::prelude::*;
use pyo3::types::PyBytes;
use runtrol_runtime_client::{ClientError, IntegrationIdentity};
use runtrol_runtime_protocol::{IntegrationGrant, MutationRequestId};

use crate::client::{ConnectConfig, PyRuntimeClient, PySubscription, connect_client};
use crate::terminal::{PyTerminalEvent, PyTerminalView};

mod exceptions {
    #![allow(
        missing_docs,
        reason = "the PyO3 exception macro cannot attach Rust documentation to its generated struct"
    )]

    use pyo3::create_exception;
    use pyo3::exceptions::PyException;

    create_exception!(runtrol_runtime_native, NativeError, PyException);
}

use exceptions::NativeError;

/// A consumer-owned Ed25519 integration identity.
#[pyclass(module = "runtrol_runtime._native")]
struct PyIdentity {
    identity: IntegrationIdentity,
}

#[pymethods]
impl PyIdentity {
    /// Generate a new consumer-owned signing identity from the operating system random source.
    #[staticmethod]
    fn generate() -> PyResult<Self> {
        IntegrationIdentity::generate()
            .map(|identity| Self { identity })
            .map_err(|error| NativeError::new_err(error_json(&error)))
    }

    /// Restore one exact 32-byte consumer-owned signing identity.
    #[staticmethod]
    fn from_secret(secret: &[u8]) -> PyResult<Self> {
        let secret: [u8; 32] = secret.try_into().map_err(|_| {
            NativeError::new_err(native_error_payload(
                "invalidRequest",
                "an integration identity must contain exactly 32 bytes",
            ))
        })?;
        Ok(Self {
            identity: IntegrationIdentity::from_secret_bytes(secret),
        })
    }

    /// Return the secret bytes the consuming application must protect durably.
    fn secret_bytes<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        PyBytes::new(py, &self.identity.secret_bytes())
    }

    /// Return the public verification key for fingerprint comparison.
    fn public_key_base64(&self) -> String {
        self.identity.public_key_base64()
    }

    /// Sign the canonical self-approval proof for a first-party local integration.
    fn self_approval_signature(&self, pending_id: &str) -> PyResult<String> {
        let pending_id = serde_json::from_value(serde_json::Value::String(pending_id.to_owned()))
            .map_err(|error| {
            NativeError::new_err(native_error_payload(
                "invalidRequest",
                &format!("pending enrollment ID is malformed: {error}"),
            ))
        })?;
        self.identity
            .self_approval_signature(&pending_id)
            .map_err(|error| NativeError::new_err(error_json(&error)))
    }
}

/// Connect to the explicitly installed shared Runtime. This never installs or starts Runtime.
#[pyfunction]
#[pyo3(signature = (name, version, identity=None, grant_json=None))]
fn connect<'py>(
    py: Python<'py>,
    name: String,
    version: String,
    identity: Option<PyRef<'_, PyIdentity>>,
    grant_json: Option<String>,
) -> PyResult<Bound<'py, PyAny>> {
    let identity = identity.map(|value| value.identity.clone());
    let grant: Option<IntegrationGrant> = grant_json
        .map(|value| serde_json::from_str(&value))
        .transpose()
        .map_err(|error| {
            NativeError::new_err(native_error_payload(
                "invalidRequest",
                &format!("integration grant has the wrong shape: {error}"),
            ))
        })?;
    let config = ConnectConfig::new(name, version, identity, grant)?;
    pyo3_async_runtimes::tokio::future_into_py(py, async move {
        connect_client(config).await.map_err(NativeError::new_err)
    })
}

/// Mint one canonical UUIDv7 mutation identity for an exact state-changing request.
#[pyfunction]
fn new_mutation_request_id() -> String {
    MutationRequestId::now().to_string()
}

pub(crate) fn error_json(error: &ClientError) -> String {
    match error {
        ClientError::Runtime(runtime) => serde_json::to_string(runtime)
            .unwrap_or_else(|_| native_error_payload("internal", "Runtime error encoding failed")),
        ClientError::Locator(locator) => {
            native_error_payload("runtimeUnavailable", &locator.to_string())
        }
        ClientError::Transport { .. } => {
            native_error_payload("runtimeUnavailable", &error.to_string())
        }
        ClientError::Protocol(_) => {
            native_error_payload("protocolIncompatible", &error.to_string())
        }
        _ => native_error_payload("internal", &error.to_string()),
    }
}

pub(crate) fn native_error(code: &str, message: &str) -> PyErr {
    NativeError::new_err(native_error_payload(code, message))
}

fn native_error_payload(code: &str, message: &str) -> String {
    serde_json::json!({
        "code": code,
        "message": message,
        "retryable": false,
        "action": null,
        "correlationId": "python-client",
    })
    .to_string()
}

/// Install the private native module used by the typed Python package.
#[pymodule]
fn _native(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add("NativeError", module.py().get_type::<NativeError>())?;
    module.add_class::<PyIdentity>()?;
    module.add_class::<PyRuntimeClient>()?;
    module.add_class::<PySubscription>()?;
    module.add_class::<PyTerminalView>()?;
    module.add_class::<PyTerminalEvent>()?;
    module.add_function(wrap_pyfunction!(connect, module)?)?;
    module.add_function(wrap_pyfunction!(new_mutation_request_id, module)?)?;
    Ok(())
}
