//! One request in, one answer out.
//!
//! # Nothing here panics and nothing here is silent
//!
//! Every request produces an answer, including the ones that cannot be carried out. A dispatcher that returned nothing
//! for a case it did not expect would leave the command surface waiting forever, which looks exactly like a daemon that
//! has stopped and is much harder to diagnose than a refusal.
//!
//! # The greeting comes first, and it is enforced rather than assumed
//!
//! A connection that has not agreed on a wire format is answered with a refusal to every other request. Reading a
//! request from a build that speaks a different format would mean acting on somebody else's meaning, and the failure
//! that produces is a command landing somewhere the operator did not intend.
//!
//! # Where the scope wall goes
//!
//! Here, at the boundary, and not deeper. A request that arrives from somewhere other than this machine has to be
//! refused before it reaches anything that can act, and the place a request arrives is the only place that knows where
//! it came from. The wall itself lives in the security crate, the table of what each request needs lives in
//! [`crate::scope`], and this is where the two are asked.
//!
//! Consulted **before** the request is read for anything else, and before the greeting is answered. A check that ran
//! after some other branch had already acted would be a check on the way out.

use std::collections::BTreeMap;
use std::sync::Arc;

use runtrol_core::registry::KindStatus;
use runtrol_core::session::SessionError;
use runtrol_core::{SessionManager, SessionView};
use runtrol_drivers::DriverContext;
use runtrol_ipc::wire::{ProviderLine, Request, Response, SessionLine, SessionListing, WireError};
use runtrol_provider::{
    AbsPath, AgentCommand, CloseMode, ContentBlock, Disposition, NativeSessionId, OpenIntent,
    Provider, ProviderId, SessionId, WallMs,
};
use runtrol_security::Caller;
use runtrol_store::{SessionRow as StoredSession, StoreError};

use crate::compose::Composed;

/// What a request produced.
pub enum Reply {
    /// One answer, and the connection is free for the next request.
    One(Response),
    /// The caller is now watching a session.
    ///
    /// A separate shape because watching is not a question with an answer: it is the connection changing what it is for,
    /// and a dispatcher that pretended otherwise would have to answer once and then keep writing.
    Watching(Box<SessionView>),
    /// The session is closed, and its process is still being stopped.
    ///
    /// A separate shape because stopping is a wait, and the answer is not known until it is over. Handing the wait out
    /// is what keeps one session being closed from stopping every other session's output for as long as it takes: by
    /// the time this is returned the sessions are already correct, and all that is left is a process.
    Stopping {
        /// The driver, with nothing left holding it.
        agent: Box<dyn runtrol_provider::Agent>,
        /// How much time the process is given.
        how: CloseMode,
    },
}

/// One connection's state.
///
/// Small on purpose: what a connection knows is who is on it and whether they have greeted, and nothing else. A
/// connection that remembered which session the caller "meant" would be the second place that notion lives.
#[derive(Debug)]
pub struct Conversation {
    /// Who is on the other end.
    ///
    /// Decided when the connection was accepted, from which endpoint it arrived on, and never afterwards. There is
    /// deliberately no way to set this from a request: a caller that could say who it was would say whatever got it
    /// the most authority.
    caller: Caller,
    /// The wire format has been agreed.
    greeted: bool,
}

impl Conversation {
    /// A connection from somebody at the machine, which has said nothing yet.
    ///
    /// The only constructor today, because the local endpoint is the only way in. A remote transport arrives with
    /// its own constructor taking the device it authenticated, and until then there is no way to build a
    /// conversation that claims to be one.
    #[must_use]
    pub const fn at_the_machine() -> Self {
        Self {
            caller: Caller::AtTheMachine,
            greeted: false,
        }
    }

    /// Who is on the other end.
    #[must_use]
    pub const fn caller(&self) -> &Caller {
        &self.caller
    }

    /// Whether the wire format has been agreed.
    #[must_use]
    pub const fn greeted(&self) -> bool {
        self.greeted
    }
}

