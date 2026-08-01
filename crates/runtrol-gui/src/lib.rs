//! The desktop window: one list of every session, whichever CLI it belongs to.
//!
//! # What this crate is allowed to do
//!
//! Ask the daemon, and draw the answer. Its dependency list is the enforcement: it can see the vocabulary, the
//! wire, and the code that reaches a daemon, and it cannot see the kernel, storage, or a driver. So "the
//! window supervises nothing" is a fact the compiler holds rather than a promise in a comment.
//!
//! # Why the window is a personality of the one binary
//!
//! Installing runtrol installs one file. A separate desktop executable would be a second thing to ship, to
//! sign, and to keep in step with the daemon's wire version, and the first time those drifted the operator
//! would see a window that cannot talk to its own daemon.
//!
//! # The frontend here is deliberately small, and that is staged rather than owed
//!
//! [`docs/frontendStack.md`](../../../docs/frontendStack.md) settles the component layer: Astryx, shared by
//! the landing page, the phone surface, and this window. That layer arrives with the conversation surface,
//! because it is the conversation that needs a component library and a list of rows does not. What is here is
//! the seam underneath it, and that seam does not change when the components land: the same commands, the
//! same shapes, the same daemon conversation.

pub mod ask;
pub mod view;

use std::path::PathBuf;

use runtrol_ipc::wire::{Request, Response};
use tauri::{Emitter as _, Manager as _};

pub use ask::Failed;
pub use view::{Offered, Row};

/// What the window needs to reach its daemon, decided once at startup.
///
/// Both values are named by whoever starts the window rather than worked out here. Asking the operating system
/// what the running program is would make this behave differently depending on what linked it, and inside a
/// test that answer is the test runner.
#[derive(Clone, Debug)]
pub struct Reaching {
    /// Where a daemon for this home listens.
    pub address: String,
    /// The executable a daemon is started from, when none is listening.
    pub runtrol: PathBuf,
}

/// What a command hands back to the page.
///
/// A refusal is not an error here. The daemon answering "no" is an answer, it carries the daemon's own words,
/// and the window shows it and stays open. Collapsing it into a failure would put "the provider said no" and
/// "the connection broke" in the same red box.
// `rename_all` renames the variants and **not the fields inside them**, which is a distinction the page pays
// for: it reads `needsTheOperator`, that would have been `undefined` forever, and the one honest answer to a
// provider asking for authentication would have quietly stopped being shown. Caught by a test rather than by
// somebody failing to log in.
#[derive(Debug, serde::Serialize)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "outcome"
)]
pub enum Answered<T> {
    /// It worked.
    Ok {
        /// Whatever was asked for.
        value: T,
    },
    /// The daemon answered, and the answer was no.
    Refused {
        /// The daemon's own words.
        message: String,
        /// This needs the operator at the machine runtrol runs on.
        needs_the_operator: bool,
        /// Trying the same thing again could plausibly work.
        retryable: bool,
    },
    /// Nothing could be asked at all.
    Broken {
        /// What went wrong, in words a person reads.
        message: String,
    },
}

impl<T> Answered<T> {
    /// Wrap a failure that stopped the question from being asked.
    fn broken(failed: &Failed) -> Self {
        Self::Broken {
            message: failed.to_string(),
        }
    }

    /// Wrap a refusal the daemon sent back.
    fn refused(error: &runtrol_ipc::wire::WireError) -> Self {
        Self::Refused {
            message: error.message.to_string(),
            needs_the_operator: error.needs_the_operator,
            retryable: error.retryable,
        }
    }

    /// Wrap an answer from a daemon newer than this window.
    ///
    /// Shown rather than dropped. The operator asked for something and deserves to know an answer came back,
    /// even one this build cannot lay out.
    fn unreadable(answer: &Response) -> Self {
        Self::Broken {
            message: format!("the daemon answered something this window cannot read: {answer:?}"),
        }
    }
}

