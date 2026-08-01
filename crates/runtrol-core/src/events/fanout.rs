//! Handing one frame to every watcher, with a bound that cannot be exceeded.
//!
//! A phone on a slow link, a terminal that stopped reading, a window whose process was suspended: each is
//! a reader that accepts frames more slowly than a session produces them. Without a bound, the daemon's
//! memory is decided by the slowest reader on the network.
//!
//! # What overflows, and what is thrown away
//!
//! **The subscriber's position is thrown away. Its data is not.** When a queue fills, that subscriber gets
//! one frame saying where it had reached and which cursor to resume the provider's own store from, and its
//! queue is not grown by one byte.
//!
//! This is only safe because runtrol does not own the data. The provider's transcript is the record, so a
//! dropped position costs a positioned read and costs no content at all. It is the clearest place in the
//! product where being thin buys correctness rather than costing it: a design that kept its own copy would
//! have to choose between dropping content and running out of memory.
//!
//! # Why the bound is two numbers and one reserved slot
//!
//! [`QUEUE_FRAMES`] bounds the envelopes and [`QUEUE_BYTES`] bounds the payloads, for the same reason the
//! replay ring needs both. The channel is built one slot deeper than the frame bound, and that slot is
//! never used for data: it is where the lag frame goes. Without it, the one frame that has to reach a
//! stalled subscriber would be the one frame there is no room for.
//!
//! # Where the numbers sit against the memory contract
//!
//! The contract budgets 20 KiB per subscriber in steady state, which is what a reader keeping up actually
//! holds. These bounds are the ceiling for one that has stopped: 64 envelopes plus 256 KiB of payload, or
//! about 264 KiB. Four stalled subscribers is roughly a megabyte, which the contract's "eight hot sessions
//! and four subscribers at 18 MB" line absorbs. The test below holds that arithmetic.
//!
//! # Cloning a frame is not copying a payload
//!
//! Every subscriber gets its own [`AgentEvent`], and the payload inside is a shared buffer. Fanning one
//! frame out to a dozen watchers costs a dozen refcount bumps and zero payload bytes, which is what makes
//! a per-subscriber queue affordable in the first place.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use runtrol_provider::{AgentEvent, EventBody};
use tokio::sync::mpsc;

/// How many frames one subscriber may have waiting.
pub const QUEUE_FRAMES: usize = 64;

/// How many bytes of provider payload one subscriber may have waiting.
pub const QUEUE_BYTES: usize = 256 * 1024;

/// Which subscriber, for reporting and for tests. Never leaves the process.
/// Two subscriptions never share one, and the number is never reused within a session.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct SubscriberId(u64);

/// What one publish did.
///
/// Returned rather than logged. The caller decides what a person sees, and a caller that ignores this is
/// visibly ignoring a value rather than invisibly ignoring a log line.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Delivery {
    /// Subscribers the frame reached.
    pub delivered: usize,
    /// Subscribers told they had fallen behind, on this frame.
    pub lagged: usize,
    /// Subscribers that were already behind and stayed behind.
    pub still_behind: usize,
    /// Subscribers whose receiver was gone, and which were removed.
    pub departed: usize,
}

/// The receiving end of a subscription.
///
/// Dropping it is how a subscriber unsubscribes. The fan-out notices on its next publish and stops
/// accounting for it.
#[derive(Debug)]
pub struct Subscription {
    /// Which subscriber this is.
    id: SubscriberId,
    /// The queue.
    rx: mpsc::Receiver<AgentEvent>,
    /// Payload bytes waiting, shared with the sending side.
    ///
    /// The sender adds when a frame goes in and this side subtracts when one comes out, so the budget
    /// reflects what is actually held rather than what was ever sent.
    queued: Arc<AtomicUsize>,
}

impl Subscription {
    /// Which subscriber this is.
    #[must_use]
    pub const fn id(&self) -> SubscriberId {
        self.id
    }

    /// Wait for the next frame, or `None` once the session is gone.
    pub async fn recv(&mut self) -> Option<AgentEvent> {
        let event = self.rx.recv().await?;
        self.release(&event);
        Some(event)
    }

    /// Take a frame if one is waiting.
    ///
    /// The bounds are exercised through this, which is why this module needs no runtime to prove they hold.
    pub fn try_recv(&mut self) -> Option<AgentEvent> {
        match self.rx.try_recv() {
            Ok(event) => {
                self.release(&event);
                Some(event)
            }
            // Empty means nothing is waiting; disconnected means nothing ever will be again. Neither is a
            // failure being discarded: both are the answer to a poll. A caller that needs to tell them
            // apart awaits `recv`, which returns `None` only for the second.
            Err(mpsc::error::TryRecvError::Empty | mpsc::error::TryRecvError::Disconnected) => None,
        }
    }

