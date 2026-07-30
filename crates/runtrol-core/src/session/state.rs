//! The one place a session's state may change.
//!
//! Every transition goes through [`Lifecycle::after`]. Nothing else may assign a state, which is what makes
//! "a child process failure is promoted to session state, never only logged" checkable rather than a habit: a
//! failure has nowhere else to go.
//!
//! # Facts, not commands
//!
//! The input is [`Observed`]: something runtrol saw. Not "close this session" but "this session was closed".
//! A state machine driven by intentions has to guess what happened when an intention fails; one driven by
//! observations does not have to guess at all.
//!
//! # Refusing a transition is a result, not a panic
//!
//! An impossible transition means a driver reported something that cannot have happened, and a supervisor that
//! aborted on it would take every other session down with the one misbehaving driver. So it comes back as a
//! [`Refused`], which the caller turns into a notice.
//!
//! # Silence is not evidence, and there is no turn timeout
//!
//! A turn ends when something says it ended. If nothing arrives for a long time, runtrol still does not know
//! whether the turn is finished, so it does not decide. What it does instead is [`SessionState::quiet_since`],
//! which records that nothing has arrived and **leaves the turn running**. A subscriber can show "this looks
//! stuck" and offer to stop it; what it must not show is a completion runtrol invented.
//!
//! That is the same rule as not swallowing an error. Reporting an outcome nobody observed is a lie whichever
//! direction it points.

use core::fmt;

use runtrol_provider::{TurnId, WallMs};

/// What a session is doing.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Lifecycle {
    /// No driver is bound.
    ///
    /// Where every session starts, including one runtrol has only ever read about from a provider's own store.
    Detached,
    /// A driver is binding.
    Starting,
    /// Bound, with no turn running.
    Idle,
    /// Bound, with a turn running.
    Busy {
        /// Which turn.
        turn: TurnId,
    },
    /// Something went wrong. Visible, and still resumable.
    ///
    /// Resumable because the conversation is not runtrol's to lose: the provider's own store still has it, so
    /// what failed is the attachment rather than the session. That is why this state accepts a new attach and
    /// why it is not the end of anything.
    Failed {
        /// When runtrol saw it.
        at: WallMs,
        /// What kind of failure.
        code: FailureCode,
        /// What runtrol observed, for a person to read.
        detail: String,
    },
    /// Deliberately ended.
    ///
    /// The one state nothing leaves. Eviction is not this: an evicted session has no child and is still there,
    /// which is a tier and not an ending.
    Closed {
        /// When.
        at: WallMs,
        /// Why.
        reason: CloseReason,
    },
}

/// What kind of failure runtrol observed.
///
/// Every one is something that happened, not a conclusion about what it meant. None of them decides whether a
/// turn succeeded: a turn that was running when this happened is reported as an unknown outcome, because that
/// is what it is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum FailureCode {
    /// The child could not be started at all.
    CannotStart,
    /// The child exited without runtrol asking it to.
    ChildExited,
    /// Framing or envelope parsing failed, or a line exceeded the transport's bound.
    Protocol,
    /// A shared provider process went away, taking every session it served.
    HostGone,
    /// Another client took the provider session.
    ///
    /// Two writers to one transcript is a corruption runtrol declines to take part in.
    Superseded,
}

impl fmt::Display for FailureCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::CannotStart => "the provider could not be started",
            Self::ChildExited => "the provider exited on its own",
            Self::Protocol => "the provider broke its own protocol",
            Self::HostGone => "the provider process went away",
            Self::Superseded => "another client took this session",
        })
    }
}

/// Why a session was ended on purpose.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum CloseReason {
    /// The operator asked for it to finish.
    Requested,
    /// The operator asked for it to stop now.
    Killed,
    /// The operator deleted it.
    Deleted,
}