/// The variable that turns measurement on.
///
/// Off by default, so nothing is printed during ordinary use.
pub const TRACE_ENV: &str = "RUNTROL_GUI_TRACE";

/// Whether this window was started to be measured.
///
/// Asked once by the page at startup, so an ordinary run costs one call and then never reports again.
#[tauri::command]
fn tracing() -> bool {
    std::env::var_os(TRACE_ENV).is_some_and(|value| !value.is_empty())
}

/// One measurement from the page.
///
/// # Why the window can report on itself at all
///
/// Two of this initiative's completion criteria are numbers only the page can produce: how long after a click
/// something is on the screen, and whether frames hold up while output pours in. A harness outside the process
/// cannot see either. So the page measures and hands the numbers out through here, and a gate reads them off
/// the process's own output.
///
/// It is also how anything about this window is verifiable at all. A window that opened without complaining
/// says nothing about whether it drew what it was supposed to.
#[tauri::command]
#[expect(
    clippy::print_stdout,
    reason = "the measurement channel. it is what a gate reads, and it is silent unless asked for"
)]
#[expect(
    clippy::needless_pass_by_value,
    reason = "a command's arguments are deserialized off the wire, so they arrive owned"
)]
fn trace(line: String) {
    if tracing() {
        println!("{line}");
    }
}

/// What this window was told about reaching its daemon.
///
/// Taken by value from the handle rather than borrowed, because a command that holds a borrow across an await
/// is not `'static` and the toolkit then requires every one of them to be fallible. That would mean each
/// command carrying a second, always-unused failure channel beside [`Answered`], which already says everything
/// that can go wrong.
fn reaching(app: &tauri::AppHandle) -> Reaching {
    tauri::Manager::state::<Reaching>(app).inner().clone()
}

/// Every session on this machine, whichever CLI it belongs to.
///
/// The north star axis, as one call. Nothing here groups by provider: a provider is a badge on a row, not a
/// tab, because a list that splits by provider is the thing every vendor's own app already does.
#[tauri::command]
async fn sessions(app: tauri::AppHandle) -> Answered<Vec<Row>> {
    let reaching = reaching(&app);
    let asked = ask::once(&reaching.address, &reaching.runtrol, Request::List).await;
    match asked {
        Err(failed) => Answered::broken(&failed),
        Ok(Response::Sessions(lines)) => Answered::Ok {
            value: lines.iter().map(Row::from).collect(),
        },
        Ok(Response::Failed(error)) => Answered::refused(&error),
        Ok(other) => Answered::unreadable(&other),
    }
}

/// Which providers this build can drive.
///
/// Read from the greeting, which every connection already performs, so this costs no extra request. Providers
/// this build cannot serve are handed over marked rather than hidden: an operator with a manifest for a kind
/// nothing serves should see it, not wonder where their provider went.
#[tauri::command]
async fn providers(app: tauri::AppHandle) -> Answered<Vec<Offered>> {
    let reaching = reaching(&app);
    let greeted = ask::greet(&reaching.address, &reaching.runtrol).await;
    let mut connection = match greeted {
        Ok(connection) => connection,
        Err(failed) => return Answered::broken(&failed),
    };
    // The greeting's answer was consumed inside `greet`, so it is asked for again the only way the wire
    // offers: a second hello on the same connection.
    let asked = ask::exchange(
        &mut connection,
        &Request::Hello {
            wire: runtrol_ipc::WIRE_VERSION,
        },
    )
    .await;
    match asked {
        Err(failed) => Answered::broken(&failed),
        Ok(Response::Welcome { providers, .. }) => Answered::Ok {
            value: providers.iter().map(Offered::from).collect(),
        },
        Ok(Response::Failed(error)) => Answered::refused(&error),
        Ok(other) => Answered::unreadable(&other),
    }
}

