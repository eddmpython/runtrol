//! The seam. The kernel names no provider, ever.
//!
//! A manifest names a `kind`; a kind selects code. This file defines what "selects code" means and holds
//! not one kind string of its own. That absence is the mechanism behind "adding a provider does not touch
//! the kernel": the kernel cannot mention a provider, because it has nowhere to put the name.
//!
//! # Why a table of values and not a registration macro
//!
//! Measured: a distributed-slice registry returns a silently empty slice across a crate boundary unless the
//! binary already references the registering crate. A driver that fails to register then presents as a
//! provider that is simply absent, which is the silent failure this repository refuses above all others.
//!
//! When the binary does reference the crate, a plain table of values is strictly better: the same cost, no
//! macro, and a missing driver is a compile error rather than an empty list.
//!
//! # Why "known but not served" is a value
//!
//! A build may know a kind exists and be unable to serve it: a driver behind a feature, or one that has not
//! been written. Answering "unknown kind" there would send the operator hunting for a typo they did not
//! make. So a table entry may carry a reason instead of a driver, and the reason reaches the operator.

use runtrol_provider::Kind;

/// What a build can do about one kind.
///
/// Held as data so that a table of these is a `const`. Whatever constructs a driver arrives with the first
/// driver; until then an entry declares what a kind is and whether this build serves it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct KindEntry {
    /// The kind, as a manifest spells it.
    pub kind: &'static str,
    /// Why this build cannot serve it, or `None` when it can.
    ///
    /// A sentence an operator reads, not an error code. The difference between "this build has no generic
    /// driver for that protocol" and "unknown kind" is the difference between an answer and a wild goose
    /// chase.
    pub unavailable: Option<&'static str>,
}

impl KindEntry {
    /// Whether this build can serve the kind.
    #[must_use]
    pub const fn is_available(&self) -> bool {
        self.unavailable.is_none()
    }
}

/// Every kind a build knows about.
///
/// Borrowed rather than owned: the entries are `const` data in whatever crate ships drivers, and the kernel
/// only ever looks things up in them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct KindTable {
    /// The entries, in the order the driver crate declares them.
    entries: &'static [KindEntry],
}

impl KindTable {
    /// Wrap a driver crate's table.
    #[must_use]
    pub const fn new(entries: &'static [KindEntry]) -> Self {
        Self { entries }
    }

    /// An empty table, for a build with no drivers wired in yet.
    ///
    /// Not a fallback and never a default anywhere: an empty table answers every lookup with "unknown",
    /// which is correct for a build that ships no drivers and wrong for one that does. Composing the real
    /// table is the daemon's job, and this exists so the kernel's own tests can run without one.
    pub const EMPTY: Self = Self::new(&[]);

    /// What this build can do about a kind.
    #[must_use]
    pub fn lookup(&self, kind: &Kind) -> KindStatus {
        match self
            .entries
            .iter()
            .find(|entry| entry.kind == kind.as_str())
        {
            None => KindStatus::Unknown,
            Some(entry) => match entry.unavailable {
                None => KindStatus::Available,
                Some(why) => KindStatus::Unavailable { why },
            },
        }
    }

    /// Every kind in the table.
    pub fn kinds(&self) -> impl Iterator<Item = &'static str> {
        self.entries.iter().map(|entry| entry.kind)
    }

    /// How many kinds the table declares.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the table declares nothing.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// What a build can do about one kind, as an answer to a question.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum KindStatus {
    /// This build serves it.
    Available,
    /// This build knows the kind and cannot serve it.
    Unavailable {
        /// The sentence to show the operator.
        why: &'static str,
    },
    /// No table entry names it.
    ///
    /// The only case where "check your spelling" is honest advice.
    Unknown,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A table shaped like a real one: two served kinds and one known and not served.
    const TABLE: &[KindEntry] = &[
        KindEntry {
            kind: "example-structured",
            unavailable: None,
        },
        KindEntry {
            kind: "example-lines",
            unavailable: None,
        },
        KindEntry {
            kind: "example-heavy",
            unavailable: Some("this build does not include the heavy transport"),
        },
    ];

    fn kind(text: &str) -> Kind {
        Kind::parse(text).expect("the test's own kind must be valid")
    }

    #[test]
    fn the_kernel_writes_no_kind_of_its_own() {
        // The mechanism behind "adding a provider does not touch the kernel". The table comes from outside;
        // this file has nowhere to put a provider's name. A gate asserts the same thing across the crate,
        // and this is the unit-level statement of it.
        assert!(KindTable::EMPTY.is_empty());
        assert_eq!(KindTable::EMPTY.kinds().count(), 0);
    }

    #[test]
    fn a_served_kind_resolves() {
        let table = KindTable::new(TABLE);
        assert_eq!(
            table.lookup(&kind("example-structured")),
            KindStatus::Available
        );
        assert_eq!(table.len(), 3);
    }

    #[test]
    fn a_known_kind_this_build_cannot_serve_says_so_instead_of_denying_it_exists() {
        // Answering "unknown kind" here would send the operator looking for a typo they did not make.
        match KindTable::new(TABLE).lookup(&kind("example-heavy")) {
            KindStatus::Unavailable { why } => {
                assert!(why.contains("this build"), "{why}");
            }
            other => panic!("expected a named unavailability, got {other:?}"),
        }
    }

    #[test]
    fn a_kind_nobody_declared_is_unknown() {
        assert_eq!(
            KindTable::new(TABLE).lookup(&kind("nothing-declares-this")),
            KindStatus::Unknown
        );
    }

    #[test]
    fn an_empty_table_answers_unknown_and_not_available() {
        // A build with no drivers must not appear to serve everything. This is why the empty table is
        // never a default anywhere: it is an honest answer for a build that ships nothing.
        assert_eq!(
            KindTable::EMPTY.lookup(&kind("example-structured")),
            KindStatus::Unknown
        );
    }

    #[test]
    fn availability_is_readable_from_an_entry_alone() {
        assert!(
            KindEntry {
                kind: "x",
                unavailable: None
            }
            .is_available()
        );
        assert!(
            !KindEntry {
                kind: "x",
                unavailable: Some("no")
            }
            .is_available()
        );
    }
}
