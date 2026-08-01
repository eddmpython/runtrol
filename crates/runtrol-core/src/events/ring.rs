//! The replay ring: a latency window, not a history.
//!
//! A subscriber whose connection drops for a moment should not have to go anywhere to catch up. That is
//! what this holds: the last few frames, so a reconnect inside the window is served from memory.
//!
//! Anything older is served from the provider's own store, by positioned read. That is not a compromise,
//! it is measured: a positioned 64 KiB read costs 1.1 ms and 64 KiB, while materializing the same
//! transcript costs 173 ms and 145 MB. The ring is small **because** the provider's file is the record and
//! runtrol keeps no copy of it. Thinness buying a bound, at the one place a slow reader could otherwise
//! make the daemon grow without limit.
//!
//! # The two bounds, and why one is not enough
//!
//! [`RING_FRAMES`] bounds the envelopes. [`RING_BYTES`] bounds the payloads. Either alone leaves a hole:
//! sixty-four large frames would blow the byte budget, while a byte budget alone would admit thousands of
//! tiny frames whose fixed-width envelopes cost more than the payloads they carry.
//!
//! Together they fit the memory contract's hot-session line. Sixty-four envelopes at 128 bytes is 8 KiB,
//! plus 64 KiB of payload, is 72 KiB inside a 128 KiB budget, and the arithmetic is asserted below rather
//! than left for a reader to redo.
//!
//! # Why the ring is emptied on a reattach
//!
//! Positions restart at zero in a new epoch, so a ring holding two epochs could not answer "what came
//! after position 12" at all. A subscriber that reconnects across a reattach is served from the provider's
//! store, which is what the attach frame's replay source is for.

use std::collections::VecDeque;

use runtrol_provider::AgentEvent;

/// How many frames the ring holds.
///
/// Sixty-four, which is the same number the fan-out queue uses, for the same reason: it is the depth at
/// which a reader that is merely busy catches up and a reader that has stopped is distinguishable from
/// one that has not.
pub const RING_FRAMES: usize = 64;

/// How many bytes of provider payload the ring holds.
///
/// The measured size of one positioned read, so the window in memory and the read that replaces it are
/// the same size. Making the window larger would buy a longer reconnect grace at the cost of the very
/// budget that lets the daemon sit unnoticed all day.
pub const RING_BYTES: usize = 64 * 1024;

/// What the ring can do about a subscriber's position.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Reach {
    /// The subscriber has everything the ring has.
    UpToDate,
    /// The ring holds every frame after that position.
    Held,
    /// The gap begins before the ring does.
    ///
    /// The subscriber reads the provider's own store from this cursor to close it. The frames the ring
    /// still holds pick up from there.
    Gap {
        /// The cursor to resume reading the provider's store from.
        resume_from: u64,
    },
}

/// The last few frames of one session, bounded twice.
#[derive(Clone, Debug, Default)]
pub struct ReplayRing {
    /// Oldest first.
    frames: VecDeque<AgentEvent>,
    /// Provider payload bytes currently held, kept as a running total rather than summed on demand.
    bytes: usize,
}

impl ReplayRing {
    /// An empty ring.
    #[must_use]
    pub fn new() -> Self {
        Self {
            frames: VecDeque::with_capacity(RING_FRAMES),
            bytes: 0,
        }
    }

    /// Add a frame, evicting from the old end until both bounds hold again.
    ///
    /// The newest frame is always kept, even when it alone exceeds the byte budget. A ring that could
    /// refuse the latest frame would answer "you are up to date" while holding nothing, and the worst case
    /// is bounded anyway: a single frame cannot exceed the transport's own maximum line length, which the
    /// driver's framing enforces before a frame ever reaches here.
    pub fn push(&mut self, event: AgentEvent) {
        self.bytes = self.bytes.saturating_add(event.body.payload_bytes());
        self.frames.push_back(event);

        while self.frames.len() > RING_FRAMES || (self.bytes > RING_BYTES && self.frames.len() > 1)
        {
            match self.frames.pop_front() {
                Some(evicted) => {
                    self.bytes = self.bytes.saturating_sub(evicted.body.payload_bytes());
                }
                // The loop condition requires at least two frames, so the queue cannot be empty here.
                // Breaking rather than asserting keeps a supervisor supervising.
                None => break,
            }
        }
    }

