//! The replay ring: a latency window, not a history.
//!
//! A subscriber whose connection drops for a moment should not have to go anywhere to catch up. That is
//! what this holds: the last few frames, so a reconnect inside the window is served from memory.
//!
//! Anything older is reported as a visible gap. runtrol neither reads a provider transcript nor pretends it can fill
//! that gap. The provider remains the transcript owner, while this ring remains only a small latency window.
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
//! Positions restart at zero in a new epoch, so a ring holding two epochs could not answer which frame follows a
//! cursor. The watch acknowledgement reports that boundary change as a gap.

use std::collections::VecDeque;

use runtrol_provider::{AgentEvent, StreamId, WatchCursor};

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
    /// The requested boundary is outside the bounded window or crosses a deliberately unretained frame.
    Gap,
}

/// One bounded replay entry.
#[derive(Clone, Debug)]
enum RingEntry {
    Event(AgentEvent),
    /// A payload larger than the complete byte budget was relayed live but not retained here.
    Lost(WatchCursor),
}

/// The last few frames of one session, bounded twice.
#[derive(Clone, Debug, Default)]
pub struct ReplayRing {
    /// Oldest first.
    entries: VecDeque<RingEntry>,
    /// Provider payload bytes currently held, kept as a running total rather than summed on demand.
    bytes: usize,
}

impl ReplayRing {
    /// An empty ring.
    #[must_use]
    pub fn new() -> Self {
        Self {
            entries: VecDeque::with_capacity(RING_FRAMES),
            bytes: 0,
        }
    }

    /// Add a frame, evicting from the old end until both bounds hold again.
    ///
    /// A single payload larger than the byte budget is never retained. A fixed-width loss marker replaces it so a
    /// reconnect cannot mistake an empty ring for complete coverage.
    pub fn push(&mut self, stream: StreamId, event: AgentEvent) {
        let payload = event.body.payload_bytes();
        if payload > RING_BYTES {
            self.clear();
            self.entries.push_back(RingEntry::Lost(WatchCursor {
                stream,
                epoch: event.epoch,
                seq: event.seq,
            }));
            return;
        }

        self.bytes = self.bytes.saturating_add(payload);
        self.entries.push_back(RingEntry::Event(event));

        while self.entries.len() > RING_FRAMES || self.bytes > RING_BYTES {
            match self.entries.pop_front() {
                Some(RingEntry::Event(evicted)) => {
                    self.bytes = self.bytes.saturating_sub(evicted.body.payload_bytes());
                }
                Some(RingEntry::Lost(_)) => {}
                None => break,
            }
        }
    }

    /// Forget everything, because positions are about to restart.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.bytes = 0;
    }

    /// What can be done for a subscriber asking for `requested` at the current live boundary.
    #[must_use]
    pub fn reach(&self, requested: WatchCursor, live_at: WatchCursor) -> Reach {
        if requested.stream != live_at.stream
            || requested.epoch != live_at.epoch
            || requested.seq > live_at.seq
        {
            return Reach::Gap;
        }
        if requested.seq == live_at.seq {
            return Reach::UpToDate;
        }
        if self.entries.iter().any(|entry| {
            matches!(entry, RingEntry::Lost(lost) if lost.stream == requested.stream
                && lost.epoch == requested.epoch && lost.seq >= requested.seq)
        }) {
            return Reach::Gap;
        }

        match self.frames().next() {
            Some(oldest) if oldest.epoch == requested.epoch && oldest.seq <= requested.seq => {
                Reach::Held
            }
            Some(_) | None => Reach::Gap,
        }
    }

    /// Every frame still held, oldest first.
    ///
    /// Cloning these frames shares their provider payload allocations. A caller gets a bounded view without
    /// materializing a transcript or copying its content bytes.
    #[must_use]
    pub fn frames(&self) -> impl DoubleEndedIterator<Item = &AgentEvent> {
        self.entries.iter().filter_map(|entry| match entry {
            RingEntry::Event(event) => Some(event),
            RingEntry::Lost(_) => None,
        })
    }

    /// The frames after `after`, oldest first.
    ///
    /// Yields only what the ring still holds. Whether that is the whole gap is [`ReplayRing::reach`]'s
    /// question, and a caller that skips asking it serves a silent hole.
    pub fn frames_from(&self, cursor: WatchCursor) -> impl Iterator<Item = &AgentEvent> {
        self.frames()
            .filter(move |frame| frame.epoch == cursor.epoch && frame.seq >= cursor.seq)
    }

    /// The oldest position still held.
    #[must_use]
    pub fn oldest_seq(&self) -> Option<u64> {
        self.frames().next().map(|frame| frame.seq)
    }

    /// The newest position held.
    #[must_use]
    pub fn newest_seq(&self) -> Option<u64> {
        self.frames().next_back().map(|frame| frame.seq)
    }

    /// How many frames are held.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the ring is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// How many bytes of provider payload are held.
    #[must_use]
    pub const fn payload_bytes(&self) -> usize {
        self.bytes
    }
}

