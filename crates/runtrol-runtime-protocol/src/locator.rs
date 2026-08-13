//! Platform-neutral Runtime locator record.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Current atomic Runtime locator record schema.
pub const RUNTIME_LOCATOR_SCHEMA: u32 = 1;

/// Local transport kind named by the platform locator.
#[derive(Clone, Copy, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RuntimeEndpointKind {
    /// Windows current-user named pipe.
    NamedPipe,
    /// Unix owner-only domain socket.
    UnixSocket,
}

/// Operational bootstrap data published only after the public endpoint is ready.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeLocatorRecord {
    /// Locator schema version.
    pub schema: u32,
    /// Durable installed Runtime identity.
    pub instance_id: String,
    /// Platform-local transport kind.
    pub endpoint_kind: RuntimeEndpointKind,
    /// Exact local endpoint selected by Runtime.
    pub endpoint: String,
    /// Runtime product version.
    pub runtime_version: String,
    /// Runtime process identifier used only for stale-state diagnostics.
    pub process_id: u32,
}
