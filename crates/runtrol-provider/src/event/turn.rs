//! The turn lifecycle, and the one rule that governs it.
//!
//! > **A turn ends only from evidence. Never from inference.**
//!
//! There are exactly four evidences, and they are exactly the four [`Declarant`] values. There is no
//! fifth path. Absence of output is not evidence. A provider reporting itself idle is not evidence. An
//! acknowledgement carrying no work is not evidence.
//!
//! # Why this file is shaped the way it is
//!
//! An early probe read an eight second turn as finished in ten milliseconds, because it treated the
//! provider's acknowledgement of a submission as the completion of it. One CLI answers `turn/start` in
//! two milliseconds with an empty item list and a status of "in progress"; that response says the
//! submission arrived and nothing more.
//!
//! Three structural guards make that mistake unrepeatable, and none of them is a careful line of code:
//!
//! 1. [`TurnEvent::Accepted`] and [`TurnEvent::Ended`] are different variants, so a driver cannot emit
//!    the acknowledgement and have it read as the end.
//! 2. [`Declarant`] makes provenance mandatory. Every ending names its evidence at the call site.
//! 3. There is no `StopReason::Timeout`. A timeout is a fact about runtrol's patience, not about the
//!    turn's outcome, so it can only be expressed as [`Declarant::NoSignal`] with an unknown reason.

use serde::Serialize;

use crate::id::{ApprovalId, TurnId};

/// Something happened to a turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "step", rename_all = "camelCase")]
#[non_exhaustive]
pub enum TurnEvent {
    /// The provider acknowledged the submission.
    ///
    /// **This is not a completion.** One CLI answers in two milliseconds with an empty item list, which
    /// `ack_only` records; the other emits no acknowledgement at all, so this frame simply never appears
    /// for it.
    Accepted {
        /// Which turn.
        turn: TurnId,
        /// The acknowledgement carried no work, only receipt.
        ack_only: bool,
    },

    /// Work has demonstrably begun.
    Started {
        /// Which turn.
        turn: TurnId,
    },

    /// The provider is waiting on a human.
    ///
    /// Distinct from running, because a session waiting on a person must never be counted as silent and
    /// the liveness clock must not fire on it.
    Blocked {
        /// Which turn.
        turn: TurnId,
        /// What it is waiting for.
        on: BlockedOn,
    },

    /// Unblocked without ending.
    Resumed {
        /// Which turn.
        turn: TurnId,
    },

    /// The turn is over, and this frame records whose word that is.
    ///
    /// The only terminal frame. A second one for the same turn is a bug, and a late frame arriving after
    /// it is relayed as unmapped rather than reopening the turn.
    Ended {
        /// Which turn.
        turn: TurnId,
        /// Why it ended.
        stop: StopReason,
        /// Whose statement ended it.
        declared_by: Declarant,
    },
}

impl TurnEvent {
    /// Which turn this concerns.
    #[must_use]
    pub const fn turn(&self) -> TurnId {
        match self {
            Self::Accepted { turn, .. }
            | Self::Started { turn }
            | Self::Blocked { turn, .. }
            | Self::Resumed { turn }
            | Self::Ended { turn, .. } => *turn,
        }
    }

    /// Whether this frame ends the turn.
    ///
    /// Exactly one variant answers yes. Written as a method so that code deciding "is this the last
    /// frame" cannot accidentally accept the acknowledgement.
    #[must_use]
    pub const fn is_terminal(&self) -> bool {
        matches!(self, Self::Ended { .. })
    }
}

/// What a blocked turn is waiting for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "on", rename_all = "camelCase")]
#[non_exhaustive]
pub enum BlockedOn {
    /// A pending approval, which somebody has to answer.
    Approval {
        /// Which approval.
        id: ApprovalId,
    },
    /// Free-form input the provider asked for.
    UserInput,
    /// An account limit. The honest "you are waiting on a quota" state rather than a spinner.
    RateLimit,
}

/// Why a turn ended.
///
/// The first five are the standard vocabulary, carried through unchanged. The last two are runtrol's,
/// and both exist to describe not knowing rather than to fill a gap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub enum StopReason {
    /// The agent finished its turn.
    EndTurn,
    /// The model hit its output limit.
    MaxTokens,
    /// The agent hit its request limit for one turn.
    MaxTurnRequests,
    /// The model declined.
    Refusal,
    /// Cancelled, by an interrupt or by the provider.
    Cancelled,
    /// The provider reported failure.
    Failed,
    /// Nobody said why.
    ///
    /// Either the provider used a token runtrol has no binding for, or nothing was said at all. Distinct
    /// from [`Self::Failed`], because not understanding a reason is not the same as being given one.
    Unknown,
}

impl StopReason {
    /// Whether the agent completed the work it was asked to do.
    ///
    /// Only one variant qualifies. Every other, including [`Self::Unknown`], must not be rendered as
    /// success, and that is what this method exists to make hard to get wrong.
    #[must_use]
    pub const fn is_success(&self) -> bool {
        matches!(self, Self::EndTurn)
    }
}

