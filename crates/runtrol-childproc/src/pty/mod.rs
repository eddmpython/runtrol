//! A child on a pseudo terminal: the coding CLI's own screen, hosted by runtrol.
//!
//! The conversation surface shows the provider's terminal interface as the provider drew it
//! (`docs/terminalSurface.md`). That needs a real terminal on the other side of the child, because these
//! programs check for one and draw nothing into a pipe. This module makes that terminal: `ConPTY` on Windows,
//! `openpty` elsewhere, with one surface over both so nothing above knows which.
//!
//! Owned here rather than taken from a crate, deliberately. The one general crate for this pulls a second
//! set of Windows bindings beside the one this workspace already links, a serial-port library, and a
//! process-group model that disagrees with [`crate::contain`]. A pseudo console is one kernel object and one
//! process attribute; this crate is where audited process FFI lives, and it stays about that size here.
//!
//! What this is not: a screen model, a transport, or a place where bytes are read for meaning. It hands
//! over two ends of a byte stream and a way to resize and stop the child, and nothing else.

use std::io::{Read, Write};

use runtrol_provider::AbsPath;

use crate::error::SpawnError;
use crate::resolve::Program;

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

#[cfg(unix)]
use unix as platform;
#[cfg(windows)]
use windows as platform;

/// Terminal dimensions in character cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PtySize {
    /// Columns.
    pub cols: u16,
    /// Rows.
    pub rows: u16,
}

/// Everything a child on a terminal is started with.
#[derive(Debug, Clone, Copy)]
pub struct PtySpawn<'a> {
    /// The explicit owner and resource class. Standalone callers choose local lifetime deliberately.
    pub containment: PtyContainment<'a>,
    /// The program, already resolved past any launcher.
    pub program: &'a Program,
    /// The caller's arguments. The program's own leading arguments go first.
    pub arguments: &'a [String],
    /// The working directory. A coding CLI reads trust from it, so it is never a temporary folder.
    pub cwd: &'a AbsPath,
    /// Variables set for the child, on top of this process's environment.
    pub env: &'a [(String, String)],
    /// Variables removed from the child's environment before `env` is applied. A name matches exactly, and a
    /// name ending in `*` matches every variable with that prefix. What this exists for: a daemon started from
    /// inside a coding session inherits that session's markers, and the CLI it hosts would read them as
    /// "you are my child" and behave as one (measured 2026-08-25: transcript saving switched itself off).
    pub env_unset: &'a [String],
    /// The initial terminal size.
    pub size: PtySize,
}

/// Explicit lifetime and capacity ownership for terminal and short-command pseudo consoles.
#[derive(Debug, Clone, Copy)]
pub enum PtyContainment<'a> {
    /// A caller-owned local Job, without crash-completion publication to another process.
    Local,
    /// A managed terminal charged to the Runtime's terminal reservation class.
    Terminal(&'a crate::Containment),
    /// A short helper charged to the Runtime's command reservation class.
    Command(&'a crate::Containment),
}

#[cfg(windows)]
impl PtyContainment<'_> {
    fn job(self) -> Result<crate::contain::job::Job, SpawnError> {
        match self {
            Self::Local => crate::contain::job::Job::new(crate::contain::TERMINATED_BY_RUNTROL),
            Self::Terminal(owner) => owner.scope(crate::contain::ScopeKind::Terminal),
            Self::Command(owner) => owner.scope(crate::contain::ScopeKind::Command),
        }
    }

    fn kept(self) -> bool {
        match self {
            Self::Local => false,
            Self::Terminal(owner) | Self::Command(owner) => owner.keeper_identity().is_some(),
        }
    }
}

/// A running child attached to a pseudo terminal.
///
/// Dropping it closes the terminal, which ends the child the way closing a terminal window does.
/// On Windows its private Job also terminates every contained descendant.
#[derive(Debug)]
pub struct PtyChild {
    inner: platform::Child,
}

/// One execution permission for a Windows child created suspended.
///
/// The caller separately owns the child and must stop it if this permission is discarded.
#[cfg(windows)]
#[derive(Debug)]
pub struct PtyResume {
    thread: std::os::windows::io::OwnedHandle,
    job: std::sync::Arc<crate::contain::job::Job>,
}

#[cfg(windows)]
impl PtyResume {
    /// Resume the exact primary thread once, closing its handle after the attempt.
    ///
    /// # Errors
    ///
    /// [`SpawnError::Containment`] when the thread is not suspended exactly once. The child remains
    /// caller-owned and must be stopped on failure.
    pub fn resume(self) -> Result<(), SpawnError> {
        self.job.ready()?;
        crate::contain::resume_suspended_thread(&self.thread, "resuming the prepared terminal")
    }
}

