//! Separate public Runtime listener and initial read-only method table.
//!
//! This module cannot accept a private control request because it does not deserialize the private request enum. The
//! initial slice negotiates the public contract and then returns default-deny enrollment failures for inventory until
//! the integration grant slice lands.

use runtrol_ipc::transport::Connection;
use runtrol_runtime_protocol::{
    ErrorResponse, FINALIZED_REVISIONS, InitializeParams, InitializeResult, JsonRpcId,
    JsonRpcNotification, JsonRpcRequest, JsonRpcResponse, RuntimeCapabilities, RuntimeError,
    RuntimeErrorKind, RuntimeInstance, RuntimeLimits, RuntimeMethod, SuccessResponse, negotiate,
};
use serde::Serialize;

/// Serve one public connection until it closes or violates the public frame contract.
pub(crate) async fn serve_connection(mut connection: Connection, instance_id: String) {
    let mut state = PublicState::Fresh;
    loop {
        let Ok(Some(payload)) = connection.recv().await else {
            return;
        };
        if matches!(state, PublicState::Negotiated) {
            let Ok(notification) = serde_json::from_slice::<JsonRpcNotification>(&payload) else {
                if let Ok(request) = serde_json::from_slice::<JsonRpcRequest>(&payload) {
                    let response = answer(&mut state, &instance_id, request);
                    if send_response(&mut connection, &response).await.is_err() {
                        return;
                    }
                    continue;
                }
                return;
            };
            if notification.jsonrpc != "2.0"
                || notification.method.parse::<RuntimeMethod>() != Ok(RuntimeMethod::Initialized)
                || serde_json::from_value::<EmptyParams>(notification.params).is_err()
            {
                return;
            }
            state = match state {
                PublicState::Negotiated => PublicState::Ready,
                PublicState::Fresh | PublicState::Ready => return,
            };
            continue;
        }

        let Ok(request) = serde_json::from_slice::<JsonRpcRequest>(&payload) else {
            return;
        };
        let response = answer(&mut state, &instance_id, request);
        if send_response(&mut connection, &response).await.is_err() {
            return;
        }
    }
}

#[derive(Clone, Copy)]
enum PublicState {
    Fresh,
    Negotiated,
    Ready,
}

fn answer(state: &mut PublicState, instance_id: &str, request: JsonRpcRequest) -> JsonRpcResponse {
    let id = request.id;
    if request.jsonrpc != "2.0" {
        return failure(
            id,
            RuntimeErrorKind::InvalidRequest,
            "JSON-RPC version must be 2.0",
        );
    }
    let Ok(method) = request.method.parse::<RuntimeMethod>() else {
        return failure(
            id,
            RuntimeErrorKind::MethodNotFound,
            "the public Runtime method does not exist",
        );
    };
    match (*state, method) {
        (_, RuntimeMethod::PanicStop) => failure(
            id,
            RuntimeErrorKind::CapabilityUnavailable,
            "panic stop is not admitted by the read-only Runtime slice",
        ),
        (PublicState::Fresh, RuntimeMethod::Initialize) => {
            initialize(state, instance_id, id, request.params)
        }
        (PublicState::Fresh | PublicState::Negotiated, _) => failure(
            id,
            RuntimeErrorKind::NotInitialized,
            "Runtime initialization is not complete",
        ),
        (PublicState::Ready, RuntimeMethod::ProvidersList) => enrollment_required(
            id,
            request.params,
            "provider list parameters are invalid",
            "local integration approval is required before provider inventory",
        ),
        (PublicState::Ready, RuntimeMethod::SessionsList) => enrollment_required(
            id,
            request.params,
            "session list parameters are invalid",
            "local integration approval is required before managed sessions",
        ),
        (PublicState::Ready, RuntimeMethod::Initialize | RuntimeMethod::Initialized) => failure(
            id,
            RuntimeErrorKind::InvalidRequest,
            "Runtime initialization cannot be repeated on one connection",
        ),
    }
}

fn initialize(
    state: &mut PublicState,
    instance_id: &str,
    id: JsonRpcId,
    params: serde_json::Value,
) -> JsonRpcResponse {
    let Ok(params) = serde_json::from_value::<InitializeParams>(params) else {
        return failure(
            id,
            RuntimeErrorKind::InvalidRequest,
            "initialization parameters are invalid",
        );
    };
    let Some(revision) = negotiate(&params.supported_revisions, &FINALIZED_REVISIONS) else {
        return failure(
            id,
            RuntimeErrorKind::ProtocolIncompatible,
            "no finalized Runtime revision is shared",
        );
    };
    *state = PublicState::Negotiated;
    success(
        id,
        &InitializeResult {
            selected_revision: revision,
            runtime: RuntimeInstance {
                instance_id: instance_id.to_owned(),
                version: env!("CARGO_PKG_VERSION").to_owned(),
                platform: platform_name().to_owned(),
            },
            server_capabilities: RuntimeCapabilities {
                provider_inventory: true,
                managed_session_list: true,
            },
            limits: RuntimeLimits::default(),
        },
    )
}

