//! The error taxonomy a driver returns.
//!
//! This is the whole of it. A driver, including one written outside this repository, fails with a
//! [`ProviderError`] and nothing else, because the supervisor takes real decisions on the variant:
//! whether to retry, whether to freeze the session or end it, and whether the operator has to do
//! something at their machine.
//!
//! Two rules shape the list.
//!
//! Every variant names what runtrol was doing, not just what went wrong. There is deliberately no
//! `From<std::io::Error>`, because `?` would then turn every failure into an undifferentiated "IO
//! error" and the operator would be told that something failed without being told what.
//!
//! Provider text is carried, never interpreted. A message from a CLI is untrusted input: it is
//! shown to a person and included in a report, and it is never parsed to decide anything and never
//! interpolated into a command line or a path.

use core::fmt;

use crate::id::ProviderId;

/// Everything a driver may fail with.
///
/// `#[non_exhaustive]`: a driver outside this repository matches on this type, and adding a variant
/// must not break that driver's build.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ProviderError {
    /// The provider's executable is not installed, or not where the manifest says.
    ///
    /// The operator's next move is to install the CLI or correct a path, so the message names every
    /// place runtrol looked rather than only reporting absence.
    #[error("{provider}: executable not found. looked in: {searched}")]
    BinNotFound {
        /// Which provider.
        provider: ProviderId,
        /// Where runtrol looked, in the order it looked.
        searched: String,
    },

    /// More than one candidate executable resolved, and runtrol will not choose.
    ///
    /// Choosing would mean picking silently between two versions of a CLI that own real
    /// conversations. The operator's `PATH` decides, or the manifest pins an absolute path.
    #[error("{provider}: several executables match, refusing to guess between: {candidates}")]
    BinAmbiguous {
        /// Which provider.
        provider: ProviderId,
        /// The candidates found.
        candidates: String,
    },

    /// The child process could not be started.
    #[error("{provider}: cannot start {program}: {source}")]
    Spawn {
        /// Which provider.
        provider: ProviderId,
        /// The program runtrol tried to run.
        program: String,
        /// What the OS said.
        #[source]
        source: std::io::Error,
    },

    /// The provider broke its own protocol.
    ///
    /// Unparseable framing, a response that does not answer any outstanding request, an identifier
    /// runtrol cannot accept, or a line past the transport's bound. This is the variant that fires
    /// when a CLI changes shape underneath runtrol, so `detail` has to be specific enough to be
    /// filed as a bug against the vendor.
    #[error("{provider}: protocol violation while {doing}: {detail}")]
    Protocol {
        /// Which provider.
        provider: ProviderId,
        /// What runtrol was doing.
        doing: &'static str,
        /// What was wrong. Provider text, carried verbatim and never parsed.
        detail: String,
    },

    /// The provider did not answer inside a budget runtrol was given.
    ///
    /// Only ever a budget somebody set. runtrol does not invent deadlines for a coding agent, so
    /// this variant cannot appear as a guess about a turn that is merely slow.
    #[error("{provider}: no answer while {doing} after {waited_ms} ms")]
    Timeout {
        /// Which provider.
        provider: ProviderId,
        /// What runtrol was waiting for.
        doing: &'static str,
        /// How long runtrol waited.
        waited_ms: u64,
    },

    /// The provider needs the operator to authenticate.
    ///
    /// runtrol holds no credential for any provider and will not accept one. The session freezes in
    /// place, the operator authenticates with the CLI's own command at their machine, and the
    /// session resumes. Never a reason to end a session or to touch a credential store.
    #[error("{provider}: not authenticated. authenticate with: {how}")]
    AuthRequired {
        /// Which provider.
        provider: ProviderId,
        /// The command the operator runs, as the provider itself names it.
        how: String,
    },

    /// The account is blocked on a quota.
    #[error("{provider}: quota reached{}", ResetsIn(*resets_in_ms))]
    Quota {
        /// Which provider.
        provider: ProviderId,
        /// Milliseconds until the limit lifts, when the provider says.
        resets_in_ms: Option<u64>,
    },

    /// This build cannot do what was asked, and knows it.
    ///
    /// Distinct from a protocol violation: nothing is broken, the capability is simply absent. The
    /// message says so plainly instead of sending the operator hunting for a typo.
    #[error("{provider}: {what} is not supported: {why}")]
    Unsupported {
        /// Which provider.
        provider: ProviderId,
        /// What was asked for.
        what: String,
        /// Why this build cannot serve it.
        why: &'static str,
    },

    /// The provider was asked and said no.
    ///
    /// The request was well formed and the provider declined it. Carried separately from a protocol
    /// violation because there is nothing to fix in runtrol.
    #[error("{provider}: refused to {doing}: {detail}")]
    NativeRefused {
        /// Which provider.
        provider: ProviderId,
        /// What runtrol asked for.
        doing: &'static str,
        /// The provider's reason, verbatim and never parsed.
        detail: String,
    },

    /// An operating system call failed.
    ///
    /// `doing` is required. An IO error without the operation that produced it tells the operator
    /// that something failed and nothing else.
    #[error("{provider}: {doing} failed: {source}")]
    Io {
        /// Which provider.
        provider: ProviderId,
        /// What runtrol was doing.
        doing: &'static str,
        /// What the OS said.
        #[source]
        source: std::io::Error,
    },
}