#[cfg(test)]
mod tests {
    use runtrol_provider::{EventBody, Opaque, SessionId, StreamId};

    use super::*;
    use crate::events::seq::Sequencer;

    /// A frame whose payload is a given number of bytes.
    fn sized(payload: usize) -> EventBody {
        EventBody::Plan {
            payload: Opaque::owned("x".repeat(payload)),
        }
    }

    struct Filled {
        ring: ReplayRing,
        stream: StreamId,
        live_at: WatchCursor,
    }

    /// A ring filled through the sequencer, so positions are the real ones.
    fn filled(count: usize, payload: usize) -> Filled {
        let session = SessionId::now();
        let stream = StreamId::now();
        let mut seq = Sequencer::new();
        let mut ring = ReplayRing::new();
        for index in 0..count {
            let cursor = (u64::try_from(index).expect("a test pushes few frames") + 1) * 100;
            ring.push(stream, seq.stamp(session, cursor, sized(payload)).0);
        }
        Filled {
            ring,
            stream,
            live_at: WatchCursor {
                stream,
                epoch: seq.epoch(),
                seq: seq.next_seq(),
            },
        }
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
        let ring = filled(RING_FRAMES * 4, 1).ring;
        assert_eq!(ring.len(), RING_FRAMES);
        assert!(ring.payload_bytes() <= RING_BYTES);
    }

    #[test]
    fn the_byte_bound_holds_however_few_the_frames_are() {
        // A frame budget alone would admit sixty-four large frames and blow the memory contract.
        let ring = filled(8, RING_BYTES / 4).ring;
        assert!(
            ring.payload_bytes() <= RING_BYTES,
            "{} bytes held",
            ring.payload_bytes()
        );
        assert!(ring.len() < 8, "older frames should have been evicted");
    }

    #[test]
    fn a_single_frame_over_budget_becomes_a_fixed_loss_marker() {
        let filled = filled(1, RING_BYTES * 2);
        let ring = filled.ring;
        assert_eq!(ring.len(), 1);
        assert_eq!(ring.payload_bytes(), 0);
        assert_eq!(
            ring.frames().count(),
            0,
            "the oversized payload is not retained"
        );
        assert_eq!(
            ring.reach(
                WatchCursor {
                    stream: filled.stream,
                    epoch: 0,
                    seq: 0,
                },
                filled.live_at,
            ),
            Reach::Gap
        );
    }

    #[test]
    fn a_payload_at_the_exact_byte_bound_is_retained() {
        let filled = filled(1, RING_BYTES);
        assert_eq!(filled.ring.payload_bytes(), RING_BYTES);
        assert_eq!(filled.ring.frames().count(), 1);
    }

    #[test]
    fn replay_resumes_immediately_after_an_oversize_barrier() {
        let session = SessionId::now();
        let stream = StreamId::now();
        let mut seq = Sequencer::new();
        let mut ring = ReplayRing::new();
        ring.push(stream, seq.stamp(session, 1, sized(RING_BYTES + 1)).0);
        ring.push(stream, seq.stamp(session, 2, sized(8)).0);
        let live_at = WatchCursor {
            stream,
            epoch: 0,
            seq: 2,
        };

        assert_eq!(
            ring.reach(WatchCursor { seq: 0, ..live_at }, live_at),
            Reach::Gap
        );
        let after_loss = WatchCursor { seq: 1, ..live_at };
        assert_eq!(ring.reach(after_loss, live_at), Reach::Held);
        assert_eq!(
            ring.frames_from(after_loss)
                .map(|event| event.seq)
                .collect::<Vec<_>>(),
            vec![1]
        );
    }