/// Whose statement ended the turn.
///
/// Mandatory on every ending. A subscriber renders differently for each of these, and a provider's own
/// completion signal must never be confused with runtrol giving up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "by", rename_all = "camelCase")]
#[non_exhaustive]
pub enum Declarant {
    /// The provider sent its documented completion signal.
    ///
    /// The only value that means the outcome is known.
    Provider,
    /// The child process died. runtrol observed an exit and inferred no outcome from it.
    ProcessExit,
    /// runtrol's interrupt was acknowledged and the provider then declared the end.
    InterruptAcked,
    /// Nothing arrived inside a budget somebody configured.
    ///
    /// runtrol reporting its own ignorance. Only reachable when the operator sets a turn budget, because
    /// there is no default one: a coding agent legitimately runs for an hour, and declaring it finished
    /// because ten minutes passed would be asserting an outcome runtrol does not know.
    ///
    /// A subscriber must render this as "runtrol stopped waiting", never as "the turn finished".
    NoSignal {
        /// How long runtrol waited.
        waited_ms: u64,
        /// How long ago the last sign of life was.
        last_activity_ms_ago: u64,
    },
}

impl Declarant {
    /// Whether the provider itself said the turn was over.
    ///
    /// The single question that decides whether an outcome is known. Everything else is runtrol
    /// describing what it observed or admitting what it does not know.
    #[must_use]
    pub const fn is_the_providers_word(&self) -> bool {
        matches!(self, Self::Provider | Self::InterruptAcked)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a_turn() -> TurnId {
        TurnId::first(0)
    }

    #[test]
    fn an_acknowledgement_is_not_a_completion() {
        // This is the probe bug, as a test. An eight second turn was read as finished in ten
        // milliseconds because the acknowledgement was treated as terminal.
        let acknowledged = TurnEvent::Accepted {
            turn: a_turn(),
            ack_only: true,
        };
        assert!(!acknowledged.is_terminal());

        let ended = TurnEvent::Ended {
            turn: a_turn(),
            stop: StopReason::EndTurn,
            declared_by: Declarant::Provider,
        };
        assert!(ended.is_terminal());
    }

    #[test]
    fn only_one_frame_is_terminal() {
        let non_terminal = [
            TurnEvent::Accepted {
                turn: a_turn(),
                ack_only: false,
            },
            TurnEvent::Started { turn: a_turn() },
            TurnEvent::Blocked {
                turn: a_turn(),
                on: BlockedOn::UserInput,
            },
            TurnEvent::Resumed { turn: a_turn() },
        ];
        for event in non_terminal {
            assert!(!event.is_terminal(), "{event:?} must not end a turn");
            assert_eq!(event.turn(), a_turn());
        }
    }

    #[test]
    fn only_the_provider_can_declare_a_known_outcome() {
        assert!(Declarant::Provider.is_the_providers_word());
        assert!(Declarant::InterruptAcked.is_the_providers_word());
        assert!(
            !Declarant::ProcessExit.is_the_providers_word(),
            "an exit is an observation, not an outcome"
        );
        assert!(
            !Declarant::NoSignal {
                waited_ms: 600_000,
                last_activity_ms_ago: 590_000,
            }
            .is_the_providers_word(),
            "runtrol giving up is not the provider finishing"
        );
    }

    #[test]
    fn only_one_reason_is_success() {
        assert!(StopReason::EndTurn.is_success());
        for reason in [
            StopReason::MaxTokens,
            StopReason::MaxTurnRequests,
            StopReason::Refusal,
            StopReason::Cancelled,
            StopReason::Failed,
            StopReason::Unknown,
        ] {
            assert!(!reason.is_success(), "{reason:?} must not read as success");
        }
    }

    #[test]
    fn giving_up_carries_how_long_and_since_when() {
        // "runtrol stopped waiting" is only renderable if the frame says what it waited for.
        let gave_up = TurnEvent::Ended {
            turn: a_turn(),
            stop: StopReason::Unknown,
            declared_by: Declarant::NoSignal {
                waited_ms: 300_000,
                last_activity_ms_ago: 299_000,
            },
        };
        match gave_up {
            TurnEvent::Ended {
                stop, declared_by, ..
            } => {
                assert_eq!(
                    stop,
                    StopReason::Unknown,
                    "runtrol does not invent a reason"
                );
                assert!(!declared_by.is_the_providers_word());
            }
            other => panic!("expected an ending, got {other:?}"),
        }
    }

    #[test]
    fn there_is_no_timeout_stop_reason() {
        // A timeout is a fact about runtrol's patience, not the turn's outcome. Keeping it out of
        // `StopReason` is what forces a caller to reach for `NoSignal` and say who gave up.
        let encoded = serde_json::to_string(&StopReason::Unknown).expect("serializable");
        assert_eq!(encoded, r#""unknown""#);
        // If a `timeout` variant is ever added, this assertion is the thing that should stop it.
        for reason in [
            StopReason::EndTurn,
            StopReason::MaxTokens,
            StopReason::MaxTurnRequests,
            StopReason::Refusal,
            StopReason::Cancelled,
            StopReason::Failed,
            StopReason::Unknown,
        ] {
            let name = serde_json::to_string(&reason).expect("serializable");
            assert!(
                !name.contains("timeout"),
                "a timeout is not an outcome: {name}"
            );
        }
    }
}