impl ProviderError {
    /// Whether trying the same thing again could plausibly succeed without anyone intervening.
    ///
    /// Consulted by the supervisor before a retry and reported to a subscriber so a phone can show
    /// "retrying" instead of "failed". Conservative by design: a variant is retryable only when
    /// repetition is known to be harmless, because a wrong `true` here spends the operator's tokens
    /// in a loop.
    #[must_use]
    #[expect(
        clippy::match_same_arms,
        reason = "two arms answer true for unrelated reasons (a clock lifts the condition, versus a \
                  transient OS state). merging them would delete the distinction the comments carry, \
                  and the two classes diverge the moment either grows a backoff policy"
    )]
    pub const fn retryable(&self) -> bool {
        match self {
            // The condition is time based and lifts on its own.
            Self::Quota { .. } | Self::Timeout { .. } => true,
            // A transient OS condition (a busy pipe, an interrupted read) is worth one more try;
            // a permanent one fails again immediately and cheaply.
            Self::Io { .. } | Self::Spawn { .. } => true,
            // Nothing changes without a person acting: installing a CLI, fixing a manifest,
            // authenticating, or a vendor shipping a fix.
            Self::BinNotFound { .. }
            | Self::BinAmbiguous { .. }
            | Self::Protocol { .. }
            | Self::AuthRequired { .. }
            | Self::Unsupported { .. }
            | Self::NativeRefused { .. } => false,
        }
    }

    /// Whether the operator has to do something at their own machine before this can work.
    ///
    /// Drives the one honest answer a phone can give for these: "go to your PC". Authentication in
    /// particular is unfixable from anywhere else, because runtrol does not carry credentials and a
    /// remote caller has no way to supply one.
    #[must_use]
    pub const fn needs_operator_at_the_machine(&self) -> bool {
        matches!(
            self,
            Self::AuthRequired { .. } | Self::BinNotFound { .. } | Self::BinAmbiguous { .. }
        )
    }

    /// Which provider produced this.
    #[must_use]
    pub const fn provider(&self) -> ProviderId {
        match self {
            Self::BinNotFound { provider, .. }
            | Self::BinAmbiguous { provider, .. }
            | Self::Spawn { provider, .. }
            | Self::Protocol { provider, .. }
            | Self::Timeout { provider, .. }
            | Self::AuthRequired { provider, .. }
            | Self::Quota { provider, .. }
            | Self::Unsupported { provider, .. }
            | Self::NativeRefused { provider, .. }
            | Self::Io { provider, .. } => *provider,
        }
    }
}

/// Renders the optional reset delay of [`ProviderError::Quota`] without a second message string.
struct ResetsIn(Option<u64>);

