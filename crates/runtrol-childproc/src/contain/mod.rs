//! When runtrol dies, the coding agents it started die with it.
//!
//! This is the crate's reason for existing. A supervised agent has been granted write access to the
//! operator's project, and an orphan that keeps running after runtrol is gone is writing to files nobody
//! is watching. There is no version of that which is acceptable, so containment is established before any
//! child is started rather than cleaned up afterwards.
//!
//! # What each platform can actually promise
//!
//! The promise is not the same everywhere, and pretending it is would be the more comfortable lie.
//! [`Containment::strength`] says which one the operator got, so a surface can show it rather than imply a
//! guarantee the platform does not make.
//!
//! - **Windows.** [`Strength::EvenIfKilled`]. A job object with kill-on-close holds every descendant, and
//!   the kernel enforces it when the last handle closes. That happens whether runtrol exits cleanly, panics,
//!   or is killed outright.
//! - **Unix without durable tracking.** [`Strength::CleanShutdownOnly`]. A direct process group handles shutdowns
//!   runtrol can see coming.
//! - **Unix with durable tracking.** [`Strength::EvenIfKilled`]. A stable group keeper observes its private daemon
//!   control channel closing and atomically terminates its own group. Startup recovery never signals a numeric PID or
//!   process-group identifier.
//!
//! # Holding it is what makes it work
//!
//! [`Containment`] is a guard. Holding the value is what holds the containment, and dropping it is the kill
//! switch: on Windows the job's last handle closes and the kernel terminates everything inside. So the
//! daemon establishes one at startup and keeps it for the process lifetime.
//!
//! That also makes it the mechanism behind the one capability the security posture requires to work without
//! any permission at all: killing every session from anywhere. [`Containment::terminate_all`] is that,
//! and it consults nothing.
//!
//! # Establishing this cannot be unit tested, and finding that out was the point
//!
//! The first version of the tests below called [`Containment::establish`] and let the guard drop. On Windows
//! that assigns the **test process** to the kill-on-close job, so the drop closed the job's last handle and
//! the kernel terminated the test runner. The test did not fail; it vanished mid-run.
//!
//! Which is the mechanism working exactly as designed, and it means establishing containment is a
//! process-lifetime action that no in-process test can exercise. So the unit tests here cover only what has no
//! side effects, [`Containment::platform_strength`] exists so a caller can ask what this platform promises
//! without establishing anything, and the guarantee itself is proven by `tests/containment.rs`, which drives a
//! real helper process and kills it the hard way.

#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as platform;

#[cfg(unix)]
mod unix;
#[cfg(unix)]
use unix as platform;

#[cfg(unix)]
mod bootstrap;
#[cfg(unix)]
mod identity;
#[cfg(unix)]
mod registry;
mod tracked;

use std::process::Command;

use crate::error::SpawnError;

#[cfg(unix)]
pub use bootstrap::{BOOTSTRAP_ARGUMENT, bootstrap_if_requested};
pub use tracked::{ChildGuard, TrackedChild, TrackedCommand};

/// What containment this platform can actually enforce.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strength {
    /// Children die even if runtrol is killed without warning.
    EvenIfKilled,
    /// Children die on any shutdown runtrol can see coming, and survive one it cannot.
    ///
    /// The gap is real and cannot be closed from inside the process being killed. What closes it is
    /// noticing the orphans on the next start.
    CleanShutdownOnly {
        /// Why this platform cannot promise more.
        why: &'static str,
    },
}

impl Strength {
    /// Whether an unclean kill of runtrol leaves agents running.
    ///
    /// The question a surface asks before deciding whether to warn the operator.
    #[must_use]
    pub const fn survives_an_unclean_kill(&self) -> bool {
        matches!(self, Self::CleanShutdownOnly { .. })
    }
}

/// A guarantee that children die with this process.
///
/// Hold it for as long as children should live. Dropping it is deliberate and destructive: see the module
/// documentation.
#[derive(Debug)]
pub struct Containment {
    /// The platform's own mechanism, or nothing.
    inner: Inner,
    /// Durable process-group recovery, when production supplied its bounded guard directory.
    #[cfg(unix)]
    recovery: Option<registry::Registry>,
    /// Records the recovery pass kept because their recorded generation could not be confirmed.
    #[cfg(unix)]
    ambiguous_guards: usize,
}

/// What is actually holding the children.
#[derive(Debug)]
enum Inner {
    /// The platform's mechanism.
    Platform(platform::Containment),
    /// Nothing.
    Nothing,
}

