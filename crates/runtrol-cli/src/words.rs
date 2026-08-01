//! Turning what somebody typed into a request.
//!
//! Nothing here is guessed. Every command that needs something says what is missing rather than filling it in, because
//! a command that ran something other than what the operator asked for is worse than one that asks them to say it
//! again. The one exception is a directory, which means the one they are standing in, and that is not a guess: it is
//! the only reading of a start with no directory that anybody has ever meant.
//!
//! This touches nothing. It is a function of its arguments, which is why every rule above is checked here without a
//! daemon, a socket, or a session.

use runtrol_ipc::wire::Request;
use runtrol_provider::SessionId;

/// What somebody typed could not be understood.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Misunderstood {
    /// Nothing was typed.
    #[error("no command. try: list, models, start, resume, say, stop, watch, close, panic")]
    Nothing,

    /// The command is not one runtrol has.
    ///
    /// Names what was typed, because the operator's next move is to correct it and a message that does not repeat it
    /// makes them guess what runtrol thought they said.
    #[error(
        "no command called {typed:?}. try: list, models, start, resume, say, stop, watch, close, panic"
    )]
    NoSuchCommand {
        /// What they typed.
        typed: String,
    },

    /// The command needs something that was not given.
    #[error("{command} needs {missing}")]
    Missing {
        /// Which command.
        command: &'static str,
        /// What it needs.
        missing: &'static str,
    },

    /// A session was named in a way runtrol cannot read.
    #[error("{typed:?} is not a session name runtrol issued")]
    NotASession {
        /// What they typed.
        typed: String,
    },
}

/// Turn what somebody typed into a request.
///
/// `here` is the directory they are in, which is what a start means by "where the agent works" unless they said
/// otherwise. Taken as an argument rather than read from the process, so this stays a function of its inputs and can be
/// tested without a working directory.
///
/// # Errors
///
/// [`Misunderstood`] in every case where runtrol cannot tell what was meant. Nothing is guessed: a command that ran
/// something other than what the operator asked for is worse than one that asked them to say it again.
pub fn understand(words: &[String], here: &str) -> Result<Request, Misunderstood> {
    let Some(command) = words.first() else {
        return Err(Misunderstood::Nothing);
    };
    let rest = words.get(1..).unwrap_or_default();

    match command.as_str() {
        "list" => Ok(Request::List),

        "models" => Ok(Request::Models {
            provider: word(rest, 0, "models", "which provider")?.into(),
        }),

        "start" => Ok(Request::Start {
            provider: word(rest, 0, "start", "which provider")?.into(),
            workspace: word(rest, 1, "start", "a directory").unwrap_or(here).into(),
            model: rest.get(2).map(|one| one.as_str().into()),
            permission: None,
        }),

        "resume" => Ok(Request::Resume {
            provider: word(rest, 0, "resume", "which provider")?.into(),
            native: word(rest, 1, "resume", "which conversation to continue")?.into(),
            workspace: word(rest, 2, "resume", "a directory")
                .unwrap_or(here)
                .into(),
        }),

        "say" => {
            let session = session_of(rest, 0, "say")?;
            let text = rest.get(1..).unwrap_or_default().join(" ");
            if text.is_empty() {
                return Err(Misunderstood::Missing {
                    command: "say",
                    missing: "something to say",
                });
            }
            // Joined back as typed. Nothing is added and nothing is trimmed off the ends: what the operator wrote is
            // what the provider receives.
            Ok(Request::Prompt {
                session,
                text: text.into(),
            })
        }

        "stop" => Ok(Request::Interrupt {
            session: session_of(rest, 0, "stop")?,
        }),

        "watch" => Ok(Request::Watch {
            session: session_of(rest, 0, "watch")?,
        }),

        "close" => Ok(Request::Close {
            session: session_of(rest, 0, "close")?,
            now: rest.iter().any(|one| one == "--now"),
        }),

        "panic" => Ok(Request::StopEverything),

        typed => Err(Misunderstood::NoSuchCommand {
            typed: typed.to_owned(),
        }),
    }
}

