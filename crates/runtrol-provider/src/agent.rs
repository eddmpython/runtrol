//! The two traits a driver implements, arriving with their first implementation.
//!
//! These were deliberately absent until now. A trait with no implementor is a guess about a shape the implementor
//! gets to decide, and the guess would have been wrong here in at least one measurable way: the design note said a
//! turn ends on one frame and the CLI ends it on another.
//!
//! # Why a turn's ending is not a return value
//!
//! [`Agent::send`] returns when the command has been handed over, not when the work is done. Measured on the two
//! supported CLIs: one acknowledges a turn in two milliseconds with nothing in it, and the other says nothing at
//! all until it has something to say. A `send` that returned on completion would have to decide what completion
//! means, and neither CLI lets it.
//!
//! So a turn ends the way everything else happens: as an event, from [`Agent::next`], carrying whose word it was.
//!
//! # Why `next` and not a stream type
//!
//! One method that yields the next event, pulled by whoever owns the session. A stream type in the contract would
//! put a futures dependency into every third-party driver's build for something a loop already does, and it would
//! hide the one property that matters here: exactly one reader, pulling in order, so the hub can number what comes
//! out without a lock.
//!
//! # What a driver never does
//!
//! It does not number events, keep a transcript, own storage, or reach the operator's surfaces. It reaches its own
//! CLI and translates. Its dependency list is the enforcement: a driver crate that cannot see storage cannot start
//! keeping a copy of a conversation.

use async_trait::async_trait;

use crate::capability::ProviderCapabilities;
use crate::catalog::ModelCatalog;
use crate::command::{AgentCommand, CloseMode, OpenIntent, Produced};
use crate::error::ProviderError;
use crate::event::ApprovalRequest;
use crate::id::{ApprovalId, ProviderId, SessionId};
use crate::native_catalogue::{
    NativeSessionArchival, NativeSessionCatalogue, NativeSessionDeletion, NativeSessionQuery,
};

/// One coding CLI, as runtrol talks to it.
///
/// Stateless and built once per provider at boot, so constructing one must not spawn anything or ask anything.
/// What it does is open sessions.
#[async_trait]
pub trait Provider: Send + Sync + 'static {
    /// Which provider this is.
    fn id(&self) -> ProviderId;

    /// Report structural operations supported by this exact prepared driver.
    ///
    /// The default is deliberately unknown so a third-party driver built for an older SPI never gains a capability
    /// merely because Runtime learned a new public method.
    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities::unknown()
    }

    /// Discover the model choices this provider can truthfully offer now.
    ///
    /// A default keeps an older third-party driver honest when used with a newer runtrol: it reports that the
    /// binding is absent instead of fabricating a list or preventing the driver from opening sessions.
    ///
    /// # Errors
    ///
    /// Any [`ProviderError`] produced while asking the provider's own discovery surface.
    ///
    /// # Cancellation
    ///
    /// Dropping this future must synchronously begin cleanup of every resource the query created. A driver must not
    /// detach a task or process that can outlive the future. Child processes must use kill-on-drop containment and a
    /// reader task must remain owned by a value dropped with this future.
    async fn models(&self) -> Result<ModelCatalog, ProviderError> {
        Ok(ModelCatalog::unsupported(
            "this driver does not provide model discovery",
        ))
    }

    /// Discover one official provider-native session page for one approved root.
    ///
    /// A default keeps third-party drivers source compatible and honest. It never scans provider storage, help text,
    /// logs, or transcripts to approximate a catalogue.
    ///
    /// # Errors
    ///
    /// Any [`ProviderError`] produced while calling the provider's registered official discovery surface.
    ///
    /// # Cancellation
    ///
    /// Dropping this future must synchronously begin cleanup of every process, task, or stream created by the query.
    async fn native_sessions(
        &self,
        _query: NativeSessionQuery,
    ) -> Result<NativeSessionCatalogue, ProviderError> {
        Ok(NativeSessionCatalogue::unsupported(
            "this driver does not provide official native session discovery",
        ))
    }

    /// Whether [`Self::native_sessions`] answers a query with no folder by naming every
    /// conversation this provider knows about, wherever it happened.
    ///
    /// Default `false`, which keeps a driver written before this existed asked one folder at a
    /// time exactly as it was. A driver returns `true` only after its provider's own surface was
    /// measured answering without a folder filter; every row it then returns must carry its own
    /// `cwd`, because that is what places the conversation once the query no longer does.
    ///
    /// This is a fact about the provider's surface, not a preference: enumerating the machine is
    /// what the product promises, so a driver that can do it should, and one that cannot must say
    /// so here rather than silently returning one folder's worth and letting it read as all.
    fn enumerates_machine(&self) -> bool {
        false
    }

    /// Delete one provider-native conversation through the provider's own surface.
    ///
    /// The default refuses: a driver written before this existed has not said its provider can do it, and
    /// deleting is the one act a surface must never guess at. A driver that can either asks its CLI's own delete
    /// command, or, for a store it already reads under its contract, moves the conversation out of that store
    /// reversibly. It removes the entry the operator asked to remove and interprets no content; the provider's
    /// store stays the record of what exists.
    ///
    /// # Errors
    ///
    /// [`ProviderError::Unsupported`] by default; otherwise whatever the provider's own surface answered.
    async fn delete_native_session(
        &self,
        _deletion: NativeSessionDeletion,
    ) -> Result<(), ProviderError> {
        Err(ProviderError::Unsupported {
            provider: self.id(),
            what: "deleting a provider-native conversation".to_owned(),
            why: "this driver reports no provider surface for deleting a stored conversation",
        })
    }

    /// Archive one provider-native conversation through the provider's own surface.
    ///
    /// # Errors
    ///
    /// [`ProviderError::Unsupported`] by default; otherwise whatever the provider's own surface answered.
    async fn archive_native_session(
        &self,
        _archival: NativeSessionArchival,
    ) -> Result<(), ProviderError> {
        Err(ProviderError::Unsupported {
            provider: self.id(),
            what: "archiving a provider-native conversation".to_owned(),
            why: "this driver reports no provider surface for archiving a stored conversation",
        })
    }

    /// Open a session and bind to it.
    ///
    /// Returns once the provider has acknowledged the session enough for work to flow. Whether that means a
    /// process started or a request was accepted is the driver's business.
    ///
    /// # Errors
    ///
    /// Any [`ProviderError`]. A failure here becomes session state rather than a log line, so the variant matters:
    /// it decides whether the operator sees "not installed", "authenticate at your machine", or "it broke".
    async fn open(&self, intent: OpenIntent) -> Result<Box<dyn Agent>, ProviderError>;
}