fn enrollment_required(
    id: JsonRpcId,
    params: serde_json::Value,
    invalid_message: &str,
    enrollment_message: &str,
) -> JsonRpcResponse {
    if serde_json::from_value::<EmptyParams>(params).is_err() {
        return failure(id, RuntimeErrorKind::InvalidRequest, invalid_message);
    }
    failure(id, RuntimeErrorKind::EnrollmentPending, enrollment_message)
}

fn success<T: Serialize>(id: JsonRpcId, result: &T) -> JsonRpcResponse {
    match serde_json::to_value(result) {
        Ok(result) => JsonRpcResponse::Success(SuccessResponse {
            jsonrpc: "2.0".to_owned(),
            id,
            result,
        }),
        Err(_) => failure(
            id,
            RuntimeErrorKind::Internal,
            "Runtime could not encode its public result",
        ),
    }
}

fn failure(id: JsonRpcId, code: RuntimeErrorKind, message: &str) -> JsonRpcResponse {
    JsonRpcResponse::Error(ErrorResponse {
        jsonrpc: "2.0".to_owned(),
        id,
        error: RuntimeError::plain(code, message, "runtime-public"),
    })
}

async fn send_response(
    connection: &mut Connection,
    response: &JsonRpcResponse,
) -> Result<(), runtrol_ipc::transport::TransportError> {
    let encoded = serde_json::to_vec(response).map_err(|error| {
        runtrol_ipc::transport::TransportError::Io {
            doing: "encoding a public Runtime response",
            detail: error.to_string(),
        }
    })?;
    connection.send(&encoded).await
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct EmptyParams {}

#[cfg(windows)]
const fn platform_name() -> &'static str {
    if cfg!(target_arch = "x86_64") {
        "windows-x86_64"
    } else {
        "windows-aarch64"
    }
}

#[cfg(target_os = "macos")]
const fn platform_name() -> &'static str {
    if cfg!(target_arch = "x86_64") {
        "macos-x86_64"
    } else {
        "macos-aarch64"
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
const fn platform_name() -> &'static str {
    if cfg!(target_arch = "x86_64") {
        "linux-x86_64"
    } else {
        "linux-aarch64"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_control_names_never_enter_the_public_method_table() {
        for private in [
            "hello",
            "list",
            "start",
            "providerUpdate",
            "private/control",
        ] {
            assert!(
                private.parse::<RuntimeMethod>().is_err(),
                "admitted {private:?}"
            );
        }
    }

    #[tokio::test]
    async fn real_owner_only_runtime_initializes_but_reveals_nothing_before_enrollment() {
        let directory =
            std::env::temp_dir().join(format!("runtrol-runtime-public-{}", std::process::id()));
        drop(std::fs::remove_dir_all(&directory));
        std::fs::create_dir_all(&directory).expect("create Runtime test directory");
        let endpoint = if cfg!(windows) {
            format!(r"\\.\pipe\runtrol-runtime-public-{}", std::process::id())
        } else {
            directory
                .join("runtrol-runtime.sock")
                .to_string_lossy()
                .into_owned()
        };
        let locator_path = directory.join("runtime.locator.json");
        let locator_abs = runtrol_provider::AbsPath::new(
            locator_path.to_str().expect("UTF-8 Runtime test locator"),
        )
        .expect("absolute Runtime test locator");
        let instance = "rtm_0123456789abcdef0123456789abcdef";
        let mut listener = runtrol_ipc::transport::Listener::bind_owner_only(&endpoint)
            .await
            .expect("bind owner-only Runtime endpoint");
        let published =
            crate::runtime_locator::PublishedLocator::publish(&locator_abs, instance, &endpoint)
                .expect("publish owner-only locator");
        let serving = tokio::spawn(async move {
            let connection = listener.accept().await.expect("accept public client");
            serve_connection(connection, instance.to_owned()).await;
        });

        let locator = runtrol_runtime_client::RuntimeLocator::for_testing(&locator_path);
        let mut client = locator
            .connect(runtrol_runtime_client::ClientOptions::new(
                "contract fixture",
                "1.0.0",
            ))
            .await
            .expect("initialize public client");
        let refused = client
            .providers()
            .list()
            .await
            .expect_err("inventory requires enrollment");
        assert!(matches!(
            refused,
            runtrol_runtime_client::ClientError::Runtime(error)
                if error.code == RuntimeErrorKind::EnrollmentPending
        ));

        drop(client);
        serving.await.expect("public server task finishes");
        drop(published);
        drop(std::fs::remove_dir_all(directory));
    }
}
