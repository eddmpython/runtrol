//! One independent same-image owner retains Windows Job proofs across Runtime exit.

use std::path::{Path, PathBuf};

use runtrol_provider::ProcessIdentity;

use super::job::failure;
use crate::SpawnError;

mod bootstrap;
mod client;
mod handles;
mod protocol;
mod server;
mod wait;
pub use wait::KeeperWait;

pub(crate) use client::{Client, Scope};

#[expect(
    unsafe_code,
    reason = "inherited numeric handles are validated before entering the owned-handle type"
)]
fn job_handle(raw: usize) -> Result<std::os::windows::io::OwnedHandle, SpawnError> {
    let handle = raw as windows_sys::Win32::Foundation::HANDLE;
    let mut flags = 0;
    // SAFETY: this query accepts untrusted numeric handles and reports invalid handles without dereferencing them.
    if unsafe { windows_sys::Win32::Foundation::GetHandleInformation(handle, &raw mut flags) } == 0
    {
        return Err(super::job::last_error("validating a private keeper handle"));
    }
    super::job::owned(handle, "receiving a private keeper handle")
}

/// Scope reservations charged to the caller's existing resource admission policy.
#[derive(Clone, Copy, Debug)]
pub struct KeeperLimits {
    terminals: u16,
    commands: u16,
}

impl KeeperLimits {
    /// Bind terminal and short-command reservations to one finite generation budget.
    ///
    /// # Errors
    /// Returns an error for an empty class or a total that cannot fit the bounded scope index.
    pub fn new(terminals: u16, commands: u16) -> Result<Self, SpawnError> {
        if terminals == 0 || commands == 0 || terminals.checked_add(commands).is_none() {
            return Err(failure(
                "setting keeper admission",
                "invalid scope capacity",
            ));
        }
        Ok(Self {
            terminals,
            commands,
        })
    }

    const fn total(self) -> usize {
        self.terminals as usize + self.commands as usize
    }
}

/// The existing Runtime home object whose registry may receive a completion proof.
///
/// The caller captures the identity with its filesystem authority implementation. The child-process
/// layer carries these structural bytes without inventing another path or ownership registry.
#[derive(Clone, Debug)]
pub struct KeeperTarget {
    directory: PathBuf,
    identity: [u8; 24],
}

impl KeeperTarget {
    /// Retain an absolute home and its already-captured filesystem identity.
    ///
    /// # Errors
    /// Returns an error when the target is relative or exceeds the private bootstrap frame bound.
    pub fn new(directory: PathBuf, identity: [u8; 24]) -> Result<Self, SpawnError> {
        use std::os::windows::ffi::OsStrExt as _;
        if !directory.is_absolute()
            || directory.as_os_str().encode_wide().count() > protocol::MAX_TARGET_UNITS
        {
            return Err(failure(
                "setting keeper completion target",
                "invalid bounded absolute directory",
            ));
        }
        Ok(Self {
            directory,
            identity,
        })
    }

    /// Existing Runtime home location, never a worktree selected by the keeper.
    #[must_use]
    pub fn directory(&self) -> &Path {
        &self.directory
    }

    /// Exact directory object captured by the Runtime's existing filesystem authority.
    #[must_use]
    pub const fn identity(&self) -> [u8; 24] {
        self.identity
    }
}

/// Exact OS identities of the Runtime and its independent proof owner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeeperIdentity {
    runtime: ProcessIdentity,
    keeper: ProcessIdentity,
}

impl KeeperIdentity {
    /// Runtime generation process whose children are retained by the keeper.
    #[must_use]
    pub const fn runtime(self) -> ProcessIdentity {
        self.runtime
    }

    /// Exact keeper process, suitable for a read-only process-event wait during recovery.
    #[must_use]
    pub const fn keeper(self) -> ProcessIdentity {
        self.keeper
    }
}

/// A positive completion proof minted only after exact Runtime and Job member events.
///
/// The daemon callback may apply it by exact existing-row CAS. It grants no authority to remove Git
/// data, replace a newer occupant, or infer completion for a generation without this proof.
#[derive(Debug)]
pub struct KeeperCompletion {
    target: KeeperTarget,
    owner: KeeperIdentity,
}

impl KeeperCompletion {
    /// Exact home object whose existing ownership records can be compared.
    #[must_use]
    pub const fn target(&self) -> &KeeperTarget {
        &self.target
    }

    /// The original Runtime and keeper incarnation proved complete together.
    #[must_use]
    pub const fn owner(&self) -> KeeperIdentity {
        self.owner
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Terminal,
    Command,
}

pub use bootstrap::bootstrap_if_requested;
