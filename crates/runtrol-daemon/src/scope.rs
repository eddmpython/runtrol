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
use runtrol_provider::WorkspaceAccess;
use runtrol_security::{Caller, DeviceScope, GrantLedger, LocalScope, SecurityError};

use crate::compose::DeviceAuthority;

/// What a request needs.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Needed {
    /// A device must hold this scope. Somebody at the machine always may.
    Scope(DeviceScope),

    /// Only somebody at the machine, ever. No grant can carry it to a device.
    ///
    /// The [`LocalScope`] names which capability this is in the audit vocabulary. The type it belongs to is
    /// the enforcement: there is no conversion from a local scope into anything the grant ledger accepts, so
    /// a device cannot hold this by construction and the check here is presence and nothing else.
    AtTheMachine(LocalScope),

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

        // The scope admits the question. The answer is projected per caller: a device receives only the
        // rows inside its live workspace roots (`dispatch::sessions_visible_to`), so holding this scope
        // alone discloses nothing about projects the device was never granted.
        Request::List | Request::WatchSessions => Needed::Scope(DeviceScope::SessionList),
        // Model discovery and consult status both read configuration and touch nothing.
        Request::Models { .. }
        | Request::ProviderUpdates
        | Request::RemoteConnection
        | Request::Consult => Needed::Scope(DeviceScope::ConfigRead),
        Request::ProviderUpdate { .. } => Needed::AtTheMachine(LocalScope::ProviderUpdate),
        Request::RemoteConfigure { .. } | Request::AgentToolsWire | Request::AgentToolsUnwire => {
            Needed::AtTheMachine(LocalScope::ConfigWrite)
        }
        Request::PairingBegin
        | Request::PairingProposals
        | Request::PairingApprovalBegin { .. }
        | Request::PairingApprovalFinish { .. }
        | Request::PairingDeny { .. }
        | Request::Devices
        | Request::DeviceRevoke { .. }
        | Request::DeviceAuthorityBegin { .. }
        | Request::DeviceAuthorityFinish { .. } => Needed::AtTheMachine(LocalScope::DevicePair),
        Request::PushSubscription { .. } => Needed::Anyone(
            "an authenticated phone may replace only its own push endpoint, which grants no machine capability",
        ),
        Request::IntegrationEnrollments
        | Request::IntegrationApprovalBegin { .. }
        | Request::IntegrationApprovalFinish { .. }
        | Request::IntegrationSelfApprove { .. }
        | Request::IntegrationEnrollmentDeny { .. }
        | Request::Integrations
        | Request::IntegrationRevoke { .. }
        | Request::IntegrationGrantChange { .. }
        | Request::RuntimeForgetRequests
        | Request::RuntimeForgetConfirm { .. }
        | Request::RuntimeKeyRotationRequests
        | Request::RuntimeKeyRotationConfirm { .. }
        | Request::RuntimeSharedOpenRequests
        | Request::RuntimeSharedOpenConfirm { .. } => {
            Needed::AtTheMachine(LocalScope::IntegrationAdmin)
        }
        Request::WorkspaceIsolatePrepare { .. }
        | Request::WorkspaceIsolateList
        | Request::WorkspaceIsolateBind { .. }
        | Request::WorkspaceIsolateRelease { .. } => {
            Needed::AtTheMachine(LocalScope::WorkspaceShare)
        }
        Request::Start {
            workspace_access: WorkspaceAccess::Shared,
            ..
        }
        | Request::Resume {
            workspace_access: WorkspaceAccess::Shared,
            ..
        } => Needed::AtTheMachine(LocalScope::WorkspaceShare),
        // Opening a terminal on a fresh conversation starts a provider process in a folder, which is what
        // starting a session is; opening one on a stored conversation is what resuming is.
        Request::Start {
            workspace_access: WorkspaceAccess::Exclusive,
            ..
        }
        | Request::TerminalOpen { native: None, .. } => Needed::Scope(DeviceScope::SessionStart),
        Request::Resume {
            workspace_access: WorkspaceAccess::Exclusive,
            ..
        }
        | Request::TerminalOpen {
            native: Some(_), ..
        } => Needed::Scope(DeviceScope::SessionResume),
        // Joining an open terminal is reading its screen and typing into it; typing is the stronger of the
        // two, so a read-only grant cannot join and then type.
        Request::Prompt { .. }
        | Request::Rename { .. }
        | Request::TerminalAttach { .. }
        | Request::TerminalInput { .. } => Needed::Scope(DeviceScope::SessionInputWrite),
        Request::AnswerApproval { .. } => Needed::ApprovalResponse,
        // A resize only changes how the screen is drawn for this viewer: a read.
        Request::Watch { .. } | Request::TerminalResize { .. } => {
            Needed::Scope(DeviceScope::SessionOutputRead)
        }
        Request::Interrupt { .. } => Needed::Scope(DeviceScope::SessionStop),
        // Close also removes runtrol's durable pointer. The provider still owns its conversation, but removing
        // the only runtrol list entry is irreversible here and therefore needs the separate delete authority.
        Request::Close { .. } => Needed::Scope(DeviceScope::SessionDelete),

        Request::Drain => Needed::AtTheMachine(LocalScope::RuntimeDrain),

        Request::StopEverything => Needed::Anyone(
            "the security posture requires the panic button to work from anywhere with no permission, and the \
             worst it achieves is that work stops",
        ),

        // Wiring expands what an agent can reach mid-turn and edits the CLIs' own configuration. Capability
        // growth is answered at the keyboard or not at all, so no grant can carry it.
        Request::ConsultWire { .. } | Request::ConsultUnwire { .. } => {
            Needed::AtTheMachine(LocalScope::ConsultWire)
        }

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
        Needed::AtTheMachine(capability) => {
            if caller.is_at_the_machine() {
                Ok(())
            } else {
                Err(WallRefusal::NeverRemote { capability })
            }
        }
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

