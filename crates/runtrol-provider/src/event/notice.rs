//! Something the operator should know about, that is not conversation.
//!
//! A notice is how runtrol says "the session is degraded" without pretending to know why in the
//! provider's own words. The code is a closed set runtrol switches on; the provider's message text rides
//! in the payload, unread.

use serde::Serialize;

use crate::event::Opaque;

/// How much attention a notice deserves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Level {
    /// Worth recording, not worth interrupting anyone.
    Info,
    /// Something is degraded and still working.
    Warn,
    /// Something is not working.
    Error,
}

/// A condition worth reporting.
#[derive(Debug, Clone, Serialize)]
pub struct Notice {
    /// How much attention it deserves.
    pub level: Level,
    /// What kind of condition it is.
    pub code: NoticeCode,
    /// The provider said it will try again.
    ///
    /// Load bearing: **a retryable error is not the end of a turn.** One CLI emits errors mid-turn with a
    /// will-retry flag, and treating those as terminal would end turns that were still running.
    pub retryable: bool,
    /// The provider's own message, verbatim and unread.
    pub payload: Opaque,
}

/// What kind of condition a notice reports.
///
/// A closed set, because runtrol switches on it. The catch-all is a distinct value rather than a default
/// that pretends to have understood.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub enum NoticeCode {
    /// The provider needs the operator to authenticate.
    ///
    /// Freezes the session; never kills it, and never touches the provider's credential store. The
    /// operator authenticates with the CLI's own command and the session continues in place.
    AuthRequired,
    /// An account limit is blocking.
    QuotaExhausted,
    /// The conversation no longer fits.
    ContextExceeded,
    /// The provider hit a transient failure and will try again.
    ProviderRetry,
    /// runtrol's own report that nothing has arrived for a while.
    ///
    /// **Never the end of a turn.** Silence is not evidence of anything, so this frame says how long it
    /// has been quiet and leaves the turn running.
    ProviderSilent,
    /// The provider compacted the conversation.
    Compaction,
    /// The provider announced something is going away.
    Deprecation,
    /// A tool call was denied without anybody being asked.
    ///
    /// The operator should know a capability was blocked by a rule rather than by them.
    PermissionAutoDenied,
    /// The provider served a different model than the one requested.
    ModelRerouted,
    /// The provider refused to switch permission mode, and the mode in force is unchanged.
    ///
    /// Carries the provider's own refusal sentence in the payload, because the vocabulary of valid modes
    /// is the provider's and its refusal names it better than runtrol could.
    ModeRefused,
    /// A capability runtrol negotiated is no longer there.
    ///
    /// The visible half of drift tolerance. When a provider surface runtrol relied on disappears, the
    /// feature degrades and says so, instead of failing in a way that looks like a runtrol bug.
    TierDowngraded,
    /// The provider broke its own protocol.
    ProtocolViolation,
    /// The provider asked runtrol for a credential and runtrol refused.
    ///
    /// One CLI asks its client to refresh an authentication token. runtrol holds no provider credential
    /// and will not proxy one, and it must answer rather than stay silent, because silence hangs the
    /// provider's daemon. So it answers with an error and emits this.
    CredentialRequestRefused,
    /// Something else worth reporting.
    Other,
}

impl NoticeCode {
    /// Whether this condition needs the operator to act at their own machine.
    ///
    /// Drives the one honest answer a phone can give: "go to your PC". Authentication in particular is
    /// unfixable from anywhere else, because runtrol carries no credential and a remote caller has no way
    /// to supply one.
    #[must_use]
    pub const fn needs_operator_at_the_machine(&self) -> bool {
        matches!(self, Self::AuthRequired | Self::CredentialRequestRefused)
    }

    /// Whether this is worth waking a phone for.
    ///
    /// Deliberately short. A notification that fires for everything is a notification nobody reads, so
    /// only conditions where the session stops making progress until somebody acts are included.
    #[must_use]
    pub const fn deserves_a_notification(&self) -> bool {
        match self {
            Self::AuthRequired
            | Self::QuotaExhausted
            | Self::ContextExceeded
            | Self::CredentialRequestRefused => true,
            // Progress continues, or resumes on its own, or the condition is informational. A phone
            // buzzing for a compaction is a phone the operator silences.
            Self::ProviderRetry
            | Self::ProviderSilent
            | Self::Compaction
            | Self::Deprecation
            | Self::PermissionAutoDenied
            | Self::ModelRerouted
            | Self::ModeRefused
            | Self::TierDowngraded
            | Self::ProtocolViolation
            | Self::Other => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn every_code() -> Vec<NoticeCode> {
        vec![
            NoticeCode::AuthRequired,
            NoticeCode::QuotaExhausted,
            NoticeCode::ContextExceeded,
            NoticeCode::ProviderRetry,
            NoticeCode::ProviderSilent,
            NoticeCode::Compaction,
            NoticeCode::Deprecation,
            NoticeCode::PermissionAutoDenied,
            NoticeCode::ModelRerouted,
            NoticeCode::ModeRefused,
            NoticeCode::TierDowngraded,
            NoticeCode::ProtocolViolation,
            NoticeCode::CredentialRequestRefused,
            NoticeCode::Other,
        ]
    }

    #[test]
    fn a_retryable_notice_is_not_a_turn_ending() {
        // The field exists because one CLI emits mid-turn errors with a will-retry flag, and treating
        // those as terminal ends turns that are still running.
        let transient = Notice {
            level: Level::Warn,
            code: NoticeCode::ProviderRetry,
            retryable: true,
            payload: Opaque::owned(r#"{"message":"upstream hiccup"}"#.to_owned()),
        };
        assert!(transient.retryable);
        assert!(!transient.code.deserves_a_notification());
    }

    #[test]
    fn only_credential_conditions_send_the_operator_to_their_machine() {
        for code in every_code() {
            let expected = matches!(
                code,
                NoticeCode::AuthRequired | NoticeCode::CredentialRequestRefused
            );
            assert_eq!(
                code.needs_operator_at_the_machine(),
                expected,
                "{code:?} is classified wrongly"
            );
        }
    }

    #[test]
    fn notifications_are_reserved_for_stalled_progress() {
        // A phone that buzzes for everything is a phone with notifications turned off.
        let noisy: Vec<NoticeCode> = every_code()
            .into_iter()
            .filter(NoticeCode::deserves_a_notification)
            .collect();
        assert_eq!(noisy.len(), 4, "the notifying set grew: {noisy:?}");
    }

    #[test]
    fn a_provider_message_never_reaches_a_log_line() {
        let notice = Notice {
            level: Level::Error,
            code: NoticeCode::ProtocolViolation,
            retryable: false,
            payload: Opaque::owned(r#"{"message":"failed on /home/me/secret.key"}"#.to_owned()),
        };
        let printed = format!("{notice:?}");
        assert!(!printed.contains("secret.key"), "leaked: {printed}");
        assert!(
            printed.contains("ProtocolViolation"),
            "but the code is visible"
        );
    }

    #[test]
    fn levels_order_from_quiet_to_loud() {
        assert!(Level::Info < Level::Warn);
        assert!(Level::Warn < Level::Error);
    }
}
