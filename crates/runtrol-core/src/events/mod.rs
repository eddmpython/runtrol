//! The single point a driver's output enters a session.
//!
//! One hub per session. Everything a driver produces passes through [`SessionHub::publish`], which is what
//! makes the ordering guarantee possible at all: positions are assigned in one place, so a driver that
//! turns one provider line into three frames does not have to think about numbering, and two drivers
//! serving one session across a reattach cannot collide.
//!
//! # The three parts, and why they are three files
//!
//! - [`seq`] assigns positions and holds the source cursor. Pure arithmetic with one rule.
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
//! The hub touches an event's envelope: which session, which position, which cursor, how many bytes. The
//! payload is counted and moved and never opened. That is not a matter of discipline here; there is no code
//! in this module that could read one.

pub mod fanout;
pub mod ring;
pub mod seq;

use runtrol_provider::{AgentEvent, EventBody, Level, Notice, NoticeCode, Opaque, SessionId};

pub use fanout::{Delivery, FanOut, QUEUE_BYTES, QUEUE_FRAMES, SubscriberId, Subscription};
pub use ring::{RING_BYTES, RING_FRAMES, Reach, ReplayRing};
pub use seq::{CursorRegression, Sequencer};

/// What one publish produced.
#[derive(Clone, Debug)]
pub struct Published {
    /// The frame as it went out, with its position and cursor assigned.
    ///
    /// Handed back so the caller can persist the cursor. That is the caller's job rather than the hub's:
    /// the hub owns ordering, and the database is somebody else's concern.
    pub event: AgentEvent,
    /// Who got it and who fell behind.
    pub delivery: Delivery,
    /// The driver reported a cursor behind one it had already reported.
    ///
    /// Already turned into a notice frame by the time this is returned. Reported here as well because the
    /// caller may want to count it, and because a value nobody can miss is better than a log line.
    pub regression: Option<CursorRegression>,
}

/// Every event of one session, in order, on its way out.
///
/// No `Default`: a hub with no session is a hub that cannot stamp a frame, and there is no session
/// identifier worth defaulting to.
#[derive(Debug)]
pub struct SessionHub {
    /// Which session.
    session: SessionId,
    /// Position and cursor assignment.
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

    /// How far into the provider's own store this attach has reached.
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
    /// against each other, and a window holding both could not answer what came after a given position. A
    /// subscriber reconnecting across this boundary recovers from the provider's own store, which is what
    /// the attach frame's replay source is for.
    pub fn attach(&mut self) -> u32 {
        self.ring.clear();
        self.seq.attach()
    }

    /// Add a watcher.
    pub fn subscribe(&mut self) -> Subscription {
        self.fanout.subscribe()
    }

    /// Stamp a frame, keep it in the window, and give it to every watcher.
    ///
    /// A cursor that went backwards is corrected in the frame and then reported as a notice of its own, in
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
        self.ring.push(event);
        delivery
    }

    /// Say out loud that a driver reported a cursor behind one it had already reported.
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

    fn a_body(payload: &str) -> EventBody {
        EventBody::Plan(Opaque::owned(payload.to_owned()))
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
            positions.push(frame.seq);
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
            watcher.try_recv().map(|frame| frame.src_end),
            Some(700),
            "the watcher gets the same cursor the window kept"
        );
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
            "the frame carries the cursor that held"
        );

        let mut frames = Vec::new();
        while let Some(frame) = watcher.try_recv() {
            frames.push(frame);
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
        assert!(notice.payload.as_str().contains("900"), "the kept cursor");
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
            positions.push(frame.seq);
        }
        assert_eq!(positions, vec![0, 1, 2], "the notice is frame two");
    }

    #[test]
    fn a_reattach_moves_the_epoch_and_empties_the_window() {
        let mut hub = SessionHub::new(SessionId::now());
        hub.publish(500, a_body("{}"));
        assert_eq!(hub.ring().len(), 1);

        let epoch = hub.attach();

        assert_eq!(epoch, 1);
        assert!(
            hub.ring().is_empty(),
            "positions restarted, so the old ones cannot be answered for"
        );
        let published = hub.publish(20, a_body("{}"));
        assert_eq!(published.event.epoch, 1);
        assert_eq!(published.event.seq, 0);
        assert_eq!(
            published.event.src_end, 20,
            "a new attach may resume anywhere in the provider's store"
        );
    }

    #[test]
    fn a_watcher_that_joins_late_is_told_what_it_can_recover() {
        // The hub does not replay into a new subscription by itself. It answers what is recoverable, and
        // the caller decides between the window and the provider's own store.
        let mut hub = SessionHub::new(SessionId::now());
        for index in 0..(RING_FRAMES * 2) {
            let cursor = u64::try_from(index).expect("a test publishes few frames") * 10 + 10;
            hub.publish(cursor, a_body("{}"));
        }

        assert!(matches!(hub.ring().reach(0), Reach::Gap { .. }));
        let newest = hub.ring().newest_seq().expect("frames were published");
        assert_eq!(hub.ring().reach(newest), Reach::UpToDate);
    }

    #[test]
    fn the_hub_tracks_its_own_cursor_for_whoever_persists_it() {
        let mut hub = SessionHub::new(SessionId::now());
        assert_eq!(hub.src_end(), 0);
        hub.publish(4_096, a_body("{}"));
        assert_eq!(hub.src_end(), 4_096);
        assert_eq!(hub.epoch(), 0, "publishing is not attaching");
    }

    #[test]
    fn a_hub_with_nobody_watching_still_keeps_its_window() {
        // A session runs whether or not anyone is looking. The window is what lets somebody look later
        // without going to the provider's file for the last second of output.
        let mut hub = SessionHub::new(SessionId::now());
        assert_eq!(hub.watchers(), 0);
        hub.publish(10, a_body("{}"));
        assert_eq!(hub.ring().len(), 1);
    }
}