/// Answer one request.
///
/// Takes the assembled daemon and the sessions, because a request is about one or the other and usually both.
pub async fn answer(
    conversation: &mut Conversation,
    composed: &Composed,
    sessions: &mut SessionManager,
    request: Request,
) -> Reply {
    // Before anything else looks at the request. A wall consulted after some other branch has acted is a wall on
    // the way out, and the thing it was supposed to prevent has already happened.
    if let Err(refusal) = crate::scope::allowed(&conversation.caller, &request, &composed.granted) {
        return Reply::One(refuse(&refusal.to_string()));
    }

    // The greeting is the one request that may arrive first, and everything else is refused until it has.
    if let Request::Hello { wire } = request {
        return match runtrol_ipc::wire::agree(wire) {
            Ok(agreed) => {
                conversation.greeted = true;
                Reply::One(Response::Welcome {
                    wire: agreed,
                    providers: providers_of(composed),
                })
            }
            Err(ours) => Reply::One(refuse(&format!(
                "this daemon speaks wire format {ours} and the caller speaks {wire}"
            ))),
        };
    }
    if !conversation.greeted {
        return Reply::One(refuse(
            "this connection has not agreed a wire format, so nothing on it can be acted on",
        ));
    }

    match request {
        // Answered above, and matched here so that adding a request cannot fall through to a wildcard that does nothing.
        Request::Hello { .. } => Reply::One(refuse("the wire format is already agreed")),

        Request::List => Reply::One(list(composed, sessions)),

        Request::Models { provider } => Reply::One(models(composed, &provider).await),

        Request::Start {
            provider,
            workspace,
            model,
            permission,
        } => Reply::One(
            open(
                composed,
                sessions,
                &provider,
                &workspace,
                Disposition::Fresh,
                model,
                permission,
            )
            .await,
        ),

        Request::Resume {
            provider,
            native,
            workspace,
        } => Reply::One(
            open(
                composed,
                sessions,
                &provider,
                &workspace,
                Disposition::Resume { native },
                None,
                None,
            )
            .await,
        ),

        Request::Prompt { session, text } => Reply::One(
            send(
                sessions,
                session,
                AgentCommand::Prompt(vec![ContentBlock::Text(text)]),
            )
            .await,
        ),

        Request::Interrupt { session } => {
            Reply::One(send(sessions, session, AgentCommand::Interrupt).await)
        }

        Request::Watch { session } => match sessions.subscribe(session) {
            Ok(watching) => Reply::Watching(Box::new(watching)),
            Err(error) => Reply::One(refuse(&error.to_string())),
        },

        Request::Close { session, now } => {
            // The vocabulary's own answer, not one driver's. Reaching into a driver for it would give every
            // other provider that driver's patience, and adding a second one would change how long the first is
            // waited for depending on which import somebody wrote.
            let how = if now {
                CloseMode::Kill
            } else {
                CloseMode::graceful()
            };
            let removed = match composed.store.remove_session(session) {
                Ok(removed) => removed,
                Err(error) => return Reply::One(refuse(&error.to_string())),
            };
            match sessions.close(session) {
                Ok(agent) => Reply::Stopping { agent, how },
                Err(SessionError::NotLive { .. }) if removed => Reply::One(Response::Done),
                Err(error) => Reply::One(from_session_error(&error)),
            }
        }

        // Consults nothing: no ledger, no scope, no configuration. The security posture requires this to work from
        // anywhere with no permission at all, and the worst a hostile caller achieves through it is stopping work.
        Request::StopEverything => match composed.containment.terminate_all() {
            Ok(()) => Reply::One(Response::Done),
            // Reported rather than swallowed. An operator who pressed the panic button has to know whether it worked.
            Err(error) => Reply::One(refuse(&error.to_string())),
        },

        // A request that arrived after this build was made. Refused by name, because a wildcard that answered "done"
        // would report something as carried out when nothing happened.
        other => Reply::One(refuse(&format!("this daemon has no binding for {other:?}"))),
    }
}

/// Open a session, fresh or continuing.
async fn open(
    composed: &Composed,
    sessions: &mut SessionManager,
    provider: &str,
    workspace: &str,
    disposition: Disposition,
    model: Option<Box<str>>,
    permission: Option<Box<str>>,
) -> Response {
    let driver = match driver(composed, provider) {
        Ok(driver) => driver,
        Err(response) => return response,
    };
    let Ok(workspace) = AbsPath::canonicalize(workspace) else {
        return refuse(&format!(
            "{workspace:?} is not a directory runtrol can work in"
        ));
    };
    let intent = OpenIntent {
        session: SessionId::now(),
        workspace,
        disposition,
        model,
        permission,
    };

    match sessions.start(driver.as_ref(), intent).await {
        Ok(session) => match persist_live(composed, sessions, session) {
            Ok(()) => Response::Started { session },
            Err(error) => {
                let stopping = match sessions.close(session) {
                    Ok(agent) => agent.close(CloseMode::Kill).await.err(),
                    Err(close_error) => {
                        return refuse(&format!(
                            "{error}; the unrecorded session also could not be detached: {close_error}"
                        ));
                    }
                };
                match stopping {
                    Some(close_error) => refuse(&format!(
                        "{error}; the unrecorded session also could not be stopped: {close_error}"
                    )),
                    None => refuse(&error.to_string()),
                }
            }
        },
        Err(error) => from_session_error(&error),
    }
}

