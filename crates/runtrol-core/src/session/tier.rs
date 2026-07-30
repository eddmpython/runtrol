//! Cold, warm, hot: how many sessions may cost a process.
//!
//! A thousand sessions in a list must not mean a thousand child processes. The list is built by reading the
//! providers' own stores, and a child exists only for a session somebody is actually working in.
//!
//! # The three tiers, and what each one costs runtrol
//!
//! | Tier | What exists | runtrol's own cost |
//! |---|---|---:|
//! | cold | a row read from a provider's store | 256 B |
//! | warm | a reader open on the provider's file | 8 KiB |
//! | hot | a child process, bound and running | 128 KiB |
//!
//! Those are runtrol's numbers, and they are the small half. Measured on this machine, one of these CLIs is a
//! 265 MB executable with observed working sets of 110, 215, and 699 MB. **That memory is not runtrol's and
//! cannot be reduced by runtrol.** What runtrol decides is how many of them exist at once, which is why the
//! hot bound is about the operator's machine rather than about the daemon's own budget.
//!
//! # Why the list is not built by asking
//!
//! Measured: one CLI answers a list query in 39.9 seconds, and reading the same information out of its own
//! files takes 4.4 milliseconds. Nine thousand times. The thin rule and the fast answer point at the same
//! design, which does not happen often enough to waste.
//!
//! # What may be evicted, and what may not
//!
//! A session with a turn running is never evicted. Ending a turn to save memory would throw away work the
//! operator is waiting for, and the memory in question is mostly the child's rather than runtrol's anyway. When
//! every hot session is busy, a new one is refused with a reason instead of taking a running one's place.

use runtrol_provider::{SessionId, WallMs};

/// What runtrol holds for a session it has only read about.
pub const COLD_BYTES: usize = 256;

/// What runtrol holds for a session with a reader open on the provider's file.
pub const WARM_BYTES: usize = 8 * 1024;

/// What runtrol holds for a session with a child process bound.
///
/// runtrol's own share. The child's working set is measured in hundreds of megabytes and belongs to the child.
pub const HOT_BYTES: usize = 128 * 1024;

/// How many sessions may have a child process at once.
///
/// Not a limit on runtrol's memory: eight hot sessions cost runtrol a megabyte, which is nothing. It is a limit
/// on the operator's machine. At the measured working sets of these CLIs (110 to 699 MB each), eight of them is
/// one to five gigabytes, and that is the ceiling worth respecting. Eight is the number the memory contract
/// models, so it is the number here rather than a second opinion.
pub const MAX_HOT: usize = 8;

/// How much of a session exists right now.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Tier {
    /// A row, read from the provider's own store. No process, no open file.
    Cold,
    /// A reader is open on the provider's file. Still no process.
    Warm,
    /// A child process is bound.
    Hot,
}

impl Tier {
    /// What runtrol holds for a session in this tier.
    #[must_use]
    pub const fn bytes(self) -> usize {
        match self {
            Self::Cold => COLD_BYTES,
            Self::Warm => WARM_BYTES,
            Self::Hot => HOT_BYTES,
        }
    }

    /// Whether a child process exists.
    #[must_use]
    pub const fn has_a_process(self) -> bool {
        matches!(self, Self::Hot)
    }

    /// A name for a list and for a message.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Cold => "cold",
            Self::Warm => "warm",
            Self::Hot => "hot",
        }
    }
}

/// A hot session, for the purpose of deciding which one gives way.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct HotSession {
    /// Which session.
    pub session: SessionId,
    /// When anything last arrived from it.
    pub last_seen: WallMs,
    /// A turn is running.
    pub busy: bool,
}

/// Why a session cannot become hot right now.
#[derive(Clone, Copy, PartialEq, Eq, Debug, thiserror::Error)]
#[error(
    "all {held} sessions with a running process are busy, so starting another would have to interrupt one"
)]
pub struct NoRoom {
    /// How many are held.
    pub held: usize,
}

/// What has to happen for another session to become hot.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Admit {
    /// There is room.
    Straight,
    /// This one gives way first.
    Evicting {
        /// The session to detach.
        session: SessionId,
    },
}

