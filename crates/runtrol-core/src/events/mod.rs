//! The single point a driver's output enters a session.
//!
//! One hub per session. Everything a driver produces passes through [`SessionHub::publish`], which is what
//! makes the ordering guarantee possible at all: positions are assigned in one place, so a driver that
//! turns one provider line into three frames does not have to think about numbering, and two drivers
//! serving one session across a reattach cannot collide.
//!
//! # The three parts, and why they are three files
//!
//! - [`seq`] assigns positions and holds the live provider source boundary. Pure arithmetic with one rule.
//! - [`ring`] keeps the last few frames so a brief reconnect is served from memory.
//! - [`fanout`] hands frames to watchers under a bound a slow reader cannot exceed.
//!
//! They are separate because each fails differently and each is proved differently. Numbering is proved by
//! arithmetic, the ring by filling it past its bounds, the fan-out by refusing to read.
//!
//! # The name
//!
//! The design notes called this an event bus. The vocabulary crate already says "session hub" in the field
//! documentation a driver author reads, and one name has to win, so it is the one already published.
//!
//! # Nothing here reads a payload
//!
//! The hub touches an event's envelope: session, stream, epoch, sequence, source boundary, and byte count. The
//! payload is counted and moved and never opened. That is not a matter of discipline here; there is no code in this
//! module that could read one.

pub mod fanout;
pub mod ring;
pub mod seq;

use std::collections::VecDeque;
use std::sync::Arc;

use runtrol_provider::{
    AgentEvent, EventBody, Level, Notice, NoticeCode, Opaque, SessionId, StreamId, WatchCursor,
    WatchGap,
};

pub use fanout::{
    Delivery, FanOut, MAX_LIVE_PAYLOAD_BYTES, QUEUE_BYTES, QUEUE_FRAMES, SubscriberId,
    Subscription, SubscriptionItem,
};
pub use ring::{RING_BYTES, RING_FRAMES, Reach, ReplayRing};
pub use seq::{CursorRegression, Sequencer};

/// One positioned event and its complete wire encoding, shared across live watchers.
///
/// The provider event stays lean because every hot session retains many of them. This wrapper exists only at a watch
/// boundary, where one encoding is shared by every subscriber instead of being rebuilt per connection.
pub struct WatchEvent {
    inner: Arc<WatchEventInner>,
    lease: Option<QueueLease>,
}

struct QueueLease {
    bytes: Arc<std::sync::atomic::AtomicUsize>,
    frames: Arc<std::sync::atomic::AtomicUsize>,
    held: usize,
}

impl Drop for QueueLease {
    fn drop(&mut self) {
        use std::sync::atomic::Ordering;

        self.bytes.fetch_sub(self.held, Ordering::AcqRel);
        self.frames.fetch_sub(1, Ordering::AcqRel);
    }
}

impl core::fmt::Debug for WatchEvent {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("WatchEvent")
            .field("event", &self.inner.event)
            .field("leased", &self.lease.is_some())
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
struct WatchEventInner {
    event: AgentEvent,
    wire: Result<Opaque, Box<str>>,
}

impl WatchEvent {
    fn encode(event: AgentEvent) -> Self {
        let wire = serde_json::to_string(&event)
            .map(Opaque::owned)
            .map_err(|error| error.to_string().into_boxed_str());
        Self {
            inner: Arc::new(WatchEventInner { event, wire }),
            lease: None,
        }
    }

    /// The positioned event envelope.
    #[must_use]
    pub fn event(&self) -> &AgentEvent {
        &self.inner.event
    }

    /// The complete event bytes, or the serialization defect recorded when this shared event was built.
    ///
    /// # Errors
    ///
    /// When this build could not serialize its own positioned event envelope.
    pub fn wire(&self) -> Result<&Opaque, &str> {
        self.inner.wire.as_ref().map_err(Box::as_ref)
    }

    fn retained_bytes(&self) -> usize {
        let wire = match &self.inner.wire {
            Ok(wire) => wire.len(),
            Err(error) => error.len(),
        };
        self.inner.event.body.payload_bytes().saturating_add(wire)
    }