/// Current model choices for one provider, discovered by its driver.
#[tauri::command]
async fn models(
    app: tauri::AppHandle,
    provider: String,
) -> Answered<runtrol_provider::ModelCatalog> {
    let reaching = reaching(&app);
    let asked = ask::once(
        &reaching.address,
        &reaching.runtrol,
        Request::Models {
            provider: provider.into(),
        },
    )
    .await;
    match asked {
        Err(failed) => Answered::broken(&failed),
        Ok(Response::Models(catalogue)) => Answered::Ok { value: catalogue },
        Ok(Response::Failed(error)) => Answered::refused(&error),
        Ok(other) => Answered::unreadable(&other),
    }
}

/// Start a session.
#[tauri::command]
async fn start(
    app: tauri::AppHandle,
    provider: String,
    workspace: String,
    model: Option<String>,
) -> Answered<String> {
    let request = Request::Start {
        provider: provider.into(),
        workspace: workspace.into(),
        // An absent choice means the provider's own setting. The window never invents a default.
        model: model.filter(|value| !value.is_empty()).map(Into::into),
        permission: None,
    };
    started(&reaching(&app), request).await
}

/// Continue a conversation the provider already has.
#[tauri::command]
async fn resume(
    app: tauri::AppHandle,
    provider: String,
    native: String,
    workspace: String,
) -> Answered<String> {
    let request = Request::Resume {
        provider: provider.into(),
        native: native.into(),
        workspace: workspace.into(),
    };
    started(&reaching(&app), request).await
}

/// Send what the operator wrote.
///
/// Relayed and never rewritten. There is nowhere in this path for runtrol to add a word of its own, and the
/// text reaches the provider as it was typed.
#[tauri::command]
async fn prompt(app: tauri::AppHandle, session: String, text: String) -> Answered<()> {
    let Some(session) = parse_session(&session) else {
        return Answered::Broken {
            message: "that is not a session identifier".to_owned(),
        };
    };
    let reaching = reaching(&app);
    let asked = ask::once(
        &reaching.address,
        &reaching.runtrol,
        Request::Prompt {
            session,
            text: text.into(),
        },
    )
    .await;
    match asked {
        Err(failed) => Answered::broken(&failed),
        Ok(Response::Done) => Answered::Ok { value: () },
        Ok(Response::Failed(error)) => Answered::refused(&error),
        Ok(other) => Answered::unreadable(&other),
    }
}

/// The name of the event the window pushes each frame out on.
///
/// One name for every session, with the session on the frame, rather than a name per session. The page decides
/// what it is looking at, and a stream of names would make that decision twice.
///
/// # No dot in the name
///
/// The toolkit refuses an event name containing one, and it refuses it by returning an error from the send
/// rather than by failing to start. Measured: every frame was relayed and every one was refused, while the
/// page sat there showing an empty conversation with nothing anywhere saying why.
pub const FRAME_EVENT: &str = "session-frame";

/// The name of the event that says a view has ended.
pub const OVER_EVENT: &str = "session-over";

/// One frame from a session, on its way to the page.
///
/// The provider's own bytes, untouched. runtrol does not lay a conversation out, reorder it, or summarize it,
/// so what the page receives is what the provider wrote, with only the session it belongs to attached.
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct Frame {
    /// Which session it belongs to, so a page that has moved on can ignore it.
    session: String,
    /// The provider's own frame, as text.
    frame: String,
}

