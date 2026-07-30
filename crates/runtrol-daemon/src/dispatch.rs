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
//! it came from. The wall itself lives in the security crate; this is where it is consulted.

use std::sync::Arc;

use runtrol_core::registry::KindStatus;
use runtrol_core::session::SessionError;
use runtrol_core::{SessionManager, Subscription};
use runtrol_drivers::DriverContext;
use runtrol_ipc::wire::{ProviderLine, Request, Response, SessionLine, WireError};
use runtrol_provider::{
    AbsPath, AgentCommand, CloseMode, ContentBlock, Disposition, OpenIntent, ProviderId, SessionId,
};

use crate::compose::Composed;

/// What a request produced.
pub enum Reply {
    /// One answer, and the connection is free for the next request.
    One(Response),
    /// The caller is now watching a session.
    ///
    /// A separate shape because watching is not a question with an answer: it is the connection changing what it is for,
    /// and a dispatcher that pretended otherwise would have to answer once and then keep writing.
    Watching(Box<Subscription>),
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
/// Small on purpose: what a connection knows is whether it has greeted, and nothing else. A connection that remembered
/// which session the caller "meant" would be the second place that notion lives.
#[derive(Debug, Default)]
pub struct Conversation {
    /// The wire format has been agreed.
    greeted: bool,
}

impl Conversation {
    /// A connection that has said nothing yet.
    #[must_use]
    pub const fn new() -> Self {
        Self { greeted: false }
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

        Request::List => Reply::One(Response::Sessions(list(sessions))),

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
            match sessions.close(session) {
                Ok(agent) => Reply::Stopping { agent, how },
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
    let Ok(id) = ProviderId::parse(provider) else {
        return refuse(&format!(
            "{provider:?} is not a provider name runtrol accepts"
        ));
    };
    let Some(declared) = composed.registry.get(id) else {
        return refuse(&format!("no provider called {provider}"));
    };
    match declared.kind {
        KindStatus::Available => {}
        KindStatus::Unavailable { why } => return refuse(why),
        KindStatus::Unknown => {
            return refuse(&format!(
                "{provider} names a kind nothing in this build declares"
            ));
        }
    }

    let Some(entry) = composed.driver_for(declared.manifest.kind.as_str()) else {
        return refuse("this build has no driver for that kind");
    };
    let Some(make) = entry.make else {
        return refuse(
            entry
                .unavailable
                .unwrap_or("this build cannot serve that kind"),
        );
    };

    // Resolution belongs to the probe, so the driver runs the same program the probe examined.
    let program = match runtrol_core::locate(&declared.manifest) {
        Ok(program) => program,
        Err(error) => return refuse(&error.to_string()),
    };
    let Ok(workspace) = AbsPath::canonicalize(workspace) else {
        return refuse(&format!(
            "{workspace:?} is not a directory runtrol can work in"
        ));
    };

    let driver = make(&DriverContext {
        provider: id,
        program,
        contained_by: Arc::clone(&composed.containment),
    });
    let intent = OpenIntent {
        session: SessionId::now(),
        workspace,
        disposition,
        model,
        permission,
    };

    match sessions.start(driver.as_ref(), intent).await {
        Ok(session) => Response::Started { session },
        Err(error) => from_session_error(&error),
    }
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
fn list(sessions: &SessionManager) -> Vec<SessionLine> {
    sessions
        .live_sessions()
        .map(|one| SessionLine {
            session: one.session,
            provider: one.provider.as_str().into(),
            native: one.native.map(Into::into),
            hot: one.tier.has_a_process(),
            doing: one.state.lifecycle().name().into(),
            looks_stuck: one.state.looks_stuck(),
        })
        .collect()
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

    fn clean(path: &str) {
        drop(std::fs::remove_dir_all(path));
    }

    #[tokio::test]
    async fn nothing_can_be_asked_before_the_wire_format_is_agreed() {
        // Acting on a request from a build that speaks a different format means acting on somebody else's meaning, and
        // the failure that produces is a command landing where the operator did not intend.
        let (composed, path) = composed_for("ungreeted");
        let mut sessions = SessionManager::new();
        let mut conversation = Conversation::new();
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
        clean(&path);
    }

    #[tokio::test]
    async fn the_greeting_answers_with_every_provider_this_build_knows() {
        let (composed, path) = composed_for("greeting");
        let mut sessions = SessionManager::new();
        let mut conversation = Conversation::new();

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
        clean(&path);
    }

    #[tokio::test]
    async fn a_caller_speaking_another_wire_format_is_told_both_numbers() {
        let (composed, path) = composed_for("mismatch");
        let mut sessions = SessionManager::new();
        let mut conversation = Conversation::new();

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
        clean(&path);
    }

    #[tokio::test]
    async fn a_command_for_a_session_that_is_not_live_is_refused_by_name() {
        let (composed, path) = composed_for("absent");
        let mut sessions = SessionManager::new();
        let mut conversation = Conversation::new();
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
        clean(&path);
    }

    #[tokio::test]
    async fn a_provider_nobody_declared_is_refused_rather_than_started() {
        let (composed, path) = composed_for("noprovider");
        let mut sessions = SessionManager::new();
        let mut conversation = Conversation::new();
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
        clean(&path);
    }

    #[tokio::test]
    async fn a_workspace_that_is_not_a_directory_is_refused_before_anything_starts() {
        // The one field on this path that names a place on the operator's disk. A start that accepted it would put an
        // agent somewhere nobody chose.
        let (composed, path) = composed_for("noworkspace");
        let mut sessions = SessionManager::new();
        let mut conversation = Conversation::new();
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
        clean(&path);
    }

    #[tokio::test]
    async fn listing_with_nothing_running_is_an_empty_list_and_not_a_failure() {
        let (composed, path) = composed_for("emptylist");
        let mut sessions = SessionManager::new();
        let mut conversation = Conversation::new();
        greet(&mut conversation, &composed, &mut sessions).await;

        match answer(&mut conversation, &composed, &mut sessions, Request::List).await {
            Reply::One(Response::Sessions(lines)) => assert!(lines.is_empty()),
            other => panic!("expected a listing, got {}", shape(&other)),
        }
        clean(&path);
    }

    #[tokio::test]
    async fn the_panic_button_consults_nothing_and_reports_what_happened() {
        // It has to work from anywhere with no permission at all. What it must not do is report a success it did not
        // achieve, and this daemon holds a containment that deliberately holds nothing.
        let (composed, path) = composed_for("panic");
        let mut sessions = SessionManager::new();
        let mut conversation = Conversation::new();
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
        clean(&path);
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