/// Apply the ordinary scope wall plus parameterized provider and workspace authority.
///
/// # Errors
///
/// The same refusals as [`allowed`], plus a fail-closed contextual refusal for a remote start or resume outside its
/// exact locally approved provider and workspace roots.
pub(crate) fn allowed_with_authority(
    caller: &Caller,
    request: &Request,
    authority: &DeviceAuthority,
) -> Result<(), WallRefusal> {
    let grants = authority.grants();
    allowed(caller, request, &grants)?;
    let (provider, workspace) = match request {
        Request::Start {
            provider,
            workspace,
            ..
        }
        | Request::Resume {
            provider,
            workspace,
            ..
        }
        | Request::TerminalOpen {
            provider,
            workspace,
            ..
        } => (provider.as_ref(), workspace.as_ref()),
        _ => return Ok(()),
    };
    let provider =
        runtrol_provider::ProviderId::parse(provider).map_err(|_| WallRefusal::DeviceBoundary {
            why: "the requested provider identity is invalid",
        })?;
    authority
        .authorize_open(caller, provider, workspace)
        .map_err(|why| WallRefusal::DeviceBoundary { why })
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

    /// The capability is answered at the machine and can never be granted to a device.
    ///
    /// Separate from a denial, because a denial says "ask for the permission" and there is nothing here to
    /// ask for: the honest instruction is to go to the keyboard.
    #[error(
        "{capability} is decided at the machine runtrol runs on and cannot be granted remotely"
    )]
    NeverRemote {
        /// Which capability, in the audit vocabulary.
        capability: LocalScope,
    },

    /// A parameterized remote provider or workspace boundary did not match current durable authority.
    #[error("{why}")]
    DeviceBoundary {
        /// Stable refusal safe to show without revealing any authority the caller did not name.
        why: &'static str,
    },
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
    #[expect(
        clippy::too_many_lines,
        reason = "the explicit wire request inventory must remain visible to the scope audit"
    )]
    fn every_request() -> Vec<Request> {
        vec![
            Request::Hello {
                wire: runtrol_ipc::WIRE_VERSION,
            },
            Request::List,
            Request::WatchSessions,
            Request::Models {
                provider: "example".into(),
            },
            Request::ProviderUpdates,
            Request::ProviderUpdate {
                provider: "example".into(),
            },
            Request::RemoteConnection,
            Request::RemoteConfigure {
                relay_origin: Some("https://relay.example.com".into()),
            },
            Request::PairingBegin,
            Request::PairingProposals,
            Request::PairingApprovalBegin {
                proposal_id: "pp_example".into(),
                scopes: vec!["session.list".into()],
            },
            Request::PairingApprovalFinish {
                challenge_id: "pac_example".into(),
                answer: "typed phrase".into(),
            },
            Request::PairingDeny {
                proposal_id: "pp_example".into(),
            },
            Request::Devices,
            Request::DeviceRevoke {
                device_id: "018f0000-0000-7000-8000-000000000000".into(),
            },
            Request::DeviceAuthorityBegin {
                device_id: "018f0000-0000-7000-8000-000000000000".into(),
                scopes: vec!["session.list".into()],
                roots: vec!["/work".into()],
                providers: vec!["example".into()],
            },
            Request::DeviceAuthorityFinish {
                challenge_id: "dac_example".into(),
                answer: "typed phrase".into(),
            },
            Request::PushSubscription {
                endpoint: Some("https://fcm.googleapis.com/fcm/send/example".into()),
            },
            Request::Start {
                provider: "example".into(),
                workspace: "/work".into(),
                workspace_access: WorkspaceAccess::Exclusive,
                model: None,
                permission: None,
            },
            Request::Resume {
                provider: "example".into(),
                native: "n".into(),
                workspace: "/work".into(),
                workspace_access: WorkspaceAccess::Exclusive,
            },
            Request::Prompt {
                session: SessionId::now(),
                text: "hello".into(),
            },
            Request::Rename {
                session: SessionId::now(),
                label: Some("release repair".into()),
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
            Request::TerminalOpen {
                provider: "example".into(),
                native: Some("n".into()),
                workspace: "/work".into(),
                cols: 120,
                rows: 40,
            },
            Request::TerminalAttach {
                terminal: runtrol_provider::TerminalId::now(),
                cols: 120,
                rows: 40,
            },
            Request::TerminalInput {
                bytes: runtrol_ipc::TerminalBytes::from(b"ls\r".to_vec()),
            },
            Request::TerminalResize {
                cols: 100,
                rows: 30,
            },
            Request::StopEverything,
            Request::Drain,
            Request::Consult,
            Request::ConsultWire {
                from: "claude".into(),
                to: "codex".into(),
            },
            Request::ConsultUnwire {
                from: "claude".into(),
                to: "codex".into(),
            },
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
                Needed::Scope(_) | Needed::ApprovalResponse | Needed::AtTheMachine(_) => assert!(
                    allowed(&caller, &request, &ledger).is_err(),
                    "{request:?} was allowed to a device that holds nothing"
                ),
                Needed::Unknown => panic!("{request:?} has no rule"),
            }
        }
    }

    #[test]
    fn consult_wiring_is_refused_to_every_device_no_matter_what_it_holds() {
        // Capability growth is answered at the keyboard or not at all. "No matter what it holds" is total by
        // construction rather than by enumeration: the check is presence, the ledger is never consulted, and
        // there is no conversion from `LocalScope` into anything the grant ledger accepts. The refusal names
        // the keyboard rather than a permission, because there is none to ask for.
        let ledger = GrantLedger::new();
        let caller = Caller::Device {
            device: DeviceId::now(),
        };
        for request in [
            Request::ConsultWire {
                from: "claude".into(),
                to: "codex".into(),
            },
            Request::ConsultUnwire {
                from: "claude".into(),
                to: "codex".into(),
            },
        ] {
            match allowed(&caller, &request, &ledger) {
                Err(WallRefusal::NeverRemote { capability }) => {
                    assert_eq!(capability.name(), "consult.wire");
                }
                other => panic!("{request:?} was not refused as never-remote: {other:?}"),
            }
        }
        assert!(
            allowed(
                &Caller::AtTheMachine,
                &Request::ConsultWire {
                    from: "claude".into(),
                    to: "codex".into()
                },
                &ledger
            )
            .is_ok(),
            "presence is the one thing that opens it"
        );
    }

    #[test]
    fn sharing_a_workspace_is_refused_to_every_remote_device() {
        let ledger = GrantLedger::new();
        let caller = Caller::Device {
            device: DeviceId::now(),
        };
        let request = Request::Start {
            provider: "example".into(),
            workspace: "/work".into(),
            workspace_access: WorkspaceAccess::Shared,
            model: None,
            permission: None,
        };

        match allowed(&caller, &request, &ledger) {
            Err(WallRefusal::NeverRemote { capability }) => {
                assert_eq!(capability.name(), "workspace.share");
            }
            other => panic!("shared workspace start was not refused as never-remote: {other:?}"),
        }
        assert!(allowed(&Caller::AtTheMachine, &request, &ledger).is_ok());
    }

    #[test]
    fn retiring_the_runtime_is_never_remote() {
        // Which binary answers every later request is executable authority, and a hostile relay
        // must not be able to bounce the daemon. No grant can carry this to a device.
        let ledger = GrantLedger::new();
        let phone = Caller::Device {
            device: DeviceId::now(),
        };
        assert!(matches!(
            allowed(&phone, &Request::Drain, &ledger),
            Err(WallRefusal::NeverRemote { .. })
        ));
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
        // A synchronous test, so it takes the async lock the blocking way. That is only sound because nothing
        // here runs inside a runtime.
        let _serialised = crate::console_lock().blocking_lock();
        let console = LocalConsole::claim().expect("the console is free while this lock is held");
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
    fn closing_a_session_needs_delete_while_interrupting_needs_stop() {
        assert_eq!(
            needed(&Request::Interrupt {
                session: SessionId::now()
            }),
            Needed::Scope(DeviceScope::SessionStop)
        );
        assert_eq!(
            needed(&Request::Close {
                session: SessionId::now(),
                now: true
            }),
            Needed::Scope(DeviceScope::SessionDelete)
        );
    }
}