    /// How many payload bytes are still waiting.
    #[must_use]
    pub fn queued_bytes(&self) -> usize {
        self.queued.load(Ordering::Acquire)
    }

    /// Give back the budget a frame was holding.
    fn release(&self, event: &AgentEvent) {
        self.queued.fetch_sub(frame_bytes(event), Ordering::AcqRel);
    }
}

/// One watcher, from the sending side.
///
/// Holds no identifier. A watcher is removed when its receiving end is dropped, so nothing here ever needs
/// to name one, and a field nothing reads is the debt this repository does not carry.
#[derive(Debug)]
struct Subscriber {
    /// The queue.
    tx: mpsc::Sender<AgentEvent>,
    /// Payload bytes waiting, shared with the receiving side.
    queued: Arc<AtomicUsize>,
    /// The last position that got in.
    last_delivered_seq: u64,
    /// The cursor of the last frame that got in, which is where recovery starts.
    last_delivered_src_end: u64,
    /// This subscriber has been told it fell behind and has not caught up yet.
    behind: bool,
}

/// Every watcher of one session.
#[derive(Debug, Default)]
pub struct FanOut {
    /// The watchers, in the order they subscribed.
    subscribers: Vec<Subscriber>,
    /// The next identifier to hand out.
    next_id: u64,
}

impl FanOut {
    /// A fan-out with no subscribers.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            subscribers: Vec::new(),
            next_id: 0,
        }
    }

    /// Add a watcher and hand back its receiving end.
    pub fn subscribe(&mut self) -> Subscription {
        // One slot deeper than the frame bound. The extra slot is never used for data, so there is always
        // room for the frame that tells a stalled subscriber it stalled.
        let (tx, rx) = mpsc::channel(QUEUE_FRAMES + 1);
        let queued = Arc::new(AtomicUsize::new(0));
        let id = SubscriberId(self.next_id);
        self.next_id = self.next_id.wrapping_add(1);

        self.subscribers.push(Subscriber {
            tx,
            queued: Arc::clone(&queued),
            last_delivered_seq: 0,
            last_delivered_src_end: 0,
            behind: false,
        });

        Subscription { id, rx, queued }
    }

    /// How many watchers there are.
    #[must_use]
    pub fn len(&self) -> usize {
        self.subscribers.len()
    }

    /// Whether nobody is watching.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.subscribers.is_empty()
    }

    /// Give the frame to every watcher that can take it, and tell the rest they fell behind.
    pub fn publish(&mut self, event: &AgentEvent) -> Delivery {
        let mut report = Delivery::default();

        self.subscribers.retain_mut(|subscriber| {
            if subscriber.tx.is_closed() {
                report.departed += 1;
                return false;
            }
            match subscriber.offer(event) {
                Offered::Took => report.delivered += 1,
                Offered::FellBehind => report.lagged += 1,
                Offered::StillBehind => report.still_behind += 1,
            }
            true
        });

        report
    }
}

/// What one subscriber did with one frame.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Offered {
    /// It went into the queue.
    Took,
    /// The queue was full, and the subscriber was told so on this frame.
    FellBehind,
    /// The subscriber was already behind and has not drained yet.
    StillBehind,
}

impl Subscriber {
    /// Offer one frame, within the bounds.
    fn offer(&mut self, event: &AgentEvent) -> Offered {
        // A subscriber that has drained what it was given is caught up again, whatever it missed. What it
        // missed is the provider's file's business now, and the lag frame told it where to look.
        if self.behind && self.is_drained() {
            self.behind = false;
        }
        if self.behind {
            return Offered::StillBehind;
        }

        if self.has_room_for(event) {
            match self.tx.try_send(event.clone()) {
                Ok(()) => {
                    self.queued.fetch_add(frame_bytes(event), Ordering::AcqRel);
                    self.last_delivered_seq = event.seq;
                    self.last_delivered_src_end = event.src_end;
                    return Offered::Took;
                }
                // The receiving end went away between the closed check and here, or the channel filled
                // from another publish. Either way this subscriber did not get the frame, and saying so is
                // the same answer as any other overflow.
                Err(_) => return self.fall_behind(event),
            }
        }
        self.fall_behind(event)
    }

    /// Whether the frame fits inside both bounds.
    fn has_room_for(&self, event: &AgentEvent) -> bool {
        // Capacity counts the reserved slot, which data may not use.
        let used_frames = self.tx.max_capacity().saturating_sub(self.tx.capacity());
        let waiting_bytes = self.queued.load(Ordering::Acquire);
        used_frames < QUEUE_FRAMES
            && waiting_bytes.saturating_add(frame_bytes(event)) <= QUEUE_BYTES
    }