impl Containment {
    /// A containment that holds nothing.
    ///
    /// For a caller that runs a child and deliberately adds no group to it: asking an installed CLI its
    /// version needs no job object, because the answer arrives in milliseconds and the run is killed when its
    /// handle is dropped. A command that only asks questions and never starts a session is the case this
    /// exists for.
    ///
    /// It is also the only containment a test can hold. [`Self::establish`] puts the calling process into the
    /// group it is about to kill, so a test that established one would terminate its own runner (measured,
    /// and the reason [`Self::platform_strength`] exists).
    ///
    /// What is given up is stated rather than implied: [`Self::strength`] reports that an unclean kill leaves
    /// children running, and [`Self::terminate_all`] refuses rather than reporting a success it did not
    /// achieve.
    #[must_use]
    pub const fn without_any() -> Self {
        Self {
            inner: Inner::Nothing,
            #[cfg(unix)]
            recovery: None,
            #[cfg(unix)]
            ambiguous_guards: 0,
        }
    }

    /// Establish containment for this process and everything it starts.
    ///
    /// Called once, at startup, before any child exists. Calling it later would leave whatever was already
    /// running outside the containment on some platforms, which is the kind of partial guarantee that reads
    /// as a full one.
    ///
    /// # Errors
    ///
    /// [`SpawnError::Containment`] when the platform refuses. Not recoverable and not worth working around:
    /// starting agents that cannot be contained is the outcome this whole module exists to prevent, so the
    /// daemon refuses to start rather than running with the guarantee quietly absent.
    pub fn establish() -> Result<Self, SpawnError> {
        Ok(Self {
            inner: Inner::Platform(platform::Containment::establish()?),
            #[cfg(unix)]
            recovery: None,
            #[cfg(unix)]
            ambiguous_guards: 0,
        })
    }

    /// Establish containment with durable Unix process-group recovery.
    ///
    /// The caller must already hold the daemon's exclusive store lock. That ordering prevents a second daemon from
    /// interpreting the first daemon's live children as orphans.
    ///
    /// On Windows the job object is already a kernel-owned unclean-exit boundary, so `directory` is accepted only to
    /// keep one construction surface and is otherwise unused.
    ///
    /// # Errors
    ///
    /// [`SpawnError::Containment`] when the guard directory cannot be made durable, an earlier process group cannot
    /// be reaped safely, or the platform containment cannot be established.
    pub fn establish_tracked(directory: &std::path::Path) -> Result<Self, SpawnError> {
        #[cfg(unix)]
        {
            let recovery = registry::Registry::open(directory)?;
            let ambiguous_guards = recovery.recover()?;
            Ok(Self {
                inner: Inner::Platform(platform::Containment::establish()?),
                recovery: Some(recovery),
                ambiguous_guards,
            })
        }
        #[cfg(windows)]
        {
            _ = directory;
            Self::establish()
        }
    }

    /// How many durable guard records the recovery pass kept because their generation is uncertain.
    ///
    /// A kept record stays in the bounded guard directory and is re-examined by every later pass;
    /// this count exists so a surface can state that condition instead of the daemon holding it
    /// silently.
    #[must_use]
    pub const fn ambiguous_guards(&self) -> usize {
        #[cfg(unix)]
        {
            self.ambiguous_guards
        }
        #[cfg(windows)]
        {
            0
        }
    }

    /// What this platform can enforce, without establishing anything.
    ///
    /// Free of side effects, unlike [`Self::establish`], so a surface can tell the operator what they are
    /// going to get before anything is set up, and a test can check the classification without terminating
    /// itself.
    #[must_use]
    pub const fn platform_strength() -> Strength {
        platform::Containment::platform_strength()
    }

    /// What this containment actually enforces.
    ///
    /// Not the same question as [`Self::platform_strength`]: one that holds nothing enforces nothing, whatever
    /// the platform is capable of, and a surface asking what the operator is going to get has to be told the
    /// truth about the value it is holding.
    #[must_use]
    pub const fn strength(&self) -> Strength {
        #[cfg(unix)]
        if self.recovery.is_some() && matches!(&self.inner, Inner::Platform(_)) {
            return Strength::EvenIfKilled;
        }
        match &self.inner {
            Inner::Platform(_) => Self::platform_strength(),
            Inner::Nothing => Strength::CleanShutdownOnly {
                why: "this containment holds nothing. a child dies when the handle to it is dropped, \
                      and an unclean kill of runtrol leaves it running",
            },
        }
    }

    /// Prepare a command so its child is inside this containment.
    ///
    /// Must be called on every command before it is spawned. On the platform where containment is
    /// inherited this does nothing, and it is still called, so that no call site has to know which platform
    /// needs what.
    pub fn prepare(&self, command: &mut Command) {
        match &self.inner {
            Inner::Platform(platform) => platform.prepare(command),
            Inner::Nothing => {}
        }
    }

