//! Strict command words; message bodies never appear among them.

use runtrol_courier::{CallId, Limits, ManagedSessionId, MessageId};

use super::CourierFailure;

pub(super) const HELP: &str = "runtrol courier list [--after SESSION]\n\
runtrol courier tell SESSION [--message-id MESSAGE] [--timeout SECONDS] < body\n\
runtrol courier ask SESSION [--message-id MESSAGE] [--timeout SECONDS] < body\n\
runtrol courier reply MESSAGE [--message-id MESSAGE] < body\n\
runtrol courier inbox [--from SESSION]\n\
runtrol courier wait [--from SESSION] [--timeout SECONDS]\n\
runtrol courier cancel CALL\n\
Bodies are opaque UTF-8 read from stdin. Each inbox or wait consumes at most one envelope.\n\
A receipt confirms admission, not model understanding. An idle agent must explicitly wait to receive.";

pub(super) fn help() -> String {
    let limits = Limits::INITIAL;
    format!(
        "{HELP}\n{}\n{}\nLimits: body {} UTF-8 bytes; mailbox {} envelopes / {} body bytes; \
        Runtime {} body bytes / {} active calls; default deadline {} seconds, maximum {} seconds; \
        forwarding {} hops / {} visited sessions.",
        super::rooms::help(),
        super::spawn::HELP,
        limits.body_bytes,
        limits.mailbox_envelopes,
        limits.mailbox_bytes,
        limits.runtime_bytes,
        limits.active_calls,
        limits.default_deadline_millis / 1000,
        limits.max_deadline_millis / 1000,
        limits.hop_count,
        limits.visited_sessions
    )
}

pub(super) fn guide() -> String {
    guide_with(
        "For this activation message, acknowledge readiness without using tools. Wait for my next explicit task before running a courier command.",
    )
}

pub(super) fn initial_guide(source: ManagedSessionId, message: MessageId) -> String {
    guide_with(&format!(
        "For this activation, consume exactly one initial envelope with courier inbox --from {source}. \
        Its message_id must be {message}. Carry out that delegated task in your current worktree. \
        If the exact envelope is unavailable, report that fact instead of guessing a task."
    ))
}

fn guide_with(instruction: &str) -> String {
    use runtrol_courier::env::{COURIER_EXE_ENV, COURIER_TOKEN_ENV, MANAGED_SESSION_ENV};
    format!(
        "I am enabling managed-session dialogue for this live process. {instruction} \
        For that task, use your normal shell tool \
        to run the executable from the {COURIER_EXE_ENV} environment value with the arguments shown below. \
        Your own session identity is in {MANAGED_SESSION_ENV}. Never print or copy {COURIER_TOKEN_ENV}. \
        Use list to find an enabled peer and its exact identity. Bodies go through UTF-8 stdin, never arguments. \
        Reply to the received message_id. A wait is bounded and does not wake an idle model. \
        Follow my task; do not start a repeated wait loop without a bounded task. \
        Use courier --help for the current command reference.\n{}",
        help()
    )
}

pub(super) enum Command {
    Spawn(super::spawn::SpawnCommand),
    Room(super::rooms::RoomCommand),
    List {
        after: Option<ManagedSessionId>,
    },
    Send {
        target: ManagedSessionId,
        ask: bool,
        message: MessageId,
        timeout_ms: u64,
    },
    Reply {
        message: MessageId,
        outgoing: MessageId,
    },
    Receive {
        source: Option<ManagedSessionId>,
        timeout_ms: u64,
    },
    Cancel {
        call: CallId,
    },
}

pub(super) fn wrong() -> CourierFailure {
    CourierFailure::Arguments(format!("invalid courier arguments\n{}", help()))
}

pub(super) fn identifier<T: std::str::FromStr>(text: Option<&str>) -> Result<T, CourierFailure> {
    text.ok_or_else(wrong)?.parse().map_err(|_invalid| wrong())
}

pub(super) fn timeout(value: &str) -> Result<u64, CourierFailure> {
    let seconds: u64 = value.parse().map_err(|_invalid| wrong())?;
    let millis = seconds.checked_mul(1000).ok_or_else(wrong)?;
    if millis == 0 || millis > Limits::INITIAL.max_deadline_millis {
        return Err(wrong());
    }
    Ok(millis)
}

pub(super) fn parse(words: &[String]) -> Result<Command, CourierFailure> {
    let Some((verb, arguments)) = words.split_first() else {
        return Err(wrong());
    };
    let verb = verb.as_str();
    if verb == "spawn" {
        return super::spawn::parse(arguments).map(Command::Spawn);
    }
    if verb == "room" {
        return super::rooms::parse(arguments).map(Command::Room);
    }
    let needs_target = matches!(verb, "tell" | "ask" | "reply" | "cancel");
    let target = needs_target
        .then(|| words.get(1).map(String::as_str))
        .flatten();
    let mut source = None;
    let mut after = None;
    let mut message = None;
    let mut timeout_ms = None;
    let mut options = words.iter().skip(if needs_target { 2 } else { 1 });
    while let Some(option) = options.next() {
        let value = options.next().map(String::as_str).ok_or_else(wrong)?;
        match option.as_str() {
            "--from" if matches!(verb, "inbox" | "wait") && source.is_none() => {
                source = Some(identifier(Some(value))?);
            }
            "--after" if verb == "list" && after.is_none() => {
                after = Some(identifier(Some(value))?);
            }
            "--message-id" if matches!(verb, "tell" | "ask" | "reply") && message.is_none() => {
                message = Some(identifier(Some(value))?);
            }
            "--timeout" if matches!(verb, "tell" | "ask" | "wait") && timeout_ms.is_none() => {
                timeout_ms = Some(timeout(value)?);
            }
            _ => return Err(wrong()),
        }
    }
    let timeout_ms = timeout_ms.unwrap_or(Limits::INITIAL.default_deadline_millis);
    match verb {
        "list" => Ok(Command::List { after }),
        "tell" | "ask" => Ok(Command::Send {
            target: identifier(target)?,
            ask: verb == "ask",
            message: message.unwrap_or_else(MessageId::now),
            timeout_ms,
        }),
        "reply" => Ok(Command::Reply {
            message: identifier(target)?,
            outgoing: message.unwrap_or_else(MessageId::now),
        }),
        "inbox" | "wait" => Ok(Command::Receive {
            source,
            timeout_ms: if verb == "inbox" { 0 } else { timeout_ms },
        }),
        "cancel" => Ok(Command::Cancel {
            call: identifier(target)?,
        }),
        _ => Err(wrong()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_body_arguments_unknown_flags_duplicates_and_unbounded_waits() {
        let id = ManagedSessionId::now().to_string();
        for args in [
            vec!["tell", &id, "secret body"],
            vec!["list", "--timeout", "2"],
            vec!["wait", "--timeout", "601"],
            vec!["wait", "--timeout", "0"],
            vec!["wait", "--timeout", "1", "--timeout", "2"],
            vec!["reply"],
            vec!["cancel", "garbage"],
        ] {
            assert!(parse(&args.into_iter().map(str::to_owned).collect::<Vec<_>>()).is_err());
        }
        assert!(matches!(
            parse(&["inbox".into()]).unwrap(),
            Command::Receive { timeout_ms: 0, .. }
        ));
    }
}
