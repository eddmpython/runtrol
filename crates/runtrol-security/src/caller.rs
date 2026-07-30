//! Who is asking, and what that alone entitles them to.
//!
//! # This is established, never claimed
//!
//! A caller does not say who it is. Where a request arrived decides that: a frame on the local endpoint came from
//! somebody at the machine, because that endpoint is inside a directory only the operator can enter and refuses
//! anything off the machine. A frame from a paired device came from that device, because pairing is what issued
//! the identifier. Neither is a field in the request, and that is the whole design: a field could be written by
//! whoever sent it.
//!
//! # Default deny, as a shape rather than a promise
//!
//! [`Caller::may`] answers for a device by asking the ledger, and an empty ledger holds nothing. A device that
//! paired and was granted nothing can do nothing, and there is no path here that treats an unknown device
//! generously. What a device may do grows only through [`crate::GrantLedger::grant`], which takes a
//! [`crate::PcPresence`], which one crate can mint.
//!
//! # Why somebody at the machine is not checked against a ledger
//!
//! Because they can do all of it anyway. Somebody at the operator's keyboard can start the CLI themselves, read
//! any file the agent could read, and stop any process. A permission check against them would be theatre: it
//! would refuse the supported path while the unsupported one stays open, which teaches people to work around
//! runtrol rather than through it.
//!
//! What still applies to them is everything that is not a permission: the workspace deny list, the argument
//! checks, and the containment. Those are about what runtrol will do, not about who asked.

use crate::error::SecurityError;
use crate::grant::GrantLedger;
use crate::id::DeviceId;
use crate::scope::DeviceScope;

/// Where a request came from.
///
/// Not `Copy` on purpose. A caller is carried, not scattered: making it cheap to duplicate is how the same
/// identity ends up in two places that later disagree about what it was allowed.
#[derive(Clone, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum Caller {
    /// Somebody at the machine runtrol runs on, through the local endpoint.
    AtTheMachine,

    /// A device that paired, reaching in from somewhere else.
    Device {
        /// Which device, as pairing issued it.
        device: DeviceId,
    },
}

impl Caller {
    /// Whether this caller may do something that needs `scope`.
    ///
    /// # Errors
    ///
    /// [`SecurityError::ScopeMissing`] when a device does not hold it. Somebody at the machine is never refused
    /// here, for the reason in the module notes.
    pub fn may(&self, scope: DeviceScope, ledger: &GrantLedger) -> Result<(), SecurityError> {
        match self {
            Self::AtTheMachine => Ok(()),
            Self::Device { device } => ledger.require(*device, scope),
        }
    }

    /// Whether this caller is at the machine.
    ///
    /// The one question that decides a capability no grant can carry. Asked as its own method rather than by
    /// matching at each call site, so that adding a way of arriving cannot silently make it count as presence:
    /// a new variant is not this one until somebody writes it here.
    #[must_use]
    pub const fn is_at_the_machine(&self) -> bool {
        matches!(self, Self::AtTheMachine)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a_device() -> DeviceId {
        DeviceId::now()
    }

    #[test]
    fn a_device_that_was_granted_nothing_may_do_nothing() {
        // Default deny, as the shape of the thing rather than a rule somebody remembered to apply. A ledger with
        // no entry for a device answers no to every question about it.
        let ledger = GrantLedger::new();
        let caller = Caller::Device { device: a_device() };

        for scope in DeviceScope::EVERY_PLAIN {
            assert!(
                caller.may(*scope, &ledger).is_err(),
                "{} was allowed with nothing granted",
                scope.name()
            );
        }
    }

    #[test]
    fn a_device_holds_what_it_was_granted_and_nothing_beside_it() {
        let mut ledger = GrantLedger::new();
        let device = a_device();
        // Built directly rather than through a console, for the reason the ledger's own tests give: routing
        // every test through a process-wide singleton serialises them for no added coverage, and the challenge
        // path has its own tests.
        let witness = crate::PcPresence::for_tests(crate::GrantRequest::DeviceScopes {
            device,
            scopes: vec![DeviceScope::SessionList],
        });
        ledger
            .grant(device, &[DeviceScope::SessionList], &witness)
            .expect("granted");

        let caller = Caller::Device { device };
        assert!(caller.may(DeviceScope::SessionList, &ledger).is_ok());
        assert!(
            caller.may(DeviceScope::SessionStart, &ledger).is_err(),
            "holding one scope must not imply another"
        );
    }

    #[test]
    fn somebody_at_the_machine_is_not_checked_against_a_ledger() {
        // They can do all of it without runtrol. Refusing the supported path while the unsupported one stays open
        // teaches people to work around runtrol rather than through it.
        let ledger = GrantLedger::new();
        let caller = Caller::AtTheMachine;
        for scope in DeviceScope::EVERY_PLAIN {
            assert!(caller.may(*scope, &ledger).is_ok(), "{}", scope.name());
        }
        assert!(caller.is_at_the_machine());
    }

    #[test]
    fn arriving_from_anywhere_else_is_never_presence() {
        // The question that decides the capabilities no grant can carry. A device holding every scope there is
        // still is not somebody standing at the machine.
        let caller = Caller::Device { device: a_device() };
        assert!(!caller.is_at_the_machine());
    }
}
