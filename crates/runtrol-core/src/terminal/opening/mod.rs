//! Failed initialization retains process admission until the owned root has ended.

use super::{
    Arc, Child, PtyChild, PtySize, PtySpawn, Shared, SpawnError, Terminal, TerminalError,
    TerminalLaunch, WRITE_QUEUE, blocking_mpsc, bounded_size,
};

#[derive(Debug)]
pub(super) struct FailedHost {
    pub(super) child: Child,
    pub(super) cause: Box<TerminalError>,
}

pub(super) fn open(
    launch: &TerminalLaunch<'_>,
    host: impl FnOnce(Child, PtySize) -> Result<Terminal, FailedHost>,
) -> Result<Terminal, TerminalError> {
    // A caller may still hold its admission/table locks here. Never wait for a live child under them;
    // the ownership-bearing error lets that caller publish the child and observe exit after unlocking.
    open_with_cleanup(launch, host, |child| {
        child.kill()?;
        child.try_wait()?.ok_or_else(|| SpawnError::Pty {
            doing: "confirming terminal process termination",
            detail: "the owned root has not yet been confirmed stopped".to_owned(),
        })
    })
}

fn open_with_cleanup(
    launch: &TerminalLaunch<'_>,
    host: impl FnOnce(Child, PtySize) -> Result<Terminal, FailedHost>,
    cleanup: impl FnOnce(&PtyChild) -> Result<i32, SpawnError>,
) -> Result<Terminal, TerminalError> {
    // The failure terminal needs this same executor to retain exact exit observation. Check before
    // birth so the absence of a runtime can never produce an unobserved child.
    let runtime = tokio::runtime::Handle::try_current()
        .map_err(|error| TerminalError::Runtime(error.to_string()))?;
    let size = bounded_size(launch.size);
    let child = PtyChild::spawn(PtySpawn {
        program: launch.program,
        arguments: &launch.arguments,
        cwd: launch.cwd,
        env: &launch.env,
        env_unset: &launch.env_unset,
        size,
    })?;
    let failure = match host(Child::Pty(child), size) {
        Ok(terminal) => return Ok(terminal),
        Err(failure) => failure,
    };
    let FailedHost { mut child, cause } = failure;
    let result = match &mut child {
        Child::Pty(child) => {
            // All fallible host steps precede its reader and async tasks. No reader survives here.
            child.abandon_output();
            cleanup(child)
        }
        Child::Fed(child) => {
            child.kill();
            Ok(0)
        }
    };
    match result {
        Ok(_) => {
            // Drop closes the failed ConPTY only after its owned root exit was confirmed. This does
            // not claim that every descendant process has ended.
            drop(child);
            Err(*cause)
        }
        Err(cleanup) => {
            let (writer, incoming) = blocking_mpsc::sync_channel(WRITE_QUEUE);
            drop(incoming);
            let shared = Arc::new(Shared::new(child, size, writer, true));
            let watcher = Arc::clone(&shared);
            runtime.spawn(async move { watcher.watch_exit().await });
            Err(TerminalError::CleanupIncomplete {
                cause,
                cleanup,
                terminal: Terminal { shared },
            })
        }
    }
}

#[cfg(test)]
#[path = "tests/native.rs"]
mod tests;
