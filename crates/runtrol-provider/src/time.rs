//! Wall clock time, and why the monotonic clock has no type here.
//!
//! runtrol keeps two kinds of time and must never confuse them. Wall clock time is what gets
//! stored, displayed, and compared across process restarts; it is [`WallMs`]. Monotonic time is
//! what gets measured, and it is meaningless after a reboot, so it must never be written down.
//!
//! The rule is enforced by omission: this crate has no monotonic type. `Instant` carries no serde
//! implementation and [`WallMs`] offers no conversion from one, so there is no path by which an
//! elapsed measurement reaches the database or the wire. A crate that measures durations holds its
//! own `Instant` locally and publishes a [`WallMs`] instead.

use core::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// A point in wall clock time, in milliseconds since the Unix epoch.
///
/// Milliseconds rather than nanoseconds because this value exists to be shown to a person and to
/// order a list. Nanosecond precision would cost eight more bytes in every stored row and answer no
/// question anyone asks.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WallMs(u64);

impl WallMs {
    /// The Unix epoch itself, and the value a broken clock produces.
    pub const EPOCH: Self = Self(0);

    /// Read the system clock.
    ///
    /// A clock set before 1970 yields [`WallMs::EPOCH`]. That is not an error being swallowed: it is
    /// a misconfigured machine, the result renders as 1970 and sorts first, so it is visibly wrong
    /// rather than quietly wrong. No runtrol decision reads the absolute value; ordering and
    /// display are the only uses, and both stay correct for every clock set after 1970.
    #[must_use]
    pub fn now() -> Self {
        match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(since_epoch) => Self(u64::try_from(since_epoch.as_millis()).unwrap_or(u64::MAX)),
            Err(_) => Self::EPOCH,
        }
    }

    /// Wrap a stored millisecond count.
    #[must_use]
    pub const fn from_millis(millis: u64) -> Self {
        Self(millis)
    }

    /// The millisecond count, for storage and for the wire.
    #[must_use]
    pub const fn as_millis(self) -> u64 {
        self.0
    }

    /// Milliseconds from `self` to `later`, or `None` if `later` is earlier.
    ///
    /// Returns `None` rather than zero so a caller cannot mistake "no time passed" for "the clock
    /// moved backwards", which is a thing wall clocks do and a thing worth noticing.
    #[must_use]
    pub const fn millis_until(self, later: Self) -> Option<u64> {
        later.0.checked_sub(self.0)
    }

    /// This instant advanced by `millis`, saturating at the end of representable time.
    #[must_use]
    pub const fn plus_millis(self, millis: u64) -> Self {
        Self(self.0.saturating_add(millis))
    }
}

impl fmt::Display for WallMs {
    /// The raw millisecond count.
    ///
    /// Deliberately not a calendar format. Rendering a date needs a timezone and a locale, both of
    /// which belong to whatever is showing it to a person, and a half-correct format chosen here
    /// would become the one everything else copied.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl fmt::Debug for WallMs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "WallMs({})", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn now_is_after_this_code_was_written() {
        // 2026-01-01T00:00:00Z. A clock behind this is either broken or a deliberate test fixture,
        // and either way the assertion documents what "now" is supposed to mean.
        const Y2026: u64 = 1_767_225_600_000;
        assert!(WallMs::now().as_millis() > Y2026);
    }

    #[test]
    fn now_moves_forward_or_stands_still_but_never_back() {
        let first = WallMs::now();
        let second = WallMs::now();
        assert!(second >= first);
    }

    #[test]
    fn millis_until_reports_a_backwards_clock_instead_of_hiding_it() {
        let early = WallMs::from_millis(1_000);
        let late = WallMs::from_millis(1_500);
        assert_eq!(early.millis_until(late), Some(500));
        assert_eq!(late.millis_until(early), None);
        assert_eq!(early.millis_until(early), Some(0));
    }

    #[test]
    fn plus_millis_saturates() {
        let end = WallMs::from_millis(u64::MAX);
        assert_eq!(end.plus_millis(1), end);
        assert_eq!(WallMs::EPOCH.plus_millis(90_000).as_millis(), 90_000);
    }

    #[test]
    fn round_trips_through_json() {
        let stamp = WallMs::from_millis(1_767_225_600_123);
        let encoded = serde_json::to_string(&stamp).expect("serializable");
        assert_eq!(encoded, "1767225600123", "must encode as a bare number");
        let decoded: WallMs = serde_json::from_str(&encoded).expect("deserializable");
        assert_eq!(stamp, decoded);
    }
}