/// Something runtrol saw happen to a session.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Observed {
    /// A driver has begun binding.
    Attaching,
    /// A driver is bound and work can flow.
    Attached,
    /// A turn started.
    TurnStarted {
        /// Which turn.
        turn: TurnId,
    },
    /// A turn ended.
    ///
    /// Carries which turn, and the state machine checks it. A driver reporting the end of a turn that is not
    /// the one running would otherwise end the running one, and the operator would watch a turn that is still
    /// going report itself finished.
    TurnEnded {
        /// Which turn.
        turn: TurnId,
    },
    /// The driver is no longer bound, and nothing went wrong.
    Detached,
    /// Something went wrong.
    Failed {
        /// What kind.
        code: FailureCode,
        /// What runtrol observed.
        detail: String,
    },
    /// The session was ended on purpose.
    Closed {
        /// Why.
        reason: CloseReason,
    },
}

/// A transition that cannot have happened.
#[derive(Clone, PartialEq, Eq, Debug, thiserror::Error)]
#[error("a session that is {from} cannot be {observed}")]
pub struct Refused {
    /// What the session was, in words.
    pub from: &'static str,
    /// What was reported, in words.
    pub observed: &'static str,
}

impl Lifecycle {
    /// A short name for this state, for a message and for a list.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Detached => "detached",
            Self::Starting => "starting",
            Self::Idle => "idle",
            Self::Busy { .. } => "busy",
            Self::Failed { .. } => "failed",
            Self::Closed { .. } => "closed",
        }
    }

    /// Whether a driver is bound, or becoming bound.
    #[must_use]
    pub const fn is_attached(&self) -> bool {
        matches!(self, Self::Starting | Self::Idle | Self::Busy { .. })
    }

    /// The turn that is running, if one is.
    #[must_use]
    pub const fn turn(&self) -> Option<TurnId> {
        match self {
            Self::Busy { turn } => Some(*turn),
            _ => None,
        }
    }

    /// Whether nothing will ever leave this state.
    #[must_use]
    pub const fn is_final(&self) -> bool {
        matches!(self, Self::Closed { .. })
    }

    /// Whether the operator can pick this session back up.
    ///
    /// True for a failure, because the conversation lives in the provider's own store and only the attachment
    /// was lost. False only for a session that was ended on purpose.
    #[must_use]
    pub const fn can_resume(&self) -> bool {
        !self.is_final()
    }

    /// The state after `observed`, or a refusal.
    ///
    /// The whole transition table, in one match, in one place. `at` is when runtrol saw it, passed in rather
    /// than read from the clock so that a caller stamping several observations at one instant records one
    /// instant.
    ///
    /// # Errors
    ///
    /// [`Refused`] when the report cannot have happened: a turn ending that was not running, work on a session
    /// nothing is bound to, or anything at all about a session that was ended on purpose.
    pub fn after(&self, observed: Observed, at: WallMs) -> Result<Self, Refused> {
        let refuse = |what: &'static str| {
            Err(Refused {
                from: self.name(),
                observed: what,
            })
        };

        match (self, observed) {
            // Nothing leaves a session that was ended on purpose. Checked first, so no rule below can
            // accidentally reopen one.
            (Self::Closed { .. }, _) => refuse("changed"),

            // Ending on purpose is possible from anywhere else, including mid-turn: an operator who wants a
            // session stopped does not have to wait for it to finish.
            (_, Observed::Closed { reason }) => Ok(Self::Closed { at, reason }),

            // A failure is possible from anywhere else too, and never decides what a running turn achieved.
            (_, Observed::Failed { code, detail }) => Ok(Self::Failed { at, code, detail }),

            // Binding. A failed session may be picked back up, which is what makes a failure not an ending.
            (Self::Detached | Self::Failed { .. }, Observed::Attaching) => Ok(Self::Starting),
            (Self::Starting, Observed::Attached) => Ok(Self::Idle),

            // Turns. Only a bound session has them, and only the turn that is running can end.
            (Self::Idle, Observed::TurnStarted { turn }) => Ok(Self::Busy { turn }),
            (Self::Busy { turn: running }, Observed::TurnEnded { turn }) => {
                if *running == turn {
                    Ok(Self::Idle)
                } else {
                    // A driver reporting the end of a turn that is not the one running. Ending the running one
                    // would show the operator a completion for work that is still going.
                    refuse("told that a different turn ended")
                }
            }

            // Letting go cleanly. From `Starting` too: a bind that was abandoned rather than failed.
            (Self::Starting | Self::Idle | Self::Busy { .. }, Observed::Detached) => {
                Ok(Self::Detached)
            }

            // Everything else is a report that cannot have happened.
            (_, Observed::Attaching) => refuse("attaching again"),
            (_, Observed::Attached) => refuse("attached"),
            (_, Observed::TurnStarted { .. }) => refuse("starting a turn"),
            (_, Observed::TurnEnded { .. }) => refuse("ending a turn"),
            (_, Observed::Detached) => refuse("detached"),
        }
    }
}

