//! The four environment values a managed process is born with.
//!
//! The Runtime sets them on the process it starts for a managed session and on nothing else: no global `PATH`
//! entry, no provider setting, no project file, no service. They are inherited down that process tree, which is
//! how a coding agent's shell finds the courier without being told, and how nothing outside that tree does.
//! The names are the contract the `session-fabric` design fixed, so both the daemon that sets them and the
//! command that reads them spell them from here.

/// The absolute path of the Runtime executable, which is also the courier command.
pub const COURIER_EXE_ENV: &str = "RUNTROL_COURIER_EXE";

/// Where the courier of the Runtime generation that started this process listens.
pub const COURIER_ENDPOINT_ENV: &str = "RUNTROL_COURIER_ENDPOINT";

/// The process-scoped secret that proves a connection speaks for its managed session.
pub const COURIER_TOKEN_ENV: &str = "RUNTROL_COURIER_TOKEN";

/// The managed session this process tree is, as canonical UUIDv7 text.
pub const MANAGED_SESSION_ENV: &str = "RUNTROL_MANAGED_SESSION";

/// The token's size before its base64url spelling: 256 bits of operating-system randomness.
pub const TOKEN_BYTES: usize = 32;
