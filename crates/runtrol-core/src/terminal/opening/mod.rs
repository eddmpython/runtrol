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

/// A fully hosted Windows child that cannot execute until final admission succeeds.
#[cfg(windows)]
#[derive(Debug)]
pub struct PreparedTerminal {
    terminal: Terminal,
    resume: Option<runtrol_childproc::PtyResume>,
}

#[cfg(windows)]
impl PreparedTerminal {
    /// The exact child owner, retained by the caller before attempting execution.
    #[must_use]
    pub fn terminal(&self) -> Terminal {
        self.terminal.clone()
    }

    /// Execute the prepared child once. The caller retains its terminal until observed exit.
    ///
    /// # Errors
    ///
    /// [`SpawnError::Containment`] if the kernel refuses execution. The caller must stop its retained
    /// terminal and keep admission until the existing exit observer confirms termination.
    pub fn resume(mut self) -> Result<(), SpawnError> {
        let resume = self.resume.take().ok_or_else(|| SpawnError::Pty {
            doing: "resuming the prepared terminal",
            detail: "the execution permission was already consumed".to_owned(),
        })?;
        resume.resume()
    }
}

#[cfg(windows)]
impl Drop for PreparedTerminal {
    fn drop(&mut self) {
        if self.resume.is_some() {
            // A dropped preparation never executes. The terminal watcher retains the exact child;
            // its admission owner separately awaits that watcher before releasing capacity.
            drop(self.terminal.kill());
        }
    }
}

#[cfg(windows)]
pub(super) fn prepare(
    launch: &TerminalLaunch<'_>,
    host: impl FnOnce(Child, PtySize) -> Result<Terminal, FailedHost>,
) -> Result<PreparedTerminal, TerminalError> {
    let runtime = tokio::runtime::Handle::try_current()
        .map_err(|error| TerminalError::Runtime(error.to_string()))?;
    let size = bounded_size(launch.size);
    let (child, resume) = PtyChild::prepare(PtySpawn {
        containment: launch.containment.map_or(
            runtrol_childproc::PtyContainment::Local,
            runtrol_childproc::PtyContainment::Terminal,
        ),
        program: launch.program,
        arguments: &launch.arguments,
        cwd: launch.cwd,
        env: &launch.env,
        env_unset: &launch.env_unset,
        size,
    })?;
    let terminal = match child.admit() {
        Ok(()) => host_or_cleanup(child, size, &runtime, host, stop_once),
        Err(cause) => cleanup_failed(
            FailedHost {
                child: Child::Pty(child),
                cause: Box::new(cause.into()),
            },
            size,
            &runtime,
            stop_once,
        ),
    }?;
    Ok(PreparedTerminal {
        terminal,
        resume: Some(resume),
    })
}

fn stop_once(child: &PtyChild) -> Result<i32, SpawnError> {
    child.kill()?;
    child.try_wait()?.ok_or_else(|| SpawnError::Pty {
        doing: "confirming terminal process termination",
        detail: "the owned root has not yet been confirmed stopped".to_owned(),
    })
}

pub(super) fn open(
    launch: &TerminalLaunch<'_>,
    host: impl FnOnce(Child, PtySize) -> Result<Terminal, FailedHost>,
) -> Result<Terminal, TerminalError> {
    #[cfg(windows)]
    if launch
        .containment
        .is_some_and(|owner| owner.keeper_identity().is_some())
    {
        let prepared = prepare(launch, host)?;
        let terminal = prepared.terminal();
        if let Err(cause) = prepared.resume() {
            let cleanup = match terminal.kill() {
                Err(TerminalError::Spawn(error)) => error,
                result => SpawnError::Containment {
                    doing: "confirming prepared terminal termination",
                    detail: match result {
                        Ok(()) => "the started process has not been confirmed stopped".to_owned(),
                        Err(error) => error.to_string(),
                    },
                },
            };
            return Err(TerminalError::CleanupIncomplete {
                cause: Box::new(cause.into()),
                cleanup,
                terminal,
            });
        }
        return Ok(terminal);
    }
    // A caller may still hold its admission/table locks here. Never wait for a live child under them;
    // the ownership-bearing error lets that caller publish the child and observe exit after unlocking.
    open_with_cleanup(launch, host, stop_once)
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
        containment: launch.containment.map_or(
            runtrol_childproc::PtyContainment::Local,
            runtrol_childproc::PtyContainment::Terminal,
        ),
        program: launch.program,
        arguments: &launch.arguments,
        cwd: launch.cwd,
        env: &launch.env,
        env_unset: &launch.env_unset,
        size,
    })?;
    host_or_cleanup(child, size, &runtime, host, cleanup)
}

fn host_or_cleanup(
    child: PtyChild,
    size: PtySize,
    runtime: &tokio::runtime::Handle,
    host: impl FnOnce(Child, PtySize) -> Result<Terminal, FailedHost>,
    cleanup: impl FnOnce(&PtyChild) -> Result<i32, SpawnError>,
) -> Result<Terminal, TerminalError> {
    let failure = match host(Child::Pty(child), size) {
        Ok(terminal) => return Ok(terminal),
        Err(failure) => failure,
    };
    cleanup_failed(failure, size, runtime, cleanup)
}

fn cleanup_failed(
    failure: FailedHost,
    size: PtySize,
    runtime: &tokio::runtime::Handle,
    cleanup: impl FnOnce(&PtyChild) -> Result<i32, SpawnError>,
) -> Result<Terminal, TerminalError> {
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
