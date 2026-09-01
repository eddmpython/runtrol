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
use runtrol_provider::{ApprovalId, OptionId, SessionId, StreamId, WatchCursor, WorkspaceAccess};

/// What somebody typed could not be understood.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Misunderstood {
    /// Nothing was typed.
    #[error(
        "no command. try: list, models, start, resume, say, answer, stop, watch, close, consult, panic"
    )]
    Nothing,

    /// The command is not one runtrol has.
    ///
    /// Names what was typed, because the operator's next move is to correct it and a message that does not repeat it
    /// makes them guess what runtrol thought they said.
    #[error(
        "no command called {typed:?}. try: list, models, start, resume, say, answer, stop, watch, close, consult, panic"
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

    /// A watch cursor was not the exact stream, epoch, and next sequence boundary.
    #[error("{typed:?} is not a watch cursor (expected STREAM:EPOCH:SEQ)")]
    NotAWatchCursor {
        /// What they typed.
        typed: String,
    },

    /// An approval was named in a way runtrol cannot read.
    #[error("{typed:?} is not an approval name runtrol issued")]
    NotAnApproval {
        /// What was typed.
        typed: String,
    },

    /// An approval option was not a non-negative 32-bit integer.
    #[error("{typed:?} is not an approval option number")]
    NotAnOption {
        /// What was typed.
        typed: String,
    },

    /// A subject digest was not the exact 32-byte value shown with the approval.
    #[error("{typed:?} is not a 64-character hexadecimal subject digest")]
    NotASubjectDigest {
        /// What was typed.
        typed: String,
    },

    /// A command received a word beyond its complete shape.
    #[error("{command} does not accept extra word {typed:?}")]
    Extra {
        /// Which command was complete before this word.
        command: &'static str,
        /// The first extra word.
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
            workspace_access: WorkspaceAccess::Exclusive,
            model: rest.get(2).map(|one| one.as_str().into()),
            permission: None,
        }),

        "resume" => Ok(Request::Resume {
            provider: word(rest, 0, "resume", "which provider")?.into(),
            native: word(rest, 1, "resume", "which conversation to continue")?.into(),
            workspace: word(rest, 2, "resume", "a directory")
                .unwrap_or(here)
                .into(),
            workspace_access: WorkspaceAccess::Exclusive,
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

        "answer" => {
            if let Some(typed) = rest.get(4) {
                return Err(Misunderstood::Extra {
                    command: "answer",
                    typed: typed.clone(),
                });
            }
            Ok(Request::AnswerApproval {
                session: session_of(rest, 0, "answer")?,
                approval: approval_of(rest, 1)?,
                option: option_of(rest, 2)?,
                subject_digest: subject_digest_of(rest, 3)?,
            })
        }

        "stop" => Ok(Request::Interrupt {
            session: session_of(rest, 0, "stop")?,
        }),

        "watch" => {
            if let Some(typed) = rest.get(3) {
                return Err(Misunderstood::Extra {
                    command: "watch",
                    typed: typed.clone(),
                });
            }
            let after = match rest.get(1) {
                None => None,
                Some(flag) if flag == "--after" => Some(watch_cursor_of(rest, 2)?),
                Some(typed) => {
                    return Err(Misunderstood::Extra {
                        command: "watch",
                        typed: typed.clone(),
                    });
                }
            };
            Ok(Request::Watch {
                session: session_of(rest, 0, "watch")?,
                after,
            })
        }

        "close" => Ok(Request::Close {
            session: session_of(rest, 0, "close")?,
            now: rest.iter().any(|one| one == "--now"),
        }),

        "panic" => Ok(Request::StopEverything),

        "consult" => consult_of(rest),

        typed => Err(Misunderstood::NoSuchCommand {
            typed: typed.to_owned(),
        }),
    }
}

