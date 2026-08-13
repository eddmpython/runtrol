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

        Request::List | Request::WatchSessions => Needed::Scope(DeviceScope::SessionList),
        // Model discovery and consult status both read configuration and touch nothing.
        Request::Models { .. } | Request::ProviderUpdates | Request::Consult => {
            Needed::Scope(DeviceScope::ConfigRead)
        }
        Request::ProviderUpdate { .. } => Needed::AtTheMachine(LocalScope::ProviderUpdate),
        Request::IntegrationEnrollments
        | Request::IntegrationApprovalBegin { .. }
        | Request::IntegrationApprovalFinish { .. }
        | Request::IntegrationEnrollmentDeny { .. }
        | Request::Integrations
        | Request::IntegrationRevoke { .. }
        | Request::IntegrationGrantChange { .. }
        | Request::RuntimeForgetRequests
        | Request::RuntimeForgetConfirm { .. }
        | Request::RuntimeKeyRotationRequests
        | Request::RuntimeKeyRotationConfirm { .. } => {
            Needed::AtTheMachine(LocalScope::IntegrationAdmin)
        }
        Request::MissionRegisterGate { .. } => Needed::AtTheMachine(LocalScope::GateRegister),
        Request::MissionValidate { .. } => Needed::AtTheMachine(LocalScope::MissionCreate),
        Request::MissionList | Request::MissionGet { .. } => {
            Needed::Scope(DeviceScope::MissionRead)
        }
        Request::MissionStart { .. }
        | Request::MissionPrepareTask { .. }
        | Request::MissionBindSession { .. } => Needed::AtTheMachine(LocalScope::MissionStart),
        Request::MissionSendTaskInstruction { .. } => {
            Needed::AtTheMachine(LocalScope::MissionSendTaskInstruction)
        }
        Request::MissionVerifyTask { .. } | Request::MissionCompleteIntegration { .. } => {
            Needed::AtTheMachine(LocalScope::MissionIntegrate)
        }
        Request::MissionRetryTask { .. } => Needed::AtTheMachine(LocalScope::MissionRetryTask),
        Request::MissionArchive { .. } => Needed::AtTheMachine(LocalScope::MissionArchive),
        Request::CapabilityPropose { .. }
        | Request::CapabilityList
        | Request::CapabilityVerify { .. }
        | Request::CapabilityApprove { .. }
        | Request::CapabilityReject { .. }
        | Request::CapabilityQuarantine { .. } => {
            Needed::AtTheMachine(LocalScope::CapabilityPromote)
        }
        Request::CapabilityRollback { .. } => Needed::AtTheMachine(LocalScope::CapabilityRollback),
        Request::CapabilityArchive { .. } => Needed::AtTheMachine(LocalScope::CapabilityArchive),
        Request::MissionPause { .. } => Needed::Scope(DeviceScope::MissionPause),
        Request::MissionResumeSafe { .. } => Needed::Scope(DeviceScope::MissionResumeSafe),
        Request::MissionCancel { .. } => Needed::Scope(DeviceScope::MissionCancel),
        Request::Start {
            workspace_access: WorkspaceAccess::Shared,
            ..
        }
        | Request::Resume {
            workspace_access: WorkspaceAccess::Shared,
            ..
        } => Needed::AtTheMachine(LocalScope::WorkspaceShare),
        Request::Start {
            workspace_access: WorkspaceAccess::Exclusive,
            ..
        } => Needed::Scope(DeviceScope::SessionStart),
        Request::Resume {
            workspace_access: WorkspaceAccess::Exclusive,
            ..
        } => Needed::Scope(DeviceScope::SessionResume),
        Request::Prompt { .. } | Request::Rename { .. } => {
            Needed::Scope(DeviceScope::SessionInputWrite)
        }
        Request::AnswerApproval { .. } => Needed::ApprovalResponse,
        Request::Watch { .. } => Needed::Scope(DeviceScope::SessionOutputRead),
        Request::Interrupt { .. } => Needed::Scope(DeviceScope::SessionStop),
        // Close also removes runtrol's durable pointer. The provider still owns its conversation, but removing
        // the only runtrol list entry is irreversible here and therefore needs the separate delete authority.
        Request::Close { .. } => Needed::Scope(DeviceScope::SessionDelete),

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
            Request::StopEverything,
            Request::Consult,
            Request::ConsultWire {
                from: "claude".into(),
                to: "codex".into(),
            },
            Request::ConsultUnwire {
                from: "claude".into(),
                to: "codex".into(),
            },
            Request::MissionRegisterGate {
                gate_id: "check".into(),
                program: "cargo".into(),
                arguments: vec!["test".into()],
                timeout_ms: 1_000,
            },
            Request::MissionValidate {
                project: "/work".into(),
                mission_ref: "mission.toml".into(),
            },
            Request::MissionList,
            Request::MissionGet {
                mission_id: "msn_fixture".into(),
            },
            Request::MissionStart {
                mission_id: "msn_fixture".into(),
                mission_sha256: "11".repeat(32).into(),
            },
            Request::MissionPrepareTask {
                mission_id: "msn_fixture".into(),
                task_id: "tsk_fixture".into(),
            },
            Request::MissionBindSession {
                mission_id: "msn_fixture".into(),
                task_id: "tsk_fixture".into(),
                session_id: SessionId::now().to_string().into(),
                provider_runtime_id: "fixture".into(),
                native_session_id: None,
                workspace: "/work".into(),
            },
            Request::MissionSendTaskInstruction {
                mission_id: "msn_fixture".into(),
                task_id: "tsk_fixture".into(),
                instruction_sha256: "22".repeat(32).into(),
            },
            Request::MissionVerifyTask {
                mission_id: "msn_fixture".into(),
                task_id: "tsk_fixture".into(),
            },
            Request::MissionRetryTask {
                mission_id: "msn_fixture".into(),
                task_id: "tsk_fixture".into(),
            },
            Request::MissionCompleteIntegration {
                mission_id: "msn_fixture".into(),
            },
            Request::MissionArchive {
                mission_id: "msn_fixture".into(),
            },
            Request::MissionPause {
                mission_id: "msn_fixture".into(),
            },
            Request::MissionResumeSafe {
                mission_id: "msn_fixture".into(),
            },
            Request::MissionCancel {
                mission_id: "msn_fixture".into(),
            },
            Request::CapabilityPropose {
                project: "/work".into(),
                candidate_ref: ".runtrol/capabilities/candidates/one".into(),
            },
            Request::CapabilityList,
            Request::CapabilityVerify {
                project: "/work".into(),
                capability_id: "reviewed-skill".into(),
                version_sha256: "33".repeat(32).into(),
            },
            Request::CapabilityApprove {
                project: "/work".into(),
                capability_id: "reviewed-skill".into(),
                version_sha256: "33".repeat(32).into(),
            },
            Request::CapabilityReject {
                project: "/work".into(),
                capability_id: "reviewed-skill".into(),
            },
            Request::CapabilityQuarantine {
                project: "/work".into(),
                capability_id: "reviewed-skill".into(),
            },
            Request::CapabilityRollback {
                project: "/work".into(),
                capability_id: "reviewed-skill".into(),
                version_sha256: "44".repeat(32).into(),
            },
            Request::CapabilityArchive {
                project: "/work".into(),
                capability_id: "reviewed-skill".into(),
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
    fn mission_expansion_and_capability_changes_are_never_remote() {
        let ledger = GrantLedger::new();
        let caller = Caller::Device {
            device: DeviceId::now(),
        };
        for request in every_request() {
            if matches!(
                needed(&request),
                Needed::AtTheMachine(
                    LocalScope::MissionCreate
                        | LocalScope::MissionStart
                        | LocalScope::MissionRetryTask
                        | LocalScope::MissionSendTaskInstruction
                        | LocalScope::MissionIntegrate
                        | LocalScope::MissionArchive
                        | LocalScope::GateRegister
                        | LocalScope::CapabilityPromote
                        | LocalScope::CapabilityRollback
                        | LocalScope::CapabilityArchive
                )
            ) {
                assert!(
                    matches!(
                        allowed(&caller, &request, &ledger),
                        Err(WallRefusal::NeverRemote { .. })
                    ),
                    "{request:?} was not permanently local"
                );
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