/// Ask one provider driver for its current model choices.
async fn models(composed: &Composed, provider: &str) -> Response {
    let driver = match driver(composed, provider) {
        Ok(driver) => driver,
        Err(response) => return response,
    };
    match driver.models().await {
        Ok(catalogue) => Response::Models(catalogue),
        Err(error) => Response::Failed(WireError::from_provider(&error)),
    }
}

/// Build one declared and available driver, with runtime resolution owned by the probe.
fn driver(composed: &Composed, provider: &str) -> Result<Box<dyn Provider>, Response> {
    let Ok(id) = ProviderId::parse(provider) else {
        return Err(refuse(&format!(
            "{provider:?} is not a provider name runtrol accepts"
        )));
    };
    let Some(declared) = composed.registry.get(id) else {
        return Err(refuse(&format!("no provider called {provider}")));
    };
    match declared.kind {
        KindStatus::Available => {}
        KindStatus::Unavailable { why } => return Err(refuse(why)),
        KindStatus::Unknown => {
            return Err(refuse(&format!(
                "{provider} names a kind nothing in this build declares"
            )));
        }
    }

    let Some(entry) = composed.driver_for(declared.manifest.kind.as_str()) else {
        return Err(refuse("this build has no driver for that kind"));
    };
    let Some(make) = entry.make else {
        return Err(refuse(
            entry
                .unavailable
                .unwrap_or("this build cannot serve that kind"),
        ));
    };

    // Resolution belongs to the probe, so the driver runs the same program the probe examined.
    let program = match runtrol_core::locate(&declared.manifest) {
        Ok(program) => program,
        Err(error) => return Err(refuse(&error.to_string())),
    };

    Ok(make(&DriverContext {
        provider: id,
        models: declared.manifest.models.clone(),
        program,
        contained_by: Arc::clone(&composed.containment),
    }))
}

/// Hand a command to a live session.
async fn send(
    sessions: &mut SessionManager,
    session: SessionId,
    command: AgentCommand,
) -> Response {
    match sessions.send(session, command).await {
        Ok(()) => Response::Done,
        Err(error) => from_session_error(&error),
    }
}

/// The sessions this daemon can see.
///
/// Live ones only for now. The rest come from the providers' own stores and from runtrol's rows, and joining those needs
/// a driver that can read a provider's session store: measured at 4.4 milliseconds against 39.9 seconds for asking the
/// CLI, which is why that join is a file read and not a question. That reader arrives with the driver that owns it.
fn list(composed: &Composed, sessions: &SessionManager) -> Response {
    let stored = match composed.store.list_sessions() {
        Ok(stored) => stored,
        Err(error) => return refuse(&error.to_string()),
    };
    let mut joined = BTreeMap::new();
    for (session, row) in stored.sessions {
        if row.archived {
            continue;
        }
        joined.insert(
            session,
            SessionLine {
                session,
                provider: row.provider.as_str().into(),
                native: Some(row.native.as_str().into()),
                workspace: row.cwd.as_str().into(),
                hot: false,
                doing: "detached".into(),
                looks_stuck: false,
            },
        );
    }
    for one in sessions.live_sessions() {
        joined.insert(
            one.session,
            SessionLine {
                session: one.session,
                provider: one.provider.as_str().into(),
                native: one.native.map(Into::into),
                workspace: one.workspace.as_str().into(),
                hot: one.tier.has_a_process(),
                doing: one.state.lifecycle().name().into(),
                looks_stuck: one.state.looks_stuck(),
            },
        );
    }
    Response::Sessions(SessionListing {
        sessions: joined.into_values().collect(),
        warnings: stored
            .unreadable
            .into_iter()
            .map(|(session, error)| {
                format!("stored session {session} is unreadable: {error}").into()
            })
            .collect(),
    })
}

