//! Binding to a provider session, and losing it again.
//!
//! Both frames exist to make one thing impossible: a session that looks alive because nothing said
//! otherwise. [`Detached`] always names what runtrol observed, and when a turn was in flight it says so,
//! so a subscriber renders "outcome unknown" rather than the last thing it happened to see.

use serde::Serialize;

use crate::event::Opaque;
use crate::id::{NativeSessionId, TurnId};
use crate::path::AbsPath;

/// A driver has bound to a provider session and work can flow.
///
/// Boxed inside the event body: it is the richest frame and the rarest, once per attach, while the
/// frames beside it arrive thousands of times per turn.
#[derive(Debug, Clone, Serialize)]
pub struct Attached {
    /// The provider's own identifier for this session.
    pub native: NativeSessionId,
    /// How a subscriber recovers content older than the replay ring.
    pub replay: ReplaySource,
    /// The model runtrol asked for.
    ///
    /// Not the model that answered. Those differ: one CLI reported `claude-opus-5[1m]` at startup while
    /// its assistant messages said `claude-opus-5`. The answering model rides in the payload, where the
    /// subscriber can render it; runtrol records only what it requested, because that is the only one it
    /// decided.
    pub model_requested: Option<Box<str>>,
    /// Capability tokens the provider announced.
    pub caps: CapabilitySet,
    /// The provider's whole startup object, verbatim.
    pub payload: Opaque,
}

/// Where content older than the replay ring can be read from.
///
/// runtrol keeps no transcript, so a subscriber that falls far behind is served from the provider's own
/// store. This enum is what makes that possible without the core knowing which provider it is talking
/// to: the core compares cursors, and only the driver knows what the numbers count.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ReplaySource {
    /// An append-only file, addressed by byte offset.
    ///
    /// Verified append-only by controlled experiment: one CLI's transcript grew from 17,877 to 22,339
    /// bytes across a resume while the hash of its first 4,096 bytes stayed identical. That is what makes
    /// a byte offset a durable cursor rather than a guess, and it is why a positioned 64 KiB read (1.1 ms,
    /// 64 KiB of memory) can replace materializing the file (173 ms, 145 MB).
    File {
        /// Where the provider keeps it.
        path: AbsPath,
        /// Identity of the file, so a rotation or truncation is detected rather than misread.
        file_id: FileId,
    },
    /// A provider method serves the range.
    ///
    /// The cursor counts whatever that method counts. Preferred where it works, because it does not
    /// depend on a path the vendor may move.
    Protocol {
        /// The bound method name.
        method: &'static str,
    },
    /// Nothing survives past the ring.
    ///
    /// A subscriber asking for older content is told what was lost, explicitly, rather than handed a
    /// silent gap.
    None,
}

/// Enough of a file's identity to notice it was replaced.
///
/// Compared, never interpreted. A cursor into a file that has been rotated or truncated points at
/// different content, and reading it anyway would serve one session's bytes as another's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct FileId {
    /// Size when the identity was taken.
    pub len: u64,
    /// Modification time in milliseconds, when the platform reports one.
    pub modified_ms: Option<u64>,
}

/// Capability tokens a provider announced about itself.
///
/// A feature-detection channel and nothing more. Tokens are kept and compared for presence; they are
/// never parsed for structure and never matched against a version string. That rule is what stops this
/// from becoming the hardcoded capability table the discovery rule forbids.
#[derive(Debug, Clone, Default, Serialize)]
pub struct CapabilitySet(Vec<Box<str>>);

