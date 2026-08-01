//! The provider contract as an out-of-tree driver sees it.
//!
//! This test depends only on `runtrol-provider` and implements both public traits without reaching
//! the kernel, daemon, storage, or built-in drivers. If a third-party provider needs an internal
//! type, or a public contract change makes its implementation impossible, this target stops compiling.

use std::collections::VecDeque;

use async_trait::async_trait;
use runtrol_provider::{
    AbsPath, Agent, AgentCommand, CloseMode, Disposition, EventBody, ModelCatalog, Opaque,
    OpenIntent, Produced, Provider, ProviderError, ProviderId, SessionId, Unmapped,
};

/// A provider implemented outside every production crate.
struct OutsideProvider;

/// One session implemented against the public vocabulary only.
struct OutsideAgent {
    session: SessionId,
    native: Box<str>,
    events: VecDeque<Produced>,
    native_command_seen: bool,
}

#[async_trait]
impl Provider for OutsideProvider {
    fn id(&self) -> ProviderId {
        ProviderId::parse("outside").expect("the fixture id is valid")
    }

    async fn models(&self) -> Result<ModelCatalog, ProviderError> {
        Ok(ModelCatalog::unknown(
            "the fixture deliberately has no model discovery surface",
        ))
    }

    async fn open(&self, intent: OpenIntent) -> Result<Box<dyn Agent>, ProviderError> {
        let payload = Opaque::owned(r#"{"method":"future/feature","body":{"value":1}}"#.to_owned());
        let event = Produced {
            src_end: 37,
            body: EventBody::Unmapped(Unmapped {
                tag: "future/feature".into(),
                turn: None,
                payload,
                unknown_to_binding: true,
            }),
        };
        Ok(Box::new(OutsideAgent {
            session: intent.session,
            native: "outside-native-1".into(),
            events: VecDeque::from([event]),
            native_command_seen: false,
        }))
    }
}

#[async_trait]
impl Agent for OutsideAgent {
    fn session(&self) -> SessionId {
        self.session
    }

    fn native(&self) -> Option<&str> {
        Some(&self.native)
    }

    async fn send(&mut self, command: AgentCommand) -> Result<(), ProviderError> {
        self.native_command_seen = matches!(command, AgentCommand::Native(_));
        Ok(())
    }

    async fn next(&mut self) -> Option<Result<Produced, ProviderError>> {
        self.events.pop_front().map(Ok)
    }

    async fn close(self: Box<Self>, _how: CloseMode) -> Result<(), ProviderError> {
        assert!(
            self.native_command_seen,
            "the provider-specific escape hatch reached the outside driver"
        );
        Ok(())
    }
}

/// A third-party implementation can open, receive raw commands, and return unknown events whole.
#[tokio::test]
async fn an_outside_driver_needs_only_the_public_contract() {
    let provider: Box<dyn Provider> = Box::new(OutsideProvider);
    let session = SessionId::now();
    let workspace = AbsPath::new(if cfg!(windows) { r"C:\work" } else { "/work" })
        .expect("the fixture path is valid");
    let mut agent = provider
        .open(OpenIntent {
            session,
            workspace,
            disposition: Disposition::Fresh,
            model: None,
            permission: None,
        })
        .await
        .expect("the outside provider opens");

    assert_eq!(agent.session(), session);
    assert_eq!(agent.native(), Some("outside-native-1"));
    agent
        .send(AgentCommand::Native(Opaque::owned(
            r#"{"method":"future/command"}"#.to_owned(),
        )))
        .await
        .expect("a native command reaches the driver");

    let produced = agent
        .next()
        .await
        .expect("the fixture produced one event")
        .expect("the event is not an error");
    assert_eq!(produced.src_end, 37);
    let EventBody::Unmapped(unmapped) = produced.body else {
        panic!("the unknown provider event must remain unmapped");
    };
    assert!(unmapped.is_drift());
    assert!(unmapped.payload.as_str().contains("future/feature"));

    agent
        .close(CloseMode::Kill)
        .await
        .expect("the outside provider closes");
}