    /// Whether everything this subscriber was given has been taken.
    fn is_drained(&self) -> bool {
        self.tx.capacity() == self.tx.max_capacity()
    }

    /// Tell the subscriber where it had reached, and stop growing its queue.
    ///
    /// The lag frame stands in the place of the frame that did not fit, so it carries that frame's
    /// position: the subscriber then knows the gap runs from what it last received up to here.
    fn fall_behind(&mut self, event: &AgentEvent) -> Offered {
        let notice = AgentEvent {
            session: event.session,
            epoch: event.epoch,
            seq: event.seq,
            at: event.at,
            src_end: event.src_end,
            body: EventBody::Lagged {
                last_delivered_seq: self.last_delivered_seq,
                resume_from: self.last_delivered_src_end,
            },
        };
        self.behind = true;

        match self.tx.try_send(notice) {
            // The reserved slot took it, and it carries no payload, so no budget is spent.
            Ok(()) => Offered::FellBehind,
            // The receiver is gone, or has not drained since the last lag frame. In both cases it already
            // has everything it needs: either it is not listening, or the frame telling it to recover is
            // still sitting in its queue unread.
            Err(_) => Offered::StillBehind,
        }
    }
}

/// How much budget one frame occupies.
///
/// Defined once, because the sending side adds it and the receiving side subtracts it. Two spellings of
/// this would drift, and the drift would show up as a queue that believes it is permanently full.
fn frame_bytes(event: &AgentEvent) -> usize {
    event.body.payload_bytes()
}

#[cfg(test)]
mod tests {
    use runtrol_provider::{Opaque, SessionId};

    use super::*;
    use crate::events::seq::Sequencer;

    /// A publisher that stamps real positions, so lag reports carry real numbers.
    struct Source {
        session: SessionId,
        seq: Sequencer,
    }

    impl Source {
        fn new() -> Self {
            Self {
                session: SessionId::now(),
                seq: Sequencer::new(),
            }
        }

        fn frame(&mut self, payload: usize) -> AgentEvent {
            let cursor = (self.seq.next_seq() + 1) * 1_000;
            self.seq
                .stamp(
                    self.session,
                    cursor,
                    EventBody::Plan {
                        payload: Opaque::owned("x".repeat(payload)),
                    },
                )
                .0
        }
    }

    #[test]
    fn the_ceiling_for_stalled_subscribers_fits_the_memory_contract() {
        // The contract's "eight hot sessions and four subscribers" line is 18 MB. Four subscribers that
        // have all stopped reading have to fit inside it with room to spare, or the bound is decorative.
        const FOUR_SUBSCRIBERS_LINE: usize = 18 * 1024 * 1024;
        let ceiling = 4 * (QUEUE_FRAMES * size_of::<AgentEvent>() + QUEUE_BYTES);
        assert!(
            ceiling < FOUR_SUBSCRIBERS_LINE / 2,
            "four stalled subscribers would hold {ceiling} bytes"
        );
    }

    #[test]
    fn every_watcher_gets_the_frame() {
        let mut source = Source::new();
        let mut fanout = FanOut::new();
        let mut first = fanout.subscribe();
        let mut second = fanout.subscribe();

        let event = source.frame(16);
        let report = fanout.publish(&event);

        assert_eq!(report.delivered, 2);
        assert_eq!(first.try_recv().map(|frame| frame.seq), Some(event.seq));
        assert_eq!(second.try_recv().map(|frame| frame.seq), Some(event.seq));
    }

    #[test]
    fn a_payload_is_shared_and_not_copied_per_watcher() {
        // The reason a queue per subscriber is affordable at all. If this ever stops holding, fanning out
        // to a dozen watchers starts costing a dozen copies of every message.
        let mut source = Source::new();
        let mut fanout = FanOut::new();
        let mut first = fanout.subscribe();
        let mut second = fanout.subscribe();

        fanout.publish(&source.frame(4096));
        let one = first.try_recv().expect("delivered");
        let other = second.try_recv().expect("delivered");

        let (EventBody::Plan { payload: left }, EventBody::Plan { payload: right }) =
            (&one.body, &other.body)
        else {
            panic!("the fixture publishes a plan frame");
        };
        assert_eq!(
            left.bytes().as_ptr(),
            right.bytes().as_ptr(),
            "two watchers must share one buffer"
        );
    }

