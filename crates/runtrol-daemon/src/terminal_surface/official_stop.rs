//! A provider-discovered stop command uses the existing bounded command lifetime, not a hot terminal.

use std::sync::Arc;

use runtrol_childproc::{Containment, PtyChild, PtyContainment, PtySize, PtySpawn};
use runtrol_provider::AbsPath;

#[derive(Clone, Debug)]
pub(crate) struct OfficialStop {
    pub(super) containment: Arc<Containment>,
    pub(super) program: runtrol_childproc::Program,
    pub(super) arguments: Vec<String>,
    pub(super) cwd: AbsPath,
    pub(super) env: Vec<(String, String)>,
    pub(super) env_unset: Vec<String>,
}

pub(super) async fn run(stop: &OfficialStop) -> Result<(), String> {
    let stop = stop.clone();
    let runtime = tokio::runtime::Handle::current();
    // This finite owner survives cancellation. Creation, keeper ACK, output drain and completion never
    // hold the shared input reactor or terminal table, and never spend the hot terminal capacity.
    tokio::task::spawn_blocking(move || execute(&stop, &runtime))
        .await
        .map_err(|error| format!("the official stop worker failed: {error}"))?
}

fn execute(stop: &OfficialStop, runtime: &tokio::runtime::Handle) -> Result<(), String> {
    let spawn = PtySpawn {
        containment: PtyContainment::Command(&stop.containment),
        program: &stop.program,
        arguments: &stop.arguments,
        cwd: &stop.cwd,
        env: &stop.env,
        env_unset: &stop.env_unset,
        size: PtySize { cols: 80, rows: 24 },
    };
    #[cfg(windows)]
    let (mut child, resume) = PtyChild::prepare(spawn).map_err(|error| error.to_string())?;
    #[cfg(not(windows))]
    let mut child = PtyChild::spawn(spawn).map_err(|error| error.to_string())?;
    let reader = match child.reader() {
        Ok(reader) => reader,
        Err(error) => {
            child.abandon_output();
            return failed(&child, runtime, error.to_string());
        }
    };
    let draining = match std::thread::Builder::new()
        .name("official-stop-output".to_owned())
        .spawn(move || {
            use std::io::Read as _;
            let mut reader = reader;
            let mut buffer = [0_u8; 4096];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => return Ok(()),
                    Ok(_) => {}
                    Err(error) => return Err(format!("reading official stop output: {error}")),
                }
            }
        }) {
        Ok(thread) => thread,
        Err(error) => {
            child.abandon_output();
            return failed(&child, runtime, error.to_string());
        }
    };
    #[cfg(windows)]
    let admitted = child
        .admit()
        .and_then(|()| resume.resume())
        .map_err(|error| error.to_string());
    #[cfg(not(windows))]
    let admitted = Ok(());
    let outcome = admitted.and_then(|()| {
        runtime.block_on(async {
            tokio::time::timeout(std::time::Duration::from_secs(10), completed(&child))
                .await
                .map_err(|_| {
                    "the provider's official stop command exceeded 10 seconds".to_owned()
                })?
        })
    });
    let outcome = match outcome {
        Ok(code) => Ok(code),
        Err(error) => {
            let stop_result = child.kill().map_err(|stop| stop.to_string());
            match stop_result.and_then(|()| runtime.block_on(completed(&child))) {
                Ok(_) => Err(error),
                Err(cleanup) => Err(format!(
                    "{error}; process completion remains unproven: {cleanup}"
                )),
            }
        }
    };
    child.finish();
    draining
        .join()
        .map_err(|_| "the official stop output reader failed".to_owned())??;
    match outcome? {
        0 => Ok(()),
        code => Err(format!(
            "the provider's official stop command exited with code {code}"
        )),
    }
}

fn failed(child: &PtyChild, runtime: &tokio::runtime::Handle, cause: String) -> Result<(), String> {
    child
        .kill()
        .map_err(|error| format!("{cause}; stop failed: {error}"))?;
    runtime
        .block_on(completed(child))
        .map_err(|error| format!("{cause}; completion is unproven: {error}"))?;
    child.finish();
    Err(cause)
}

async fn completed(child: &PtyChild) -> Result<i32, String> {
    #[cfg(windows)]
    {
        child.wait().await.map_err(|error| error.to_string())
    }
    #[cfg(not(windows))]
    loop {
        if let Some(code) = child.try_wait().map_err(|error| error.to_string())? {
            return Ok(code);
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}