/// One live session.
///
/// Owned by whoever is supervising that session. Exactly one reader calls [`Agent::next`], which is what lets the
/// hub number events without coordinating.
#[async_trait]
pub trait Agent: Send + Sync {
    /// Which session this is, by runtrol's own name for it.
    fn session(&self) -> SessionId;

    /// The provider's own name for this session, once it has said one.
    ///
    /// `None` before the provider has announced it. The newest answer wins: a resume can produce a new name and a
    /// fork always does, and a resume command has to be given the one that names the conversation now.
    fn native(&self) -> Option<&str>;

    /// A provider approval that is still waiting for this session, if there is one by this name.
    ///
    /// The driver owns this state together with the provider-native response for each option. The supervisor only
    /// borrows the normalized request long enough to bind an answer to the exact subject and authority it requires.
    /// A default keeps drivers that do not expose approvals source compatible and honestly reports no pending prompt.
    fn approval(&self, _id: ApprovalId) -> Option<&ApprovalRequest> {
        None
    }

    /// Every provider approval still pending for this session.
    ///
    /// The returned references borrow driver-owned normalized requests. Implementations must keep the collection
    /// bounded and must not reconstruct requests from provider transcript storage.
    fn approvals(&self) -> Vec<&ApprovalRequest> {
        Vec::new()
    }

    /// Hand a command to the provider.
    ///
    /// Returns when it has been handed over. **Not when the work is done**: what finishes a turn is an event, not
    /// this returning. See the module notes.
    ///
    /// # Errors
    ///
    /// Any [`ProviderError`]. A protocol failure here is promoted to session state by the caller.
    async fn send(&mut self, command: AgentCommand) -> Result<(), ProviderError>;

    /// The next event, or `None` once nothing more will come.
    ///
    /// `None` means the session's stream is over, which is a fact and not an outcome: whether the turn that was
    /// running finished is answered by the events that came before, never by this returning.
    ///
    /// # Abandoning this must lose nothing
    ///
    /// This is a requirement on the implementation and not a note about the caller. One supervisor waits on every
    /// session at once, which it can only do by asking each in turn and setting aside the ones that have nothing
    /// to say yet. So dropping the returned future before it is ready has to leave the driver exactly where it
    /// was: anything already taken from the provider belongs in the driver, never in the future.
    ///
    /// A driver that gets this wrong does not fail. It hands back a message with its middle missing, which is why
    /// this is stated here rather than left to be discovered. Whatever the implementation waits on must lose
    /// nothing when it is abandoned, and everything read before that must already be somewhere that survives.
    async fn next(&mut self) -> Option<Result<Produced, ProviderError>>;