    /// Forget everything, because positions are about to restart.
    pub fn clear(&mut self) {
        self.frames.clear();
        self.bytes = 0;
    }

    /// What can be done for a subscriber that last saw `after`.
    #[must_use]
    pub fn reach(&self, after: u64) -> Reach {
        let Some(oldest) = self.frames.front() else {
            // Nothing here. A subscriber cannot be behind a ring that holds nothing, and a caller that
            // wants a resume point for an empty ring is asking about a session with no frames yet.
            return Reach::UpToDate;
        };

        match self.frames.back() {
            Some(newest) if newest.seq <= after => Reach::UpToDate,
            // The frame the subscriber needs next is the one after `after`. It is here only if the oldest
            // frame is that one or earlier.
            _ if oldest.seq <= after.saturating_add(1) => Reach::Held,
            _ => Reach::Gap {
                resume_from: oldest.src_end,
            },
        }
    }

    /// Every frame still held, oldest first.
    ///
    /// Cloning these frames shares their provider payload allocations. A caller gets a bounded view without
    /// materializing a transcript or copying its content bytes.
    pub fn frames(&self) -> impl Iterator<Item = &AgentEvent> {
        self.frames.iter()
    }

    /// The frames after `after`, oldest first.
    ///
    /// Yields only what the ring still holds. Whether that is the whole gap is [`ReplayRing::reach`]'s
    /// question, and a caller that skips asking it serves a silent hole.
    pub fn frames_after(&self, after: u64) -> impl Iterator<Item = &AgentEvent> {
        self.frames.iter().filter(move |frame| frame.seq > after)
    }

    /// The oldest position still held.
    #[must_use]
    pub fn oldest_seq(&self) -> Option<u64> {
        self.frames.front().map(|frame| frame.seq)
    }

    /// The newest position held.
    #[must_use]
    pub fn newest_seq(&self) -> Option<u64> {
        self.frames.back().map(|frame| frame.seq)
    }

    /// How many frames are held.
    #[must_use]
    pub fn len(&self) -> usize {
        self.frames.len()
    }

    /// Whether the ring is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    /// How many bytes of provider payload are held.
    #[must_use]
    pub const fn payload_bytes(&self) -> usize {
        self.bytes
    }
}

#[cfg(test)]
mod tests {
    use runtrol_provider::{EventBody, Opaque, SessionId};

    use super::*;
    use crate::events::seq::Sequencer;

    /// A frame whose payload is a given number of bytes.
    fn sized(payload: usize) -> EventBody {
        EventBody::Plan {
            payload: Opaque::owned("x".repeat(payload)),
        }
    }

    /// A ring filled through the sequencer, so positions are the real ones.
    fn filled(count: usize, payload: usize) -> ReplayRing {
        let session = SessionId::now();
        let mut seq = Sequencer::new();
        let mut ring = ReplayRing::new();
        for index in 0..count {
            let cursor = (u64::try_from(index).expect("a test pushes few frames") + 1) * 100;
            ring.push(seq.stamp(session, cursor, sized(payload)).0);
        }
        ring
    }

    #[test]
    fn the_two_bounds_fit_the_hot_session_budget() {
        // The contract's per-session line is 128 KiB. If this arithmetic stops holding, the ring has
        // quietly taken the budget of the thing it exists to serve.
        const HOT_SESSION_BUDGET: usize = 128 * 1024;
        let envelopes = RING_FRAMES * size_of::<AgentEvent>();
        assert!(
            envelopes + RING_BYTES <= HOT_SESSION_BUDGET,
            "{envelopes} bytes of envelope plus {RING_BYTES} of payload exceeds {HOT_SESSION_BUDGET}"
        );
    }

    #[test]
    fn the_frame_bound_holds_however_small_the_frames_are() {
        // A byte budget alone would admit thousands of tiny frames whose envelopes cost more than their
        // payloads.
        let ring = filled(RING_FRAMES * 4, 1);
        assert_eq!(ring.len(), RING_FRAMES);
        assert!(ring.payload_bytes() <= RING_BYTES);
    }