    fn queued(
        &self,
        bytes: Arc<std::sync::atomic::AtomicUsize>,
        frames: Arc<std::sync::atomic::AtomicUsize>,
    ) -> Self {
        use std::sync::atomic::Ordering;

        let held = self.retained_bytes();
        bytes.fetch_add(held, Ordering::AcqRel);
        frames.fetch_add(1, Ordering::AcqRel);
        Self {
            inner: Arc::clone(&self.inner),
            lease: Some(QueueLease {
                bytes,
                frames,
                held,
            }),
        }
    }
}

/// What one publish produced.
#[derive(Clone, Debug)]
pub struct Published {
    /// The frame as it went out, with its watch position and source boundary assigned.
    ///
    /// Handed back so the caller can persist the diagnostic source checkpoint. That is the caller's job rather than
    /// the hub's:
    /// the hub owns ordering, and the database is somebody else's concern.
    pub event: AgentEvent,
    /// Who got it and who fell behind.
    pub delivery: Delivery,
    /// The driver reported a source boundary behind one it had already reported.
    ///
    /// Already turned into a notice frame by the time this is returned. Reported here as well because the
    /// caller may want to count it, and because a value nobody can miss is better than a log line.
    pub regression: Option<CursorRegression>,
}

/// A bounded recent view followed by the live stream of one session.
///
/// The replay holds only the session hub's existing bounded ring and shares every provider payload allocation.
/// It is a latency window, not a transcript copy. Once drained, reads continue from the live subscription that
/// was installed before the snapshot was taken, so no frame can land between the two.
#[derive(Debug)]
pub struct SessionView {
    /// The acknowledgement sent before replay or live events.
    start: WatchStart,
    /// Frames already held by the bounded replay ring, oldest first.
    replay: VecDeque<WatchEvent>,
    /// Frames published after this view was opened.
    live: Subscription,
}

impl SessionView {
    /// The boundary and any visible gap for this watch.
    #[must_use]
    pub const fn start(&self) -> WatchStart {
        self.start
    }

    /// Wait for the next replayed or live frame, or `None` once the session is gone.
    pub async fn recv(&mut self) -> Option<WatchItem> {
        match self.replay.pop_front() {
            Some(event) => Some(WatchItem::Event(event)),
            None => self.live.recv().await.map(WatchItem::from),
        }
    }

    /// Take the next replayed or live frame when one is immediately available.
    pub fn try_recv(&mut self) -> Option<WatchItem> {
        self.replay
            .pop_front()
            .map(WatchItem::Event)
            .or_else(|| self.live.try_recv().map(WatchItem::from))
    }
}

/// One event or control boundary delivered to a watch connection.
#[derive(Debug)]
pub enum WatchItem {
    /// A provider event at its original dense position.
    Event(WatchEvent),
    /// The subscriber stopped before this boundary and must reconnect.
    Lagged(WatchCursor),
}

impl From<SubscriptionItem> for WatchItem {
    fn from(item: SubscriptionItem) -> Self {
        match item {
            SubscriptionItem::Event(event) => Self::Event(event),
            SubscriptionItem::Lagged(cursor) => Self::Lagged(cursor),
        }
    }
}

/// What a watch knows at the exact subscription boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WatchStart {
    /// The first event this view will deliver, or `live_at` when no replay is pending.
    pub starts_at: WatchCursor,
    /// The first event that would be live after the replay snapshot.
    pub live_at: WatchCursor,
    /// Why the requested boundary could not be replayed, when it could not.
    pub gap: Option<WatchGap>,
}

/// Every event of one session, in order, on its way out.
///
/// No `Default`: a hub with no session is a hub that cannot stamp a frame, and there is no session
/// identifier worth defaulting to.
#[derive(Debug)]
pub struct SessionHub {
    /// Which session.
    session: SessionId,
    /// This hub incarnation, distinct across close/reopen and daemon restart.
    stream: StreamId,
    /// Sequence and live source-boundary assignment.
    seq: Sequencer,
    /// The reconnect window.
    ring: ReplayRing,
    /// The watchers.
    fanout: FanOut,
}

impl SessionHub {
    /// A hub for a session with no attach yet.
    #[must_use]
    pub fn new(session: SessionId) -> Self {
        Self {
            session,
            stream: StreamId::now(),
            seq: Sequencer::new(),
            ring: ReplayRing::new(),
            fanout: FanOut::new(),
        }
    }

    /// Which session this hub serves.
    #[must_use]
    pub const fn session(&self) -> SessionId {
        self.session
    }

    /// Which attach the current positions belong to.
    #[must_use]
    pub const fn epoch(&self) -> u32 {
        self.seq.epoch()
    }

