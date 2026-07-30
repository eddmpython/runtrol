//! Why a child could not be started, or could not be contained.

use runtrol_provider::AbsPath;

/// Starting or supervising a child process failed.
#[derive(Debug, thiserror::Error)]
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

    /// The filesystem refused a path.
    #[error("cannot read {path:?}: {detail}")]
    Io {
        /// What runtrol was looking at.
        path: String,
        /// What the OS said.
        detail: String,
    },
}