/// Persist the minimal pointer for one live session once its provider has named it.
///
/// No conversation value can enter this function: [`StoredSession`] has no field capable of holding one.
pub(crate) fn persist_live(
    composed: &Composed,
    sessions: &SessionManager,
    session: SessionId,
) -> Result<(), StoreError> {
    let Some(live) = sessions.live_session(session) else {
        return Ok(());
    };
    let Some(native) = live.native else {
        return Ok(());
    };
    let native = NativeSessionId::new(native).map_err(|_| StoreError::Codec {
        field: "native id",
        why: "the live provider identifier is not storable",
    })?;

    let prior_session = composed.store.find_by_native(live.provider, &native)?;
    let prior_row = match prior_session {
        Some(prior) => composed.store.get_session(prior)?,
        None => composed.store.get_session(session)?,
    };
    if let Some(prior) = prior_session
        && prior != session
    {
        composed.store.remove_session(prior)?;
    }

    let now = WallMs::now();
    composed.store.put_session(
        session,
        &StoredSession {
            provider: live.provider,
            native,
            cwd: live.workspace.clone(),
            label: prior_row.as_ref().and_then(|row| row.label.clone()),
            created_at: prior_row.as_ref().map_or(now, |row| row.created_at),
            last_seen_at: live.state.last_seen(),
            pinned: prior_row.as_ref().is_some_and(|row| row.pinned),
            archived: false,
            forked_from: prior_row.and_then(|row| row.forked_from),
            // The shared-daemon driver has no per-session process identity. A stale PID would be worse than
            // `None`, and hotness is joined from the live manager while this daemon is running.
            live: None,
        },
    )
}

/// Every provider this build knows about, usable or not.
fn providers_of(composed: &Composed) -> Vec<ProviderLine> {
    composed
        .registry
        .all()
        .map(|provider| ProviderLine {
            id: provider.id().as_str().into(),
            display_name: provider.manifest.display_name.clone(),
            usable: provider.is_usable(),
            why_not: match provider.kind {
                KindStatus::Available => None,
                KindStatus::Unavailable { why } => Some(why.into()),
                KindStatus::Unknown => Some("nothing in this build declares that kind".into()),
            },
        })
        .collect()
}

/// A session failure, as the caller sees it.
///
/// The provider's own variant is preserved where there is one, because "not installed" and "authenticate at your
/// machine" are different next moves for the operator.
fn from_session_error(error: &SessionError) -> Response {
    match error {
        SessionError::Provider(provider) => Response::Failed(WireError::from_provider(provider)),
        other => refuse(&other.to_string()),
    }
}

