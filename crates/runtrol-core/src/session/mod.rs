//! What a session is, before anything can start one.
//!
//! Three things that answer to nothing outside this module:
//!
//! - [`mint`] the two names a session has, and which of them runtrol owns
//! - [`state`] the one place a session's state may change
//! - [`tier`] how many sessions may cost a child process, and which one gives way
//!
//! # Why starting and listing are not here yet
//!
//! Starting a session means driving a provider, and listing one means reading a provider's own store. Both need
//! the behavioural contract a driver implements, and that contract arrives with its first implementation: a
//! trait with no implementor is a guess about a shape the implementor gets to decide.
//!
//! What is here is everything that is true regardless of which provider it is, which is exactly the part that
//! can be finished and proved first.

pub mod mint;
pub mod state;
pub mod tier;

pub use mint::Identity;
pub use state::{CloseReason, FailureCode, Lifecycle, Observed, Refused, SessionState};
pub use tier::{Admit, HotSession, MAX_HOT, NoRoom, Tier};

#[cfg(test)]
mod tests {
    use runtrol_provider::{ProviderId, TurnId, WallMs};

    use super::*;

    fn now() -> WallMs {
        WallMs::from_millis(1_700_000_000_000)
    }

    #[test]
    fn what_exists_follows_from_what_the_session_is_doing() {
        // The tier is not a second, independent fact. A session with a driver bound has a process, and one
        // without does not, so two places recording it would be two places to disagree.
        let bound = [
            Lifecycle::Starting,
            Lifecycle::Idle,
            Lifecycle::Busy {
                turn: TurnId { epoch: 0, index: 0 },
            },
        ];
        for lifecycle in bound {
            assert!(
                lifecycle.is_attached(),
                "{} should be attached",
                lifecycle.name()
            );
            assert!(Tier::Hot.has_a_process());
        }

        let unbound = [
            Lifecycle::Detached,
            Lifecycle::Failed {
                at: now(),
                code: FailureCode::ChildExited,
                detail: "exit 1".to_owned(),
            },
            Lifecycle::Closed {
                at: now(),
                reason: CloseReason::Requested,
            },
        ];
        for lifecycle in unbound {
            assert!(
                !lifecycle.is_attached(),
                "{} should not be attached",
                lifecycle.name()
            );
        }
    }

    #[test]
    fn a_session_can_be_named_before_it_costs_anything() {
        // The cold tier is a row and a name. Nothing about having a name implies a process, which is what makes
        // a thousand-session list affordable.
        let identity = Identity::mint(ProviderId::parse("example").expect("valid"));
        let state = SessionState::new(now());

        assert_eq!(state.lifecycle(), &Lifecycle::Detached);
        assert!(!state.lifecycle().is_attached());
        assert!(identity.native().is_none());
        assert_eq!(Tier::Cold.bytes(), tier::COLD_BYTES);
    }
}
