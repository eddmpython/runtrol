//! Typed initialization and read-only Runtime operation groups.

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::ClientError;
use crate::connection::Connection;
use crate::locator::{LocatorState, RuntimeLocator, ValidatedLocator};
use runtrol_runtime_protocol::{
    ClientCapabilities, ClientInfo, ErrorResponse, FINALIZED_REVISIONS, InitializeParams,
    InitializeResult, JsonRpcId, JsonRpcNotification, JsonRpcRequest, JsonRpcResponse,
    ManagedSessionList, ProviderList, RuntimeMethod, SuccessResponse,
};

/// Safe client metadata used during public revision negotiation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientOptions {
    name: String,
    version: String,
    capabilities: ClientCapabilities,
}

impl ClientOptions {
    /// Describe a consumer without claiming authorization identity.
    #[must_use]
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            version: version.into(),
            capabilities: ClientCapabilities::default(),
        }
    }
}

impl RuntimeLocator {
    /// Connect, negotiate the newest common revision, prove the locator instance, and finish initialization.
    ///
    /// # Errors
    ///
    /// Locator validation, local transport, protocol, incompatibility, or Runtime failures.
    pub async fn connect(&self, options: ClientOptions) -> Result<RuntimeClient, ClientError> {
        let LocatorState::Running(locator) = self.inspect()? else {
            return Err(ClientError::Runtime(
                runtrol_runtime_protocol::RuntimeError::plain(
                    runtrol_runtime_protocol::RuntimeErrorKind::RuntimeNotInstalled,
                    "Runtrol Runtime is not installed",
                    "local-locator",
                ),
            ));
        };
        RuntimeClient::connect(locator, options).await
    }
}

/// One initialized public connection. It owns no Runtime or provider session.
pub struct RuntimeClient {
    connection: Connection,
    next_id: u64,
    initialized: InitializeResult,
}

impl RuntimeClient {
    async fn connect(
        locator: ValidatedLocator,
        options: ClientOptions,
    ) -> Result<Self, ClientError> {
        let mut connection = Connection::connect(&locator.endpoint).await?;
        let mut next_id = 1;
        let initialized: InitializeResult = call_connection(
            &mut connection,
            &mut next_id,
            RuntimeMethod::Initialize,
            &InitializeParams {
                supported_revisions: FINALIZED_REVISIONS.to_vec(),
                client: ClientInfo {
                    name: options.name,
                    version: options.version,
                },
                client_capabilities: options.capabilities,
            },
        )
        .await?;
        if initialized.runtime.instance_id != locator.instance_id {
            return Err(ClientError::Protocol(
                "the Runtime instance does not match the locator".to_owned(),
            ));
        }
        if initialized.runtime.version != locator.runtime_version {
            return Err(ClientError::Protocol(
                "the Runtime version does not match the locator".to_owned(),
            ));
        }
        if !FINALIZED_REVISIONS.contains(&initialized.selected_revision) {
            return Err(ClientError::Protocol(
                "the Runtime selected a revision the client did not offer".to_owned(),
            ));
        }
        notify_connection(&mut connection, RuntimeMethod::Initialized, &EmptyParams {}).await?;
        Ok(Self {
            connection,
            next_id,
            initialized,
        })
    }

    /// The selected revision, Runtime instance, capabilities, and limits.
    #[must_use]
    pub const fn initialization(&self) -> &InitializeResult {
        &self.initialized
    }

    /// Provider inventory operations.
    pub fn providers(&mut self) -> ProviderClient<'_> {
        ProviderClient { runtime: self }
    }

    /// Runtime-managed session operations.
    pub fn sessions(&mut self) -> SessionClient<'_> {
        SessionClient { runtime: self }
    }

    async fn call<P: Serialize, R: DeserializeOwned>(
        &mut self,
        method: RuntimeMethod,
        params: &P,
    ) -> Result<R, ClientError> {
        call_connection(&mut self.connection, &mut self.next_id, method, params).await
    }
}