impl CapabilitySet {
    /// Collect the tokens a provider announced.
    #[must_use]
    pub fn from_tokens<I, S>(tokens: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<Box<str>>,
    {
        Self(tokens.into_iter().map(Into::into).collect())
    }

    /// Whether the provider announced this exact token.
    ///
    /// Exact match only. Prefix and substring matching is how a capability check starts guessing.
    #[must_use]
    pub fn has(&self, token: &str) -> bool {
        self.0.iter().any(|held| &**held == token)
    }

    /// Every token, in the order the provider gave them.
    pub fn tokens(&self) -> impl Iterator<Item = &str> {
        self.0.iter().map(|token| &**token)
    }

    /// How many tokens were announced.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether the provider announced nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// The driver is no longer bound to the provider session.
#[derive(Debug, Clone, Serialize)]
pub struct Detached {
    /// What runtrol observed.
    pub reason: DetachReason,
    /// The child's exit code, when there was a child and it exited.
    pub exit: Option<i32>,
    /// The turn that was in flight, if one was.
    ///
    /// This field is the whole anti-falsely-alive mechanism. When it is `Some`, a turn died without any
    /// completion signal, the subscriber must render "outcome unknown", and the next attach must not
    /// claim the turn succeeded. Without it, a session that died mid-turn is indistinguishable from one
    /// that finished quietly.
    pub in_turn: Option<TurnId>,
}

/// Why a driver stopped being bound.
///
/// Every variant is something runtrol observed. None is an inference about what the provider meant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub enum DetachReason {
    /// runtrol asked to detach.
    Requested,
    /// The child process exited. runtrol observed a fact and inferred no outcome from it.
    ProcessExit,
    /// Framing or envelope parsing failed, or a line exceeded the transport's bound.
    ProtocolViolation,
    /// A shared provider daemon went away, taking every session it multiplexed with it.
    ///
    /// One observation, many detachments. Each affected session gets its own frame, because each has its
    /// own subscribers and its own in-flight turn.
    HostGone,
    /// Another client took the provider session.
    ///
    /// Two writers to one transcript is a corruption runtrol declines to participate in.
    Superseded,
    /// Evicted from the hot tier by the session tier policy. Not an error.
    Evicted,
}

impl DetachReason {
    /// Whether reattaching could plausibly work without anyone intervening.
    #[must_use]
    pub const fn can_reattach(&self) -> bool {
        match self {
            // The provider still has the session; runtrol simply stopped watching it.
            Self::Requested | Self::Evicted | Self::HostGone => true,
            // Something is wrong with the child, the protocol, or the ownership of the session, and
            // reattaching would either fail again or fight another client for the transcript.
            Self::ProcessExit | Self::ProtocolViolation | Self::Superseded => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capabilities_match_exactly_and_never_by_prefix() {
        // A prefix match is where a capability check starts guessing, and guessing is what the
        // discovery rule exists to prevent.
        let caps = CapabilitySet::from_tokens(["streamingInput", "hooks"]);
        assert!(caps.has("streamingInput"));
        assert!(!caps.has("streaming"), "a prefix is not a capability");
        assert!(!caps.has("streamingInputV2"), "nor is an extension of one");
        assert_eq!(caps.len(), 2);
    }

    #[test]
    fn an_unknown_capability_is_kept_rather_than_dropped() {
        // A vendor shipping a token runtrol has never heard of must not lose information.
        let caps = CapabilitySet::from_tokens(["somethingNew"]);
        assert_eq!(caps.tokens().collect::<Vec<_>>(), vec!["somethingNew"]);
    }

    #[test]
    fn an_empty_capability_set_is_the_default() {
        let caps = CapabilitySet::default();
        assert!(caps.is_empty());
        assert!(!caps.has("anything"));
    }

    #[test]
    fn a_detachment_during_a_turn_records_the_turn() {
        // The difference between "the turn's outcome is unknown" and "the turn ended" is this field.
        let died = Detached {
            reason: DetachReason::ProcessExit,
            exit: Some(1),
            in_turn: Some(TurnId::first(0)),
        };
        assert!(
            died.in_turn.is_some(),
            "a turn was in flight and died with it"
        );

        let closed = Detached {
            reason: DetachReason::Requested,
            exit: None,
            in_turn: None,
        };
        assert!(closed.in_turn.is_none());
    }

    #[test]
    fn reattachability_is_decided_for_every_reason() {
        for reason in [
            DetachReason::Requested,
            DetachReason::Evicted,
            DetachReason::HostGone,
        ] {
            assert!(reason.can_reattach(), "{reason:?} should be reattachable");
        }
        for reason in [
            DetachReason::ProcessExit,
            DetachReason::ProtocolViolation,
            DetachReason::Superseded,
        ] {
            assert!(
                !reason.can_reattach(),
                "{reason:?} should not be reattachable"
            );
        }
    }
}