    /// End the session.
    ///
    /// Takes ownership, so nothing can be sent afterwards.
    ///
    /// # Errors
    ///
    /// Any [`ProviderError`]. Reported rather than swallowed: an operator who asked for a session to stop has to
    /// know whether it did.
    async fn close(self: Box<Self>, how: CloseMode) -> Result<(), ProviderError>;
}

#[cfg(test)]
mod tests {
    use crate::event::{EventBody, Opaque};
    use crate::path::AbsPath;

    use super::*;

    /// A driver that answers from a script, for checking the shape of the contract rather than any behaviour.
    struct Scripted {
        session: SessionId,
        native: Option<String>,
        remaining: Vec<EventBody>,
        sent: Vec<AgentCommand>,
    }

    #[async_trait]
    impl Agent for Scripted {
        fn session(&self) -> SessionId {
            self.session
        }

        fn native(&self) -> Option<&str> {
            self.native.as_deref()
        }

        async fn send(&mut self, command: AgentCommand) -> Result<(), ProviderError> {
            self.sent.push(command);
            Ok(())
        }

        async fn next(&mut self) -> Option<Result<Produced, ProviderError>> {
            let body = self.remaining.pop()?;
            Some(Ok(Produced { src_end: 1, body }))
        }

        async fn close(self: Box<Self>, _how: CloseMode) -> Result<(), ProviderError> {
            Ok(())
        }
    }

    struct Nothing;

    #[async_trait]
    impl Provider for Nothing {
        fn id(&self) -> ProviderId {
            ProviderId::parse("example").expect("the test's own id must be valid")
        }

        async fn open(&self, intent: OpenIntent) -> Result<Box<dyn Agent>, ProviderError> {
            Ok(Box::new(Scripted {
                session: intent.session,
                native: None,
                remaining: vec![EventBody::Plan {
                    payload: Opaque::none(),
                }],
                sent: Vec::new(),
            }))
        }
    }

    fn an_intent(session: SessionId) -> OpenIntent {
        OpenIntent {
            session,
            workspace: AbsPath::new(if cfg!(windows) { r"C:\work" } else { "/work" })
                .expect("valid"),
            disposition: crate::command::Disposition::Fresh,
            model: None,
            reasoning_effort: None,
            permission: None,
        }
    }

    #[tokio::test]
    async fn a_driver_can_be_held_without_naming_its_type() {
        // The whole reason these are traits. A build's kind table hands back one of these, and the kernel holds it
        // without a line of code that mentions which CLI it is.
        let providers: Vec<Box<dyn Provider>> = vec![Box::new(Nothing)];
        let provider = providers.first().expect("one provider");
        assert_eq!(provider.id().as_str(), "example");
        let capabilities = provider.capabilities();
        assert_eq!(
            capabilities.fresh_session.state,
            crate::capability::ProviderCapabilityState::Unknown,
            "an older driver must not gain a newly published capability"
        );

        let session = SessionId::now();
        let mut agent = provider
            .open(an_intent(session))
            .await
            .expect("the scripted driver opens");
        assert_eq!(agent.session(), session);
        assert_eq!(agent.native(), None, "nothing has been announced yet");

        agent
            .send(AgentCommand::Prompt(vec![]))
            .await
            .expect("handing over a command works");
        let produced = agent
            .next()
            .await
            .expect("one event is scripted")
            .expect("and it is not a failure");
        assert_eq!(produced.src_end, 1);

        agent.close(CloseMode::Kill).await.expect("closing works");
    }

    #[tokio::test]
    async fn the_stream_ending_is_a_fact_and_not_an_outcome() {
        // `None` says nothing more will come. Whether the turn that was running finished is answered by the events
        // that came before it, and a caller that read this as "the turn succeeded" would be inventing one.
        let mut agent: Box<dyn Agent> = Box::new(Scripted {
            session: SessionId::now(),
            native: None,
            remaining: Vec::new(),
            sent: Vec::new(),
        });
        assert!(agent.next().await.is_none());
        assert!(
            agent.next().await.is_none(),
            "and it stays over rather than becoming something else"
        );
    }

    #[tokio::test]
    async fn sending_a_command_does_not_wait_for_the_work() {
        // Measured: one CLI acknowledges in two milliseconds with nothing in it, and the other says nothing until
        // it has something to say. A send that returned on completion would have to decide what completion means,
        // and neither lets it.
        let mut agent = Scripted {
            session: SessionId::now(),
            native: None,
            remaining: vec![EventBody::Plan {
                payload: Opaque::none(),
            }],
            sent: Vec::new(),
        };
        agent
            .send(AgentCommand::Prompt(vec![]))
            .await
            .expect("handed over");
        assert_eq!(agent.sent.len(), 1, "the command was handed over");
        assert_eq!(
            agent.remaining.len(),
            1,
            "and nothing was consumed, because sending is not waiting"
        );
    }
}