async fn call_connection<P: Serialize, R: DeserializeOwned>(
    connection: &mut Connection,
    next_id: &mut u64,
    method: RuntimeMethod,
    params: &P,
) -> Result<R, ClientError> {
    let id = JsonRpcId::Number(*next_id);
    *next_id = next_id.checked_add(1).ok_or_else(|| {
        ClientError::Protocol("the connection exhausted its request identifiers".to_owned())
    })?;
    let params = serde_json::to_value(params).map_err(|error| {
        ClientError::Protocol(format!("request parameters cannot be encoded: {error}"))
    })?;
    let request = JsonRpcRequest {
        jsonrpc: "2.0".to_owned(),
        id: id.clone(),
        method: method.to_string(),
        params,
    };
    let encoded = serde_json::to_vec(&request)
        .map_err(|error| ClientError::Protocol(format!("request cannot be encoded: {error}")))?;
    connection.send(&encoded).await?;
    let response = connection.receive().await?;
    let response: JsonRpcResponse = serde_json::from_slice(&response).map_err(|error| {
        ClientError::Protocol(format!("response is not valid public JSON-RPC: {error}"))
    })?;
    match response {
        JsonRpcResponse::Success(SuccessResponse {
            jsonrpc,
            id: response_id,
            result,
        }) => {
            validate_envelope(&jsonrpc, &id, &response_id)?;
            serde_json::from_value(result).map_err(|error| {
                ClientError::Protocol(format!("method result has the wrong shape: {error}"))
            })
        }
        JsonRpcResponse::Error(ErrorResponse {
            jsonrpc,
            id: response_id,
            error,
        }) => {
            validate_envelope(&jsonrpc, &id, &response_id)?;
            Err(ClientError::Runtime(error))
        }
    }
}

async fn notify_connection<P: Serialize>(
    connection: &mut Connection,
    method: RuntimeMethod,
    params: &P,
) -> Result<(), ClientError> {
    let notification = JsonRpcNotification {
        jsonrpc: "2.0".to_owned(),
        method: method.to_string(),
        params: serde_json::to_value(params).map_err(|error| {
            ClientError::Protocol(format!("notification cannot be encoded: {error}"))
        })?,
    };
    let encoded = serde_json::to_vec(&notification).map_err(|error| {
        ClientError::Protocol(format!("notification cannot be encoded: {error}"))
    })?;
    connection.send(&encoded).await
}

fn validate_envelope(
    jsonrpc: &str,
    expected: &JsonRpcId,
    actual: &JsonRpcId,
) -> Result<(), ClientError> {
    if jsonrpc != "2.0" {
        return Err(ClientError::Protocol(
            "response JSON-RPC version is not 2.0".to_owned(),
        ));
    }
    if expected != actual {
        return Err(ClientError::Protocol(
            "response request identifier does not match".to_owned(),
        ));
    }
    Ok(())
}

#[derive(Serialize)]
struct EmptyParams {}

/// Typed provider inventory methods.
pub struct ProviderClient<'a> {
    runtime: &'a mut RuntimeClient,
}

impl ProviderClient<'_> {
    /// Read the immediate structural inventory without starting providers.
    ///
    /// # Errors
    ///
    /// Public client and Runtime failures, including `enrollmentPending` before authorization lands.
    pub async fn list(&mut self) -> Result<ProviderList, ClientError> {
        self.runtime
            .call(RuntimeMethod::ProvidersList, &EmptyParams {})
            .await
    }
}

/// Typed Runtime-managed session methods.
pub struct SessionClient<'a> {
    runtime: &'a mut RuntimeClient,
}

impl SessionClient<'_> {
    /// Read the immediate Runtime-managed session snapshot.
    ///
    /// # Errors
    ///
    /// Public client and Runtime failures, including `enrollmentPending` before authorization lands.
    pub async fn list(&mut self) -> Result<ManagedSessionList, ClientError> {
        self.runtime
            .call(RuntimeMethod::SessionsList, &EmptyParams {})
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_options_contain_no_endpoint_or_provider_choice() {
        let options = ClientOptions::new("fixture", "1.0.0");
        assert_eq!(options.name, "fixture");
        assert_eq!(options.version, "1.0.0");
    }

    #[test]
    fn response_identifiers_and_json_rpc_version_are_exact() {
        let expected = JsonRpcId::Number(7);
        assert!(validate_envelope("2.0", &expected, &JsonRpcId::Number(7)).is_ok());
        assert!(validate_envelope("1.0", &expected, &JsonRpcId::Number(7)).is_err());
        assert!(validate_envelope("2.0", &expected, &JsonRpcId::Number(8)).is_err());
    }
}
