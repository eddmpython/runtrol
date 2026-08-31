//! Platform-neutral Runtime locator record.
//!
//! One file names every daemon generation serving one Runtime home. A generation is one running build of the
//! daemon, identified by the SHA-256 of its executable, listening on endpoints that carry that identity. A newer
//! build starts beside the older one and the older one drains; both are listed while both live, so a client
//! reading this file can choose the newest for new work and still reach the generation that holds a conversation
//! it was already watching.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Current atomic Runtime locator record schema.
pub const RUNTIME_LOCATOR_SCHEMA: u32 = 2;

/// Maximum simultaneously published daemon generations for one Runtime home.
///
/// A normal upgrade needs two entries. The larger fixed ceiling leaves room for long-lived terminals across
/// repeated upgrades while bounding every client fleet and generation-handoff receipt set.
pub const MAX_RUNTIME_GENERATIONS: usize = 16;

/// Local transport kind named by the platform locator.
#[derive(Clone, Copy, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RuntimeEndpointKind {
    /// Windows current-user named pipe.
    NamedPipe,
    /// Unix owner-only domain socket.
    UnixSocket,
}

/// Operational bootstrap data published only after each generation's public endpoint is ready.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeLocatorRecord {
    /// Locator schema version.
    pub schema: u32,
    /// Durable installed Runtime identity, shared by every generation of one home.
    pub instance_id: String,
    /// Every daemon generation currently serving this home, oldest start first.
    pub generations: Vec<RuntimeGeneration>,
}

/// One running daemon build and where it listens.
#[derive(Clone, Debug, PartialEq, Eq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeGeneration {
    /// SHA-256 of the daemon executable, lowercase hex. The generation's identity.
    pub digest: String,
    /// Platform-local transport kind of both endpoints.
    pub endpoint_kind: RuntimeEndpointKind,
    /// Exact local public Runtime endpoint of this generation.
    pub endpoint: String,
    /// Exact local private control endpoint of this generation, for the executable's own command surface.
    pub control_endpoint: String,
    /// Runtime product version of this generation.
    pub runtime_version: String,
    /// Generation process identifier, used only for stale-state diagnostics.
    pub process_id: u32,
    /// When this generation published itself, Unix milliseconds. The newest is the one new work goes to.
    pub started_at_ms: u64,
    /// Conversations this generation still has mid-turn. A draining generation exits when this reaches zero.
    pub live_sessions: u32,
    /// Whether a newer generation has taken over new work from this one.
    pub draining: bool,
}

impl RuntimeLocatorRecord {
    /// The generation new work goes to: the newest that is not draining, or none when every one is draining.
    #[must_use]
    pub fn current(&self) -> Option<&RuntimeGeneration> {
        self.generations
            .iter()
            .filter(|generation| !generation.draining)
            .max_by_key(|generation| (generation.started_at_ms, generation.process_id))
    }

    /// The generation running one exact executable digest, if it is listed.
    #[must_use]
    pub fn with_digest(&self, digest: &str) -> Option<&RuntimeGeneration> {
        self.generations
            .iter()
            .find(|generation| generation.digest == digest)
    }
}
