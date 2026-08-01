//! What each request needs before anybody is allowed to make it.
//!
//! # Why the mapping lives here and not beside either vocabulary
//!
//! It joins two things that must not know about each other. The requests are the wire's vocabulary and the scopes
//! are the security crate's, and an edge either way would put one product's shape inside the other: the wire would
//! learn what a permission is, or the wall would learn what a request is and could no longer be a leaf. Assembly
//! is where two vocabularies are allowed to meet, and this is assembly.
//!
//! # Nothing is allowed by falling through
//!
//! A request this build does not recognise is refused by name. That is the whole reason this is a table rather
//! than a check written into each handler: a handler somebody forgets to write is a handler with no check, and it
//! looks exactly like one that decided the request was harmless. Here, forgetting means the request is refused,
//! and a gate holds the table against the request vocabulary so that forgetting is caught before it ships.
//!
//! # The two requests that need no permission, and why each is safe
//!
//! Agreeing a wire format decides nothing and touches nothing; refusing it would mean refusing a caller before
//! they can be told what they are refused for.
//!
//! Stopping every agent on the machine is deliberately open. The security posture requires it to work from
//! anywhere with no permission at all, and the worst a hostile caller achieves through it is that work stops,
//! which is the safe direction. A panic button behind a permission is a panic button the operator does not have
//! when they need it.

use runtrol_ipc::wire::Request;
use runtrol_security::{Caller, DeviceScope, GrantLedger, SecurityError};

/// What a request needs.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Needed {
    /// A device must hold this scope. Somebody at the machine always may.
    Scope(DeviceScope),

    /// Answering needs either low or high approval authority at the boundary.
    ///
    /// The pending provider request decides which one is sufficient after this wall. Risk is deliberately absent
    /// from the wire, so a caller cannot lower the required authority by changing request data.
    ApprovalResponse,

    /// Anyone may, and the sentence says why that is safe.
    ///
    /// Carried rather than implied, because "no permission" is the one answer that has to justify itself, and a
    /// bare variant would let a later request join this arm without anybody writing down why.
    Anyone(&'static str),

    /// This build has no rule for it.
    ///
    /// A request that arrived after this daemon was made. Refused by name, because a mapping that guessed would
    /// be guessing about authority.
    Unknown,
}

/// What this request needs before anybody may make it.
///
/// The wildcard arm answers [`Needed::Unknown`], so a request added to the wire after this build is refused
/// rather than allowed. `Request` is open ended on purpose, which is what makes that arm necessary and what makes
/// `scopeWall.py` necessary beside it: the compiler cannot tell anyone here that a variant went unmapped.
#[must_use]
pub fn needed(request: &Request) -> Needed {
    match request {
        Request::Hello { .. } => {
            Needed::Anyone("agreeing a wire format decides nothing and touches nothing")
        }

        Request::List => Needed::Scope(DeviceScope::SessionList),
        Request::Models { .. } => Needed::Scope(DeviceScope::ConfigRead),
        Request::Start { .. } => Needed::Scope(DeviceScope::SessionStart),
        Request::Resume { .. } => Needed::Scope(DeviceScope::SessionResume),
        Request::Prompt { .. } => Needed::Scope(DeviceScope::SessionInputWrite),
        Request::AnswerApproval { .. } => Needed::ApprovalResponse,
        Request::Watch { .. } => Needed::Scope(DeviceScope::SessionOutputRead),
        // Both take work away from a turn that is running, so both need the same thing. Written apart rather
        // than merged: they are different requests that agree today, and merging them would hide the day one of
        // them needs something else.
        #[expect(
            clippy::match_same_arms,
            reason = "interrupting and closing are different requests that need the same scope today"
        )]
        Request::Interrupt { .. } => Needed::Scope(DeviceScope::SessionStop),
        Request::Close { .. } => Needed::Scope(DeviceScope::SessionStop),

        Request::StopEverything => Needed::Anyone(
            "the security posture requires the panic button to work from anywhere with no permission, and the \
             worst it achieves is that work stops",
        ),

        _ => Needed::Unknown,
    }
}

/// Whether this caller may make this request.
///
/// # Errors
///
/// [`SecurityError::ScopeMissing`] when a device does not hold what the request needs. A request this build has
/// no rule for produces [`WallRefusal::Unknown`], which is not a security error: nothing was denied to anybody,
/// the daemon simply does not know what was asked.
pub fn allowed(
    caller: &Caller,
    request: &Request,
    ledger: &GrantLedger,
) -> Result<(), WallRefusal> {
    match needed(request) {
        Needed::Anyone(_) => Ok(()),
        Needed::Scope(scope) => caller
            .may(scope, ledger)
            .map_err(|source| WallRefusal::Denied { source }),
        Needed::ApprovalResponse => match caller.may(DeviceScope::ApprovalRespondLow, ledger) {
            Ok(()) => Ok(()),
            Err(low) => match caller.may(DeviceScope::ApprovalRespondHigh, ledger) {
                Ok(()) => Ok(()),
                Err(_) => Err(WallRefusal::Denied { source: low }),
            },
        },
        Needed::Unknown => Err(WallRefusal::Unknown),
    }
}

/// Why the wall said no.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum WallRefusal {
    /// The caller does not hold what the request needs.
    #[error(transparent)]
    Denied {
        /// What the wall said.
        source: SecurityError,
    },

    /// This build has no rule for the request, so it cannot decide who may make it.
    ///
    /// Separate from a denial, because the two send an operator in different directions: one means ask for the
    /// permission, the other means the two ends are different builds.
    #[error("this daemon has no rule about who may make that request")]
    Unknown,
}