    #[test]
    fn a_subscriber_that_stops_reading_is_bounded_by_frames() {
        let mut source = Source::new();
        let mut fanout = FanOut::new();
        let subscription = fanout.subscribe();

        // Tiny payloads, so the frame bound is the one that has to bite.
        let mut lagged = 0;
        for _ in 0..(QUEUE_FRAMES * 3) {
            lagged += fanout.publish(&source.frame(1)).lagged;
        }

        assert_eq!(
            lagged, 1,
            "a subscriber is told it fell behind exactly once"
        );
        assert!(
            subscription.queued_bytes() <= QUEUE_BYTES,
            "{} bytes waiting",
            subscription.queued_bytes()
        );
    }

    #[test]
    fn a_subscriber_that_stops_reading_is_bounded_by_bytes() {
        // Few frames, each large. A frame bound alone would let this grow to sixty-four large payloads.
        let mut source = Source::new();
        let mut fanout = FanOut::new();
        let subscription = fanout.subscribe();

        for _ in 0..16 {
            fanout.publish(&source.frame(QUEUE_BYTES / 4));
        }

        assert!(
            subscription.queued_bytes() <= QUEUE_BYTES,
            "{} bytes waiting, budget is {QUEUE_BYTES}",
            subscription.queued_bytes()
        );
    }

    #[test]
    fn the_lag_frame_says_where_the_gap_starts_and_where_to_recover_from() {
        // This frame is the entire reason dropping a position is safe. Without both numbers the subscriber
        // knows only that it lost something, which is a silent hole with extra steps.
        let mut source = Source::new();
        let mut fanout = FanOut::new();
        let mut subscription = fanout.subscribe();

        let mut last_taken = None;
        for _ in 0..(QUEUE_FRAMES * 2) {
            let event = source.frame(1);
            if fanout.publish(&event).delivered == 1 {
                last_taken = Some(event);
            }
        }
        let last_taken = last_taken.expect("early frames must have been delivered");

        let mut frames = Vec::new();
        while let Some(frame) = subscription.try_recv() {
            frames.push(frame);
        }
        let lag = frames
            .iter()
            .find(|frame| matches!(frame.body, EventBody::Lagged { .. }))
            .expect("the subscriber must be told it fell behind");

        match lag.body {
            EventBody::Lagged {
                last_delivered_seq,
                resume_from,
            } => {
                assert_eq!(
                    last_delivered_seq, last_taken.seq,
                    "the gap starts after the last frame that actually arrived"
                );
                assert_eq!(
                    resume_from, last_taken.src_end,
                    "recovery starts at that frame's cursor in the provider's own store"
                );
            }
            ref other => panic!("expected lag, got {other:?}"),
        }
        assert_eq!(lag.session, last_taken.session);
    }

    #[test]
    fn a_subscriber_that_catches_up_starts_receiving_again() {
        // Falling behind is a moment, not a sentence. A subscriber that drains has to come back, or a
        // phone that was briefly on a bad link never recovers without reconnecting.
        let mut source = Source::new();
        let mut fanout = FanOut::new();
        let mut subscription = fanout.subscribe();

        for _ in 0..(QUEUE_FRAMES * 2) {
            fanout.publish(&source.frame(1));
        }
        while subscription.try_recv().is_some() {}
        assert_eq!(subscription.queued_bytes(), 0);

        let resumed = source.frame(1);
        assert_eq!(fanout.publish(&resumed).delivered, 1);
        assert_eq!(
            subscription.try_recv().map(|frame| frame.seq),
            Some(resumed.seq)
        );
    }

    #[test]
    fn a_watcher_that_went_away_is_forgotten() {
        let mut source = Source::new();
        let mut fanout = FanOut::new();
        let subscription = fanout.subscribe();
        let mut staying = fanout.subscribe();
        drop(subscription);

        let report = fanout.publish(&source.frame(8));
        assert_eq!(report.departed, 1);
        assert_eq!(report.delivered, 1);
        assert_eq!(fanout.len(), 1);
        assert!(staying.try_recv().is_some());
    }

    #[test]
    fn publishing_with_nobody_watching_does_nothing_and_says_so() {
        let mut source = Source::new();
        let mut fanout = FanOut::new();
        assert!(fanout.is_empty());
        assert_eq!(fanout.publish(&source.frame(8)), Delivery::default());
    }

    #[tokio::test]
    async fn a_watcher_can_wait_for_the_next_frame() {
        // The path the daemon's socket writer takes. The synchronous tests carry the bounds; this one
        // carries the shape.
        let mut source = Source::new();
        let mut fanout = FanOut::new();
        let mut subscription = fanout.subscribe();

        let event = source.frame(8);
        fanout.publish(&event);
        let received = subscription.recv().await.expect("a frame was published");
        assert_eq!(received.seq, event.seq);
        assert_eq!(
            subscription.queued_bytes(),
            0,
            "waiting released the budget"
        );
    }
}
