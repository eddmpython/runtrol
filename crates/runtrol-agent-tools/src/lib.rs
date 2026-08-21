//! Permission-bounded tools through which coding agents can use the public Runtrol Runtime.
//!
//! The crate is a product surface, not a second orchestrator. It transports caller-owned input and public
//! Runtime events unchanged, keeps no transcript, has no model API key, and cannot answer approvals. Each
//! server process selects exactly one locally approved project root from its starting directory.

mod command;
mod credentials;
mod mcp;
mod runtime;

pub use command::{CommandContext, run_command};
pub use mcp::serve;

/// Agent Tools could not establish or use its bounded local authority.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum AgentToolsError {
    /// Runtrol home layout failed.
    #[error("cannot open the Runtrol home for Agent Tools: {0}")]
    Home(#[from] runtrol_core::HomeError),
    /// A project or credential path was invalid or could not be resolved.
    #[error("Agent Tools path is not usable: {0}")]
    Path(#[from] runtrol_provider::PathError),
    /// Protected identity storage failed.
    #[error("Agent Tools cannot use its protected identity: {0}")]
    Vault(#[from] runtrol_vault::VaultError),
    /// Public Runtime communication failed.
    #[error("Agent Tools Runtime request failed: {0}")]
    Runtime(#[from] runtrol_runtime_client::ClientError),
    /// A public Runtime DTO could not be represented as structured MCP content.
    #[error("Agent Tools could not encode a Runtime result: {0}")]
    Json(#[from] serde_json::Error),
    /// Private local administration failed.
    #[error("Agent Tools local administration failed: {0}")]
    Local(#[from] runtrol_cli::Failed),
    /// A bounded credential file operation failed.
    #[error("Agent Tools credential I/O failed while {doing} at {path}: {detail}")]
    Io {
        /// Exact operation.
        doing: &'static str,
        /// Exact Runtrol-owned path.
        path: String,
        /// Operating-system detail.
        detail: String,
    },
    /// A credential file was malformed, mismatched, or outside its contract.
    #[error("Agent Tools credential at {path} is invalid: {why}")]
    Credential {
        /// Exact Runtrol-owned path.
        path: String,
        /// Safe structural reason.
        why: String,
    },
    /// The requested project has not been enabled for this server process.
    #[error("{0}")]
    Authority(String),
    /// The local daemon refused a bounded administration request.
    #[error("{0}")]
    Refused(String),
    /// MCP input or output could not be processed.
    #[error("Agent Tools MCP failed: {0}")]
    Mcp(String),
}

impl AgentToolsError {
    fn io(doing: &'static str, path: &std::path::Path, error: &std::io::Error) -> Self {
        Self::Io {
            doing,
            path: path.display().to_string(),
            detail: error.to_string(),
        }
    }
}
