//! Why authority was refused.
//!
//! Every variant is a refusal. There is no variant meaning "allowed", because this crate answers
//! only one kind of question and the affirmative answer is a value, not an error.

use runtrol_provider::{AbsPath, PathError};

use crate::id::DeviceId;

/// A request for authority was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum SecurityError {
    /// The device does not hold the scope this action needs.
    ///
    /// The default answer for any device the ledger has never seen, which is what makes the ledger
    /// default-deny rather than default-deny-by-convention.
    #[error("device {device} does not hold {scope}")]
    ScopeMissing {
        /// Which device asked.
        device: DeviceId,
        /// The scope it would have needed, by name.
        scope: &'static str,
    },

    /// Nobody answered the presence challenge in time.
    #[error("nobody answered at the machine within {waited_ms} ms")]
    PresenceTimeout {
        /// How long the challenge stood open.
        waited_ms: u64,
    },

    /// The wrong word was typed.
    ///
    /// Carries no hint about the expected word and no attempt count, because a challenge is single
    /// use: a wrong answer ends it and the next attempt is a fresh word.
    #[error("the word typed at the machine did not match")]
    PresenceDenied,

    /// The operating system would not supply randomness for a challenge phrase.
    ///
    /// No challenge opens. A phrase a caller could predict proves nothing, so the honest outcome is an
    /// error the operator sees rather than a permission granted on a formality.
    #[error("cannot generate a challenge phrase: {detail}")]
    ChallengeUnavailable {
        /// What the randomness source reported.
        detail: String,
    },

    /// This process has no local surface to challenge anyone through.
    ///
    /// Reached when something asks for a presence challenge on a plane that cannot display one, and
    /// the honest answer is that the operator has to act at their machine.
    #[error("no local surface is available to ask the operator through")]
    ConsoleUnavailable,

    /// The witness was issued for a different request.
    ///
    /// A witness is not a blank cheque. Typing the word to add a workspace root must not also
    /// authorize turning off a permission prompt, so each witness names what it answered and is
    /// checked against what it is being spent on.
    #[error("the operator approved {approved}, not {attempted}")]
    WitnessMismatch {
        /// What the operator was shown and agreed to.
        approved: String,
        /// What the caller tried to spend the witness on.
        attempted: String,
    },

    /// The witness is too old to spend.
    ///
    /// Presence means the operator is at the machine now. A witness that could be stashed and spent
    /// later would only prove that somebody was there once.
    #[error("the operator's approval is {age_ms} ms old, past the {limit_ms} ms limit")]
    WitnessExpired {
        /// Age of the witness.
        age_ms: u64,
        /// How old a witness may be when spent.
        limit_ms: u64,
    },

    /// The proposed workspace root overlaps a directory that no configuration may open.
    ///
    /// Overlap in either direction: the root is inside a denied directory, or a denied directory is
    /// inside the root. The second case is the one that matters in practice, because granting a home
    /// directory as a workspace grants every credential store inside it.
    #[error("{candidate} overlaps {denied}, which no configuration may open ({why})")]
    WorkspaceDenied {
        /// The proposed root, canonical.
        candidate: AbsPath,
        /// The denied path it overlaps, canonical where it exists.
        denied: AbsPath,
        /// Why that path is denied.
        ///
        /// Owned, because some denied paths come from a provider's manifest and a manifest is read at runtime.
        /// The operator sees this sentence verbatim, so it travels with the failure rather than being looked up
        /// again by whoever renders it.
        why: Box<str>,
    },

    /// A path was offered as a workspace but is not a directory that exists.
    #[error("{candidate} is not an existing directory")]
    WorkspaceNotADirectory {
        /// The path as offered.
        candidate: String,
    },

    /// The filesystem could not resolve the proposed root.
    ///
    /// A workspace root must be resolved through the OS before it can be compared against the deny
    /// list, because a symbolic link makes text comparison answer the wrong question.
    #[error("cannot resolve the proposed workspace root: {source}")]
    WorkspaceUnresolvable {
        /// What went wrong.
        #[source]
        source: PathError,
    },

    /// A grant named a workspace root the operator has not configured.
    ///
    /// Refused rather than ignored. A root can be removed while a grant naming it still exists, and
    /// treating the unknown root as "no restriction" would turn removal into a widening.
    #[error("workspace root {root} is not configured")]
    RootUnknown {
        /// The root the grant named.
        root: crate::id::WorkspaceRootId,
    },

    /// An untrusted device name or platform cannot be shown safely in a presence prompt.
    #[error("pairing {field} is invalid: {why}")]
    InvalidPairingIdentity {
        /// Which display field was refused.
        field: &'static str,
        /// The validation rule, without echoing untrusted text into diagnostics.
        why: &'static str,
    },
}
