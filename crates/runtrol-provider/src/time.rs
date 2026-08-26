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

    /// Read an ISO 8601 instant a provider wrote, or nothing when it is not one.
    ///
    /// Providers state reset times two ways, and both have to land on the same type. One counts unix
    /// seconds, which [`Self::from_millis`] already takes; another writes
    /// `2026-08-26T18:09:59.801396+00:00`, measured on Claude Code 2.1.246's usage answer. Without this a
    /// driver either dropped every reset instant that arrived as text or grew its own date reader, and the
    /// second one is how two drivers come to disagree about what a timestamp means.
    ///
    /// Deliberately strict, and deliberately no calendar dependency. It accepts the profile providers
    /// actually write (a full date, a full time, optional fractional seconds, and `Z` or `±HH:MM`) and
    /// answers `None` for everything else, including dates before the epoch. A reset time this cannot read
    /// is reported as absent, never as a guess: a wrong instant renders as a real countdown.
    #[must_use]
    pub fn from_iso8601(text: &str) -> Option<Self> {
        let text = text.trim();
        let (date, rest) = text.split_once(['T', 't', ' '])?;
        let mut date = date.split('-');
        let year: i64 = number(date.next()?)?;
        let month: u32 = number(date.next()?)?;
        let day: u32 = number(date.next()?)?;
        if date.next().is_some() || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
            return None;
        }

        // The offset is split off before the clock is read, because the date's own separators are also
        // hyphens and a `Z` is an offset of zero.
        let (clock, offset_minutes) = split_offset(rest)?;
        let mut clock = clock.split(':');
        let hour: u64 = number(clock.next()?)?;
        let minute: u64 = number(clock.next()?)?;
        let (second, fraction) = match clock.next() {
            Some(seconds) => match seconds.split_once('.') {
                Some((whole, frac)) => (number::<u64>(whole)?, read_millis(frac)?),
                None => (number::<u64>(seconds)?, 0),
            },
            None => (0, 0),
        };
        if clock.next().is_some() || hour > 23 || minute > 59 || second > 60 {
            return None;
        }

        let days = days_from_civil(year, month, day);
        let clock_seconds = i64::try_from(hour * 3_600 + minute * 60 + second);
        let Ok(clock_seconds) = clock_seconds else {
            return None;
        };
        let seconds = days.checked_mul(86_400)? + clock_seconds - i64::from(offset_minutes) * 60;
        let Ok(seconds) = u64::try_from(seconds) else {
            // Before the epoch. Not a reset time anything in this product is waiting for, and reading it
            // as a huge positive would render as a countdown of tens of thousands of years.
            return None;
        };
        Some(Self(seconds.checked_mul(1_000)?.checked_add(fraction)?))
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

/// One field of an instant, or nothing when it is not a number at all.
///
/// The parse failure is answered rather than dropped: an instant this cannot read is reported absent by
/// every caller of [`WallMs::from_iso8601`], which is the whole contract that function documents.
fn number<T: core::str::FromStr>(text: &str) -> Option<T> {
    let Ok(value) = text.parse() else {
        return None;
    };
    Some(value)
}

/// Split an ISO 8601 time from its zone, answering the clock text and the zone's offset in minutes.
///
/// A naive instant is refused rather than assumed to be UTC. Providers that state a reset time state a
/// zone with it, and inventing one would move every reset by up to a day.
fn split_offset(rest: &str) -> Option<(&str, i32)> {
    if let Some(clock) = rest.strip_suffix(['Z', 'z']) {
        return Some((clock, 0));
    }
    // Searched from the end, because a sign can only be the zone here: the date is already off the front
    // and a clock carries none.
    let sign_at = rest.rfind(['+', '-'])?;
    let (clock, zone) = rest.split_at(sign_at);
    let negative = zone.starts_with('-');
    let zone = &zone[1..];
    let (hours, minutes) = match zone.split_once(':') {
        Some((hours, minutes)) => (hours, minutes),
        // `+0000` and `+00` are both written in the wild.
        None if zone.len() == 4 => zone.split_at(2),
        None if zone.len() == 2 => (zone, "0"),
        None => return None,
    };
    let hours: i32 = number(hours)?;
    let minutes: i32 = number(minutes)?;
    if !(0..=23).contains(&hours) || !(0..=59).contains(&minutes) {
        return None;
    }
    let total = hours * 60 + minutes;
    Some((clock, if negative { -total } else { total }))
}

/// Fractional seconds as whole milliseconds, truncated the way a clock truncates.
fn read_millis(fraction: &str) -> Option<u64> {
    if fraction.is_empty() || !fraction.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let mut millis = 0_u64;
    for index in 0..3 {
        let digit = fraction.as_bytes().get(index).map_or(0, |byte| {
            // An ASCII digit by the guard above, so this subtraction cannot underflow.
            u64::from(byte - b'0')
        });
        millis = millis * 10 + digit;
    }
    Some(millis)
}

/// Days since 1970-01-01 for a civil date (Howard Hinnant's algorithm), the inverse of rendering one.
///
/// The same algorithm a driver already carries for the other direction, kept here so the pair lives with
/// the type it converts and no second copy has to be trusted.
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let month = i64::from(month);
    let day = i64::from(day);
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
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

#[cfg(test)]
mod iso8601_tests {
    use super::*;

    #[test]
    fn the_measured_reset_instant_reads_back_exactly() {
        // The literal a real usage answer carried (Claude Code 2.1.246, 2026-08-26). Cross-checked against
        // the unix second count for the same instant, 1787767799.
        let read =
            WallMs::from_iso8601("2026-08-26T18:09:59.801396+00:00").expect("a real instant");
        assert_eq!(read.as_millis(), 1_787_767_799_801);
    }

    #[test]
    fn a_zone_moves_the_instant() {
        let utc = WallMs::from_iso8601("2026-08-26T18:00:00Z").expect("utc");
        let plus_nine = WallMs::from_iso8601("2026-08-27T03:00:00+09:00").expect("kst");
        let minus_five = WallMs::from_iso8601("2026-08-26T13:00:00-05:00").expect("est");
        assert_eq!(utc, plus_nine);
        assert_eq!(utc, minus_five);
    }

    #[test]
    fn the_shapes_providers_actually_write_all_read() {
        for text in [
            "2026-08-26T18:00:00Z",
            "2026-08-26T18:00:00+0000",
            "2026-08-26T18:00:00+00",
            "2026-08-26 18:00:00Z",
            "2026-08-26T18:00:00.5Z",
        ] {
            assert!(
                WallMs::from_iso8601(text).is_some(),
                "expected {text} to read"
            );
        }
        assert_eq!(
            WallMs::from_iso8601("2026-08-26T18:00:00.5Z")
                .expect("half a second")
                .as_millis()
                % 1_000,
            500
        );
    }

    #[test]
    fn what_is_not_an_instant_is_absent_rather_than_a_guess() {
        // Absent beats wrong: every one of these would otherwise render as a real countdown.
        for text in [
            "",
            "tomorrow",
            "2026-08-26",
            "2026-08-26T18:00:00",
            "2026-13-01T00:00:00Z",
            "2026-08-26T25:00:00Z",
            "1969-12-31T23:59:59Z",
        ] {
            assert!(
                WallMs::from_iso8601(text).is_none(),
                "expected {text:?} to be refused"
            );
        }
    }
}