    #[test]
    fn the_byte_bound_holds_however_few_the_frames_are() {
        // A frame budget alone would admit sixty-four large frames and blow the memory contract.
        let ring = filled(8, RING_BYTES / 4);
        assert!(
            ring.payload_bytes() <= RING_BYTES,
            "{} bytes held",
            ring.payload_bytes()
        );
        assert!(ring.len() < 8, "older frames should have been evicted");
    }

    #[test]
    fn a_single_frame_over_budget_is_still_served() {
        // Refusing it would leave the ring reporting that a subscriber is up to date while holding
        // nothing. The worst case is one frame, which the transport's line bound already limits.
        let ring = filled(1, RING_BYTES * 2);
        assert_eq!(ring.len(), 1);
        assert!(ring.payload_bytes() > RING_BYTES);
    }

    #[test]
    fn eviction_keeps_the_running_byte_total_honest() {
        // The total is incremental, so an eviction that forgot to subtract would leave the ring believing
        // it is permanently full and serving one frame at a time forever.
        let ring = filled(RING_FRAMES * 2, 4);
        let held: usize = ring
            .frames
            .iter()
            .map(|frame| frame.body.payload_bytes())
            .sum();
        assert!(held > 0, "the ring should be holding something");
        assert_eq!(ring.payload_bytes(), held);
    }

    #[test]
    fn a_reader_inside_the_window_is_served_from_memory() {
        let ring = filled(4, 8);
        assert_eq!(ring.reach(1), Reach::Held);
        let served: Vec<u64> = ring.frames_after(1).map(|frame| frame.seq).collect();
        assert_eq!(served, vec![2, 3]);
    }

    #[test]
    fn a_reader_at_the_newest_frame_is_told_there_is_nothing_to_do() {
        let ring = filled(4, 8);
        let newest = ring.newest_seq().expect("frames were pushed");
        assert_eq!(ring.reach(newest), Reach::UpToDate);
        assert_eq!(ring.frames_after(newest).count(), 0);
    }

    #[test]
    fn a_reader_older_than_the_window_is_sent_to_the_providers_store() {
        // The ring is a latency window. Past it, the answer is a cursor into the provider's own file,
        // because runtrol has no copy to offer.
        let ring = filled(RING_FRAMES * 2, 4);
        let oldest = ring.oldest_seq().expect("frames were pushed");
        match ring.reach(0) {
            Reach::Gap { resume_from } => {
                let first = ring
                    .frames_after(oldest - 1)
                    .next()
                    .expect("the oldest frame is held");
                assert_eq!(
                    resume_from, first.src_end,
                    "the resume point is where the ring's own oldest frame begins"
                );
            }
            other => panic!("expected a gap, got {other:?}"),
        }
    }

    #[test]
    fn the_frame_immediately_before_the_window_still_counts_as_held() {
        // Off by one here would send a subscriber that lost exactly nothing to the provider's file, on
        // every single reconnect.
        let ring = filled(RING_FRAMES + 3, 4);
        let oldest = ring.oldest_seq().expect("frames were pushed");
        assert!(oldest >= 2, "the fixture must leave room to look behind it");
        assert_eq!(
            ring.reach(oldest - 1),
            Reach::Held,
            "the next frame this subscriber needs is the oldest one held"
        );
        assert!(matches!(ring.reach(oldest - 2), Reach::Gap { .. }));
    }

    #[test]
    fn an_empty_ring_asks_nobody_to_recover_anything() {
        let ring = ReplayRing::new();
        assert!(ring.is_empty());
        assert_eq!(ring.reach(0), Reach::UpToDate);
        assert_eq!(ring.oldest_seq(), None);
        assert_eq!(ring.payload_bytes(), 0);
    }

    #[test]
    fn clearing_forgets_positions_that_are_about_to_restart() {
        let mut ring = filled(4, 8);
        ring.clear();
        assert!(ring.is_empty());
        assert_eq!(ring.payload_bytes(), 0);
    }
}
