//! Handing one frame to every watcher, with a bound that cannot be exceeded.
//!
//! A phone on a slow link, a terminal that stopped reading, a window whose process was suspended: each is
//! a reader that accepts frames more slowly than a session produces them. Without a bound, the daemon's
//! memory is decided by the slowest reader on the network.
//!
//! # What overflows, and what is thrown away
//!
//! When a queue fills, that subscriber receives one frame naming the exact next event it needed and is retired. It
//! reconnects with that boundary. The bounded replay ring either fills the gap or the next acknowledgement reports
//! that it cannot. Delivery never resumes silently after skipped events, and runtrol never reads a transcript.
//!
//! # Why the bound is two numbers and one reserved slot
//!
//! [`QUEUE_FRAMES`] bounds the envelopes and [`QUEUE_BYTES`] bounds ordinary queued payloads, for the same reason the
//! replay ring needs both. One event may itself be larger than that byte budget, so an otherwise empty subscriber
//! may hold exactly one event up to [`MAX_LIVE_PAYLOAD_BYTES`]. The channel is built one slot deeper than the frame
//! bound, and that slot is
//! never used for data: it is where the lag frame goes. Without it, the one frame that has to reach a
//! stalled subscriber would be the one frame there is no room for.
//!
//! # Where the numbers sit against the memory contract
//!
//! The retained-byte counter includes both the normalized provider payload and the complete wire encoding. An
//! ordinary stalled subscriber therefore holds no more than 256 KiB. A caught-up subscriber may hold one larger
//! event, but never one whose provider payload exceeds 1 MiB. The live-process gate measures the resulting daemon
//! RSS against the platform's hard ceiling instead of relying on an estimate here.
//!
//! # Cloning a frame is not copying a payload
//!
//! Every subscriber gets its own [`AgentEvent`], and the payload inside is a shared buffer. Fanning one
//! frame out to a dozen watchers costs a dozen refcount bumps and zero payload bytes, which is what makes
//! a per-subscriber queue affordable in the first place.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use runtrol_provider::{AgentEvent, WatchCursor};
use tokio::sync::mpsc;

use super::WatchEvent;

/// How many frames one subscriber may have waiting.
pub const QUEUE_FRAMES: usize = 64;

/// How many retained event bytes one subscriber may have waiting.
pub const QUEUE_BYTES: usize = 256 * 1024;

/// The largest single provider payload admitted for live delivery.
///
/// A provider line can be larger, but retaining both its normalized event and complete wire encoding would violate the
/// daemon's live memory ceiling. Such a frame produces an explicit reconnect gap instead of an unbounded memory
/// spike.
pub const MAX_LIVE_PAYLOAD_BYTES: usize = 1024 * 1024;

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
    rx: mpsc::Receiver<SubscriptionItem>,
    /// Payload bytes waiting, shared with the sending side.
    ///
    /// The sender adds when a frame goes in and this side subtracts when one comes out, so the budget
    /// reflects what is actually held rather than what was ever sent.
    queued: Arc<AtomicUsize>,
    /// Frames waiting in the channel or held by the connection writer.
    held_frames: Arc<AtomicUsize>,
}

/// One item on a watcher's bounded live queue.
#[derive(Debug)]
pub enum SubscriptionItem {
    /// A provider event that retained its original dense position.
    Event(WatchEvent),
    /// Delivery stopped before this exact boundary and reconnect is required.
    Lagged(WatchCursor),
}

impl Subscription {
    /// Which subscriber this is.
    #[must_use]
    pub const fn id(&self) -> SubscriberId {
        self.id
    }

    /// Wait for the next frame, or `None` once the session is gone.
    pub async fn recv(&mut self) -> Option<SubscriptionItem> {
        self.rx.recv().await
    }

    /// Take a frame if one is waiting.
    ///
    /// The bounds are exercised through this, which is why this module needs no runtime to prove they hold.
    #[expect(
        clippy::manual_ok_err,
        reason = "Result::ok is forbidden because it usually hides failures, while both non-item states are this poll's documented None answer"
    )]
    pub fn try_recv(&mut self) -> Option<SubscriptionItem> {
        // Empty means nothing is waiting; disconnected means nothing ever will be again. Neither is a failure being
        // discarded: both are the answer to a poll. A caller that needs to tell them apart awaits `recv`, which
        // returns `None` only for the second.
        match self.rx.try_recv() {
            Ok(item) => Some(item),
            Err(mpsc::error::TryRecvError::Empty | mpsc::error::TryRecvError::Disconnected) => None,
        }
    }

    /// How many payload bytes are still waiting.
    #[must_use]
    pub fn queued_bytes(&self) -> usize {
        self.queued.load(Ordering::Acquire)
    }

    /// How many frames are waiting or currently being written.
    #[must_use]
    pub fn queued_frames(&self) -> usize {
        self.held_frames.load(Ordering::Acquire)
    }
}

