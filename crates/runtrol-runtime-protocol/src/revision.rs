//! Finalized protocol revision parsing and negotiation.

use core::fmt;
use core::str::FromStr;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The first finalized public Runtime contract.
pub const REVISION_2026_08_13: ProtocolRevision = ProtocolRevision::new(2026, 8, 13);

/// The public terminal session and independent Runtime administration contract.
pub const REVISION_2026_08_27: ProtocolRevision = ProtocolRevision::new(2026, 8, 27);

/// Every finalized revision implemented by this package, newest first.
pub const FINALIZED_REVISIONS: [ProtocolRevision; 2] = [REVISION_2026_08_27, REVISION_2026_08_13];

/// A finalized public Runtime contract date.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, JsonSchema)]
#[schemars(with = "String")]
pub struct ProtocolRevision {
    year: u16,
    month: u8,
    day: u8,
}

impl ProtocolRevision {
    /// Construct a revision known at compile time.
    #[must_use]
    pub const fn new(year: u16, month: u8, day: u8) -> Self {
        Self { year, month, day }
    }
}

impl fmt::Display for ProtocolRevision {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{:04}-{:02}-{:02}",
            self.year, self.month, self.day
        )
    }
}

impl FromStr for ProtocolRevision {
    type Err = RevisionError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let bytes = value.as_bytes();
        if bytes.len() != 10
            || bytes.get(4) != Some(&b'-')
            || bytes.get(7) != Some(&b'-')
            || bytes
                .iter()
                .enumerate()
                .any(|(index, byte)| index != 4 && index != 7 && !byte.is_ascii_digit())
        {
            return Err(RevisionError::Shape(value.to_owned()));
        }
        let year = digits(bytes, 0, 4).ok_or_else(|| RevisionError::Shape(value.to_owned()))?;
        let month = digits(bytes, 5, 7).ok_or_else(|| RevisionError::Shape(value.to_owned()))?;
        let day = digits(bytes, 8, 10).ok_or_else(|| RevisionError::Shape(value.to_owned()))?;
        if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
            return Err(RevisionError::Calendar(value.to_owned()));
        }
        Ok(Self {
            year,
            month: u8::try_from(month).map_err(|_| RevisionError::Calendar(value.to_owned()))?,
            day: u8::try_from(day).map_err(|_| RevisionError::Calendar(value.to_owned()))?,
        })
    }
}

fn digits(bytes: &[u8], start: usize, end: usize) -> Option<u16> {
    bytes
        .get(start..end)?
        .iter()
        .try_fold(0_u16, |value, byte| {
            value
                .checked_mul(10)?
                .checked_add(u16::from(byte.checked_sub(b'0')?))
        })
}

impl Serialize for ProtocolRevision {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for ProtocolRevision {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        value.parse().map_err(serde::de::Error::custom)
    }
}

/// Why a public revision string was refused.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum RevisionError {
    /// It was not the closed date shape.
    #[error("protocol revision {0:?} is not YYYY-MM-DD")]
    Shape(String),
    /// Its numeric month or day was outside the structural calendar bounds.
    #[error("protocol revision {0:?} is not a valid calendar date")]
    Calendar(String),
}

/// Select the newest common finalized revision independently of either list's order.
#[must_use]
pub fn negotiate(
    client: &[ProtocolRevision],
    server: &[ProtocolRevision],
) -> Option<ProtocolRevision> {
    client
        .iter()
        .filter(|candidate| server.contains(candidate))
        .max()
        .copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn revision_round_trips_as_its_finalization_date() {
        let revision: ProtocolRevision = "2026-08-13".parse().expect("valid revision");
        assert_eq!(revision, REVISION_2026_08_13);
        assert_eq!(revision.to_string(), "2026-08-13");
        assert_eq!(
            serde_json::to_string(&revision).expect("serializable"),
            r#""2026-08-13""#
        );
    }

    #[test]
    fn finalized_revisions_are_newest_first() {
        assert_eq!(FINALIZED_REVISIONS.first(), Some(&REVISION_2026_08_27));
        assert!(
            FINALIZED_REVISIONS
                .windows(2)
                .all(|pair| matches!(pair, [newer, older] if newer > older))
        );
    }

    #[test]
    fn malformed_and_structurally_impossible_dates_are_refused() {
        for value in ["2026-8-13", "2026-13-01", "2026-01-00", "latest"] {
            assert!(
                value.parse::<ProtocolRevision>().is_err(),
                "accepted {value:?}"
            );
        }
    }

    #[test]
    fn negotiation_selects_the_newest_common_revision_without_trusting_order() {
        let older = ProtocolRevision::new(2026, 5, 1);
        let current = REVISION_2026_08_13;
        let future = ProtocolRevision::new(2026, 12, 1);
        assert_eq!(
            negotiate(&[older, future, current], &[current, older]),
            Some(current)
        );
        assert_eq!(negotiate(&[future], &[current, older]), None);
    }
}