/// One word of a command, or a message saying what is missing.
fn word<'words>(
    rest: &'words [String],
    at: usize,
    command: &'static str,
    missing: &'static str,
) -> Result<&'words str, Misunderstood> {
    rest.get(at)
        .map(String::as_str)
        .ok_or(Misunderstood::Missing { command, missing })
}

/// A session name, read strictly.
///
/// Reading it loosely would send a command to whichever session the guess happened to name.
fn session_of(
    rest: &[String],
    at: usize,
    command: &'static str,
) -> Result<SessionId, Misunderstood> {
    let typed = word(rest, at, command, "which session")?;
    typed.parse().map_err(|_| Misunderstood::NotASession {
        typed: typed.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use runtrol_ipc::wire::Request;
    use runtrol_provider::SessionId;

    use super::*;

    fn typed(line: &str) -> Vec<String> {
        line.split_whitespace().map(str::to_owned).collect()
    }

    fn here() -> &'static str {
        if cfg!(windows) { r"C:\work" } else { "/work" }
    }

    #[test]
    fn nothing_is_guessed_when_something_is_missing() {
        // A command that ran something other than what the operator asked for is worse than one that asks them to say it
        // again. Every one of these could have been guessed at, and none of them is.
        for line in [
            "",
            "models",
            "start",
            "resume claude",
            "say",
            "stop",
            "watch",
            "close",
        ] {
            let words = typed(line);
            assert!(
                understand(&words, here()).is_err(),
                "{line:?} was guessed at instead of refused"
            );
        }
    }

    #[test]
    fn a_command_runtrol_does_not_have_repeats_what_was_typed() {
        let words = typed("lst");
        match understand(&words, here()) {
            Err(Misunderstood::NoSuchCommand { typed }) => assert_eq!(typed, "lst"),
            other => panic!("expected a refusal naming what was typed, got {other:?}"),
        }
    }

    #[test]
    fn what_was_said_is_carried_word_for_word() {
        // The one place a prompt passes through this surface. Anything trimmed, joined differently, or added is a prompt
        // the operator did not write.
        let session = SessionId::now();
        let words = typed(&format!("say {session} write   the thing"));
        match understand(&words, here()).expect("understandable") {
            Request::Prompt {
                session: named,
                text,
            } => {
                assert_eq!(named, session);
                assert_eq!(&*text, "write the thing");
            }
            other => panic!("expected a prompt, got {other:?}"),
        }
    }

    #[test]
    fn a_session_named_in_a_way_runtrol_did_not_issue_is_refused() {
        let words = typed("say not-a-session-name hello");
        match understand(&words, here()) {
            Err(Misunderstood::NotASession { typed }) => assert_eq!(typed, "not-a-session-name"),
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    #[test]
    fn starting_without_a_directory_means_the_one_the_operator_is_in() {
        // The obvious meaning, and the alternative is making somebody type their own directory every time.
        let words = typed("start claude");
        match understand(&words, here()).expect("understandable") {
            Request::Start {
                provider,
                workspace,
                ..
            } => {
                assert_eq!(&*provider, "claude");
                assert_eq!(&*workspace, here());
            }
            other => panic!("expected a start, got {other:?}"),
        }
    }

    #[test]
    fn model_discovery_names_the_provider_and_nothing_else() {
        match understand(&typed("models example"), here()).expect("understandable") {
            Request::Models { provider } => assert_eq!(&*provider, "example"),
            other => panic!("expected model discovery, got {other:?}"),
        }
    }

    #[test]
    fn stopping_now_is_asked_for_explicitly() {
        // Ending a turn the operator is waiting on must not be the default reading of a word that also means "finish".
        let session = SessionId::now();
        match understand(&typed(&format!("close {session}")), here()).expect("understandable") {
            Request::Close { now, .. } => assert!(!now),
            other => panic!("expected a close, got {other:?}"),
        }
        match understand(&typed(&format!("close {session} --now")), here()).expect("understandable")
        {
            Request::Close { now, .. } => assert!(now),
            other => panic!("expected a close, got {other:?}"),
        }
    }

    #[test]
    fn the_panic_button_needs_nothing_typed_after_it() {
        // It has to work when somebody is in a hurry, and anything else to remember is something to get wrong.
        assert!(matches!(
            understand(&typed("panic"), here()).expect("understandable"),
            Request::StopEverything
        ));
    }
}
