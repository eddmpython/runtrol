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
use tauri::Manager as _;

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

/// Start a session.
#[tauri::command]
async fn start(app: tauri::AppHandle, provider: String, workspace: String) -> Answered<String> {
    let request = Request::Start {
        provider: provider.into(),
        workspace: workspace.into(),
        // Neither is decided here. The provider's own settings choose, which is the honest default: runtrol
        // has no opinion about which model somebody wants.
        model: None,
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
        .invoke_handler(tauri::generate_handler![
            sessions, providers, start, resume, close, tracing, trace
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
