//! Why a child could not be started, or could not be contained.

use runtrol_provider::AbsPath;

/// Starting or supervising a child process failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum SpawnError {
    /// No executable by that name exists on this machine.
    ///
    /// Names every place that was searched, because the operator's next move is either to install the
    /// program or to correct a path, and "not found" alone tells them which of those to try.
    #[error("{program:?} not found. searched: {searched}")]
    NotFound {
        /// What was asked for.
        program: String,
        /// Where runtrol looked, in the order it looked.
        searched: String,
    },

    /// The path given is not something that can be executed.
    #[error("{path} is not an executable file")]
    NotExecutable {
        /// The path as resolved.
        path: AbsPath,
    },

    /// An argument contains a character that must not reach a command line.
    ///
    /// Refused before the spawn rather than after. The operating system's own refusal, measured on this
    /// toolchain, is `batch file arguments are invalid`, which does not say which argument or which
    /// character, and an operator reading that has nowhere to start.
    #[error(
        "argument {index} ({argument:?}) contains {what} at byte {at}, which cannot be passed on a command line"
    )]
    ArgvUnsafe {
        /// Which argument, counting from zero.
        index: usize,
        /// The argument, as offered.
        argument: String,
        /// What was wrong with it.
        what: &'static str,
        /// Where in the argument.
        at: usize,
    },

    /// Resolving a launcher script to what it actually runs went too deep.
    ///
    /// A bound rather than a loop detector: a launcher that points at a launcher that points at a launcher
    /// is either a cycle or a configuration nobody intended, and following it forever is not an option.
    #[error("resolving {program:?} went through {depth} launchers without reaching an executable")]
    LauncherTooDeep {
        /// What was asked for.
        program: String,
        /// How many layers were unwrapped before giving up.
        depth: usize,
    },

    /// Containment could not be established, or could not be enforced.
    ///
    /// Not recoverable and not worth working around. Starting an agent that cannot be contained is the
    /// outcome the containment module exists to prevent, so the daemon refuses to start rather than running
    /// with the guarantee quietly absent.
    #[error("cannot contain child processes while {doing}: {detail}")]
    Containment {
        /// What runtrol was doing.
        doing: &'static str,
        /// What the platform said.
        detail: String,
    },

    /// A program runtrol asked a question did not answer in time.
    ///
    /// Only for a one-shot question, never for a session. A turn has no timeout by design, because deciding
    /// a turn ended because nothing arrived would be reporting an outcome runtrol does not know. Asking a CLI
    /// its version is the opposite case: there is a right answer, it arrives in milliseconds when it arrives
    /// at all, and waiting forever would hang whatever asked.
    #[error("{path} did not answer within {after_ms} ms")]
    Timeout {
        /// The program that was asked.
        path: AbsPath,
        /// How long it was given.
        after_ms: u64,
    },

    /// A process could not be asked how much memory it is holding.
    ///
    /// Reported rather than answered with a zero. A budget gate that measured a process which had already ended
    /// would pass forever while the thing it watches grows without limit.
    #[error("cannot measure what process {pid} is holding: {detail}")]
    Footprint {
        /// Which process was asked about.
        pid: u32,
        /// What the platform said.
        detail: String,
    },

    /// This process's own handles could not be kept from travelling to what it starts.
    ///
    /// Reported rather than swallowed. What it prevents is a shell that waits forever with nothing to show
    /// for it, which is the hardest kind of defect for an operator to attribute to anything.
    #[error("cannot keep runtrol's own handles from being passed on: {detail}")]
    Handoff {
        /// What the platform said.
        detail: String,
    },

    /// The filesystem refused a path.
    #[error("cannot read {path:?}: {detail}")]
    Io {
        /// What runtrol was looking at.
        path: String,
        /// What the OS said.
        detail: String,
    },
}