/// A refusal with a message and no claim about retrying.
pub(crate) fn refuse(message: &str) -> Response {
    Response::Failed(WireError::plain(message))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn composed_for(name: &str) -> (crate::compose::Composed, String) {
        let root = std::env::temp_dir().join(format!("runtrol-dispatch-{name}"));
        if root.exists() {
            std::fs::remove_dir_all(&root).expect("clear the previous run");
        }
        let text = root
            .to_str()
            .expect("the temporary path is UTF-8")
            .to_owned();
        // Composing without establishing containment: doing that in a test terminates the runner on one platform.
        let composed = crate::compose::Composed::for_tests(&text, runtrol_drivers::builtin())
            .expect("a fresh home composes");
        (composed, text)
    }

    fn clean(composed: crate::compose::Composed, path: &str) {
        // The store owns an exclusive file handle. Release it before removing the scratch home, especially on Windows.
        drop(composed);
        std::fs::remove_dir_all(path).expect("remove the scratch home");
    }

    #[tokio::test]
    async fn nothing_can_be_asked_before_the_wire_format_is_agreed() {
        // Acting on a request from a build that speaks a different format means acting on somebody else's meaning, and
        // the failure that produces is a command landing where the operator did not intend.
        let (composed, path) = composed_for("ungreeted");
        let mut sessions = SessionManager::new();
        let mut conversation = Conversation::at_the_machine();
        assert!(!conversation.greeted());

        match answer(&mut conversation, &composed, &mut sessions, Request::List).await {
            Reply::One(Response::Failed(failure)) => {
                assert!(
                    failure.message.contains("wire format"),
                    "{}",
                    failure.message
                );
            }
            other => panic!("expected a refusal, got {}", shape(&other)),
        }
        clean(composed, &path);
    }

    #[tokio::test]
    async fn the_greeting_answers_with_every_provider_this_build_knows() {
        let (composed, path) = composed_for("greeting");
        let mut sessions = SessionManager::new();
        let mut conversation = Conversation::at_the_machine();

        match answer(
            &mut conversation,
            &composed,
            &mut sessions,
            Request::Hello {
                wire: runtrol_ipc::WIRE_VERSION,
            },
        )
        .await
        {
            Reply::One(Response::Welcome { wire, providers }) => {
                assert_eq!(wire, runtrol_ipc::WIRE_VERSION);
                assert!(!providers.is_empty(), "a fresh install has providers");
                assert!(providers.iter().any(|one| one.usable));
            }
            other => panic!("expected a welcome, got {}", shape(&other)),
        }
        assert!(conversation.greeted());
        clean(composed, &path);
    }

    #[tokio::test]
    async fn a_caller_speaking_another_wire_format_is_told_both_numbers() {
        let (composed, path) = composed_for("mismatch");
        let mut sessions = SessionManager::new();
        let mut conversation = Conversation::at_the_machine();

        match answer(
            &mut conversation,
            &composed,
            &mut sessions,
            Request::Hello {
                wire: runtrol_ipc::WIRE_VERSION + 1,
            },
        )
        .await
        {
            Reply::One(Response::Failed(failure)) => {
                assert!(
                    failure
                        .message
                        .contains(&runtrol_ipc::WIRE_VERSION.to_string()),
                    "{}",
                    failure.message
                );
                assert!(
                    failure
                        .message
                        .contains(&(runtrol_ipc::WIRE_VERSION + 1).to_string()),
                    "{}",
                    failure.message
                );
            }
            other => panic!("expected a refusal, got {}", shape(&other)),
        }
        assert!(
            !conversation.greeted(),
            "a connection that did not agree must not count as greeted"
        );
        clean(composed, &path);
    }

    #[tokio::test]
    async fn a_command_for_a_session_that_is_not_live_is_refused_by_name() {
        let (composed, path) = composed_for("absent");
        let mut sessions = SessionManager::new();
        let mut conversation = Conversation::at_the_machine();
        greet(&mut conversation, &composed, &mut sessions).await;

        let absent = SessionId::now();
        for request in [
            Request::Prompt {
                session: absent,
                text: "anything".into(),
            },
            Request::Interrupt { session: absent },
            Request::Watch { session: absent },
            Request::Close {
                session: absent,
                now: true,
            },
        ] {
            match answer(&mut conversation, &composed, &mut sessions, request).await {
                Reply::One(Response::Failed(failure)) => {
                    assert!(
                        failure.message.contains(&absent.to_string()),
                        "the refusal has to name the session: {}",
                        failure.message
                    );
                }
                other => panic!("expected a refusal, got {}", shape(&other)),
            }
        }
        clean(composed, &path);
    }

    #[tokio::test]
    async fn a_provider_nobody_declared_is_refused_rather_than_started() {
        let (composed, path) = composed_for("noprovider");
        let mut sessions = SessionManager::new();
        let mut conversation = Conversation::at_the_machine();
        greet(&mut conversation, &composed, &mut sessions).await;

        match answer(
            &mut conversation,
            &composed,
            &mut sessions,
            Request::Start {
                provider: "nothing-declares-this".into(),
                workspace: std::env::temp_dir().to_string_lossy().into_owned().into(),
                model: None,
                permission: None,
            },
        )
        .await
        {
            Reply::One(Response::Failed(failure)) => {
                assert!(
                    failure.message.contains("nothing-declares-this"),
                    "{}",
                    failure.message
                );
            }
            other => panic!("expected a refusal, got {}", shape(&other)),
        }
        assert_eq!(sessions.hot(), 0, "nothing was started");
        clean(composed, &path);
    }

    #[tokio::test]
    async fn a_workspace_that_is_not_a_directory_is_refused_before_anything_starts() {
        // The one field on this path that names a place on the operator's disk. A start that accepted it would put an
        // agent somewhere nobody chose.
        let (composed, path) = composed_for("noworkspace");
        let mut sessions = SessionManager::new();
        let mut conversation = Conversation::at_the_machine();
        greet(&mut conversation, &composed, &mut sessions).await;

        let provider = composed
            .registry
            .usable()
            .next()
            .expect("a usable provider")
            .id()
            .as_str()
            .to_owned();

        match answer(
            &mut conversation,
            &composed,
            &mut sessions,
            Request::Start {
                provider: provider.into(),
                workspace: "this/is/not/a/real/place".into(),
                model: None,
                permission: None,
            },
        )
        .await
        {
            Reply::One(Response::Failed(_)) => {}
            other => panic!("expected a refusal, got {}", shape(&other)),
        }
        assert_eq!(sessions.hot(), 0);
        clean(composed, &path);
    }

    #[tokio::test]
    async fn listing_with_nothing_running_is_an_empty_list_and_not_a_failure() {
        let (composed, path) = composed_for("emptylist");
        let mut sessions = SessionManager::new();
        let mut conversation = Conversation::at_the_machine();
        greet(&mut conversation, &composed, &mut sessions).await;

        match answer(&mut conversation, &composed, &mut sessions, Request::List).await {
            Reply::One(Response::Sessions(listing)) => {
                assert!(listing.sessions.is_empty());
                assert!(listing.warnings.is_empty());
            }
            other => panic!("expected a listing, got {}", shape(&other)),
        }
        clean(composed, &path);
    }

    #[tokio::test]
    async fn a_stored_session_is_listed_cold_and_can_be_removed_without_a_process() {
        // A daemon restart begins with an empty live manager. The durable pointer must still appear, and closing that
        // cold row removes only runtrol's pointer rather than requiring a process that no longer exists.
        let (composed, path) = composed_for("storedlist");
        let mut sessions = SessionManager::new();
        let mut conversation = Conversation::at_the_machine();
        greet(&mut conversation, &composed, &mut sessions).await;

        let session = SessionId::now();
        let provider = ProviderId::parse("stored-provider").expect("valid provider id");
        let native = NativeSessionId::new("provider-session-1").expect("valid native id");
        let workspace = AbsPath::canonicalize(&path).expect("the scratch home exists");
        let now = WallMs::now();
        composed
            .store
            .put_session(
                session,
                &StoredSession {
                    provider,
                    native: native.clone(),
                    cwd: workspace.clone(),
                    label: None,
                    created_at: now,
                    last_seen_at: now,
                    pinned: false,
                    archived: false,
                    forked_from: None,
                    live: None,
                },
            )
            .expect("store the pointer");

        match answer(&mut conversation, &composed, &mut sessions, Request::List).await {
            Reply::One(Response::Sessions(listing)) => {
                assert!(listing.warnings.is_empty());
                let [line] = listing.sessions.as_slice() else {
                    panic!("expected one stored session, got {:?}", listing.sessions);
                };
                assert_eq!(line.session, session);
                assert_eq!(line.provider.as_ref(), provider.as_str());
                assert_eq!(line.native.as_deref(), Some(native.as_str()));
                assert_eq!(line.workspace.as_ref(), workspace.as_str());
                assert!(!line.hot);
                assert_eq!(line.doing.as_ref(), "detached");
            }
            other => panic!("expected a listing, got {}", shape(&other)),
        }

        assert!(matches!(
            answer(
                &mut conversation,
                &composed,
                &mut sessions,
                Request::Close {
                    session,
                    now: false
                },
            )
            .await,
            Reply::One(Response::Done)
        ));
        assert!(
            composed
                .store
                .get_session(session)
                .expect("the store remains readable")
                .is_none(),
            "closing a cold row removes its pointer"
        );
        clean(composed, &path);
    }

    #[tokio::test]
    async fn the_panic_button_consults_nothing_and_reports_what_happened() {
        // It has to work from anywhere with no permission at all. What it must not do is report a success it did not
        // achieve, and this daemon holds a containment that deliberately holds nothing.
        let (composed, path) = composed_for("panic");
        let mut sessions = SessionManager::new();
        let mut conversation = Conversation::at_the_machine();
        greet(&mut conversation, &composed, &mut sessions).await;

        match answer(
            &mut conversation,
            &composed,
            &mut sessions,
            Request::StopEverything,
        )
        .await
        {
            Reply::One(Response::Failed(failure)) => {
                assert!(
                    failure.message.contains("holds nothing"),
                    "a kill that did nothing must say so: {}",
                    failure.message
                );
            }
            other => panic!(
                "expected the refusal this containment gives, got {}",
                shape(&other)
            ),
        }
        clean(composed, &path);
    }

    /// Agree a wire format, so the rest of a test can ask for something.
    async fn greet(
        conversation: &mut Conversation,
        composed: &Composed,
        sessions: &mut SessionManager,
    ) {
        let reply = answer(
            conversation,
            composed,
            sessions,
            Request::Hello {
                wire: runtrol_ipc::WIRE_VERSION,
            },
        )
        .await;
        assert!(matches!(reply, Reply::One(Response::Welcome { .. })));
    }

    /// What a reply is, for a message that has to say what arrived instead.
    fn shape(reply: &Reply) -> String {
        match reply {
            Reply::One(response) => format!("{response:?}"),
            Reply::Watching(_) => "a subscription".to_owned(),
            Reply::Stopping { how, .. } => format!("a process still stopping, {how:?}"),
        }
    }
}