    #[test]
    fn eviction_keeps_the_running_byte_total_honest() {
        // The total is incremental, so an eviction that forgot to subtract would leave the ring believing
        // it is permanently full and serving one frame at a time forever.
        let ring = filled(RING_FRAMES * 2, 4).ring;
        let held: usize = ring.frames().map(|frame| frame.body.payload_bytes()).sum();
        assert!(held > 0, "the ring should be holding something");
        assert_eq!(ring.payload_bytes(), held);
    }

    #[test]
    fn a_reader_inside_the_window_is_served_from_memory() {
        let filled = filled(4, 8);
        let requested = WatchCursor {
            stream: filled.stream,
            epoch: 0,
            seq: 2,
        };
        assert_eq!(filled.ring.reach(requested, filled.live_at), Reach::Held);
        let served: Vec<u64> = filled
            .ring
            .frames_from(requested)
            .map(|frame| frame.seq)
            .collect();
        assert_eq!(served, vec![2, 3]);
    }

    #[test]
    fn a_reader_at_the_newest_frame_is_told_there_is_nothing_to_do() {
        let filled = filled(4, 8);
        assert_eq!(
            filled.ring.reach(filled.live_at, filled.live_at),
            Reach::UpToDate
        );
        assert_eq!(filled.ring.frames_from(filled.live_at).count(), 0);
    }

    #[test]
    fn a_reader_older_than_the_window_gets_an_explicit_gap() {
        let filled = filled(RING_FRAMES * 2, 4);
        let requested = WatchCursor {
            stream: filled.stream,
            epoch: 0,
            seq: 0,
        };
        assert_eq!(filled.ring.reach(requested, filled.live_at), Reach::Gap);
    }

    #[test]
    fn the_frame_immediately_before_the_window_still_counts_as_held() {
        // Off by one here would report a gap to a subscriber that lost exactly nothing on every reconnect.
        let filled = filled(RING_FRAMES + 3, 4);
        let oldest = filled.ring.oldest_seq().expect("frames were pushed");
        assert!(oldest >= 2, "the fixture must leave room to look behind it");
        let held = WatchCursor {
            stream: filled.stream,
            epoch: 0,
            seq: oldest,
        };
        assert_eq!(
            filled.ring.reach(held, filled.live_at),
            Reach::Held,
            "the oldest retained frame is a reachable next boundary"
        );
        assert_eq!(
            filled.ring.reach(
                WatchCursor {
                    seq: oldest - 1,
                    ..held
                },
                filled.live_at,
            ),
            Reach::Gap
        );
    }

    #[test]
    fn an_empty_ring_distinguishes_live_past_and_future_boundaries() {
        let ring = ReplayRing::new();
        let stream = StreamId::now();
        let live_at = WatchCursor {
            stream,
            epoch: 4,
            seq: 0,
        };
        assert!(ring.is_empty());
        assert_eq!(ring.reach(live_at, live_at), Reach::UpToDate);
        assert_eq!(
            ring.reach(
                WatchCursor {
                    stream: StreamId::now(),
                    ..live_at
                },
                live_at,
            ),
            Reach::Gap
        );
        assert_eq!(
            ring.reach(WatchCursor { seq: 1, ..live_at }, live_at),
            Reach::Gap
        );
        assert_eq!(ring.oldest_seq(), None);
        assert_eq!(ring.payload_bytes(), 0);
    }

    #[test]
    fn clearing_forgets_positions_that_are_about_to_restart() {
        let mut ring = filled(4, 8).ring;
        ring.clear();
        assert!(ring.is_empty());
        assert_eq!(ring.payload_bytes(), 0);
    }
}
