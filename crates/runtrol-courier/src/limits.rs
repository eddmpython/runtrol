//! The ceilings, once.
//!
//! `mainPlan/session-fabric` fixes these as development ceilings and requires that they live in executable code
//! exactly once. This is that place. Every check in the crate reads the table it was handed, so a test can shrink
//! a ceiling to reach an overflow with a few bytes, and a Runtime runs exactly the table it was built with.

use core::fmt;

/// A moment on the Unix clock, in milliseconds. The courier never reads a clock; every operation is handed the
/// moment it happens at, which is what makes expiry a fact a test can state instead of a race it has to win.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct UnixMillis(pub u64);

impl UnixMillis {
    /// This moment plus `millis`, saturating at the end of the clock.
    #[must_use]
    pub const fn plus(self, millis: u64) -> Self {
        Self(self.0.saturating_add(millis))
    }

    /// How far this moment lies past `other`, or zero when it does not.
    #[must_use]
    pub const fn since(self, other: Self) -> u64 {
        self.0.saturating_sub(other.0)
    }
}

impl fmt::Display for UnixMillis {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

/// The resource ceilings one courier enforces.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Bytes of UTF-8 in one message body.
    pub body_bytes: usize,
    /// Envelopes one session's mailbox holds at once.
    pub mailbox_envelopes: usize,
    /// Body bytes one session's mailbox holds at once.
    pub mailbox_bytes: usize,
    /// Body bytes every mailbox together may hold at once.
    pub runtime_bytes: usize,
    /// Asks waiting for a reply at once.
    pub active_calls: usize,
    /// The deadline an envelope gets when its caller names none.
    pub default_deadline_millis: u64,
    /// The furthest ahead a caller may place a deadline.
    pub max_deadline_millis: u64,
    /// Hops a message may already have travelled and still be routed once more.
    pub hop_count: u8,
    /// Sessions a message may have visited and still be routed once more.
    pub visited_sessions: usize,
    /// Sessions one room may hold. Rooms open in a later stamp; the ceiling is declared with the others.
    pub room_participants: usize,
    /// Rounds one room may run before it closes.
    pub room_rounds: u8,
    /// Message identifiers remembered after their envelope is gone, so a late duplicate is still refused.
    pub remembered_messages: usize,
}

impl Limits {
    /// The initial development ceilings.
    pub const INITIAL: Self = Self {
        body_bytes: 16 * 1024,
        mailbox_envelopes: 16,
        mailbox_bytes: 128 * 1024,
        runtime_bytes: 512 * 1024,
        active_calls: 32,
        default_deadline_millis: 120_000,
        max_deadline_millis: 600_000,
        hop_count: 4,
        visited_sessions: 8,
        room_participants: 3,
        room_rounds: 6,
        remembered_messages: 256,
    };

    /// The deadline an envelope sent at `now` gets when its caller names none.
    #[must_use]
    pub const fn default_deadline(&self, now: UnixMillis) -> UnixMillis {
        now.plus(self.default_deadline_millis)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_initial_table_is_the_one_the_design_fixed() {
        let limits = Limits::INITIAL;
        assert_eq!(limits.body_bytes, 16_384);
        assert_eq!(limits.mailbox_envelopes, 16);
        assert_eq!(limits.mailbox_bytes, 131_072);
        assert_eq!(limits.runtime_bytes, 524_288);
        assert_eq!(limits.active_calls, 32);
        assert_eq!(limits.default_deadline_millis, 120_000);
        assert_eq!(limits.max_deadline_millis, 600_000);
        assert_eq!(limits.hop_count, 4);
        assert_eq!(limits.visited_sessions, 8);
        assert_eq!(limits.room_participants, 3);
        assert_eq!(limits.room_rounds, 6);
        assert!(limits.default_deadline_millis <= limits.max_deadline_millis);
    }

    #[test]
    fn the_clock_saturates_instead_of_wrapping() {
        let end = UnixMillis(u64::MAX);
        assert_eq!(end.plus(5), end);
        assert_eq!(UnixMillis(3).since(UnixMillis(10)), 0);
        assert_eq!(UnixMillis(10).since(UnixMillis(3)), 7);
        assert_eq!(
            Limits::INITIAL.default_deadline(UnixMillis(1_000)),
            UnixMillis(121_000)
        );
    }
}
