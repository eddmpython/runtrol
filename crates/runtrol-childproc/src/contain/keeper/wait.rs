//! The external launcher retains both exact process objects before requesting shutdown.

use std::os::windows::io::AsHandle as _;

use runtrol_provider::ProcessIdentity;

use super::handles::Process;
use crate::SpawnError;
use crate::contain::job::{failure, wait};

/// Positive normal shutdown observation. Process absence alone cannot distinguish keeper failure.
#[derive(Debug)]
pub struct KeeperWait {
    runtime: Process,
    keeper: Process,
}

impl KeeperWait {
    /// Retain exact Runtime and keeper handles before sending a stop request.
    ///
    /// # Errors
    /// Refuses missing, changed or uninspectable process incarnations.
    pub fn open(runtime: ProcessIdentity, keeper: ProcessIdentity) -> Result<Self, SpawnError> {
        if runtime == keeper {
            return Err(failure(
                "retaining Runtime completion",
                "the keeper is not independent",
            ));
        }
        Ok(Self {
            runtime: Process::open(runtime)?,
            keeper: Process::open(keeper)?,
        })
    }

    /// Await Runtime exit, then keeper exit with successful same-Job completion publication.
    ///
    /// # Errors
    /// Reports event failure or nonzero keeper exit without claiming that cleanup completed.
    pub async fn wait(self) -> Result<(), SpawnError> {
        wait::signaled(self.runtime.handle.as_handle()).await?;
        wait::signaled(self.keeper.handle.as_handle()).await?;
        let code = self.keeper.exit_code()?;
        if code != 0 {
            return Err(failure(
                "confirming Runtime process completion",
                format!("the keeper exited with code {code}"),
            ));
        }
        Ok(())
    }
}
