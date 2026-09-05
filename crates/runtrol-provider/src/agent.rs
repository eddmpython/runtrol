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

use crate::account::AccountReport;
use crate::capability::ProviderCapabilities;
use crate::catalog::ModelCatalog;
use crate::command::{AgentCommand, CloseMode, OpenIntent, Produced};
use crate::error::ProviderError;
use crate::event::ApprovalRequest;
use crate::id::{ApprovalId, NativeSessionId, NativeTerminalTarget, ProviderId, SessionId};
use crate::native_catalogue::{
    NativeSessionArchival, NativeSessionCatalogue, NativeSessionDeletion, NativeSessionQuery,
};

/// How Runtime can reach the live terminal surface owned by another process.
///
/// This is a structural capability reported by the provider driver. Runtime chooses the strongest honest
/// route and never infers one from a provider name or terminal output.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum NativeTerminalAccess {
    /// The process is live, but no safe terminal attachment is known. A console the process may own is not a
    /// route: an arbitrary external terminal is focus-only, and its owning window is proved and raised instead.
    #[default]
    Unavailable,
    /// The provider publishes an official command that attaches its TUI to this live conversation.
    Official {
        /// Provider-owned opaque target accepted by that attachment command. It may differ from the durable
        /// conversation identity, as it does for a background job that owns one conversation.
        target: NativeTerminalTarget,
    },
}

/// A provider-verified operating-system process to native-conversation binding.
///
/// This is transport metadata only. It lets Runtime bind a terminal opened before the provider minted its native
/// identity without parsing terminal output or reading conversation content.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeProcessBinding {
    /// Operating-system process identity whose start generation the provider driver already validated.
    pub pid: u32,
    /// Provider-owned conversation identity held by that exact process.
    pub native: NativeSessionId,
    /// Where that process works, when the provider's roster says. A mirrored terminal is filed under it.
    pub cwd: Option<String>,
    /// The strongest live terminal route the driver measured for this process.
    pub terminal_access: NativeTerminalAccess,
}

/// Provider-owned process roster, separated into existence and current model activity.
///
/// `live` answers whether a provider process still owns the conversation. `active` is the subset whose model is
/// answering now. Neither collection carries terminal bytes or conversation content.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NativeProcessActivity {
    /// Conversations owned by a still-running provider process, regardless of turn state.
    pub live: Vec<NativeSessionId>,
    /// Live conversations whose model is answering now.
    pub active: Vec<NativeSessionId>,
    /// Exact process bindings available from the provider's bounded live-process roster.
    pub processes: Vec<NativeProcessBinding>,
}

/// One coding CLI, as runtrol talks to it.
///
/// Stateless and built once per provider at boot, so constructing one must not spawn anything or ask anything.
/// What it does is open sessions.
#[async_trait]
pub trait Provider: Send + Sync + 'static {
    /// Which provider this is.
    fn id(&self) -> ProviderId;

    /// Arguments for an explicitly selected model in a fresh native terminal.
    /// The driver must require the installed CLI's discovered option support. The caller validates
    /// the selection against `models` before passing it here. No provider argv enters the core.
    ///
    /// # Errors
    /// Refuses when this driver or installed CLI has no confirmed native terminal model option.
    fn terminal_model_arguments(&self, _model: &str) -> Result<Vec<String>, ProviderError> {
        Err(ProviderError::Unsupported {
            provider: self.id(),
            what: "native terminal model selection".to_owned(),
            why: "this driver has no discovered terminal model option",
        })
    }

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

    /// Where the operator's account with this service stands, by the service's own status surface.
    ///
    /// The default says the driver publishes no such surface. It never reads credential files, help text
    /// or transcripts to approximate an answer: a signed-in guess that is wrong sends a person to a
    /// conversation that fails, and a signed-out guess hides a service that works.
    ///
    /// # Errors
    ///
    /// Any [`ProviderError`] produced while asking the service's own status surface.
    ///
    /// # Cancellation
    ///
    /// As for [`Self::models`]: dropping the future must begin cleanup of everything it started.
    async fn account(&self) -> Result<AccountReport, ProviderError> {
        Ok(AccountReport::unpublished(
            "this driver publishes no account status surface",
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

    /// Which of this provider's conversations have a model answering in them right now.
    ///
    /// This is the only signal Runtrol has about a conversation it did not start itself: a person who runs a
    /// CLI in their own terminal still expects the panel to show it working (operator, 2026-08-28, measured
    /// against a live turn that every row showed as idle).
    ///
    /// How a driver knows is its own business, and no threshold is imposed here. What is required is that the
    /// answer be about a turn that is running now, not about a file that changed lately: the difference is a
    /// conversation that stops turning the moment it stops working, rather than one that flickers through a
    /// long tool call and keeps turning after the answer arrived. The sidebar asks this on a short clock, so
    /// the answer must cost about what a process list costs and must read no conversation's content.
    ///
    /// # Errors
    ///
    /// Any [`ProviderError`] produced while asking the provider's own surface.
    ///
    /// # Cancellation
    ///
    /// Dropping this future must synchronously begin cleanup of anything the question created.
    async fn active_native_sessions(&self) -> Result<Vec<NativeSessionId>, ProviderError> {
        Ok(Vec::new())
    }

    /// The one directory whose file set changing means this CLI's open conversations changed.
    ///
    /// A CLI that keeps one file per open conversation (a process record, a writer lock) says by creating and
    /// removing those files exactly when a session started or ended. A Runtime that waits on that directory
    /// notices at once and costs nothing while nothing happens, instead of asking on a clock forever and still
    /// answering late. `None` means this driver has no such directory, and the Runtime falls back to noticing
    /// on the requests that can see a change.
    ///
    /// The path is this driver's own business. Nothing outside it knows or names the directory.
    fn session_directory(&self) -> Option<std::path::PathBuf> {
        None
    }

    /// Which conversations have a provider process now, and which of those are answering.
    ///
    /// The default preserves source compatibility with older drivers: activity they already report also proves
    /// the process is live. A driver with a cheap provider-owned process roster should override this method so
    /// idle and waiting processes are discovered without opening or reading a conversation.
    async fn native_process_activity(&self) -> Result<NativeProcessActivity, ProviderError> {
        let active = self.active_native_sessions().await?;
        Ok(NativeProcessActivity {
            live: active.clone(),
            active,
            processes: Vec::new(),
        })
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
    /// command, or permanently removes the exact provider-owned records from a store it already reads under its
    /// contract. It removes the entry the operator asked to remove and interprets no content; the provider's
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

    /// The operating-system process this agent owns, when it owns one.
    ///
    /// Structural, never conversational: it lets a surface ask the operating system what a session costs in
    /// memory. A driver that shares one process among its sessions answers with that shared process, and a
    /// driver with no process of its own answers `None`.
    fn pid(&self) -> Option<u32> {
        None
    }

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