    /// Kill every contained process now.
    ///
    /// The panic button. The security posture requires killing every session to work from anywhere with no
    /// permission at all, and this consults nothing: no ledger, no scope, no configuration. The worst thing
    /// a hostile caller can achieve through it is stopping work, which is the safe direction.
    ///
    /// # Errors
    ///
    /// [`SpawnError::Containment`] when the platform's own call fails. Reported rather than swallowed: an
    /// operator who pressed the panic button has to know whether it worked.
    pub fn terminate_all(&self) -> Result<(), SpawnError> {
        #[cfg(unix)]
        if let Some(recovery) = &self.recovery {
            return recovery.terminate_all();
        }
        match &self.inner {
            Inner::Platform(platform) => platform.terminate_all(),
            // A panic button that did nothing must not report success. Same rule as the platform that
            // cannot enforce this: the operator who pressed it has to know.
            Inner::Nothing => Err(SpawnError::Containment {
                doing: "terminating every child",
                detail: "this containment holds nothing".to_owned(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Nothing here calls `establish`. On Windows that puts the test process itself into a kill-on-close job,
    // and letting the guard drop terminates the runner: measured, and the reason `platform_strength` exists.
    // The guarantee is proven in `tests/containment.rs`, which has a process it is allowed to kill.

    #[test]
    fn this_platform_declares_what_it_can_enforce() {
        let strength = Containment::platform_strength();
        if cfg!(windows) {
            assert_eq!(
                strength,
                Strength::EvenIfKilled,
                "this platform has a kernel mechanism for the unclean case"
            );
            assert!(!strength.survives_an_unclean_kill());
        } else {
            assert!(
                matches!(strength, Strength::CleanShutdownOnly { .. }),
                "no parent-death signal and no job object here, and saying so is the point"
            );
            assert!(strength.survives_an_unclean_kill());
        }
    }

    #[test]
    fn a_containment_can_be_shared_by_whatever_starts_children() {
        // One containment per process, held by every driver a runtime may move between threads. Without this a
        // driver would need its own, and a containment per driver is the partial guarantee that reads as a full one.
        fn needs_both<T: Send + Sync>() {}
        needs_both::<Containment>();
        needs_both::<std::sync::Arc<Containment>>();
    }

    #[test]
    fn a_containment_that_holds_nothing_says_so_rather_than_claiming_the_platform() {
        // The platform is capable of more, and this value is not doing it. A surface asking what the operator
        // gets has to be told about the value in hand, not about the machine.
        let nothing = Containment::without_any();
        match nothing.strength() {
            Strength::CleanShutdownOnly { why } => {
                assert!(why.contains("holds nothing"), "{why}");
            }
            other @ Strength::EvenIfKilled => {
                panic!("expected the weaker promise, got {other:?}")
            }
        }
        assert!(nothing.strength().survives_an_unclean_kill());
    }

    #[test]
    fn a_panic_button_that_did_nothing_does_not_report_success() {
        // The one capability the security posture requires to work from anywhere with no permission at all. An
        // operator who pressed it and was told it worked, when it did not, is worse off than one told it failed.
        let nothing = Containment::without_any();
        match nothing.terminate_all() {
            Err(SpawnError::Containment { doing, detail }) => {
                assert!(doing.contains("terminating"), "{doing}");
                assert!(detail.contains("holds nothing"), "{detail}");
            }
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    #[test]
    fn a_containment_that_holds_nothing_leaves_a_command_alone() {
        // It is the absence of containment, not a different one. Preparing a command through it must add
        // nothing, or the name would be a lie in the other direction.
        let mut command = Command::new("does-not-matter");
        Containment::without_any().prepare(&mut command);
        assert_eq!(command.get_program(), "does-not-matter");
        assert_eq!(command.get_args().count(), 0);
    }

    #[test]
    fn the_weaker_promise_says_why() {
        // An operator told "children might survive" needs the reason, or the warning is noise.
        let weak = Strength::CleanShutdownOnly {
            why: "no parent-death signal on this platform",
        };
        match weak {
            Strength::CleanShutdownOnly { why } => assert!(!why.is_empty()),
            Strength::EvenIfKilled => {
                panic!("constructed the weaker promise, got the stronger one")
            }
        }
    }

    #[test]
    fn only_the_weaker_promise_admits_orphans() {
        assert!(!Strength::EvenIfKilled.survives_an_unclean_kill());
        assert!(
            Strength::CleanShutdownOnly { why: "any reason" }.survives_an_unclean_kill(),
            "the whole point of the weaker variant is that it admits the gap"
        );
    }
}