/// A session's state, and how long it has been silent.
///
/// The two are kept together and can only be changed apart, which is the point: recording silence must not be
/// able to change what the session is doing.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SessionState {
    /// What it is doing.
    lifecycle: Lifecycle,
    /// When runtrol last saw anything from it.
    last_seen: WallMs,
    /// Since when nothing has arrived, once that has gone on long enough to be worth showing.
    quiet_since: Option<WallMs>,
}

impl SessionState {
    /// A session nothing is bound to.
    #[must_use]
    pub const fn new(at: WallMs) -> Self {
        Self {
            lifecycle: Lifecycle::Detached,
            last_seen: at,
            quiet_since: None,
        }
    }

    /// What it is doing.
    #[must_use]
    pub const fn lifecycle(&self) -> &Lifecycle {
        &self.lifecycle
    }

    /// When runtrol last saw anything.
    #[must_use]
    pub const fn last_seen(&self) -> WallMs {
        self.last_seen
    }

    /// Since when it has been silent, when that is worth showing.
    #[must_use]
    pub const fn quiet_since(&self) -> Option<WallMs> {
        self.quiet_since
    }

    /// Whether it looks stuck.
    ///
    /// A presentation question, never a lifecycle one. A session can look stuck and have a turn running, which
    /// is exactly the case this exists for.
    #[must_use]
    pub const fn looks_stuck(&self) -> bool {
        self.quiet_since.is_some()
    }

    /// Apply an observation.
    ///
    /// Anything arriving is also evidence of life, so this clears the silence.
    ///
    /// # Errors
    ///
    /// [`Refused`] when the observation cannot have happened.
    pub fn observe(&mut self, observed: Observed, at: WallMs) -> Result<(), Refused> {
        self.lifecycle = self.lifecycle.after(observed, at)?;
        self.last_seen = at;
        self.quiet_since = None;
        Ok(())
    }

