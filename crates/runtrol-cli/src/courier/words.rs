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

pub(super) enum Command {
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

fn wrong() -> CourierFailure {
    CourierFailure::Arguments(format!("invalid courier arguments\n{HELP}"))
}

fn identifier<T: std::str::FromStr>(text: Option<&str>) -> Result<T, CourierFailure> {
    text.ok_or_else(wrong)?.parse().map_err(|_invalid| wrong())
}

pub(super) fn parse(words: &[String]) -> Result<Command, CourierFailure> {
    let Some(verb) = words.first().map(String::as_str) else {
        return Err(wrong());
    };
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
                let seconds: u64 = value.parse().map_err(|_invalid| wrong())?;
                let millis = seconds.checked_mul(1000).ok_or_else(wrong)?;
                if millis == 0 || millis > Limits::INITIAL.max_deadline_millis {
                    return Err(wrong());
                }
                timeout_ms = Some(millis);
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