    /// The exact next boundary for a new live subscriber.
    #[must_use]
    pub const fn live_at(&self) -> WatchCursor {
        WatchCursor {
            stream: self.stream,
            epoch: self.seq.epoch(),
            seq: self.seq.next_seq(),
        }
    }

    /// The highest source boundary reported in this live provider attachment.
    #[must_use]
    pub const fn src_end(&self) -> u64 {
        self.seq.src_end()
    }

    /// The reconnect window, for a subscriber asking to catch up.
    #[must_use]
    pub const fn ring(&self) -> &ReplayRing {
        &self.ring
    }

    /// How many watchers there are.
    #[must_use]
    pub fn watchers(&self) -> usize {
        self.fanout.len()
    }

    /// Begin a new attach.
    ///
    /// Positions restart, so the window is emptied with them: frames from two attaches cannot be ordered
    /// against each other, and a window holding both could not answer what came after a given position. Existing
    /// subscriptions end immediately so every watcher receives a new acknowledgement for the new epoch.
    pub fn attach(&mut self) -> u32 {
        self.fanout.close();
        self.ring.clear();
        self.seq.attach()
    }

    /// Open a bounded recent view and then keep watching live frames.
    ///
    /// The live subscription is installed before the replay snapshot is taken. The manager is single-owned, so
    /// nothing can publish in between, and every frame belongs to exactly one side of that boundary.
    pub fn view(&mut self, requested: Option<WatchCursor>) -> SessionView {
        let live_at = self.live_at();
        let live = self.fanout.subscribe(live_at);
        let (replay, starts_at, gap) = match requested {
            None => {
                let replay: VecDeque<_> = self
                    .ring
                    .frames()
                    .cloned()
                    .map(WatchEvent::encode)
                    .collect();
                let starts_at = replay.front().map_or(live_at, |frame| WatchCursor {
                    stream: self.stream,
                    epoch: frame.event().epoch,
                    seq: frame.event().seq,
                });
                (replay, starts_at, None)
            }
            Some(cursor) => match self.ring.reach(cursor, live_at) {
                Reach::UpToDate => (VecDeque::new(), live_at, None),
                Reach::Held => (
                    self.ring
                        .frames_from(cursor)
                        .cloned()
                        .map(WatchEvent::encode)
                        .collect(),
                    cursor,
                    None,
                ),
                Reach::Gap => (
                    VecDeque::new(),
                    live_at,
                    Some(WatchGap {
                        requested: cursor,
                        live_at,
                    }),
                ),
            },
        };
        SessionView {
            start: WatchStart {
                starts_at,
                live_at,
                gap,
            },
            replay,
            live,
        }
    }

    /// Add a watcher without replaying the recent window.
    ///
    /// Used by focused kernel tests and callers that already hold an exact cursor.
    pub fn subscribe(&mut self) -> Subscription {
        self.fanout.subscribe(self.live_at())
    }

    /// Stamp a frame, keep it in the window, and give it to every watcher.
    ///
    /// A source boundary that went backwards is corrected in the frame and then reported as a notice of its own, in
    /// that order. Doing it here rather than leaving it to a caller is what makes the guarantee mechanical:
    /// there is no path through this function on which a misbehaving driver goes unremarked.
    pub fn publish(&mut self, src_end: u64, body: EventBody) -> Published {
        let (event, regression) = self.seq.stamp(self.session, src_end, body);
        let delivery = self.emit(event.clone());

        if let Some(regression) = regression {
            self.emit_regression_notice(regression);
        }

        Published {
            event,
            delivery,
            regression,
        }
    }

    /// Put a stamped frame in the window and out to the watchers.
    fn emit(&mut self, event: AgentEvent) -> Delivery {
        let delivery = self.fanout.publish(&event);
        self.ring.push(self.stream, event);
        delivery
    }