/// Watch a session, replacing whatever was being watched.
///
/// # Why only one at a time
///
/// The window shows one conversation. A second subscription would hold a second connection and a second
/// provider's output for a panel nobody is looking at, which is the cost the memory contract exists to refuse.
/// Whoever wants two conversations at once opens two windows, and the daemon serves both the same way.
#[tauri::command]
async fn watch(app: tauri::AppHandle, session: String) -> Answered<()> {
    let Some(parsed) = parse_session(&session) else {
        return Answered::Broken {
            message: "that is not a session identifier".to_owned(),
        };
    };
    let reaching = reaching(&app);

    // Stopped before the next one starts, so there is never a moment with two views open.
    stop_watching(&app).await;

    let greeted = ask::greet(&reaching.address, &reaching.runtrol).await;
    let mut connection = match greeted {
        Ok(connection) => connection,
        Err(failed) => return Answered::broken(&failed),
    };
    // **Watching is not a question with an answer.** The daemon writes nothing when it accepts one: the
    // connection simply becomes a view, and the next thing on it is the session's first event. Asking for an
    // answer here would block until the agent happened to say something, which for an idle session is never,
    // and the window would sit there looking like it had failed to subscribe.
    let asked = serde_json::to_vec(&Request::Watch { session: parsed });
    let frame = match asked {
        Ok(frame) => frame,
        Err(error) => {
            return Answered::Broken {
                message: error.to_string(),
            };
        }
    };
    if let Err(error) = connection.send(&frame).await {
        return Answered::Broken {
            message: error.to_string(),
        };
    }

    let handle = app.clone();
    let watching = session.clone();
    let task = tauri::async_runtime::spawn(async move {
        loop {
            // Spelled as a match on purpose: the two arms are different facts. One is a frame, and the other
            // is the view being over, which the page has to be told about because a view that stops without
            // saying so looks exactly like a session that went quiet.
            #[expect(
                clippy::single_match_else,
                reason = "the ending arm is a decision, not a fallthrough"
            )]
            match connection.recv().await {
                Ok(Some(bytes)) => {
                    // The daemon puts each event in a `Response::Event` envelope. The page wants what the
                    // provider wrote, not runtrol's envelope around it, so the envelope is opened here and
                    // nothing inside it is read.
                    let text = match serde_json::from_slice::<Response>(&bytes) {
                        Ok(Response::Event(payload)) => payload.as_str().to_owned(),
                        // Anything else on a connection that has become a view is worth showing rather than
                        // dropping: it is the daemon saying something about the session.
                        Ok(other) => {
                            format!("{{\"body\":{{\"event\":\"daemon\",\"said\":{other:?}}}}}")
                        }
                        Err(_) => String::from_utf8_lossy(&bytes).into_owned(),
                    };
                    // Pushed as it arrived. A window that parsed this to decide what to send would be reading
                    // a conversation, which is the one thing runtrol does not do.
                    let sent = handle.emit(
                        FRAME_EVENT,
                        Frame {
                            session: watching.clone(),
                            frame: text,
                        },
                    );
                    // Whether a frame left this side, and whether the toolkit took it. The page reports what
                    // it received; without this, a frame that never arrived and a frame that arrived and was
                    // not drawn look identical from outside.
                    trace(format!(
                        "relayed a frame to the page: {}",
                        if sent.is_ok() { "taken" } else { "refused" }
                    ));
                }
                // The stream ended, or it broke. Either way the view is over and the page is told, because a
                // view that stops without saying so looks exactly like a session that went quiet.
                Ok(None) | Err(_) => {
                    drop(handle.emit(OVER_EVENT, watching.clone()));
                    return;
                }
            }
        }
    });

    *tauri::Manager::state::<Watching>(&app).0.lock().await = Some(task);
    Answered::Ok { value: () }
}

/// Stop watching, if anything was.
#[tauri::command]
async fn unwatch(app: tauri::AppHandle) {
    stop_watching(&app).await;
}

/// The task pushing frames at the page, when there is one.
#[derive(Default)]
struct Watching(tauri::async_runtime::Mutex<Option<tauri::async_runtime::JoinHandle<()>>>);

/// End the current view and let its connection go.
async fn stop_watching(app: &tauri::AppHandle) {
    let held = tauri::Manager::state::<Watching>(app).0.lock().await.take();
    if let Some(task) = held {
        // Aborted rather than asked to finish. The daemon notices the connection go and drops the subscription
        // on its side, which is the only thing that has to happen; waiting for a frame that may never come
        // would make switching sessions depend on the provider saying something.
        task.abort();
    }
}