/// Decide whether another session may become hot, and what gives way if so.
///
/// # Errors
///
/// [`NoRoom`] when every hot session has a turn running. Refused rather than resolved by force: ending a turn
/// to make room throws away work the operator is waiting for, and the memory it would save is mostly the
/// child's rather than runtrol's.
pub fn admit(held: &[HotSession]) -> Result<Admit, NoRoom> {
    if held.len() < MAX_HOT {
        return Ok(Admit::Straight);
    }

    // Least recently used among the ones not working. Least recently used rather than first in: the session an
    // operator has not looked at for an hour is the one they will notice least, and the one they were in a
    // minute ago is the one they are coming back to.
    let victim = held
        .iter()
        .filter(|candidate| !candidate.busy)
        .min_by_key(|candidate| candidate.last_seen.as_millis());

    match victim {
        Some(chosen) => Ok(Admit::Evicting {
            session: chosen.session,
        }),
        None => Err(NoRoom { held: held.len() }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(millis: u64) -> WallMs {
        WallMs::from_millis(millis)
    }

    fn hot(last_seen: u64, busy: bool) -> HotSession {
        HotSession {
            session: SessionId::now(),
            last_seen: at(last_seen),
            busy,
        }
    }

    fn full(busy: bool) -> Vec<HotSession> {
        (0..MAX_HOT)
            .map(|index| hot(1_000 + u64::try_from(index).expect("a small index"), busy))
            .collect()
    }

    #[test]
    fn a_thousand_sessions_do_not_cost_a_thousand_processes() {
        // The reason the tiers exist. A cold list of a thousand is a quarter of a megabyte, and the same
        // thousand as children would be hundreds of gigabytes of the operator's memory.
        let thousand_cold = 1_000 * Tier::Cold.bytes();
        assert!(
            thousand_cold < 1024 * 1024,
            "a thousand cold rows is {thousand_cold} bytes"
        );
        assert!(!Tier::Cold.has_a_process());
        assert!(!Tier::Warm.has_a_process());
        assert!(Tier::Hot.has_a_process());
    }

    #[test]
    fn the_tiers_cost_more_the_more_of_a_session_exists() {
        assert!(Tier::Cold.bytes() < Tier::Warm.bytes());
        assert!(Tier::Warm.bytes() < Tier::Hot.bytes());
        assert!(Tier::Cold < Tier::Warm);
        assert!(Tier::Warm < Tier::Hot);
    }

    #[test]
    fn the_numbers_are_the_ones_the_memory_contract_fixed() {
        // A second opinion about these numbers is how a contract becomes a suggestion. If they change, they
        // change here and in the contract at the same time, and only downwards.
        assert_eq!(COLD_BYTES, 256);
        assert_eq!(WARM_BYTES, 8 * 1024);
        assert_eq!(HOT_BYTES, 128 * 1024);
        assert_eq!(MAX_HOT, 8);
    }

    #[test]
    fn there_is_room_until_there_is_not() {
        let nearly = full(false)
            .into_iter()
            .take(MAX_HOT - 1)
            .collect::<Vec<_>>();
        assert_eq!(admit(&nearly), Ok(Admit::Straight));
        assert_eq!(admit(&[]), Ok(Admit::Straight));
    }

    #[test]
    fn the_session_nobody_has_looked_at_gives_way_first() {
        // The one an operator has not touched for an hour is the one they will notice least. The one they were
        // in a minute ago is the one they are coming back to.
        let mut held = full(false);
        let forgotten = hot(1, false);
        if let Some(slot) = held.first_mut() {
            *slot = forgotten;
        }

        assert_eq!(
            admit(&held),
            Ok(Admit::Evicting {
                session: forgotten.session
            })
        );
    }

    #[test]
    fn a_session_with_a_turn_running_is_never_the_one_to_give_way() {
        // Ending a turn to save memory throws away work the operator is waiting for, and most of that memory
        // is the child's rather than runtrol's.
        let mut held = full(false);
        let oldest_but_working = hot(1, true);
        if let Some(slot) = held.first_mut() {
            *slot = oldest_but_working;
        }

        match admit(&held) {
            Ok(Admit::Evicting { session }) => assert_ne!(
                session, oldest_but_working.session,
                "the busy session was chosen despite being the oldest"
            ),
            other => panic!("expected an eviction of an idle session, got {other:?}"),
        }
    }

    #[test]
    fn when_everything_is_busy_the_new_session_is_refused_with_a_reason() {
        // Refused rather than resolved by force. An operator told why can wait or stop something; one whose
        // running turn was interrupted to make room has lost work and does not know it.
        let all_working = full(true);
        match admit(&all_working) {
            Err(refusal) => {
                assert_eq!(refusal.held, MAX_HOT);
                assert!(refusal.to_string().contains("busy"), "{refusal}");
            }
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    #[test]
    fn each_tier_can_name_itself() {
        for tier in [Tier::Cold, Tier::Warm, Tier::Hot] {
            assert!(!tier.name().is_empty());
        }
    }
}