    /// Say out loud that a driver reported a source boundary behind one it had already reported.
    ///
    /// A frame runtrol originates, so its payload is runtrol's own text. Only two numbers go into it, which
    /// is why building the JSON by hand here is safe: nothing a provider wrote is interpolated.
    fn emit_regression_notice(&mut self, regression: CursorRegression) {
        let detail = format!(
            r#"{{"reported":{},"kept":{}}}"#,
            regression.reported, regression.kept
        );
        let notice = EventBody::Notice(Box::new(Notice {
            level: Level::Warn,
            code: NoticeCode::ProtocolViolation,
            // Not something that resolves itself. The driver reported an impossible cursor and will keep
            // doing so until somebody changes it.
            retryable: false,
            payload: Opaque::owned(detail),
        }));
        let (event, _) = self.seq.stamp(self.session, regression.kept, notice);
        self.emit(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn subscription_event(item: SubscriptionItem) -> AgentEvent {
        match item {
            SubscriptionItem::Event(event) => event.event().clone(),
            SubscriptionItem::Lagged(cursor) => panic!("unexpected lag at {cursor:?}"),
        }
    }

    fn watch_event(item: WatchItem) -> AgentEvent {
        match item {
            WatchItem::Event(event) => event.event().clone(),
            WatchItem::Lagged(cursor) => panic!("unexpected lag at {cursor:?}"),
        }
    }

    fn a_body(payload: &str) -> EventBody {
        EventBody::Plan {
            payload: Opaque::owned(payload.to_owned()),
        }
    }

    #[test]
    fn positions_are_assigned_in_one_place_and_come_out_dense() {
        let mut hub = SessionHub::new(SessionId::now());
        let mut watcher = hub.subscribe();

        for index in 0..4 {
            hub.publish((index + 1) * 10, a_body("{}"));
        }

        let mut positions = Vec::new();
        while let Some(frame) = watcher.try_recv() {
            positions.push(subscription_event(frame).seq);
        }
        assert_eq!(positions, vec![0, 1, 2, 3]);
    }

    #[test]
    fn a_frame_goes_to_the_window_and_to_the_watchers() {
        let mut hub = SessionHub::new(SessionId::now());
        let mut watcher = hub.subscribe();

        let published = hub.publish(700, a_body(r#"{"steps":[]}"#));

        assert_eq!(published.delivery.delivered, 1);
        assert_eq!(hub.ring().len(), 1);
        assert_eq!(hub.ring().newest_seq(), Some(published.event.seq));
        assert_eq!(
            watcher
                .try_recv()
                .map(subscription_event)
                .map(|frame| frame.src_end),
            Some(700),
            "the watcher gets the same source boundary the window kept"
        );
    }

    #[test]
    fn the_complete_wire_event_is_encoded_once_before_fan_out() {
        let mut hub = SessionHub::new(SessionId::now());
        let mut first = hub.subscribe();
        let mut second = hub.subscribe();
        hub.publish(1, a_body(r#"{"large":"shared"}"#));

        let one = match first.try_recv().expect("first watcher") {
            SubscriptionItem::Event(event) => event,
            SubscriptionItem::Lagged(cursor) => panic!("unexpected lag at {cursor:?}"),
        };
        let two = match second.try_recv().expect("second watcher") {
            SubscriptionItem::Event(event) => event,
            SubscriptionItem::Lagged(cursor) => panic!("unexpected lag at {cursor:?}"),
        };
        let one_wire = one.wire().expect("serializable").bytes();
        let two_wire = two.wire().expect("serializable").bytes();
        assert_eq!(one_wire.as_ptr(), two_wire.as_ptr());
    }

    #[test]
    fn a_cursor_that_went_backwards_produces_a_notice_nobody_had_to_remember_to_send() {
        // The point of doing this inside publish: there is no call path that can drop it. A driver bug
        // becomes something the operator can see, without a caller having to opt in.
        let mut hub = SessionHub::new(SessionId::now());
        let mut watcher = hub.subscribe();

        hub.publish(900, a_body("{}"));
        let published = hub.publish(100, a_body("{}"));

        assert_eq!(
            published.regression,
            Some(CursorRegression {
                reported: 100,
                kept: 900,
            })
        );
        assert_eq!(
            published.event.src_end, 900,
            "the frame carries the source boundary that held"
        );

        let mut frames = Vec::new();
        while let Some(frame) = watcher.try_recv() {
            frames.push(subscription_event(frame));
        }
        let notice = frames
            .iter()
            .find_map(|frame| match &frame.body {
                EventBody::Notice(notice) => Some(notice),
                _ => None,
            })
            .expect("the regression must reach a watcher");
        assert_eq!(notice.code, NoticeCode::ProtocolViolation);
        assert_eq!(notice.level, Level::Warn);
        assert!(!notice.retryable);
        assert!(
            notice.payload.as_str().contains("900"),
            "the kept source boundary"
        );
    }

    #[test]
    fn the_notice_keeps_the_stream_dense() {
        // The notice is a frame like any other. If it skipped numbering, every subscriber would read a gap
        // where runtrol itself had spoken.
        let mut hub = SessionHub::new(SessionId::now());
        let mut watcher = hub.subscribe();

        hub.publish(900, a_body("{}"));
        hub.publish(100, a_body("{}"));

        let mut positions = Vec::new();
        while let Some(frame) = watcher.try_recv() {
            positions.push(subscription_event(frame).seq);
        }
        assert_eq!(positions, vec![0, 1, 2], "the notice is frame two");
    }

    #[test]
    fn a_reattach_moves_the_epoch_and_empties_the_window() {
        let mut hub = SessionHub::new(SessionId::now());
        let before = hub.live_at();
        hub.publish(500, a_body("{}"));
        assert_eq!(hub.ring().len(), 1);

        let epoch = hub.attach();

        assert_eq!(epoch, 1);
        assert_eq!(hub.live_at().stream, before.stream);
        assert!(
            hub.ring().is_empty(),
            "positions restarted, so the old ones cannot be answered for"
        );
        let published = hub.publish(20, a_body("{}"));
        assert_eq!(published.event.epoch, 1);
        assert_eq!(published.event.seq, 0);
        assert_eq!(
            published.event.src_end, 20,
            "a new attach starts its own live source boundary"
        );
    }

    #[tokio::test]
    async fn a_reattach_drains_then_ends_every_old_subscription() {
        let mut hub = SessionHub::new(SessionId::now());
        let mut watcher = hub.subscribe();
        hub.publish(10, a_body("{}"));

        hub.attach();

        assert!(matches!(
            watcher.recv().await,
            Some(SubscriptionItem::Event(_))
        ));
        assert!(watcher.recv().await.is_none());
        assert_eq!(hub.watchers(), 0);
    }

    #[test]
    fn replay_and_live_delivery_meet_once_at_the_ack_boundary() {
        let mut hub = SessionHub::new(SessionId::now());
        for cursor in 1..=4 {
            hub.publish(cursor, a_body("{}"));
        }
        let requested = WatchCursor {
            seq: 2,
            ..hub.live_at()
        };
        let mut view = hub.view(Some(requested));
        assert_eq!(view.start().starts_at, requested);
        assert_eq!(view.start().live_at.seq, 4);
        assert!(view.start().gap.is_none());

        hub.publish(5, a_body("{}"));
        let delivered = (0..3)
            .map(|_| watch_event(view.try_recv().expect("replay and live frame")).seq)
            .collect::<Vec<_>>();
        assert_eq!(delivered, vec![2, 3, 4]);
        assert!(view.try_recv().is_none());
    }

    #[test]
    fn a_watcher_that_joins_late_is_told_what_it_can_recover() {
        let mut hub = SessionHub::new(SessionId::now());
        for index in 0..(RING_FRAMES * 2) {
            let cursor = u64::try_from(index).expect("a test publishes few frames") * 10 + 10;
            hub.publish(cursor, a_body("{}"));
        }

        let old = WatchCursor {
            seq: 0,
            ..hub.live_at()
        };
        let gap = hub.view(Some(old));
        assert!(gap.start().gap.is_some());

        let live_at = hub.live_at();
        let current = hub.view(Some(live_at));
        assert_eq!(current.start().gap, None);
        assert_eq!(current.start().live_at, live_at);
    }

    #[test]
    fn a_new_hub_never_accepts_the_previous_hubs_cursor() {
        let session = SessionId::now();
        let first = SessionHub::new(session).live_at();
        let mut reopened = SessionHub::new(session);

        let view = reopened.view(Some(first));
        assert_eq!(view.start().gap.map(|gap| gap.requested), Some(first));
        assert_ne!(view.start().live_at.stream, first.stream);
    }

    #[test]
    fn the_hub_tracks_its_source_boundary_for_whoever_persists_it() {
        let mut hub = SessionHub::new(SessionId::now());
        assert_eq!(hub.src_end(), 0);
        hub.publish(4_096, a_body("{}"));
        assert_eq!(hub.src_end(), 4_096);
        assert_eq!(hub.epoch(), 0, "publishing is not attaching");
    }

    #[test]
    fn a_hub_with_nobody_watching_still_keeps_its_window() {
        // A session runs whether or not anyone is looking. The bounded window lets somebody look later without
        // turning runtrol into another transcript owner.
        let mut hub = SessionHub::new(SessionId::now());
        assert_eq!(hub.watchers(), 0);
        hub.publish(10, a_body("{}"));
        assert_eq!(hub.ring().len(), 1);
    }
}