#[cfg(test)]
mod tests {
    use runtrol_provider::{ApprovalId, OptionId, SessionId};
    use runtrol_security::{DeviceId, GrantRequest, LocalConsole, PresenceChallenge};

    use super::*;

    /// Every request this build knows, one of each.
    ///
    /// Written out rather than generated, because the point of the list is that a person compared it against the
    /// wire vocabulary. `scopeWall.py` compares it again, mechanically, which is what catches the day nobody did.
    fn every_request() -> Vec<Request> {
        vec![
            Request::Hello {
                wire: runtrol_ipc::WIRE_VERSION,
            },
            Request::List,
            Request::Models {
                provider: "example".into(),
            },
            Request::Start {
                provider: "example".into(),
                workspace: "/work".into(),
                model: None,
                permission: None,
            },
            Request::Resume {
                provider: "example".into(),
                native: "n".into(),
                workspace: "/work".into(),
            },
            Request::Prompt {
                session: SessionId::now(),
                text: "hello".into(),
            },
            Request::AnswerApproval {
                session: SessionId::now(),
                approval: ApprovalId::now(),
                option: OptionId(0),
                subject_digest: [0; 32],
            },
            Request::Interrupt {
                session: SessionId::now(),
            },
            Request::Watch {
                session: SessionId::now(),
                after: None,
            },
            Request::Close {
                session: SessionId::now(),
                now: false,
            },
            Request::StopEverything,
        ]
    }

    #[test]
    fn every_request_this_build_knows_has_a_rule() {
        // A request with no rule is a request nobody decided about. It is refused rather than allowed, which is
        // safe, but it is also a session an operator cannot start and would have no idea why.
        for request in every_request() {
            assert_ne!(
                needed(&request),
                Needed::Unknown,
                "{request:?} has no rule about who may make it"
            );
        }
    }

    #[test]
    fn a_device_that_was_granted_nothing_is_refused_everything_it_could_be_refused() {
        // Default deny, at the boundary rather than in a promise. The two open requests are open on purpose and
        // say why in the table.
        let ledger = GrantLedger::new();
        let caller = Caller::Device {
            device: DeviceId::now(),
        };

        for request in every_request() {
            match needed(&request) {
                Needed::Anyone(why) => {
                    assert!(!why.is_empty(), "{request:?} is open and does not say why");
                    assert!(allowed(&caller, &request, &ledger).is_ok());
                }
                Needed::Scope(_) | Needed::ApprovalResponse => assert!(
                    allowed(&caller, &request, &ledger).is_err(),
                    "{request:?} was allowed to a device that holds nothing"
                ),
                Needed::Unknown => panic!("{request:?} has no rule"),
            }
        }
    }

    #[test]
    fn the_panic_button_works_with_no_permission_at_all() {
        // Required by the security posture. An operator reaching for it is already having a bad day, and the
        // worst a hostile caller achieves through it is that work stops.
        let ledger = GrantLedger::new();
        let stranger = Caller::Device {
            device: DeviceId::now(),
        };
        assert!(allowed(&stranger, &Request::StopEverything, &ledger).is_ok());
    }

    #[test]
    fn somebody_at_the_machine_may_make_every_request_this_build_knows() {
        let ledger = GrantLedger::new();
        for request in every_request() {
            assert!(
                allowed(&Caller::AtTheMachine, &request, &ledger).is_ok(),
                "{request:?}"
            );
        }
    }

    #[test]
    fn a_device_may_do_exactly_what_it_holds() {
        // Granted the way an operator would: a challenge on the console, and the phrase typed back. This is the
        // one test here that goes through the whole path, because a scope check is only worth as much as the
        // thing that hands out scopes.
        let mut ledger = GrantLedger::new();
        let device = DeviceId::now();
        let approved = [DeviceScope::SessionList];
        let console = LocalConsole::claim().expect("nothing else in this crate claims the console");
        let challenge = PresenceChallenge::issue(
            &console,
            GrantRequest::DeviceScopes {
                device,
                scopes: approved.to_vec(),
            },
        )
        .expect("a console can be asked");
        let prompt = challenge.prompt();
        let phrase = prompt
            .rsplit_once(": ")
            .map(|(_, phrase)| phrase.to_owned())
            .expect("the prompt ends with the phrase to type");
        let witness = challenge.answer(&phrase).expect("typed at the machine");
        ledger.grant(device, &approved, &witness).expect("granted");

        let caller = Caller::Device { device };
        assert!(allowed(&caller, &Request::List, &ledger).is_ok());
        assert!(
            allowed(
                &caller,
                &Request::Prompt {
                    session: SessionId::now(),
                    text: "hello".into(),
                },
                &ledger
            )
            .is_err(),
            "listing sessions must not imply writing to one"
        );
    }

    #[test]
    fn stopping_a_session_and_interrupting_one_need_the_same_thing() {
        // Both take work away from a turn that is running. Splitting them would let a device hold one and be
        // surprised by the other, and neither is the irreversible one: removing a session from the list is
        // `SessionDelete`, which nothing here maps to yet.
        assert_eq!(
            needed(&Request::Interrupt {
                session: SessionId::now()
            }),
            needed(&Request::Close {
                session: SessionId::now(),
                now: true
            })
        );
    }
}