impl fmt::Display for ResetsIn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Some(ms) => write!(f, ", resets in {} s", ms / 1_000),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider() -> ProviderId {
        ProviderId::parse("codex").expect("valid built-in provider id")
    }

    /// One of every variant, so the exhaustive matches below cannot be satisfied by a subset.
    fn one_of_each() -> Vec<ProviderError> {
        vec![
            ProviderError::BinNotFound {
                provider: provider(),
                searched: "PATH".to_owned(),
            },
            ProviderError::BinAmbiguous {
                provider: provider(),
                candidates: "a, b".to_owned(),
            },
            ProviderError::Spawn {
                provider: provider(),
                program: "codex".to_owned(),
                source: std::io::Error::from(std::io::ErrorKind::PermissionDenied),
            },
            ProviderError::Protocol {
                provider: provider(),
                doing: "reading a notification",
                detail: "unbalanced braces".to_owned(),
            },
            ProviderError::Timeout {
                provider: provider(),
                doing: "starting a thread",
                waited_ms: 5_000,
            },
            ProviderError::AuthRequired {
                provider: provider(),
                how: "codex login".to_owned(),
            },
            ProviderError::Quota {
                provider: provider(),
                resets_in_ms: Some(120_000),
            },
            ProviderError::Unsupported {
                provider: provider(),
                what: "pty transport".to_owned(),
                why: "not built into this binary",
            },
            ProviderError::NativeRefused {
                provider: provider(),
                doing: "resume a thread",
                detail: "thread is archived".to_owned(),
            },
            ProviderError::Io {
                provider: provider(),
                doing: "reading the transcript",
                source: std::io::Error::from(std::io::ErrorKind::NotFound),
            },
        ]
    }

    #[test]
    fn every_message_names_the_provider_and_what_was_happening() {
        for error in one_of_each() {
            let message = error.to_string();
            assert!(
                message.starts_with("codex: "),
                "message must name the provider: {message}"
            );
            assert!(
                message.len() > "codex: ".len() + 8,
                "message must say something: {message}"
            );
            assert_eq!(error.provider(), provider());
        }
    }

    #[test]
    fn retryable_is_decided_for_every_variant() {
        // `retryable` matches exhaustively, so this test failing to compile is the real assertion.
        // What it checks at runtime is that the two classes are not accidentally the same.
        let errors = one_of_each();
        assert!(errors.iter().any(ProviderError::retryable));
        assert!(errors.iter().any(|error| !error.retryable()));
    }

    #[test]
    fn authentication_is_never_retried_and_always_sends_the_operator_to_their_machine() {
        // runtrol holds no credential, so nothing about a retry could change the outcome, and no
        // remote caller can supply one.
        let error = ProviderError::AuthRequired {
            provider: provider(),
            how: "codex login".to_owned(),
        };
        assert!(!error.retryable());
        assert!(error.needs_operator_at_the_machine());
    }

    #[test]
    fn a_protocol_violation_is_never_retried() {
        // Retrying a vendor's shape change spends tokens and produces the same failure.
        let error = ProviderError::Protocol {
            provider: provider(),
            doing: "reading a notification",
            detail: "unknown envelope".to_owned(),
        };
        assert!(!error.retryable());
        assert!(!error.needs_operator_at_the_machine());
    }

    #[test]
    fn quota_reports_its_reset_delay_when_the_provider_gave_one() {
        let known = ProviderError::Quota {
            provider: provider(),
            resets_in_ms: Some(120_000),
        };
        assert_eq!(known.to_string(), "codex: quota reached, resets in 120 s");

        let unknown = ProviderError::Quota {
            provider: provider(),
            resets_in_ms: None,
        };
        assert_eq!(unknown.to_string(), "codex: quota reached");
    }

    #[test]
    fn the_source_chain_survives() {
        // The operator needs the OS's own words, not a paraphrase.
        let error = ProviderError::Io {
            provider: provider(),
            doing: "reading the transcript",
            source: std::io::Error::from(std::io::ErrorKind::NotFound),
        };
        let source = std::error::Error::source(&error).expect("io errors keep their source");
        assert!(!source.to_string().is_empty());
    }
}
