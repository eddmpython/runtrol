//! Position numbering, and the source cursor that must never go backwards.
//!
//! Every frame leaving a session carries two numbers a subscriber depends on.
//!
//! `seq` is dense and gapless inside one attach. Density is what makes a gap detectable: a subscriber
//! that receives 7 after 5 knows something was lost, without runtrol having to tell it. Drivers do not
//! assign it, because one provider line can become three frames and two drivers can serve one session
//! across a reattach, and neither situation has an answer a driver could give.
//!
//! `src_end` is how far into the provider's own event source the session has reached. It is diagnostic ordering
//! metadata, so it has to be monotone: a cursor that went backwards would make two events claim an impossible
//! source order.
//!
//! # Why a regression is kept and reported rather than trusted or hidden
//!
//! A driver that reports a cursor behind one it already reported is misbehaving. Two wrong answers are
//! available. Trusting it breaks source ordering for every subscriber. Ignoring it silently means the operator's
//! session degrades with nothing anywhere saying why, and the driver's bug survives to production. So the
//! previous value is kept, and the regression comes back as a value the caller must handle.

use runtrol_provider::{AgentEvent, EventBody, SessionId, WallMs};

/// A driver reported a source cursor behind one it had already reported.
///
/// Returned rather than logged, so the caller decides what a person sees. The hub turns it into a notice,
/// which is what makes "never silently swallowed" mechanical rather than a habit.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CursorRegression {
    /// What the driver said this time.
    pub reported: u64,
    /// What was kept instead, being the highest value the driver had already reported.
    pub kept: u64,
}

/// Assigns positions for one session, and holds the cursor.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Sequencer {
    /// Which attach. Frames from two attaches are not comparable by position.
    epoch: u32,
    /// The position the next frame gets.
    next: u64,
    /// The highest cursor any frame in this epoch has carried.
    src_end: u64,
}

impl Sequencer {
    /// A sequencer for a session that has never been attached.
    ///
    /// Epoch zero exists so that a frame can be stamped before the first attach completes. A driver's own
    /// startup produces frames, and they belong to the session that is starting.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            epoch: 0,
            next: 0,
            src_end: 0,
        }
    }

    /// Which attach these positions belong to.
    #[must_use]
    pub const fn epoch(&self) -> u32 {
        self.epoch
    }

    /// The position the next frame will get.
    #[must_use]
    pub const fn next_seq(&self) -> u64 {
        self.next
    }

    /// How far into the provider's store this epoch has reached.
    #[must_use]
    pub const fn src_end(&self) -> u64 {
        self.src_end
    }

    /// Begin a new attach: a new epoch, positions from zero, no cursor history.
    ///
    /// The cursor resets because a new attach may legitimately begin anywhere in the provider's store, and
    /// comparing this epoch's first cursor against the last one would refuse a resume that is correct.
    ///
    /// The counter wraps rather than saturating or panicking. The epoch is compared for change and not for
    /// order (that is what its documentation in the event vocabulary promises), so wrapping keeps working,
    /// while a supervisor that aborted on arithmetic would take every other session down with it.
    pub const fn attach(&mut self) -> u32 {
        self.epoch = self.epoch.wrapping_add(1);
        self.next = 0;
        self.src_end = 0;
        self.epoch
    }

    /// Stamp a frame with its position, this instant, and the cursor.
    ///
    /// The timestamp is taken here, from runtrol's clock, and is never accepted from a caller. A
    /// provider's own timestamps are not monotone across a daemon restart, and this field is used for
    /// ordering.
    ///
    /// Returns the regression when the reported cursor was behind the one already reached, in which case
    /// the frame carries the value that was kept.
    pub fn stamp(
        &mut self,
        session: SessionId,
        reported_src_end: u64,
        body: EventBody,
    ) -> (AgentEvent, Option<CursorRegression>) {
        let regression = if reported_src_end < self.src_end {
            Some(CursorRegression {
                reported: reported_src_end,
                kept: self.src_end,
            })
        } else {
            self.src_end = reported_src_end;
            None
        };

        let event = AgentEvent::new(
            session,
            self.epoch,
            self.next,
            WallMs::now(),
            self.src_end,
            body,
        );

        // Unreachable at any rate a real provider produces: at a million frames a second this takes
        // longer than the species has existed. Wrapping is chosen over a panic for the same reason as
        // above, and over saturating because two frames sharing a position would be read as a duplicate
        // rather than as the broken state it is.
        self.next = self.next.wrapping_add(1);
        (event, regression)
    }
}

impl Default for Sequencer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use runtrol_provider::Opaque;

    use super::*;

    fn a_body() -> EventBody {
        EventBody::Plan {
            payload: Opaque::owned(r#"{"steps":[]}"#.to_owned()),
        }
    }

    #[test]
    fn positions_are_dense_so_a_gap_is_detectable() {
        // Density is the whole mechanism: a subscriber detects loss by arithmetic rather than by being
        // told, which is what lets the daemon drop a position without dropping correctness.
        let session = SessionId::now();
        let mut seq = Sequencer::new();

        let positions: Vec<u64> = (0..5)
            .map(|_| seq.stamp(session, 0, a_body()).0.seq)
            .collect();
        assert_eq!(positions, vec![0, 1, 2, 3, 4]);
        assert_eq!(seq.next_seq(), 5);
    }

    #[test]
    fn a_new_attach_starts_a_new_epoch_from_zero() {
        let session = SessionId::now();
        let mut seq = Sequencer::new();
        let first = seq.stamp(session, 100, a_body()).0;

        let epoch = seq.attach();
        let second = seq.stamp(session, 40, a_body()).0;

        assert_eq!(first.epoch, 0);
        assert_eq!(second.epoch, epoch);
        assert_ne!(second.epoch, first.epoch);
        assert_eq!(
            second.seq, 0,
            "positions restart, which is why the epoch moved"
        );
        assert_eq!(
            second.src_end, 40,
            "a new attach may legitimately resume from anywhere in the provider's store"
        );
    }

    #[test]
    fn the_cursor_rises_and_is_carried_on_every_frame() {
        let session = SessionId::now();
        let mut seq = Sequencer::new();

        for expected in [10_u64, 25, 25, 900] {
            let (event, regression) = seq.stamp(session, expected, a_body());
            assert_eq!(event.src_end, expected);
            assert_eq!(regression, None, "rising and equal are both fine");
        }
        assert_eq!(seq.src_end(), 900);
    }

    #[test]
    fn a_cursor_that_goes_backwards_is_kept_out_and_reported() {
        // Trusting it would serve the same provider bytes twice to every subscriber that recovers from
        // it. Ignoring it would hide a driver bug behind a session that merely degrades.
        let session = SessionId::now();
        let mut seq = Sequencer::new();
        seq.stamp(session, 500, a_body());

        let (event, regression) = seq.stamp(session, 120, a_body());
        assert_eq!(event.src_end, 500, "the frame carries the value that held");
        assert_eq!(
            regression,
            Some(CursorRegression {
                reported: 120,
                kept: 500,
            })
        );
        assert_eq!(seq.src_end(), 500);
    }

    #[test]
    fn the_timestamp_comes_from_runtrols_own_clock() {
        let session = SessionId::now();
        let mut seq = Sequencer::new();
        let before = WallMs::now();
        let event = seq.stamp(session, 0, a_body()).0;
        assert!(
            event.at >= before,
            "the frame was stamped before it was created"
        );
    }
}
