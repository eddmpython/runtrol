//! The one entry point the daemon calls about a session.
//!
//! Starting, pumping, closing and listing. They are four operations of one responsibility, so they are one module:
//! every one of them touches the same map of live sessions and the same tier bound, and splitting them would mean
//! four files sharing one invariant.
//!
//! # Why pumping is a step and not a loop
//!
//! [`SessionManager::pump_once`] reads one event, numbers it, applies it to the session's state, and returns. It does
//! not spawn a task and it does not loop, because owning the runtime is the daemon's job: the daemon decides when a
//! session is pumped and what that is raced against, and the kernel decides what one event means. Putting a task in
//! here would put a runtime configuration in the kernel, which is the one place the memory contract says it must not
//! be.
//!
//! It also makes the whole thing testable. A session is driven one event at a time against a scripted driver, and
//! every rule below is checked without a process, a socket, or a clock.
//!
//! # Why waiting on every session is one call and not one task each
//!
//! [`SessionManager::pump_any`] waits until any live session speaks, then applies that one event. A task per session
//! is the other way to do this, and it costs the thing that makes the rest of this file true: the state of every
//! session would move on a different thread from the one that reads it, so the map, the numbering and the tier bound
//! would each need a lock, and "the kernel decides what one event means" would become "some task decided, and the
//! kernel found out later".
//!
//! One caller, one owner, no locks. What it asks of a driver is written into [`Agent::next`]: a session that has
//! nothing to say yet is set aside, so being set aside must cost that driver nothing.
//!
//! # What a failure does
//!
//! It becomes session state. A protocol failure, a child that exited, a frame runtrol cannot read: each one moves the
//! session to a state the operator can see and resume from, and each one also leaves the event stream with a notice
//! saying so. A failure that was only returned to a caller would be a failure the operator never hears about.

use core::ops::Bound;
use core::pin::pin;
use core::task::Poll;
use std::collections::BTreeMap;

use runtrol_provider::{
    AbsPath, Agent, AgentCommand, CloseMode, Disposition, EventBody, Level, Notice, NoticeCode,
    Opaque, OpenIntent, Produced, Provider, ProviderError, ProviderId, SessionId, WallMs,
};

use crate::events::{Published, SessionHub, Subscription};
use crate::session::mint::Identity;
use crate::session::state::{FailureCode, Observed, SessionState};
use crate::session::tier::{Admit, HotSession, Tier};

