//! Explicit room commands. The Runtime owns membership and speaker authority; the CLI only frames words.

use runtrol_courier::{Limits, ManagedSessionId, MessageId, RoomId};

use super::CourierFailure;
use super::words::{identifier, timeout, wrong};

const HELP: &str = "runtrol courier room open SESSION [SESSION] [--timeout SECONDS]\n\
runtrol courier room inspect ROOM\n\
runtrol courier room transfer ROOM SESSION\n\
runtrol courier room close ROOM\n\
runtrol courier room ask ROOM SESSION [--message-id MESSAGE] [--timeout SECONDS] < body\n\
Open includes you as owner and first speaker; name only your peers. Its timeout is the room lifetime.\n\
Only the owner transfers or closes. Only the selected speaker asks a participant.\n\
An ask waits for one exact reply; the receiver uses courier inbox/wait and courier reply MESSAGE.\n\
Ask timeout or closing its connection withdraws that round. An admitted round counts even when cancelled.\n\
The last reply stays readable. Close, participant exit, dialogue disable, or expiry retire room mail.\n\
Each round requires an explicit command. Rooms never start an agent or a repeated wait loop.";

pub(super) fn help() -> String {
    let limits = Limits::INITIAL;
    format!(
        "{HELP}\nRoom bounds: {} participants including you; {} admitted rounds; one in-flight ask; \
        {} rooms per Runtime; default lifetime/wait {} seconds, maximum {} seconds.",
        limits.room_participants,
        limits.room_rounds,
        limits.active_calls,
        limits.default_deadline_millis / 1000,
        limits.max_deadline_millis / 1000
    )
}

pub(super) enum RoomCommand {
    Open {
        peers: Vec<ManagedSessionId>,
        timeout_ms: u64,
    },
    Inspect {
        room: RoomId,
    },
    Transfer {
        room: RoomId,
        speaker: ManagedSessionId,
    },
    Close {
        room: RoomId,
    },
    Ask {
        room: RoomId,
        target: ManagedSessionId,
        message: MessageId,
        timeout_ms: u64,
    },
}

pub(super) fn parse(words: &[String]) -> Result<RoomCommand, CourierFailure> {
    let verb = words.first().map(String::as_str).ok_or_else(wrong)?;
    let mut arguments = words.iter().skip(1);
    let mut positional = Vec::new();
    let mut timeout_ms = None;
    let mut message = None;
    while let Some(word) = arguments.next() {
        match word.as_str() {
            "--timeout" if matches!(verb, "open" | "ask") && timeout_ms.is_none() => {
                timeout_ms = Some(timeout(
                    arguments.next().map(String::as_str).ok_or_else(wrong)?,
                )?);
            }
            "--message-id" if verb == "ask" && message.is_none() => {
                message = Some(identifier(arguments.next().map(String::as_str))?);
            }
            value
                if !value.starts_with('-')
                    && positional.len() < Limits::INITIAL.room_participants =>
            {
                positional.push(value);
            }
            _ => return Err(wrong()),
        }
    }
    let timeout_ms = timeout_ms.unwrap_or(Limits::INITIAL.default_deadline_millis);
    match (verb, positional.as_slice()) {
        ("open", peers) if !peers.is_empty() && peers.len() < Limits::INITIAL.room_participants => {
            let mut sessions = Vec::new();
            for peer in peers {
                let session = identifier(Some(peer))?;
                if sessions.contains(&session) {
                    return Err(wrong());
                }
                sessions.push(session);
            }
            Ok(RoomCommand::Open {
                peers: sessions,
                timeout_ms,
            })
        }
        ("inspect", [room]) => Ok(RoomCommand::Inspect {
            room: identifier(Some(room))?,
        }),
        ("close", [room]) => Ok(RoomCommand::Close {
            room: identifier(Some(room))?,
        }),
        ("transfer", [room, speaker]) => Ok(RoomCommand::Transfer {
            room: identifier(Some(room))?,
            speaker: identifier(Some(speaker))?,
        }),
        ("ask", [room, target]) => Ok(RoomCommand::Ask {
            room: identifier(Some(room))?,
            target: identifier(Some(target))?,
            message: message.unwrap_or_else(MessageId::now),
            timeout_ms,
        }),
        _ => Err(wrong()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn room_words_are_exact_and_body_free() {
        let room = RoomId::now().to_string();
        let peer = ManagedSessionId::now().to_string();
        let third = ManagedSessionId::now().to_string();
        for words in [
            vec!["open"],
            vec!["open", &peer, &peer],
            vec!["open", &peer, &third, &peer],
            vec!["ask", &room, &peer, "body"],
            vec!["ask", &room, &peer, "--source", &third],
            vec!["ask", &room, &peer, "--timeout", "0"],
            vec!["ask", &room, &peer, "--timeout", "601"],
            vec!["inspect", &room, "--timeout", "2"],
            vec!["transfer", &room],
            vec!["close", &room, &peer],
        ] {
            assert!(parse(&words.into_iter().map(str::to_owned).collect::<Vec<_>>()).is_err());
        }
        for words in [
            vec!["open", &peer, &third],
            vec!["inspect", &room],
            vec!["transfer", &room, &peer],
            vec!["close", &room],
            vec!["ask", &room, &peer, "--timeout", "2"],
        ] {
            assert!(parse(&words.into_iter().map(str::to_owned).collect::<Vec<_>>()).is_ok());
        }
    }

    #[test]
    fn guide_and_help_include_the_same_room_reference() {
        assert!(super::super::words::help().contains(&help()));
        assert!(super::super::words::guide().contains(&help()));
    }
}