impl PtyChild {
    /// Create a Windows child without executing its first instruction.
    ///
    /// # Errors
    ///
    /// [`SpawnError::ArgvUnsafe`] for unsafe arguments, [`SpawnError::Pty`] for console creation,
    /// or [`SpawnError::Containment`] when the private session Job cannot be established.
    #[cfg(windows)]
    pub fn prepare(spawn: PtySpawn<'_>) -> Result<(Self, PtyResume), SpawnError> {
        crate::argv::check_all(spawn.arguments)?;
        let (inner, thread) = platform::Child::prepare(spawn)?;
        let job = inner.job();
        Ok((Self { inner }, PtyResume { thread, job }))
    }

    /// Admit the already-owned suspended root to its independent keeper, on a blocking spawn lane.
    ///
    /// # Errors
    /// Failure retains the child with the caller. It must be stopped and observed before admission is released.
    #[cfg(windows)]
    pub fn admit(&self) -> Result<(), SpawnError> {
        self.inner.admit()
    }
    /// Start the program on a fresh pseudo terminal.
    ///
    /// # Errors
    ///
    /// [`SpawnError::ArgvUnsafe`] for an argument that must not reach a command line, and
    /// [`SpawnError::Pty`] when the platform refuses terminal or process creation, and
    /// [`SpawnError::Containment`] when the Windows session Job cannot be established.
    pub fn spawn(spawn: PtySpawn<'_>) -> Result<Self, SpawnError> {
        #[cfg(windows)]
        if spawn.containment.kept() {
            return Err(crate::contain::job::failure(
                "starting a kept terminal",
                "use suspended preparation and retain the child through admission",
            ));
        }
        crate::argv::check_all(spawn.arguments)?;
        Ok(Self {
            inner: platform::Child::spawn(spawn)?,
        })
    }

    /// The child's process id.
    #[must_use]
    pub fn pid(&self) -> u32 {
        self.inner.pid()
    }

    /// A reader over what the child writes to its terminal. Blocking; give it a thread.
    ///
    /// # Errors
    ///
    /// [`SpawnError::Pty`] when the platform cannot duplicate the terminal's read end.
    pub fn reader(&self) -> Result<Box<dyn TerminalRead>, SpawnError> {
        self.inner.reader()
    }

    /// A writer into the child's terminal input.
    ///
    /// # Errors
    ///
    /// [`SpawnError::Pty`] when the platform cannot duplicate the terminal's write end.
    pub fn writer(&self) -> Result<Box<dyn Write + Send>, SpawnError> {
        self.inner.writer()
    }

    /// Tell the terminal, and through it the child, that the viewer changed size.
    ///
    /// # Errors
    ///
    /// [`SpawnError::Pty`] when the platform refuses the new size.
    pub fn resize(&self, size: PtySize) -> Result<(), SpawnError> {
        self.inner.resize(size)
    }

    /// The exit code after completion; `None` while it remains owned.
    ///
    /// On Windows completion includes the exact root and every retained session Job member.
    ///
    /// # Errors
    ///
    /// [`SpawnError::Pty`] or [`SpawnError::Containment`] when the platform cannot prove completion.
    pub fn try_wait(&self) -> Result<Option<i32>, SpawnError> {
        self.inner.try_wait()
    }

    /// Wait for the exact Windows root and every sealed Job member to end.
    ///
    /// Cancelling this future leaves process and Job ownership with this child. An error is never
    /// completion evidence; the owner must retain its lifetime until a later successful proof.
    ///
    /// # Errors
    ///
    /// [`SpawnError::Containment`] or [`SpawnError::Pty`] when kernel completion cannot be proven.
    #[cfg(windows)]
    pub async fn wait(&self) -> Result<i32, SpawnError> {
        self.inner.wait().await
    }

    /// Read-only membership in this terminal's exact nested Job.
    #[cfg(windows)]
    #[must_use]
    pub fn process_scope(&self) -> crate::contain::ProcessScope {
        self.inner.process_scope()
    }

    /// Request termination of the child and, on Windows, its entire session Job.
    ///
    /// A successful request is not exit evidence. Retain ownership until completion is confirmed.
    ///
    /// # Errors
    ///
    /// [`SpawnError::Pty`] or [`SpawnError::Containment`] when the platform refuses.
    pub fn kill(&self) -> Result<(), SpawnError> {
        self.inner.kill()
    }

    /// Close the owner's unused output pipe after terminal-host initialization failed.
    ///
    /// No output reader may remain. Closing this end before releasing a failed `ConPTY` prevents its
    /// console host from waiting forever for an output consumer that was never started.
    pub fn abandon_output(&mut self) {
        self.inner.abandon_output();
    }

    /// Release the console after process completion, allowing its output reader to reach EOF.
    ///
    /// The host must keep reading and publishing output until EOF. Console closure can complete
    /// asynchronously on Windows, so this call alone never proves the final frame was drained.
    pub fn finish(&self) {
        self.inner.finish();
    }
}