/// One watcher, from the sending side.
///
/// Holds no identifier. A watcher is removed when its receiving end is dropped, so nothing here ever needs
/// to name one, and a field nothing reads is the debt this repository does not carry.
#[derive(Debug)]
struct Subscriber {
    /// The queue.
    tx: mpsc::Sender<SubscriptionItem>,
    /// Payload bytes waiting, shared with the receiving side.
    queued: Arc<AtomicUsize>,
    /// Frames waiting or currently being written.
    held_frames: Arc<AtomicUsize>,
    /// The next dense event this subscriber must receive.
    next_expected: WatchCursor,
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
    pub fn subscribe(&mut self, live_at: WatchCursor) -> Subscription {
        // One slot deeper than the frame bound. The extra slot is never used for data, so there is always
        // room for the frame that tells a stalled subscriber it stalled.
        let (tx, rx) = mpsc::channel(QUEUE_FRAMES + 1);
        let queued = Arc::new(AtomicUsize::new(0));
        let held_frames = Arc::new(AtomicUsize::new(0));
        let id = SubscriberId(self.next_id);
        self.next_id = self.next_id.wrapping_add(1);

        self.subscribers.push(Subscriber {
            tx,
            queued: Arc::clone(&queued),
            held_frames: Arc::clone(&held_frames),
            next_expected: live_at,
        });

        Subscription {
            id,
            rx,
            queued,
            held_frames,
        }
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

    /// End every existing subscription at an attachment boundary.
    pub fn close(&mut self) {
        self.subscribers.clear();
    }

    /// Give the frame to every watcher that can take it, and tell the rest they fell behind.
    pub fn publish(&mut self, event: &AgentEvent) -> Delivery {
        let mut report = Delivery::default();

        if self.subscribers.is_empty() {
            return report;
        }

        if event.body.payload_bytes() > MAX_LIVE_PAYLOAD_BYTES {
            self.subscribers.retain_mut(|subscriber| {
                if subscriber.tx.is_closed() {
                    report.departed += 1;
                } else {
                    subscriber.fall_behind();
                    report.lagged += 1;
                }
                false
            });
            return report;
        }

        let shared = WatchEvent::encode(event.clone());

        self.subscribers.retain_mut(|subscriber| {
            if subscriber.tx.is_closed() {
                report.departed += 1;
                return false;
            }
            match subscriber.offer(&shared) {
                Offered::Took => report.delivered += 1,
                Offered::FellBehind => {
                    report.lagged += 1;
                    return false;
                }
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
}

impl Subscriber {
    /// Offer one frame, within the bounds.
    fn offer(&mut self, frame: &WatchEvent) -> Offered {
        let event = frame.event();
        if event.epoch != self.next_expected.epoch || event.seq != self.next_expected.seq {
            return self.fall_behind();
        }

        if self.has_room_for(frame) {
            let leased = frame.queued(Arc::clone(&self.queued), Arc::clone(&self.held_frames));
            match self.tx.try_send(SubscriptionItem::Event(leased)) {
                Ok(()) => {
                    self.next_expected.epoch = event.epoch;
                    self.next_expected.seq = event.seq.wrapping_add(1);
                    return Offered::Took;
                }
                // The receiving end went away between the closed check and here, or the channel filled
                // from another publish. Either way this subscriber did not get the frame, and saying so is
                // the same answer as any other overflow.
                Err(_) => return self.fall_behind(),
            }
        }
        self.fall_behind()
    }

    /// Whether the frame fits inside both bounds.
    fn has_room_for(&self, frame: &WatchEvent) -> bool {
        // Capacity counts the reserved slot, which data may not use.
        let used_frames = self.held_frames.load(Ordering::Acquire);
        let waiting_bytes = self.queued.load(Ordering::Acquire);
        let bytes = frame_bytes(frame);
        if bytes > QUEUE_BYTES {
            return used_frames == 0
                && frame.event().body.payload_bytes() <= MAX_LIVE_PAYLOAD_BYTES;
        }
        used_frames < QUEUE_FRAMES && waiting_bytes.saturating_add(bytes) <= QUEUE_BYTES
    }

    /// Tell the subscriber where it had reached, then retire it.
    ///
    /// The lag frame stands in the place of the frame that did not fit, so it carries that frame's
    /// position: the subscriber then knows the gap runs from what it last received up to here.
    fn fall_behind(&mut self) -> Offered {
        // The reserved slot takes it unless the receiver went away between the closed check and this send. The sender
        // is retired either way, and the control item carries no provider payload budget.
        drop(
            self.tx
                .try_send(SubscriptionItem::Lagged(self.next_expected)),
        );
        Offered::FellBehind
    }
}

/// How much budget one frame occupies.
///
/// Defined once, because the sending side adds it and the receiving side subtracts it. Two spellings of
/// this would drift, and the drift would show up as a queue that believes it is permanently full.
fn frame_bytes(frame: &WatchEvent) -> usize {
    frame.retained_bytes()
}

#[cfg(test)]
mod tests {
    use runtrol_provider::{EventBody, Opaque, SessionId};

    use super::*;
    use crate::events::seq::Sequencer;

    /// A publisher that stamps real positions, so lag reports carry real numbers.
    struct Source {
        session: SessionId,
        stream: runtrol_provider::StreamId,
        seq: Sequencer,
    }

    impl Source {
        fn new() -> Self {
            Self {
                session: SessionId::now(),
                stream: runtrol_provider::StreamId::now(),
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

        fn live_at(&self) -> WatchCursor {
            WatchCursor {
                stream: self.stream,
                epoch: self.seq.epoch(),
                seq: self.seq.next_seq(),
            }
        }
    }

    fn delivered(item: SubscriptionItem) -> AgentEvent {
        match item {
            SubscriptionItem::Event(event) => event.event().clone(),
            SubscriptionItem::Lagged(cursor) => panic!("unexpected lag at {cursor:?}"),
        }
    }

    #[test]
    fn the_ceiling_for_stalled_subscribers_fits_the_memory_contract() {
        // Four subscribers that have all stopped reading have to leave ample room below the daemon hard ceiling,
        // or the queue bound is decorative.
        const HOT_INCREMENT_BUDGET: usize = 10 * 1024 * 1024;
        let ceiling = 4 * (QUEUE_FRAMES * size_of::<AgentEvent>() + QUEUE_BYTES);
        assert!(
            ceiling < HOT_INCREMENT_BUDGET / 2,
            "four stalled subscribers would hold {ceiling} bytes"
        );
    }

    #[test]
    fn every_watcher_gets_the_frame() {
        let mut source = Source::new();
        let mut fanout = FanOut::new();
        let mut first = fanout.subscribe(source.live_at());
        let mut second = fanout.subscribe(source.live_at());

        let event = source.frame(16);
        let report = fanout.publish(&event);

        assert_eq!(report.delivered, 2);
        assert_eq!(
            first.try_recv().map(delivered).map(|frame| frame.seq),
            Some(event.seq)
        );
        assert_eq!(
            second.try_recv().map(delivered).map(|frame| frame.seq),
            Some(event.seq)
        );
    }

    #[test]
    fn a_payload_is_shared_and_not_copied_per_watcher() {
        // The reason a queue per subscriber is affordable at all. If this ever stops holding, fanning out
        // to a dozen watchers starts costing a dozen copies of every message.
        let mut source = Source::new();
        let mut fanout = FanOut::new();
        let mut first = fanout.subscribe(source.live_at());
        let mut second = fanout.subscribe(source.live_at());

        fanout.publish(&source.frame(4096));
        let one = delivered(first.try_recv().expect("delivered"));
        let other = delivered(second.try_recv().expect("delivered"));

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
        let subscription = fanout.subscribe(source.live_at());

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
        let subscription = fanout.subscribe(source.live_at());

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
    fn one_large_event_reaches_a_live_watcher_without_becoming_an_unbounded_queue() {
        let mut source = Source::new();
        let mut fanout = FanOut::new();
        let mut subscription = fanout.subscribe(source.live_at());

        let large = source.frame(QUEUE_BYTES + 1);
        assert_eq!(fanout.publish(&large).delivered, 1);
        assert!(subscription.queued_bytes() > QUEUE_BYTES);
        assert!(subscription.queued_bytes() <= 2 * MAX_LIVE_PAYLOAD_BYTES);

        let second = source.frame(QUEUE_BYTES + 1);
        assert_eq!(fanout.publish(&second).lagged, 1);
        let delivered_large = delivered(subscription.try_recv().expect("large event delivered"));
        assert_eq!(delivered_large.seq, large.seq);
        assert!(matches!(
            subscription.try_recv(),
            Some(SubscriptionItem::Lagged(_))
        ));
    }

    #[test]
    fn an_event_being_written_still_holds_its_queue_permit() {
        let mut source = Source::new();
        let mut fanout = FanOut::new();
        let mut subscription = fanout.subscribe(source.live_at());

        let first = source.frame(QUEUE_BYTES + 1);
        assert_eq!(fanout.publish(&first).delivered, 1);
        let writing = subscription.try_recv().expect("the writer took the event");
        assert_eq!(subscription.queued_frames(), 1);
        assert!(subscription.queued_bytes() > QUEUE_BYTES);

        let second = source.frame(QUEUE_BYTES + 1);
        assert_eq!(fanout.publish(&second).lagged, 1);
        drop(writing);
        assert_eq!(subscription.queued_frames(), 0);
        assert_eq!(subscription.queued_bytes(), 0);
    }

    #[test]
    fn an_oversize_payload_is_rejected_before_live_wire_encoding() {
        let mut source = Source::new();
        let mut fanout = FanOut::new();
        let mut subscription = fanout.subscribe(source.live_at());

        let oversize = source.frame(MAX_LIVE_PAYLOAD_BYTES + 1);
        let report = fanout.publish(&oversize);

        assert_eq!(report.delivered, 0);
        assert_eq!(report.lagged, 1);
        assert_eq!(subscription.queued_bytes(), 0);
        assert_eq!(subscription.queued_frames(), 0);
        assert!(matches!(
            subscription.try_recv(),
            Some(SubscriptionItem::Lagged(_))
        ));
        assert_eq!(fanout.len(), 0);
    }

    #[test]
    fn the_lag_frame_names_the_exact_next_boundary() {
        let mut source = Source::new();
        let mut fanout = FanOut::new();
        let mut subscription = fanout.subscribe(source.live_at());

        let mut last_taken = None;
        for _ in 0..(QUEUE_FRAMES * 2) {
            let event = source.frame(1);
            if fanout.publish(&event).delivered == 1 {
                last_taken = Some(event);
            }
        }
        let last_taken = last_taken.expect("early frames must have been delivered");

        let mut lag = None;
        while let Some(item) = subscription.try_recv() {
            if let SubscriptionItem::Lagged(next_expected) = item {
                lag = Some(next_expected);
            }
        }
        assert_eq!(
            lag.expect("the subscriber must be told it fell behind"),
            WatchCursor {
                stream: source.stream,
                epoch: last_taken.epoch,
                seq: last_taken.seq.wrapping_add(1),
            }
        );
    }

    #[test]
    fn a_lagged_subscriber_must_reconnect_before_live_delivery_resumes() {
        let mut source = Source::new();
        let mut fanout = FanOut::new();
        let mut subscription = fanout.subscribe(source.live_at());

        for _ in 0..(QUEUE_FRAMES * 2) {
            fanout.publish(&source.frame(1));
        }
        while subscription.try_recv().is_some() {}
        assert_eq!(subscription.queued_bytes(), 0);

        let resumed = source.frame(1);
        assert_eq!(fanout.publish(&resumed).delivered, 0);
        assert!(subscription.try_recv().is_none());
        assert_eq!(fanout.len(), 0, "the lagged sender was retired");
    }

    #[test]
    fn a_watcher_that_went_away_is_forgotten() {
        let mut source = Source::new();
        let mut fanout = FanOut::new();
        let subscription = fanout.subscribe(source.live_at());
        let mut staying = fanout.subscribe(source.live_at());
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
        let mut subscription = fanout.subscribe(source.live_at());

        let event = source.frame(8);
        fanout.publish(&event);
        let received = delivered(subscription.recv().await.expect("a frame was published"));
        assert_eq!(received.seq, event.seq);
        assert_eq!(
            subscription.queued_bytes(),
            0,
            "waiting released the budget"
        );
    }
}
