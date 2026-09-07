//! Exact measurement membership and positive Runtime cleanup, shared by the RSS and Python gates.

use std::path::Path;
use std::time::Duration;

use runtrol_ipc::{Connection, Request, Response};
use runtrol_provider::ProcessIdentity;

pub type Error = Box<dyn std::error::Error + Send + Sync>;
pub const STOP_WITHIN: Duration = Duration::from_secs(30);

pub struct Observed {
    connection: Connection,
    pub members: Vec<ProcessIdentity>,
    #[cfg(windows)]
    completion: runtrol_childproc::KeeperWait,
}

impl Observed {
    pub async fn open(home: &Path, pid: u32) -> Result<Self, Error> {
        let identity = runtrol_childproc::process_identity(pid)
            .ok_or("the gate-owned Runtime identity is unavailable")?;
        let locator: runtrol_runtime_protocol::RuntimeLocatorRecord =
            serde_json::from_slice(&std::fs::read(home.join("runtime.locator.json"))?)?;
        let generation = locator
            .generations
            .iter()
            .find(|entry| entry.process_id == pid)
            .ok_or("the locator does not name the gate-owned Runtime")?;
        let mut connection = runtrol_ipc::connect(&generation.control_endpoint).await?;
        let welcome = exchange(
            &mut connection,
            &Request::Hello {
                wire: runtrol_ipc::WIRE_VERSION,
            },
        )
        .await?;
        let Response::Welcome {
            process_completion, ..
        } = welcome
        else {
            return Err("the Runtime did not accept the private greeting".into());
        };
        #[cfg(windows)]
        {
            let proof =
                process_completion.ok_or("the Runtime supplied no keeper completion proof")?;
            let runtime = ProcessIdentity::new(proof.runtime_pid, proof.runtime_started)
                .ok_or("the Runtime completion identity is invalid")?;
            let keeper = ProcessIdentity::new(proof.keeper_pid, proof.keeper_started)
                .ok_or("the keeper completion identity is invalid")?;
            if runtime != identity {
                return Err(
                    "the greeting does not belong to the gate-owned Runtime incarnation".into(),
                );
            }
            let completion = runtrol_childproc::KeeperWait::open(runtime, keeper)?;
            Ok(Self {
                connection,
                members: vec![runtime, keeper],
                completion,
            })
        }
        #[cfg(not(windows))]
        {
            let _not_used = process_completion;
            Ok(Self {
                connection,
                members: vec![identity],
            })
        }
    }

    pub fn resident(&self) -> Result<u64, Error> {
        self.members.iter().try_fold(0_u64, |sum, identity| {
            if !runtrol_childproc::alive(identity.pid())
                || runtrol_childproc::process_identity(identity.pid()) != Some(*identity)
            {
                return Err("a measured process incarnation changed or ended".into());
            }
            let held = runtrol_childproc::resident_bytes(identity.pid())?;
            if !runtrol_childproc::alive(identity.pid())
                || runtrol_childproc::process_identity(identity.pid()) != Some(*identity)
            {
                return Err("a measured process changed during its RSS sample".into());
            }
            sum.checked_add(held)
                .ok_or_else(|| "the resident sum overflowed".into())
        })
    }

    pub async fn stop(mut self) -> Result<(), Error> {
        tokio::time::timeout(STOP_WITHIN, async {
            let answer = exchange(&mut self.connection, &Request::StopEverything).await;
            if let Ok(Response::Failed(failed)) = answer {
                return Err(Error::from(format!(
                    "Runtime stop refused: {}",
                    failed.message
                )));
            }
            // Shutdown may close the request connection without a reply. Only the retained OS witnesses below
            // can turn that transport failure into successful cleanup; a missing or failed witness remains red.
            #[cfg(windows)]
            self.completion.wait().await?;
            // Unix callers reap their exact Child before removing its home. A zombie cannot disappear until then.
            Ok(())
        })
        .await
        .map_err(|_| "Runtime completion timed out; its home must be retained")?
    }
}

async fn exchange(connection: &mut Connection, request: &Request) -> Result<Response, Error> {
    connection.send(&serde_json::to_vec(request)?).await?;
    let bytes = connection
        .recv()
        .await?
        .ok_or("the Runtime closed the control connection")?;
    Ok(serde_json::from_slice(&bytes)?)
}