/// The child's environment: this process's, minus `env_unset`, plus `env`.
///
/// Shared by both platforms so the removal rule is one rule. Returned sorted by name, which the Windows
/// environment block requires and which costs nothing elsewhere.
fn child_environment(spawn: &PtySpawn<'_>) -> Vec<(String, String)> {
    let mut variables: Vec<(String, String)> = std::env::vars()
        .filter(|(name, _)| {
            !spawn
                .env_unset
                .iter()
                .any(|pattern| unset_matches(pattern, name))
        })
        .collect();
    for (name, value) in spawn.env {
        variables.retain(|(existing, _)| !same_name(existing, name));
        variables.push((name.clone(), value.clone()));
    }
    variables.sort_by_key(|(name, _)| name.to_uppercase());
    variables
}

/// Whether one `env_unset` pattern names this variable. Case-insensitive, because Windows environment names
/// are, and a rule that removed `CLAUDE_CODE_*` but kept `Claude_Code_*` would be a hole rather than a rule.
fn unset_matches(pattern: &str, name: &str) -> bool {
    match pattern.strip_suffix('*') {
        Some(prefix) => {
            name.len() >= prefix.len() && name[..prefix.len()].eq_ignore_ascii_case(prefix)
        }
        None => same_name(pattern, name),
    }
}

fn same_name(left: &str, right: &str) -> bool {
    if cfg!(windows) {
        left.eq_ignore_ascii_case(right)
    } else {
        left == right
    }
}

/// The full argument vector: the program's leading arguments, then the caller's.
fn full_arguments(spawn: &PtySpawn<'_>) -> Vec<String> {
    spawn
        .program
        .leading()
        .iter()
        .chain(spawn.arguments.iter())
        .cloned()
        .collect()
}

/// What the host reads a terminal through: a blocking reader that can also say whether more is already waiting.
///
/// A pseudo terminal hands out output in many small pieces under load (measured 2026-09-02: a 840 KB echo burst
/// arrived as 1947 reads of about 300 bytes). A host that publishes every piece as it comes spends its bounded
/// ring on scraps and makes a healthy viewer lag. Asking whether more is waiting lets the host fill one read to
/// its chunk size without ever blocking for bytes that are not there; the bytes and their order are untouched.
pub trait TerminalRead: Read + Send {
    /// How many bytes can be read right now without blocking. Zero when unknown or none.
    fn available(&mut self) -> usize {
        0
    }
}

impl TerminalRead for std::process::ChildStdout {}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real child on a real terminal on this machine: the platform shell echoes one word, the word comes
    /// back through the terminal, and the exit is observed. What the platform pieces are for, end to end.
    #[test]
    fn a_shell_on_the_terminal_echoes_and_exits() {
        let (shell, arguments): (&str, Vec<String>) = if cfg!(windows) {
            ("cmd", vec!["/c".to_owned(), "echo pty-hello".to_owned()])
        } else {
            ("sh", vec!["-c".to_owned(), "echo pty-hello".to_owned()])
        };
        let program = crate::resolve::resolve(shell).expect("the platform shell resolves");
        let cwd = AbsPath::canonicalize(std::env::temp_dir().to_str().expect("utf-8 temp dir"))
            .expect("the temp dir is absolute");
        let child = PtyChild::spawn(PtySpawn {
            containment: crate::PtyContainment::Local,
            program: &program,
            arguments: &arguments,
            cwd: &cwd,
            env: &[("PTY_PROBE".to_owned(), "1".to_owned())],
            env_unset: &["PTY_UNSET_*".to_owned()],
            size: PtySize { cols: 80, rows: 24 },
        })
        .expect("the shell starts on a terminal");
        assert!(child.pid() > 0);
        // The shape the host uses: one thread blocks on the terminal's output, another observes the exit.
        // The reader sees end of stream only once the exit has been observed (that is what closes the
        // console), so the two must be separate or the test would deadlock the way a naive host would.
        let mut reader = child.reader().expect("a reader");
        let reading = std::thread::spawn(move || {
            let mut seen = Vec::new();
            let mut buffer = [0u8; 1024];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => seen.extend_from_slice(buffer.get(..n).unwrap_or(&[])),
                }
            }
            seen
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let mut exit = None;
        while exit.is_none() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(20));
            exit = child.try_wait().expect("the exit can be read");
        }
        assert_eq!(exit, Some(0), "the shell exited cleanly");
        std::thread::sleep(std::time::Duration::from_millis(200));
        child.finish();
        let seen = reading
            .join()
            .expect("the reader thread ends once the console closes");
        let text = String::from_utf8_lossy(&seen);
        assert!(
            text.contains("pty-hello"),
            "the terminal carried the echo: {text:?}"
        );
    }

    #[test]
    fn unset_patterns_match_prefixes_and_exact_names() {
        assert!(unset_matches("CLAUDE_CODE_*", "CLAUDE_CODE_CHILD_SESSION"));
        assert!(unset_matches("CLAUDE_CODE_*", "claude_code_session_id"));
        assert!(!unset_matches("CLAUDE_CODE_*", "CLAUDE_PID"));
        assert!(unset_matches("CLAUDECODE", "CLAUDECODE"));
        assert!(!unset_matches("CLAUDECODE", "CLAUDECODE_X"));
    }
}
