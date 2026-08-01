//! Gate: an approval answer is bound to the pending subject and its real risk.
//!
//! The provider owns the pending native request. The kernel may relay a human choice only after checking the
//! runtrol approval id, the exact subject digest, the offered option, and the answering device's authority. The
//! wire cannot lower that authority by claiming a different risk because risk comes from the pending request.

use std::collections::VecDeque;
use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use runtrol_core::SessionManager;
use runtrol_provider::{
    AbsPath, Agent, AgentCommand, ApprovalId, ApprovalKind, ApprovalOption, ApprovalRequest,
    CloseMode, Disposition, EventBody, ModelCatalog, Opaque, OpenIntent, OptionId,
    PermissionOptionKind, Produced, Provider, ProviderError, ProviderId, RiskClass, SessionId,
    WallMs,
};
use runtrol_security::{
    Caller, DeviceId, DeviceScope, GrantLedger, GrantRequest, LocalConsole, PresenceChallenge,
};
use tokio::sync::Mutex;

static CONSOLE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn console_lock() -> &'static Mutex<()> {
    CONSOLE_LOCK.get_or_init(|| Mutex::new(()))
}

struct ApprovalProvider {
    request: ApprovalRequest,
    sent: Arc<Mutex<Vec<AgentCommand>>>,
}

struct ApprovalAgent {
    session: SessionId,
    request: Option<ApprovalRequest>,
    events: VecDeque<Produced>,
    sent: Arc<Mutex<Vec<AgentCommand>>>,
}

#[async_trait]
impl Provider for ApprovalProvider {
    fn id(&self) -> ProviderId {
        ProviderId::parse("approval-fixture").expect("the fixture id is valid")
    }

    async fn models(&self) -> Result<ModelCatalog, ProviderError> {
        Ok(ModelCatalog::unknown("the fixture has no models"))
    }

    async fn open(&self, intent: OpenIntent) -> Result<Box<dyn Agent>, ProviderError> {
        Ok(Box::new(ApprovalAgent {
            session: intent.session,
            request: Some(self.request.clone()),
            events: VecDeque::from([Produced {
                src_end: 1,
                body: EventBody::ApprovalRequested(Box::new(self.request.clone())),
            }]),
            sent: Arc::clone(&self.sent),
        }))
    }
}

#[async_trait]
impl Agent for ApprovalAgent {
    fn session(&self) -> SessionId {
        self.session
    }

    fn native(&self) -> Option<&str> {
        Some("approval-fixture-native")
    }

    fn approval(&self, id: ApprovalId) -> Option<&ApprovalRequest> {
        self.request.as_ref().filter(|request| request.id == id)
    }

    async fn send(&mut self, command: AgentCommand) -> Result<(), ProviderError> {
        if matches!(command, AgentCommand::Answer { .. }) {
            self.request = None;
        }
        self.sent.lock().await.push(command);
        Ok(())
    }

    async fn next(&mut self) -> Option<Result<Produced, ProviderError>> {
        self.events.pop_front().map(Ok)
    }

    async fn close(self: Box<Self>, _how: CloseMode) -> Result<(), ProviderError> {
        Ok(())
    }
}

fn request(risk: RiskClass, digest: [u8; 32]) -> ApprovalRequest {
    ApprovalRequest {
        id: ApprovalId::now(),
        turn: None,
        tool_call: None,
        kind: if risk == RiskClass::High {
            ApprovalKind::Command
        } else {
            ApprovalKind::Other
        },
        risk,
        options: vec![
            ApprovalOption {
                id: OptionId(0),
                label: "Allow once".into(),
                kind: PermissionOptionKind::AllowOnce,
            },
            ApprovalOption {
                id: OptionId(1),
                label: "Reject".into(),
                kind: PermissionOptionKind::RejectOnce,
            },
        ],
        subject: Opaque::owned(r#"{"command":"cargo test"}"#.to_owned()),
        subject_incomplete: false,
        subject_digest: digest,
        expires_at: WallMs::now().plus_millis(90_000),
    }
}

async fn grant(device: DeviceId, scopes: &[DeviceScope]) -> GrantLedger {
    let _console_guard = console_lock().lock().await;
    let console = LocalConsole::claim().expect("the test owns the local console");
    let challenge = PresenceChallenge::issue(
        &console,
        GrantRequest::DeviceScopes {
            device,
            scopes: scopes.to_vec(),
        },
    )
    .expect("a presence challenge opens");
    let phrase = challenge
        .prompt()
        .rsplit_once(": ")
        .map(|(_, phrase)| phrase.to_owned())
        .expect("the prompt ends with the phrase");
    let witness = challenge.answer(&phrase).expect("presence is proven");
    let mut ledger = GrantLedger::new();
    ledger
        .grant(device, scopes, &witness)
        .expect("the exact scopes are granted");
    ledger
}

async fn running(
    request: ApprovalRequest,
) -> (SessionManager, SessionId, Arc<Mutex<Vec<AgentCommand>>>) {
    let sent = Arc::new(Mutex::new(Vec::new()));
    let provider = ApprovalProvider {
        request,
        sent: Arc::clone(&sent),
    };
    let session = SessionId::now();
    let workspace = AbsPath::new(if cfg!(windows) { r"C:\work" } else { "/work" })
        .expect("the fixture path is valid");
    let mut sessions = SessionManager::new();
    sessions
        .start(
            &provider,
            OpenIntent {
                session,
                workspace,
                disposition: Disposition::Fresh,
                model: None,
                permission: None,
            },
        )
        .await
        .expect("the fixture session starts");
    sessions
        .pump_once(session)
        .await
        .expect("the approval reaches the kernel")
        .expect("the approval is published");
    (sessions, session, sent)
}

#[tokio::test]
async fn a_low_authority_device_cannot_answer_a_high_risk_pending_request() {
    let digest = [7; 32];
    let pending = request(RiskClass::High, digest);
    let approval = pending.id;
    let (mut sessions, session, sent) = running(pending).await;
    let device = DeviceId::now();
    let ledger = grant(device, &[DeviceScope::ApprovalRespondLow]).await;

    assert!(
        sessions
            .answer_approval(
                &Caller::Device { device },
                &ledger,
                session,
                approval,
                OptionId(0),
                digest,
            )
            .await
            .is_err(),
        "wire input must not lower the risk carried by the pending request"
    );
    assert!(sent.lock().await.is_empty());
}

#[tokio::test]
async fn a_digest_mismatch_keeps_the_request_pending_and_the_exact_answer_reaches_the_provider() {
    let digest = [9; 32];
    let pending = request(RiskClass::Low, digest);
    let approval = pending.id;
    let (mut sessions, session, sent) = running(pending).await;
    let device = DeviceId::now();
    let ledger = grant(device, &[DeviceScope::ApprovalRespondLow]).await;
    let caller = Caller::Device { device };

    assert!(
        sessions
            .answer_approval(&caller, &ledger, session, approval, OptionId(0), [8; 32],)
            .await
            .is_err()
    );
    sessions
        .answer_approval(&caller, &ledger, session, approval, OptionId(0), digest)
        .await
        .expect("the exact low-risk answer is authorized");

    let commands = sent.lock().await;
    assert_eq!(commands.len(), 1);
    assert!(matches!(
        commands.first(),
        Some(AgentCommand::Answer {
            id,
            option: OptionId(0),
            subject_digest,
        }) if *id == approval && *subject_digest == digest
    ));
}