/// A session operation could not be carried out.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SessionError {
    /// No session by that name is live.
    #[error("no live session {session}")]
    NotLive {
        /// The session that was asked about.
        session: SessionId,
    },

    /// Every session with a process is busy, so starting another would have to interrupt one.
    #[error(transparent)]
    NoRoom(#[from] crate::session::tier::NoRoom),

    /// The provider refused.
    ///
    /// Carried rather than flattened: the variant decides whether the operator sees "not installed", "authenticate at
    /// your machine", or "it broke", and those are three different next moves.
    #[error(transparent)]
    Provider(#[from] ProviderError),
}

/// One live session: its driver, its event hub, its names, and what it is doing.
struct Live {
    /// The driver.
    agent: Box<dyn Agent>,
    /// Where its events are numbered and fanned out.
    hub: SessionHub,
    /// Its two names.
    identity: Identity,
    /// Where the agent works.
    ///
    /// Kept because a listing has to say it. Which session is touching which folder is the whole of the
    /// `sessions do not trample each other` axis, and a surface that cannot show it cannot warn about it.
    workspace: AbsPath,
    /// What it is doing.
    state: SessionState,
}

/// One event, and which session produced it.
#[derive(Debug)]
pub struct Pumped {
    /// Whose event it was.
    pub session: SessionId,
    /// What was published, or `None` when that session's stream ended.
    pub published: Option<Published>,
}

/// Every session that has a process, and the rules about how many may.
pub struct SessionManager {
    /// The live ones, ordered by name so a listing is stable.
    live: BTreeMap<SessionId, Live>,
    /// Which session spoke last, so the next round of listening starts past it.
    ///
    /// Kept as a name rather than a position: a position would move under a session that ended, and this is asked
    /// about by exclusion, so it does not have to name a session that is still live.
    after: Option<SessionId>,
}

impl SessionManager {
    /// A manager with nothing live.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            live: BTreeMap::new(),
            after: None,
        }
    }

    /// How many sessions have a process.
    #[must_use]
    pub fn hot(&self) -> usize {
        self.live.len()
    }

    /// Whether a session is live.
    #[must_use]
    pub fn is_live(&self, session: SessionId) -> bool {
        self.live.contains_key(&session)
    }

    /// What a session is doing, if it is live.
    #[must_use]
    pub fn state(&self, session: SessionId) -> Option<&SessionState> {
        self.live.get(&session).map(|one| &one.state)
    }

    /// The provider's own name for a session, if it has announced one.
    #[must_use]
    pub fn native(&self, session: SessionId) -> Option<&str> {
        self.live
            .get(&session)?
            .identity
            .native()
            .map(AsRef::as_ref)
    }

    /// Watch a live session.
    ///
    /// # Errors
    ///
    /// [`SessionError::NotLive`] when nothing is running under that name.
    pub fn subscribe(&mut self, session: SessionId) -> Result<Subscription, SessionError> {
        let live = self
            .live
            .get_mut(&session)
            .ok_or(SessionError::NotLive { session })?;
        Ok(live.hub.subscribe())
    }

    /// Start a session, or continue one.
    ///
    /// Admission comes first, because a start that would have to interrupt a running turn is refused rather than
    /// resolved by force. Then the driver is asked, and only a driver that answered gets a place in the map: a session
    /// that failed to open is not a session with a process.
    ///
    /// # Errors
    ///
    /// [`SessionError::NoRoom`] when every session with a process is busy, [`SessionError::Provider`] when the driver
    /// refused. The provider's own variant is preserved, because "not installed" and "authenticate at your machine"
    /// are different next moves for the operator.
    pub async fn start(
        &mut self,
        provider: &dyn Provider,
        intent: OpenIntent,
    ) -> Result<SessionId, SessionError> {
        let session = intent.session;
        match self.admit()? {
            Admit::Straight => {}
            // Something has to give way first. Detaching rather than killing: the conversation is not runtrol's to
            // end, and the operator picks it back up from the provider's own store.
            Admit::Evicting { session: victim } => {
                self.detach(victim, CloseMode::Graceful { grace_ms: 0 })
                    .await;
            }
        }

        let identity = match &intent.disposition {
            Disposition::Fresh => Identity::mint(provider.id()),
            // A resume already has both names. Nothing is minted, because minting would give the same conversation
            // a second identity and the operator would see it twice in one list.
            Disposition::Resume { native } => {
                let mut identity = Identity::mint(provider.id());
                if let Ok(native) = runtrol_provider::NativeSessionId::new(native) {
                    identity.observe_native(native);
                }
                identity
            }
            other => {
                return Err(SessionError::Provider(ProviderError::Unsupported {
                    provider: provider.id(),
                    what: format!("{other:?}"),
                    why: "the kernel has no rule for that way of opening a session yet",
                }));
            }
        };

        // Kept before the intent is handed over, because opening consumes it and a listing still has to be able
        // to say which folder this session works in.
        let workspace = intent.workspace.clone();
        let agent = provider.open(intent).await?;
        let mut state = SessionState::new(WallMs::now());
        // Binding happened: the driver answered. Recorded through the transition table like everything else, so there
        // is still exactly one place a state may change.
        let now = WallMs::now();
        drop(state.observe(Observed::Attaching, now));
        drop(state.observe(Observed::Attached, now));

        self.live.insert(
            session,
            Live {
                agent,
                hub: SessionHub::new(session),
                identity,
                workspace,
                state,
            },
        );
        Ok(session)
    }

    /// Read one event from a session and let it change what runtrol believes.
    ///
    /// Returns what was published, or `None` once the session's stream is over. A stream that ended has its session
    /// removed from the live map, because a session with no process is not a hot session.
    ///
    /// # Errors
    ///
    /// [`SessionError::NotLive`] when nothing is running under that name. A **provider** failure is not returned: it
    /// is promoted to session state and published as a notice, because a failure only returned to a caller is a
    /// failure the operator never hears about.
    pub async fn pump_once(
        &mut self,
        session: SessionId,
    ) -> Result<Option<Published>, SessionError> {
        let live = self
            .live
            .get_mut(&session)
            .ok_or(SessionError::NotLive { session })?;
        let spoke = live.agent.next().await;
        Ok(self.apply(session, spoke))
    }

    /// Wait until any live session speaks, and let that event change what runtrol believes.
    ///
    /// Every live session is asked in turn, starting after the one that spoke last so that a talkative session
    /// cannot keep a quiet one from ever being heard. The first that has something ready is the one applied.
    ///
    /// # This does not finish while nothing is live
    ///
    /// With no live sessions there is nothing that could speak, so this waits. That is what a caller racing it
    /// against its listener wants: an arm that never fires rather than one that fires constantly and turns an idle
    /// daemon into a spin. Anything awaiting it alone would wait forever, which is why it is written here.
    pub async fn pump_any(&mut self) -> Pumped {
        let (session, spoke) = self.hear_one().await;
        // The next round starts after this one, which is the whole of the fairness rule.
        self.after = Some(session);
        Pumped {
            session,
            published: self.apply(session, spoke),
        }
    }

    /// The first live session with something ready, and what it said.
    ///
    /// Each session is asked with no waiting in between. A session that is not ready is set aside, which the
    /// [`Agent::next`] contract requires to cost it nothing, and asked again when something wakes this.
    async fn hear_one(&mut self) -> (SessionId, Option<Result<Produced, ProviderError>>) {
        let live = &mut self.live;
        let after = self.after;
        core::future::poll_fn(|cx| {
            // Everything past the one that spoke last, then everything up to and including it. Two passes over two
            // ranges rather than one rotated pass, because rotating would mean borrowing the map twice at the same
            // time. The one that spoke last is at the end of the second pass, so it is asked last and never
            // skipped. Naming it by exclusion also means it does not have to still be live.
            let past = match after {
                Some(after) => (Bound::Excluded(after), Bound::Unbounded),
                None => (Bound::Unbounded, Bound::Unbounded),
            };
            for (session, one) in live.range_mut(past) {
                if let Poll::Ready(spoke) = pin!(one.agent.next()).poll(cx) {
                    return Poll::Ready((*session, spoke));
                }
            }
            if let Some(after) = after {
                for (session, one) in live.range_mut(..=after) {
                    if let Poll::Ready(spoke) = pin!(one.agent.next()).poll(cx) {
                        return Poll::Ready((*session, spoke));
                    }
                }
            }
            Poll::Pending
        })
        .await
    }

    /// What one answer from a driver means for the session it came from.
    ///
    /// The one place an event changes anything. Both ways of pumping arrive here, so there is no second rule about
    /// what a broken stream or an ended one does.
    fn apply(
        &mut self,
        session: SessionId,
        spoke: Option<Result<Produced, ProviderError>>,
    ) -> Option<Published> {
        // The session is here: both callers just read from it. Asked for rather than assumed, so that a future
        // caller which does not hold that guarantee gets nothing rather than a panic.
        let live = self.live.get_mut(&session)?;

        match spoke {
            Some(Ok(produced)) => {
                // The provider's own name may have arrived with this frame. The newest answer wins.
                if let Some(native) = live.agent.native()
                    && let Ok(parsed) = runtrol_provider::NativeSessionId::new(native)
                {
                    live.identity.observe_native(parsed);
                }

                let observed = observation_of(&produced.body);
                let published = live.hub.publish(produced.src_end, produced.body);
                if let Some(observed) = observed {
                    // A driver reporting something that cannot have happened becomes a notice rather than a panic. A
                    // supervisor that aborted on one misbehaving driver would take every other session with it.
                    if let Err(refusal) = live.state.observe(observed, published.event.at) {
                        live.hub.publish(
                            published.event.src_end,
                            notice(
                                NoticeCode::ProtocolViolation,
                                Level::Warn,
                                &refusal.to_string(),
                            ),
                        );
                    }
                }
                Some(published)
            }

            Some(Err(error)) => {
                // Promoted to session state and said out loud, in that order. The session stays visible and resumable:
                // the conversation is in the provider's own store and only the attachment was lost.
                let detail = error.to_string();
                let at = WallMs::now();
                drop(live.state.observe(
                    Observed::Failed {
                        code: FailureCode::Protocol,
                        detail: detail.clone(),
                    },
                    at,
                ));
                let published = live.hub.publish(
                    live.hub.src_end(),
                    notice(NoticeCode::ProtocolViolation, Level::Error, &detail),
                );
                self.live.remove(&session);
                Some(published)
            }

            None => {
                // The stream is over. Whether the turn that was running finished is answered by the events that came
                // before, never by this: the driver already reported an ending declared by the exit if there was one.
                let at = WallMs::now();
                drop(live.state.observe(Observed::Detached, at));
                self.live.remove(&session);
                None
            }
        }
    }

    /// Send a command to a live session.
    ///
    /// # Errors
    ///
    /// [`SessionError::NotLive`] when nothing is running under that name, [`SessionError::Provider`] when the driver
    /// refused. A refusal is returned rather than swallowed: an operator who pressed something has to know it did not
    /// happen.
    pub async fn send(
        &mut self,
        session: SessionId,
        command: AgentCommand,
    ) -> Result<(), SessionError> {
        let live = self
            .live
            .get_mut(&session)
            .ok_or(SessionError::NotLive { session })?;
        live.agent.send(command).await.map_err(SessionError::from)
    }

    /// End a session, and hand back the driver that still has to be stopped.
    ///
    /// # Why this does not do the stopping
    ///
    /// Because stopping is a wait. A graceful close gives the process time to finish, and this is the one call that
    /// could take seconds; doing it here would hold the only owner of every session for that long, and every other
    /// session's output would stop while one of them was being closed. Waiting belongs to whoever owns the runtime,
    /// which is the daemon, and this returns as soon as the thing it does own has changed.
    ///
    /// The session is gone from this manager by the time this returns, whatever the caller then does with the driver.
    /// Dropping it rather than closing it still stops the process, because a driver holds its child that way.
    ///
    /// # Errors
    ///
    /// [`SessionError::NotLive`] when nothing is running under that name.
    pub fn close(&mut self, session: SessionId) -> Result<Box<dyn Agent>, SessionError> {
        let live = self
            .live
            .remove(&session)
            .ok_or(SessionError::NotLive { session })?;
        Ok(live.agent)
    }

    /// Take a session's process away without treating it as an ending.
    ///
    /// Used when the tier needs room. A failure to stop is not returned: the caller is starting a different session
    /// and cannot act on it, and the containment holds the child either way. Nothing is silent, because the session it
    /// belonged to is gone from the live map and its absence is what a listing shows.
    async fn detach(&mut self, session: SessionId, how: CloseMode) {
        if let Some(live) = self.live.remove(&session) {
            drop(live.agent.close(how).await);
        }
    }

    /// Whether another session may have a process, and what gives way if so.
    fn admit(&self) -> Result<Admit, crate::session::tier::NoRoom> {
        let held: Vec<HotSession> = self
            .live
            .iter()
            .map(|(session, live)| HotSession {
                session: *session,
                last_seen: live.state.last_seen(),
                busy: live.state.lifecycle().turn().is_some(),
            })
            .collect();
        crate::session::tier::admit(&held)
    }

    /// Every live session, oldest name first.
    ///
    /// Only the ones with a process. The rest of a listing comes from the providers' own stores and from runtrol's
    /// rows, and joining those needs a driver that can read a provider's session store: measured at 4.4 milliseconds
    /// against 39.9 seconds for asking the CLI, which is why the join is a file read and not a question. That reader
    /// arrives with the driver that owns it.
    pub fn live_sessions(&self) -> impl Iterator<Item = LiveSession<'_>> {
        self.live.iter().map(|(session, live)| LiveSession {
            session: *session,
            provider: live.identity.provider(),
            native: live.identity.native().map(AsRef::as_ref),
            workspace: &live.workspace,
            tier: Tier::Hot,
            state: &live.state,
        })
    }
}