/// Stop following a session. **Not a delete**: the conversation stays with its provider.
#[tauri::command]
async fn close(app: tauri::AppHandle, session: String, now: bool) -> Answered<()> {
    let Some(session) = parse_session(&session) else {
        return Answered::Broken {
            message: "that is not a session identifier".to_owned(),
        };
    };
    let reaching = reaching(&app);
    let asked = ask::once(
        &reaching.address,
        &reaching.runtrol,
        Request::Close { session, now },
    )
    .await;
    match asked {
        Err(failed) => Answered::broken(&failed),
        Ok(Response::Done) => Answered::Ok { value: () },
        Ok(Response::Failed(error)) => Answered::refused(&error),
        Ok(other) => Answered::unreadable(&other),
    }
}

/// The shared tail of starting and resuming: both answer with a session.
async fn started(reaching: &Reaching, request: Request) -> Answered<String> {
    match ask::once(&reaching.address, &reaching.runtrol, request).await {
        Err(failed) => Answered::broken(&failed),
        Ok(Response::Started { session }) => Answered::Ok {
            value: session.to_string(),
        },
        Ok(Response::Failed(error)) => Answered::refused(&error),
        Ok(other) => Answered::unreadable(&other),
    }
}

/// A session identifier the page sent back, when it is one.
///
/// The page only ever returns a value this crate gave it, so a value that does not parse means something is
/// wrong rather than that the operator typed badly. Answered as a failure rather than trusted.
fn parse_session(text: &str) -> Option<runtrol_provider::SessionId> {
    let Ok(session) = text.parse::<runtrol_provider::SessionId>() else {
        return None;
    };
    Some(session)
}

/// Open the window and serve it until it closes.
///
/// # Errors
///
/// [`tauri::Error`] when the window cannot be created or the runtime cannot start.
pub fn run(reaching: Reaching) -> Result<(), tauri::Error> {
    tauri::Builder::default()
        .setup(|app| {
            // Asked for by the window itself. A process raising its own new window is allowed to; a harness
            // handing it the foreground from outside is not, which the shell campaign measured the hard way.
            if let Some(window) = app.get_webview_window("main") {
                drop(window.set_focus());
            }
            Ok(())
        })
        .manage(reaching)
        .manage(Watching::default())
        .invoke_handler(tauri::generate_handler![
            sessions, providers, models, start, resume, close, prompt, watch, unwatch, tracing,
            trace
        ])
        .run(tauri::generate_context!())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refusal_and_a_breakage_are_different_answers() {
        // A window shows the daemon's own words for one and its own words for the other. Collapsing them puts
        // "the provider said no" and "the connection broke" in the same red box, and the operator cannot tell
        // whether to look at their machine or at their request.
        let refused: Answered<()> = Answered::refused(&runtrol_ipc::wire::WireError {
            message: "no rollout found for that conversation".into(),
            retryable: false,
            needs_the_operator: false,
        });
        let encoded = serde_json::to_string(&refused).expect("serializable");
        assert!(encoded.contains(r#""outcome":"refused""#), "{encoded}");
        assert!(encoded.contains("no rollout"), "{encoded}");

        let broken: Answered<()> = Answered::broken(&Failed::NoAnswer);
        let encoded = serde_json::to_string(&broken).expect("serializable");
        assert!(encoded.contains(r#""outcome":"broken""#), "{encoded}");
    }

    #[test]
    fn a_refusal_carries_where_the_operator_has_to_be() {
        // The one honest answer a surface can give about authentication: go to the machine runtrol runs on.
        // runtrol holds no credential and no window can supply one.
        let refused: Answered<()> = Answered::refused(&runtrol_ipc::wire::WireError {
            message: "the provider wants you to authenticate".into(),
            retryable: false,
            needs_the_operator: true,
        });
        let encoded = serde_json::to_string(&refused).expect("serializable");
        assert!(encoded.contains(r#""needsTheOperator":true"#), "{encoded}");
    }

    #[test]
    fn a_value_the_page_sends_back_is_parsed_rather_than_trusted() {
        assert!(parse_session("not-a-session").is_none());
        let minted = runtrol_provider::SessionId::now();
        assert_eq!(parse_session(&minted.to_string()), Some(minted));
    }
}