    /// Record that nothing has arrived for a while.
    ///
    /// Deliberately cannot change [`SessionState::lifecycle`]. A turn that has gone quiet is a turn runtrol
    /// knows nothing new about, and there is no honest state to move it to. Inventing a completion here would
    /// be the same lie as swallowing an error, pointed the other way.
    ///
    /// `const` is load bearing rather than decorative. Two of the lifecycle states own a `String`, so replacing
    /// the current one would have to drop it, and a `const fn` cannot drop. The signature therefore makes this
    /// function structurally incapable of moving the session, which is a stronger promise than the sentence
    /// above. Measured: adding the assignment does not compile until `const` is removed with it.
    pub const fn note_silence(&mut self, since: WallMs) {
        self.quiet_since = Some(since);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> WallMs {
        WallMs::from_millis(1_700_000_000_000)
    }

    fn turn(index: u32) -> TurnId {
        TurnId { epoch: 0, index }
    }

    /// Every state, for a test that has to cover the table rather than a sample of it.
    fn every_state() -> Vec<Lifecycle> {
        vec![
            Lifecycle::Detached,
            Lifecycle::Starting,
            Lifecycle::Idle,
            Lifecycle::Busy { turn: turn(0) },
            Lifecycle::Failed {
                at: now(),
                code: FailureCode::ChildExited,
                detail: "exit 1".to_owned(),
            },
            Lifecycle::Closed {
                at: now(),
                reason: CloseReason::Requested,
            },
        ]
    }

    /// Every observation.
    fn every_observation() -> Vec<Observed> {
        vec![
            Observed::Attaching,
            Observed::Attached,
            Observed::TurnStarted { turn: turn(0) },
            Observed::TurnEnded { turn: turn(0) },
            Observed::Detached,
            Observed::Failed {
                code: FailureCode::Protocol,
                detail: "bad frame".to_owned(),
            },
            Observed::Closed {
                reason: CloseReason::Killed,
            },
        ]
    }

    #[test]
    fn a_session_runs_through_its_ordinary_life() {
        let mut state = SessionState::new(now());
        assert_eq!(state.lifecycle(), &Lifecycle::Detached);

        state.observe(Observed::Attaching, now()).expect("binding");
        assert_eq!(state.lifecycle(), &Lifecycle::Starting);

        state.observe(Observed::Attached, now()).expect("bound");
        assert_eq!(state.lifecycle(), &Lifecycle::Idle);

        state
            .observe(Observed::TurnStarted { turn: turn(1) }, now())
            .expect("a turn begins");
        assert_eq!(state.lifecycle().turn(), Some(turn(1)));

        state
            .observe(Observed::TurnEnded { turn: turn(1) }, now())
            .expect("and ends");
        assert_eq!(state.lifecycle(), &Lifecycle::Idle);

        state.observe(Observed::Detached, now()).expect("let go");
        assert_eq!(state.lifecycle(), &Lifecycle::Detached);
    }

    #[test]
    fn a_driver_cannot_end_a_turn_that_is_not_the_one_running() {
        // Otherwise a stale completion ends the live turn, and the operator watches work that is still going
        // report itself finished.
        let running = Lifecycle::Busy { turn: turn(7) };
        let refusal = running
            .after(Observed::TurnEnded { turn: turn(6) }, now())
            .expect_err("a different turn's ending must be refused");
        assert!(refusal.to_string().contains("different turn"), "{refusal}");

        assert_eq!(
            running
                .after(Observed::TurnEnded { turn: turn(7) }, now())
                .expect("the running turn may end"),
            Lifecycle::Idle
        );
    }

    #[test]
    fn a_turn_cannot_start_on_a_session_nothing_is_bound_to() {
        for state in [
            Lifecycle::Detached,
            Lifecycle::Failed {
                at: now(),
                code: FailureCode::CannotStart,
                detail: "not installed".to_owned(),
            },
        ] {
            assert!(
                state
                    .after(Observed::TurnStarted { turn: turn(0) }, now())
                    .is_err(),
                "{} accepted a turn",
                state.name()
            );
        }
    }

    #[test]
    fn a_failure_is_visible_and_still_resumable() {
        // The conversation is not runtrol's to lose. The provider's own store still has it, so what failed is
        // the attachment and the operator can pick it back up.
        let failed = Lifecycle::Idle
            .after(
                Observed::Failed {
                    code: FailureCode::ChildExited,
                    detail: "exit 1".to_owned(),
                },
                now(),
            )
            .expect("a failure is always possible");

        assert!(failed.can_resume());
        assert!(!failed.is_final());
        assert!(!failed.is_attached());
        assert_eq!(
            failed
                .after(Observed::Attaching, now())
                .expect("a failed session may be picked back up"),
            Lifecycle::Starting
        );
    }

    #[test]
    fn a_failure_can_happen_at_any_point_before_the_end() {
        for state in every_state() {
            let result = state.after(
                Observed::Failed {
                    code: FailureCode::HostGone,
                    detail: "gone".to_owned(),
                },
                now(),
            );
            if state.is_final() {
                assert!(result.is_err(), "{} accepted a change", state.name());
            } else {
                assert!(
                    matches!(result, Ok(Lifecycle::Failed { .. })),
                    "{} refused a failure it has to accept",
                    state.name()
                );
            }
        }
    }

    #[test]
    fn an_operator_can_stop_a_session_without_waiting_for_it_to_finish() {
        let busy = Lifecycle::Busy { turn: turn(3) };
        let closed = busy
            .after(
                Observed::Closed {
                    reason: CloseReason::Killed,
                },
                now(),
            )
            .expect("stopping mid-turn is the point");
        assert!(closed.is_final());
        assert!(!closed.can_resume());
    }

    #[test]
    fn nothing_leaves_a_session_that_was_ended_on_purpose() {
        let closed = Lifecycle::Closed {
            at: now(),
            reason: CloseReason::Deleted,
        };
        for observed in every_observation() {
            assert!(
                closed.after(observed.clone(), now()).is_err(),
                "a closed session accepted {observed:?}"
            );
        }
    }

    #[test]
    fn every_pair_of_state_and_observation_is_decided() {
        // The table is exhaustive by construction, and this says so out loud: no pair produces a panic, and
        // every one either moves somewhere or refuses with a sentence.
        for state in every_state() {
            for observed in every_observation() {
                match state.after(observed.clone(), now()) {
                    Ok(next) => assert!(!next.name().is_empty()),
                    Err(refusal) => {
                        assert!(!refusal.from.is_empty());
                        assert!(!refusal.observed.is_empty());
                        assert!(refusal.to_string().contains(state.name()), "{refusal}");
                    }
                }
            }
        }
    }

    #[test]
    fn silence_never_ends_a_turn() {
        // The rule this whole module is arranged around. runtrol does not know whether a quiet turn is
        // finished, so it does not decide. Showing "this looks stuck" is honest; showing a completion is not.
        let mut state = SessionState::new(now());
        state.observe(Observed::Attaching, now()).expect("binding");
        state.observe(Observed::Attached, now()).expect("bound");
        state
            .observe(Observed::TurnStarted { turn: turn(1) }, now())
            .expect("a turn begins");

        let before = state.lifecycle().clone();
        state.note_silence(now());

        assert_eq!(
            state.lifecycle(),
            &before,
            "noting silence must not move the session"
        );
        assert_eq!(
            state.lifecycle().turn(),
            Some(turn(1)),
            "the turn is running"
        );
        assert!(
            state.looks_stuck(),
            "and the operator can see it looks stuck"
        );
    }

    #[test]
    fn anything_arriving_is_evidence_of_life() {
        let mut state = SessionState::new(now());
        state.observe(Observed::Attaching, now()).expect("binding");
        state.note_silence(now());
        assert!(state.looks_stuck());

        state.observe(Observed::Attached, now()).expect("bound");
        assert!(
            !state.looks_stuck(),
            "a frame arriving means it was not stuck after all"
        );
        assert_eq!(state.quiet_since(), None);
    }

    #[test]
    fn a_refused_observation_leaves_the_session_exactly_as_it_was() {
        // A misbehaving driver must not be able to damage the state by reporting nonsense.
        let mut state = SessionState::new(now());
        state.observe(Observed::Attaching, now()).expect("binding");
        let before = state.clone();

        let refusal = state
            .observe(Observed::TurnEnded { turn: turn(1) }, now())
            .expect_err("a starting session has no turn to end");
        assert!(!refusal.to_string().is_empty());
        assert_eq!(state, before);
    }

    #[test]
    fn every_failure_reads_as_a_sentence_about_something_that_happened() {
        for code in [
            FailureCode::CannotStart,
            FailureCode::ChildExited,
            FailureCode::Protocol,
            FailureCode::HostGone,
            FailureCode::Superseded,
        ] {
            let said = code.to_string();
            assert!(!said.is_empty(), "{code:?} says nothing");
            assert!(
                !said.contains("succeed") && !said.contains("failed to"),
                "{said:?} draws a conclusion instead of reporting an observation"
            );
        }
    }
}