/// The consult command: bare for status, `unwire` with both ends named to remove one direction.
///
/// There is no `wire`. Registering one CLI inside another is a retired surface, and a word that still did it
/// would be the one registration path left in the product.
fn consult_of(rest: &[String]) -> Result<Request, Misunderstood> {
    match rest.first().map(String::as_str) {
        // Bare, it asks where every direction stands. The state lives in the CLIs' own configuration, so
        // there is nothing to name.
        None => Ok(Request::Consult),
        Some("unwire") => {
            if let Some(typed) = rest.get(3) {
                return Err(Misunderstood::Extra {
                    command: "consult",
                    typed: typed.clone(),
                });
            }
            let from = word(rest, 1, "consult", "which provider holds the registration")?.into();
            let to = word(rest, 2, "consult", "which provider is unregistered")?.into();
            Ok(Request::ConsultUnwire { from, to })
        }
        Some(typed) => Err(Misunderstood::Extra {
            command: "consult",
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

fn approval_of(rest: &[String], at: usize) -> Result<ApprovalId, Misunderstood> {
    let typed = word(rest, at, "answer", "which approval")?;
    typed.parse().map_err(|_| Misunderstood::NotAnApproval {
        typed: typed.to_owned(),
    })
}

fn watch_cursor_of(rest: &[String], at: usize) -> Result<WatchCursor, Misunderstood> {
    let typed = word(rest, at, "watch", "a cursor after --after")?;
    let mut parts = typed.split(':');
    let parsed = match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some(stream), Some(epoch), Some(seq), None) => {
            let stream = stream.parse::<StreamId>();
            let epoch = epoch.parse::<u32>();
            let seq = seq.parse::<u64>();
            match (stream, epoch, seq) {
                (Ok(stream), Ok(epoch), Ok(seq)) => Some(WatchCursor { stream, epoch, seq }),
                _ => None,
            }
        }
        _ => None,
    };
    parsed.ok_or_else(|| Misunderstood::NotAWatchCursor {
        typed: typed.to_owned(),
    })
}

fn option_of(rest: &[String], at: usize) -> Result<OptionId, Misunderstood> {
    let typed = word(rest, at, "answer", "which option")?;
    typed
        .parse::<u32>()
        .map(OptionId)
        .map_err(|_| Misunderstood::NotAnOption {
            typed: typed.to_owned(),
        })
}

fn subject_digest_of(rest: &[String], at: usize) -> Result<[u8; 32], Misunderstood> {
    let typed = word(rest, at, "answer", "the subject digest")?;
    if typed.len() != 64 {
        return Err(Misunderstood::NotASubjectDigest {
            typed: typed.to_owned(),
        });
    }
    let mut digest = [0_u8; 32];
    for (index, pair) in typed.as_bytes().chunks_exact(2).enumerate() {
        let pair = str::from_utf8(pair).map_err(|_| Misunderstood::NotASubjectDigest {
            typed: typed.to_owned(),
        })?;
        let slot = digest
            .get_mut(index)
            .ok_or_else(|| Misunderstood::NotASubjectDigest {
                typed: typed.to_owned(),
            })?;
        *slot = u8::from_str_radix(pair, 16).map_err(|_| Misunderstood::NotASubjectDigest {
            typed: typed.to_owned(),
        })?;
    }
    Ok(digest)
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
            "answer",
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
    fn an_approval_answer_carries_every_boundary_value_exactly() {
        let session = SessionId::now();
        let approval = ApprovalId::now();
        let digest = [0xab_u8; 32];
        let line = format!("answer {session} {approval} 7 {}", "ab".repeat(32));
        match understand(&typed(&line), here()).expect("understandable") {
            Request::AnswerApproval {
                session: named_session,
                approval: named_approval,
                option,
                subject_digest,
            } => {
                assert_eq!(named_session, session);
                assert_eq!(named_approval, approval);
                assert_eq!(option, OptionId(7));
                assert_eq!(subject_digest, digest);
            }
            other => panic!("expected an approval answer, got {other:?}"),
        }
    }

    #[test]
    fn malformed_approval_answers_are_refused_without_guessing() {
        let session = SessionId::now();
        let approval = ApprovalId::now();
        for line in [
            format!("answer {session}"),
            format!("answer {session} not-an-approval 0 {}", "00".repeat(32)),
            format!("answer {session} {approval} no {}", "00".repeat(32)),
            format!("answer {session} {approval} 0 short"),
            format!("answer {session} {approval} 0 {}", "zz".repeat(32)),
            format!("answer {session} {approval} 0 {} extra", "00".repeat(32)),
        ] {
            assert!(
                understand(&typed(&line), here()).is_err(),
                "accepted {line:?}"
            );
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
    fn watch_carries_an_exact_optional_next_expected_cursor() {
        let session = SessionId::now();
        let stream = StreamId::now();
        let line = format!("watch {session} --after {stream}:4:19");
        match understand(&typed(&line), here()).expect("understandable") {
            Request::Watch {
                session: named,
                after: Some(after),
            } => {
                assert_eq!(named, session);
                assert_eq!(after.stream, stream);
                assert_eq!(after.epoch, 4);
                assert_eq!(after.seq, 19);
            }
            other => panic!("expected a cursor watch, got {other:?}"),
        }

        match understand(&typed(&format!("watch {session}")), here()).expect("understandable") {
            Request::Watch { after: None, .. } => {}
            other => panic!("expected an initial watch, got {other:?}"),
        }
    }

    #[test]
    fn malformed_or_ambiguous_watch_cursors_are_refused() {
        let session = SessionId::now();
        for line in [
            format!("watch {session} --after"),
            format!("watch {session} --after not-a-cursor"),
            format!("watch {session} --after {}:epoch:1", StreamId::now()),
            format!("watch {session} cursor-without-flag"),
            format!("watch {session} --after {}:0:1 extra", StreamId::now()),
        ] {
            assert!(
                understand(&typed(&line), here()).is_err(),
                "accepted {line:?}"
            );
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

    #[test]
    fn consult_asks_for_status_bare_and_names_both_ends_to_unwire() {
        assert!(matches!(
            understand(&typed("consult"), here()).expect("understandable"),
            Request::Consult
        ));
        match understand(&typed("consult unwire claude codex"), here()).expect("understandable") {
            Request::ConsultUnwire { from, to } => {
                assert_eq!(&*from, "claude");
                assert_eq!(&*to, "codex");
            }
            other => panic!("expected an unwire, got {other:?}"),
        }
    }

    #[test]
    fn a_consult_wire_or_an_unwire_missing_an_end_or_carrying_extras_is_refused() {
        // Unwiring edits another program's configuration. A guessed direction would edit one nobody named, and
        // `wire` is not a word at all: nothing in the product registers one CLI inside another any more.
        for line in [
            "consult wire",
            "consult wire claude codex",
            "consult unwire",
            "consult unwire claude",
            "consult unwire claude codex extra",
            "consult status",
        ] {
            assert!(
                understand(&typed(line), here()).is_err(),
                "accepted {line:?}"
            );
        }
    }
}