impl Default for SessionManager {
    fn default() -> Self {
        Self::new()
    }
}

/// One session that has a process, as a listing sees it.
#[derive(Clone, Copy, Debug)]
pub struct LiveSession<'manager> {
    /// runtrol's own name.
    pub session: SessionId,
    /// Which CLI it belongs to.
    pub provider: ProviderId,
    /// The provider's own name, once it has announced one.
    pub native: Option<&'manager str>,
    /// Where the agent works.
    pub workspace: &'manager AbsPath,
    /// How much of it exists. Always the hot tier here, by definition.
    pub tier: Tier,
    /// What it is doing.
    pub state: &'manager SessionState,
}

/// A notice runtrol originates.
fn notice(code: NoticeCode, level: Level, detail: &str) -> EventBody {
    // Serialized rather than pasted, because the text can carry a provider's own words and pasting would make those
    // words into structure.
    let payload = serde_json::to_string(&serde_json::json!({"detail": detail}))
        .unwrap_or_else(|_| String::from(r#"{"detail":"unprintable"}"#));
    EventBody::Notice(Box::new(Notice {
        level,
        code,
        retryable: false,
        payload: Opaque::owned(payload),
    }))
}

/// What a published event says about the session's state, when it says anything.
///
/// Most events say nothing: content is content. The ones that matter are the turn's own frames, which the driver
/// stamped, and a detach.
fn observation_of(body: &EventBody) -> Option<Observed> {
    match body {
        EventBody::Turn(turn) => match turn {
            runtrol_provider::TurnEvent::Started { turn }
            | runtrol_provider::TurnEvent::Accepted { turn, .. } => {
                Some(Observed::TurnStarted { turn: *turn })
            }
            runtrol_provider::TurnEvent::Ended { turn, .. } => {
                Some(Observed::TurnEnded { turn: *turn })
            }
            // Blocked and resumed are about what a turn is waiting for, not about whether it is running. The state
            // machine has no transition for them because there is nothing to change.
            _ => None,
        },
        EventBody::Detached(_) => Some(Observed::Detached),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use runtrol_provider::{AbsPath, Declarant, Produced, StopReason, TurnEvent, TurnId};

    use super::*;

    /// What a scripted driver answers next.
    ///
    /// A description rather than a held answer, because a provider error carries an operating system error and cannot
    /// be cloned. Describing it also makes each test read as a sequence of events rather than as a pile of literals.
    #[derive(Clone, Copy)]
    enum Step {
        /// Something that is conversation and must not move the session.
        Content,
        /// A turn beginning.
        TurnStarted(u32),
        /// A turn ending, by the provider's own word.
        TurnEnded(u32),
        /// The provider broke its own protocol.
        Broken,
        /// The stream is over.
        End,
    }

    impl Step {
        /// What the driver hands back for this step.
        fn answer(self, provider: ProviderId) -> Option<Result<Produced, ProviderError>> {
            match self {
                Self::Content => Some(Ok(content())),
                Self::TurnStarted(index) => Some(Ok(turn_started(index))),
                Self::TurnEnded(index) => Some(Ok(turn_ended(index))),
                Self::Broken => Some(Err(ProviderError::Protocol {
                    provider,
                    doing: "reading a frame",
                    detail: "the vendor changed shape".to_owned(),
                })),
                Self::End => None,
            }
        }
    }

    /// A driver that answers from a script, so every rule can be checked without a process.
    struct Scripted {
        provider: ProviderId,
        session: SessionId,
        native: Option<String>,
        /// Answered in order. Past the end it keeps answering that the stream is over.
        script: Vec<Step>,
        at: usize,
    }

    #[async_trait]
    impl Agent for Scripted {
        fn session(&self) -> SessionId {
            self.session
        }

        fn native(&self) -> Option<&str> {
            self.native.as_deref()
        }

        async fn send(&mut self, _command: AgentCommand) -> Result<(), ProviderError> {
            Ok(())
        }

        async fn next(&mut self) -> Option<Result<Produced, ProviderError>> {
            let step = self.script.get(self.at).copied().unwrap_or(Step::End);
            self.at += 1;
            step.answer(self.provider)
        }

        async fn close(self: Box<Self>, _how: CloseMode) -> Result<(), ProviderError> {
            Ok(())
        }
    }

    struct Fake {
        provider: ProviderId,
        script: Vec<Step>,
        native: Option<String>,
        /// The provider is not installed, which is the refusal a start has to preserve rather than flatten.
        not_installed: bool,
    }

    #[async_trait]
    impl Provider for Fake {
        fn id(&self) -> ProviderId {
            self.provider
        }

        async fn open(&self, intent: OpenIntent) -> Result<Box<dyn Agent>, ProviderError> {
            if self.not_installed {
                return Err(ProviderError::BinNotFound {
                    provider: self.provider,
                    searched: "nowhere".to_owned(),
                });
            }
            Ok(Box::new(Scripted {
                provider: self.provider,
                session: intent.session,
                native: self.native.clone(),
                script: self.script.clone(),
                at: 0,
            }))
        }
    }

    fn an_id() -> ProviderId {
        ProviderId::parse("example").expect("the test's own id must be valid")
    }

    fn a_fake(script: Vec<Step>) -> Fake {
        Fake {
            provider: an_id(),
            script,
            native: Some("native-name".to_owned()),
            not_installed: false,
        }
    }

    fn an_intent(session: SessionId) -> OpenIntent {
        OpenIntent {
            session,
            workspace: AbsPath::new(if cfg!(windows) { r"C:\work" } else { "/work" })
                .expect("valid"),
            disposition: Disposition::Fresh,
            model: None,
            permission: None,
        }
    }

    fn content() -> Produced {
        Produced {
            src_end: 1,
            body: EventBody::Plan(Opaque::none()),
        }
    }

    fn turn_started(index: u32) -> Produced {
        Produced {
            src_end: 2,
            body: EventBody::Turn(TurnEvent::Started {
                turn: TurnId { epoch: 0, index },
            }),
        }
    }

    fn turn_ended(index: u32) -> Produced {
        Produced {
            src_end: 3,
            body: EventBody::Turn(TurnEvent::Ended {
                turn: TurnId { epoch: 0, index },
                stop: StopReason::EndTurn,
                declared_by: Declarant::Provider,
            }),
        }
    }

    #[tokio::test]
    async fn starting_a_session_makes_it_live_and_bound() {
        let provider = a_fake(vec![]);
        let mut manager = SessionManager::new();
        let session = SessionId::now();

        let started = manager
            .start(&provider, an_intent(session))
            .await
            .expect("a scripted driver opens");

        assert_eq!(started, session);
        assert!(manager.is_live(session));
        assert_eq!(manager.hot(), 1);
        assert_eq!(
            manager.state(session).map(|state| state.lifecycle().name()),
            Some("idle"),
            "the driver answered, so the session is bound and not in a turn"
        );
    }

    #[tokio::test]
    async fn a_driver_that_refuses_leaves_no_session_with_a_process() {
        // A session that failed to open is not a session with a process. Leaving one in the map would have it show up
        // in a listing as running.
        let provider = Fake {
            provider: an_id(),
            script: vec![],
            native: None,
            not_installed: true,
        };
        let mut manager = SessionManager::new();
        let session = SessionId::now();

        match manager.start(&provider, an_intent(session)).await {
            Err(SessionError::Provider(ProviderError::BinNotFound { .. })) => {}
            other => panic!("expected the provider's own variant, got {other:?}"),
        }
        assert!(!manager.is_live(session));
        assert_eq!(manager.hot(), 0);
    }

    #[tokio::test]
    async fn a_turn_starting_and_ending_moves_the_session_and_nothing_else_does() {
        let provider = a_fake(vec![
            Step::Content,
            Step::TurnStarted(0),
            Step::Content,
            Step::TurnEnded(0),
        ]);
        let mut manager = SessionManager::new();
        let session = SessionId::now();
        manager
            .start(&provider, an_intent(session))
            .await
            .expect("opens");

        manager
            .pump_once(session)
            .await
            .expect("content")
            .expect("published");
        assert_eq!(
            manager.state(session).map(|state| state.lifecycle().name()),
            Some("idle"),
            "content is content"
        );

        manager
            .pump_once(session)
            .await
            .expect("a turn begins")
            .expect("published");
        assert!(
            manager
                .state(session)
                .and_then(|state| state.lifecycle().turn())
                .is_some(),
            "a turn is running"
        );

        manager
            .pump_once(session)
            .await
            .expect("content")
            .expect("published");
        assert!(
            manager
                .state(session)
                .and_then(|state| state.lifecycle().turn())
                .is_some(),
            "content during a turn leaves it running"
        );

        manager
            .pump_once(session)
            .await
            .expect("the turn ends")
            .expect("published");
        assert_eq!(
            manager.state(session).map(|state| state.lifecycle().name()),
            Some("idle")
        );
    }

    #[tokio::test]
    async fn events_reach_a_watcher_numbered_and_in_order() {
        let provider = a_fake(vec![Step::Content, Step::Content, Step::Content]);
        let mut manager = SessionManager::new();
        let session = SessionId::now();
        manager
            .start(&provider, an_intent(session))
            .await
            .expect("opens");

        let mut watcher = manager
            .subscribe(session)
            .expect("a live session can be watched");
        for _ in 0..3 {
            manager.pump_once(session).await.expect("published");
        }

        let mut positions = Vec::new();
        while let Some(frame) = watcher.try_recv() {
            positions.push(frame.seq);
        }
        assert_eq!(positions, vec![0, 1, 2], "dense and in order");
    }

    #[tokio::test]
    async fn a_provider_failure_becomes_session_state_and_is_said_out_loud() {
        // A failure only returned to a caller is a failure the operator never hears about.
        let provider = a_fake(vec![Step::Broken]);
        let mut manager = SessionManager::new();
        let session = SessionId::now();
        manager
            .start(&provider, an_intent(session))
            .await
            .expect("opens");
        let mut watcher = manager.subscribe(session).expect("watchable");

        let published = manager
            .pump_once(session)
            .await
            .expect("a failure is not returned as an error")
            .expect("it is published");

        match &published.event.body {
            EventBody::Notice(notice) => {
                assert_eq!(notice.code, NoticeCode::ProtocolViolation);
                assert_eq!(notice.level, Level::Error);
                assert!(notice.payload.as_str().contains("changed shape"));
            }
            other => panic!("expected a notice, got {other:?}"),
        }
        assert!(watcher.try_recv().is_some(), "and a watcher receives it");
        assert!(
            !manager.is_live(session),
            "a session whose stream broke has no process"
        );
    }

    /// A driver that never has anything ready, and never asks to be woken.
    ///
    /// Nothing waits on it alone. It is here so that a session which has nothing to say can be shown not to hold
    /// up a session that does, which is the whole reason waiting on all of them is one call.
    struct Silent(SessionId);

    #[async_trait]
    impl Agent for Silent {
        fn session(&self) -> SessionId {
            self.0
        }

        fn native(&self) -> Option<&str> {
            None
        }

        async fn send(&mut self, _command: AgentCommand) -> Result<(), ProviderError> {
            Ok(())
        }

        async fn next(&mut self) -> Option<Result<Produced, ProviderError>> {
            core::future::pending().await
        }

        async fn close(self: Box<Self>, _how: CloseMode) -> Result<(), ProviderError> {
            Ok(())
        }
    }

    struct Quiet;

    #[async_trait]
    impl Provider for Quiet {
        fn id(&self) -> ProviderId {
            an_id()
        }

        async fn open(&self, intent: OpenIntent) -> Result<Box<dyn Agent>, ProviderError> {
            Ok(Box::new(Silent(intent.session)))
        }
    }

    #[tokio::test]
    async fn a_session_with_nothing_to_say_does_not_hold_up_one_that_does() {
        // The reason waiting on every session is one call. A supervisor that had to ask them in order would stop
        // at the first quiet one, and every other session would go unheard until that one spoke.
        let mut manager = SessionManager::new();
        let quiet = SessionId::now();
        manager
            .start(&Quiet, an_intent(quiet))
            .await
            .expect("opens");
        let talkative = SessionId::now();
        manager
            .start(&a_fake(vec![Step::Content]), an_intent(talkative))
            .await
            .expect("opens");

        let heard = manager.pump_any().await;
        assert_eq!(
            heard.session, talkative,
            "the session with something ready is the one that is heard"
        );
        assert!(heard.published.is_some());
    }

    #[tokio::test]
    async fn no_session_is_heard_twice_before_every_other_one_is_heard_once() {
        // Without this, a session producing a long stream keeps every other session silent for as long as it
        // talks, and an operator watching a second session sees nothing at all.
        let provider = a_fake(vec![Step::Content, Step::Content, Step::Content]);
        let mut manager = SessionManager::new();
        let mut names = Vec::new();
        for _ in 0..3 {
            let session = SessionId::now();
            manager
                .start(&provider, an_intent(session))
                .await
                .expect("opens");
            names.push(session);
        }

        let mut heard = Vec::new();
        for _ in 0..3 {
            heard.push(manager.pump_any().await.session);
        }
        heard.sort_unstable();
        let mut expected = names.clone();
        expected.sort_unstable();
        assert_eq!(
            heard, expected,
            "every session was heard once before any was heard twice"
        );

        // And the round after that starts over rather than sticking to the last one.
        assert_eq!(
            manager.pump_any().await.session,
            *names.first().expect("one")
        );
    }

    #[tokio::test]
    async fn waiting_on_every_session_applies_the_same_rules_as_waiting_on_one() {
        // Two ways of pumping and one rule about what an event means. A stream that ends has to take its session
        // out of the live map whichever way it was heard.
        let provider = a_fake(vec![Step::End]);
        let mut manager = SessionManager::new();
        let session = SessionId::now();
        manager
            .start(&provider, an_intent(session))
            .await
            .expect("opens");

        let heard = manager.pump_any().await;
        assert_eq!(heard.session, session);
        assert!(heard.published.is_none(), "the stream is over");
        assert!(!manager.is_live(session));
    }

    #[tokio::test]
    async fn a_stream_that_ends_takes_the_session_out_of_the_live_map() {
        let provider = a_fake(vec![Step::Content, Step::End]);
        let mut manager = SessionManager::new();
        let session = SessionId::now();
        manager
            .start(&provider, an_intent(session))
            .await
            .expect("opens");

        manager
            .pump_once(session)
            .await
            .expect("content")
            .expect("published");
        assert!(
            manager
                .pump_once(session)
                .await
                .expect("the stream ends")
                .is_none()
        );
        assert!(!manager.is_live(session));
        assert_eq!(manager.hot(), 0);
    }

    #[tokio::test]
    async fn a_driver_reporting_something_impossible_produces_a_notice_and_not_a_panic() {
        // A supervisor that aborted on one misbehaving driver would take every other session with it.
        let provider = a_fake(vec![Step::TurnEnded(9)]);
        let mut manager = SessionManager::new();
        let session = SessionId::now();
        manager
            .start(&provider, an_intent(session))
            .await
            .expect("opens");
        let mut watcher = manager.subscribe(session).expect("watchable");

        manager
            .pump_once(session)
            .await
            .expect("published")
            .expect("a frame");

        let mut frames = Vec::new();
        while let Some(frame) = watcher.try_recv() {
            frames.push(frame);
        }
        let refusal = frames
            .iter()
            .find_map(|frame| match &frame.body {
                EventBody::Notice(notice) => Some(notice),
                _ => None,
            })
            .expect("the impossible report must reach a watcher");
        assert_eq!(refusal.code, NoticeCode::ProtocolViolation);
        assert!(
            manager.is_live(session),
            "one misbehaving report does not end the session"
        );
    }

    #[tokio::test]
    async fn the_providers_own_name_is_recorded_as_soon_as_it_is_announced() {
        let provider = a_fake(vec![Step::Content]);
        let mut manager = SessionManager::new();
        let session = SessionId::now();
        manager
            .start(&provider, an_intent(session))
            .await
            .expect("opens");

        manager.pump_once(session).await.expect("published");
        assert_eq!(
            manager.native(session),
            Some("native-name"),
            "a resume needs the name the provider knows"
        );
    }

    #[tokio::test]
    async fn a_resume_does_not_give_one_conversation_a_second_identity() {
        // Minting for a resume would show the same conversation twice in one list.
        let provider = a_fake(vec![]);
        let mut manager = SessionManager::new();
        let session = SessionId::now();
        let mut intent = an_intent(session);
        intent.disposition = Disposition::Resume {
            native: "already-exists".into(),
        };

        manager.start(&provider, intent).await.expect("opens");
        assert_eq!(manager.native(session), Some("already-exists"));
    }

    #[tokio::test]
    async fn the_hot_bound_holds_and_the_forgotten_session_gives_way() {
        // A thousand sessions in a list must not mean a thousand child processes. The bound is about the operator's
        // machine: at the measured working sets of these CLIs, eight of them is gigabytes.
        let provider = a_fake(vec![]);
        let mut manager = SessionManager::new();
        let mut names = Vec::new();
        for _ in 0..crate::session::tier::MAX_HOT {
            let session = SessionId::now();
            manager
                .start(&provider, an_intent(session))
                .await
                .expect("opens");
            names.push(session);
        }
        assert_eq!(manager.hot(), crate::session::tier::MAX_HOT);

        let extra = SessionId::now();
        manager
            .start(&provider, an_intent(extra))
            .await
            .expect("room is made");

        assert_eq!(
            manager.hot(),
            crate::session::tier::MAX_HOT,
            "the bound held"
        );
        assert!(manager.is_live(extra));
        assert!(
            names.iter().filter(|one| manager.is_live(**one)).count()
                == crate::session::tier::MAX_HOT - 1,
            "exactly one gave way"
        );
    }

    #[tokio::test]
    async fn a_session_with_a_turn_running_is_never_the_one_that_gives_way() {
        let provider = a_fake(vec![Step::TurnStarted(0)]);
        let mut manager = SessionManager::new();
        let mut names = Vec::new();
        for _ in 0..crate::session::tier::MAX_HOT {
            let session = SessionId::now();
            manager
                .start(&provider, an_intent(session))
                .await
                .expect("opens");
            names.push(session);
        }

        // Put the oldest into a turn, which is the one an eviction would otherwise choose.
        let oldest = *names.first().expect("one session");
        manager.pump_once(oldest).await.expect("a turn begins");

        manager
            .start(&provider, an_intent(SessionId::now()))
            .await
            .expect("room is made from an idle session");
        assert!(
            manager.is_live(oldest),
            "the busy session was evicted despite being the oldest"
        );
    }

    #[tokio::test]
    async fn when_every_session_is_busy_a_new_one_is_refused_with_a_reason() {
        // Refused rather than resolved by force. An operator told why can wait or stop something; one whose running
        // turn was interrupted has lost work and does not know it.
        let provider = a_fake(vec![Step::TurnStarted(0)]);
        let mut manager = SessionManager::new();
        for _ in 0..crate::session::tier::MAX_HOT {
            let session = SessionId::now();
            manager
                .start(&provider, an_intent(session))
                .await
                .expect("opens");
            manager.pump_once(session).await.expect("a turn begins");
        }

        match manager.start(&provider, an_intent(SessionId::now())).await {
            Err(SessionError::NoRoom(refusal)) => {
                assert_eq!(refusal.held, crate::session::tier::MAX_HOT);
            }
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn closing_takes_the_process_away() {
        let provider = a_fake(vec![]);
        let mut manager = SessionManager::new();
        let session = SessionId::now();
        manager
            .start(&provider, an_intent(session))
            .await
            .expect("opens");

        let stopping = manager.close(session).expect("closing works");
        assert!(
            !manager.is_live(session),
            "the session is gone before anything waits for the process"
        );
        stopping
            .close(CloseMode::Kill)
            .await
            .expect("the driver stops");

        match manager.close(session) {
            Err(SessionError::NotLive { session: named }) => assert_eq!(named, session),
            Ok(_) => panic!("a session that is gone cannot be closed again"),
            Err(other) => panic!("expected a refusal naming the session, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn closing_does_not_make_anybody_wait_for_a_process_to_stop() {
        // The reason this is not one call. A graceful close gives the process seconds to finish, and doing that here
        // would hold the only owner of every session for that long: one session being closed would stop every other
        // session's output. What this owns has changed by the time it returns.
        struct Slow;

        #[async_trait]
        impl Agent for Slow {
            fn session(&self) -> SessionId {
                SessionId::now()
            }
            fn native(&self) -> Option<&str> {
                None
            }
            async fn send(&mut self, _command: AgentCommand) -> Result<(), ProviderError> {
                Ok(())
            }
            async fn next(&mut self) -> Option<Result<Produced, ProviderError>> {
                core::future::pending().await
            }
            async fn close(self: Box<Self>, _how: CloseMode) -> Result<(), ProviderError> {
                core::future::pending::<()>().await;
                Ok(())
            }
        }

        struct Sluggish;

        #[async_trait]
        impl Provider for Sluggish {
            fn id(&self) -> ProviderId {
                an_id()
            }
            async fn open(&self, _intent: OpenIntent) -> Result<Box<dyn Agent>, ProviderError> {
                Ok(Box::new(Slow))
            }
        }

        let mut manager = SessionManager::new();
        let session = SessionId::now();
        manager
            .start(&Sluggish, an_intent(session))
            .await
            .expect("opens");

        // A driver that never finishes stopping. Taking it away still returns, and what it returns is the wait
        // somebody else is free to do.
        let stopping = manager.close(session).expect("the session is taken away");
        assert!(!manager.is_live(session));
        assert_eq!(manager.hot(), 0);
        drop(stopping);
    }

    #[tokio::test]
    async fn nothing_can_be_asked_of_a_session_that_is_not_live() {
        let mut manager = SessionManager::new();
        let absent = SessionId::now();
        assert!(matches!(
            manager.pump_once(absent).await,
            Err(SessionError::NotLive { .. })
        ));
        assert!(matches!(
            manager.send(absent, AgentCommand::Interrupt).await,
            Err(SessionError::NotLive { .. })
        ));
        assert!(matches!(
            manager.subscribe(absent),
            Err(SessionError::NotLive { .. })
        ));
        assert_eq!(manager.state(absent), None);
    }

    #[tokio::test]
    async fn a_listing_of_live_sessions_carries_both_names_and_the_tier() {
        let provider = a_fake(vec![Step::Content]);
        let mut manager = SessionManager::new();
        let session = SessionId::now();
        manager
            .start(&provider, an_intent(session))
            .await
            .expect("opens");
        manager.pump_once(session).await.expect("published");

        let listed: Vec<LiveSession<'_>> = manager.live_sessions().collect();
        assert_eq!(listed.len(), 1);
        let one = listed.first().expect("one session");
        assert_eq!(one.session, session);
        assert_eq!(one.provider, an_id());
        assert_eq!(one.native, Some("native-name"));
        assert_eq!(one.tier, Tier::Hot, "a listing of live sessions is all hot");
    }

    #[test]
    fn only_the_turns_own_frames_say_anything_about_the_state() {
        // Most events are content, and content must not move a session. Getting this wrong is how a message ends a
        // turn.
        assert!(observation_of(&EventBody::Plan(Opaque::none())).is_none());
        assert!(matches!(
            observation_of(&turn_started(0).body),
            Some(Observed::TurnStarted { .. })
        ));
        assert!(matches!(
            observation_of(&turn_ended(0).body),
            Some(Observed::TurnEnded { .. })
        ));
        assert!(
            observation_of(&EventBody::Turn(TurnEvent::Blocked {
                turn: TurnId { epoch: 0, index: 0 },
                on: runtrol_provider::BlockedOn::UserInput,
            }))
            .is_none(),
            "waiting on a person is not a change of whether a turn is running"
        );
    }

    #[test]
    fn a_notice_runtrol_originates_never_pastes_text_into_structure() {
        // The detail can carry a provider's own words, and pasting would make those words into structure.
        let body = notice(NoticeCode::Other, Level::Info, r#"a "quoted" }brace{ mess"#);
        match body {
            EventBody::Notice(one) => {
                let parsed: serde_json::Value =
                    serde_json::from_str(one.payload.as_str()).expect("readable JSON");
                assert_eq!(
                    parsed.pointer("/detail").and_then(|v| v.as_str()),
                    Some(r#"a "quoted" }brace{ mess"#)
                );
            }
            other => panic!("expected a notice, got {other:?}"),
        }
    }
}
