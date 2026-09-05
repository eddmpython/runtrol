//! One command per authenticated connection. Only this command's stdout receives its body.

use std::ffi::OsString;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use runtrol_courier::env::{COURIER_ENDPOINT_ENV, COURIER_TOKEN_ENV, MANAGED_SESSION_ENV};
use runtrol_courier::wire::{Answer, Hello, HelloAnswer, Invocation, MAX_FRAME_BYTES, Request};
use runtrol_courier::{BoundedUtf8, CallEnvelope, Limits, ManagedSessionId, MessageId, UnixMillis};
use tokio::io::AsyncReadExt as _;

use super::words::{Command, guide, help, parse};
use super::{Admission, CourierFailure, birth_value, courier};

const EXCHANGE_MARGIN: Duration = Duration::from_secs(10);

/// The complete stdout of one invocation and its process outcome.
pub struct CommandOutput {
    /// A JSON answer, help text, or the legacy admission word.
    pub stdout: String,
    /// False for a refusal or an unanswered bounded wait.
    pub success: bool,
}

struct Birth {
    endpoint: String,
    hello: Hello,
}

impl Birth {
    fn read() -> Result<Self, CourierFailure> {
        let session_text = birth_value(MANAGED_SESSION_ENV)?;
        let session: ManagedSessionId = session_text
            .parse()
            .map_err(|_invalid| CourierFailure::Session(session_text))?;
        Ok(Self {
            endpoint: birth_value(COURIER_ENDPOINT_ENV)?,
            hello: Hello::new(session, birth_value(COURIER_TOKEN_ENV)?),
        })
    }

    async fn exchange(
        &self,
        request: Request,
        timeout: Duration,
    ) -> Result<Answer, CourierFailure> {
        tokio::time::timeout(timeout + EXCHANGE_MARGIN, async {
            let mut connection = runtrol_ipc::connect(&self.endpoint).await?;
            let invocation = Invocation {
                hello: self.hello.clone(),
                request: Some(request),
            };
            let bytes = serde_json::to_vec(&invocation)
                .map_err(|_invalid| CourierFailure::Unintelligible)?;
            if bytes.len() > MAX_FRAME_BYTES {
                return Err(CourierFailure::Unintelligible);
            }
            connection.send(&bytes).await?;
            drop(bytes);
            drop(invocation);
            let frame = connection
                .recv_bounded(MAX_FRAME_BYTES)
                .await?
                .ok_or(CourierFailure::NoAnswer)?;
            match serde_json::from_slice::<HelloAnswer>(&frame) {
                Ok(HelloAnswer::Welcome { session }) if session == self.hello.session => {}
                Ok(HelloAnswer::Refused) => {
                    return Ok(Answer::Refused {
                        reason: "connection refused".into(),
                    });
                }
                _ => return Err(CourierFailure::Unintelligible),
            }
            let frame = connection
                .recv_bounded(MAX_FRAME_BYTES)
                .await?
                .ok_or(CourierFailure::NoAnswer)?;
            serde_json::from_slice(&frame).map_err(|_invalid| CourierFailure::Unintelligible)
        })
        .await
        .map_err(|_elapsed| CourierFailure::Timeout)?
    }
}

async fn body() -> Result<BoundedUtf8, CourierFailure> {
    let ceiling = Limits::INITIAL.body_bytes;
    let mut bytes = Vec::with_capacity(ceiling + 1);
    tokio::io::stdin()
        .take(u64::try_from(ceiling + 1).unwrap_or(u64::MAX))
        .read_to_end(&mut bytes)
        .await?;
    if bytes.len() > ceiling {
        return Err(runtrol_courier::BodyTooLarge {
            len: bytes.len(),
            ceiling,
        }
        .into());
    }
    let text = String::from_utf8(bytes).map_err(|_invalid| CourierFailure::Utf8)?;
    Ok(BoundedUtf8::new(text, ceiling)?)
}

/// Execute the courier words after `courier`. Bodies come only from bounded stdin reads.
///
/// # Errors
/// Invalid words, missing process authority, malformed input, transport failure, timeout, or user interruption.
pub async fn execute(words: Vec<OsString>) -> Result<CommandOutput, CourierFailure> {
    if words.is_empty() {
        let admission = courier().await?;
        return Ok(CommandOutput {
            stdout: format!("courier: {}", admission.word()),
            success: admission == Admission::Welcomed,
        });
    }
    let words: Vec<String> = words
        .into_iter()
        .map(|word| {
            word.into_string().map_err(|_invalid| {
                CourierFailure::Arguments("courier arguments must be UTF-8".into())
            })
        })
        .collect::<Result<_, _>>()?;
    let room_help = words.first().is_some_and(|word| word == "room")
        && matches!(words.len(), 2 | 3)
        && words.last().is_some_and(|word| word == "--help");
    if room_help {
        return Ok(CommandOutput {
            stdout: super::words::room_help(),
            success: true,
        });
    }
    if let [word] = words.as_slice()
        && matches!(word.as_str(), "--help" | "help" | "--guide")
    {
        return Ok(CommandOutput {
            stdout: if word == "--guide" { guide() } else { help() },
            success: true,
        });
    }
    if let [guide_word, from_word, source, message_word, message] = words.as_slice()
        && guide_word == "--guide"
        && from_word == "--from"
        && message_word == "--message-id"
    {
        return Ok(CommandOutput {
            stdout: super::words::initial_guide(
                super::words::identifier(Some(source))?,
                super::words::identifier(Some(message))?,
            ),
            success: true,
        });
    }
    let command = parse(&words)?;
    let birth = Birth::read()?;
    let answer = run(&birth, command).await?;
    let success = !matches!(
        answer,
        Answer::Refused { .. } | Answer::Received { envelope: None }
    );
    let stdout =
        serde_json::to_string(&answer).map_err(|_invalid| CourierFailure::Unintelligible)?;
    Ok(CommandOutput { stdout, success })
}

async fn run(birth: &Birth, command: Command) -> Result<Answer, CourierFailure> {
    let answer = match command {
        Command::Spawn(command) => {
            let task = if command.task {
                Some(body().await?)
            } else {
                None
            };
            birth
                .exchange(
                    Request::Spawn {
                        provider: command.provider,
                        model: command.model,
                        task,
                        timeout_ms: command.timeout_ms,
                    },
                    Duration::from_millis(command.timeout_ms),
                )
                .await?
        }
        Command::Room(command) => room(birth, command).await?,
        Command::List { after } => {
            birth
                .exchange(Request::List { after }, Duration::ZERO)
                .await?
        }
        Command::Reply { message, outgoing } => {
            birth
                .exchange(
                    Request::Reply {
                        message,
                        message_id: outgoing,
                        body: body().await?,
                    },
                    Duration::ZERO,
                )
                .await?
        }
        Command::Cancel { call } => {
            birth
                .exchange(
                    Request::Cancel {
                        call,
                        message_id: MessageId::now(),
                    },
                    Duration::ZERO,
                )
                .await?
        }
        Command::Receive { source, timeout_ms } => {
            wait(
                birth,
                Request::Receive {
                    source,
                    call: None,
                    timeout_ms,
                },
                timeout_ms,
            )
            .await?
        }
        Command::Send {
            target,
            ask,
            message,
            timeout_ms,
        } => {
            let body = body().await?;
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|_invalid| CourierFailure::Timeout)?;
            let deadline =
                UnixMillis(u64::try_from(now.as_millis()).unwrap_or(u64::MAX)).plus(timeout_ms);
            let mut envelope = if ask {
                CallEnvelope::ask(birth.hello.session, target, body, deadline)
            } else {
                CallEnvelope::tell(birth.hello.session, target, body, deadline)
            };
            envelope.message_id = message;
            if ask {
                wait(birth, Request::Ask { envelope }, timeout_ms).await?
            } else {
                birth
                    .exchange(Request::Send { envelope }, Duration::ZERO)
                    .await?
            }
        }
    };
    Ok(answer)
}

async fn room(birth: &Birth, command: super::rooms::RoomCommand) -> Result<Answer, CourierFailure> {
    use super::rooms::RoomCommand;
    let request = match command {
        RoomCommand::Open {
            mut peers,
            timeout_ms,
        } => {
            if peers.contains(&birth.hello.session) {
                return Err(CourierFailure::Arguments(
                    "room open lists peers only; your identity is included automatically".into(),
                ));
            }
            peers.insert(0, birth.hello.session);
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|_invalid| CourierFailure::Timeout)?;
            Request::RoomOpen {
                participants: peers,
                deadline: UnixMillis(u64::try_from(now.as_millis()).unwrap_or(u64::MAX))
                    .plus(timeout_ms),
            }
        }
        RoomCommand::Inspect { room } => Request::RoomInspect { room },
        RoomCommand::Transfer { room, speaker } => Request::RoomTransfer { room, speaker },
        RoomCommand::Close { room } => Request::RoomClose { room },
        RoomCommand::Ask {
            room,
            target,
            message,
            timeout_ms,
        } => {
            return wait(
                birth,
                Request::RoomAsk {
                    room,
                    target,
                    message_id: message,
                    body: body().await?,
                    timeout_ms,
                },
                timeout_ms,
            )
            .await;
        }
    };
    birth.exchange(request, Duration::ZERO).await
}

async fn wait(birth: &Birth, request: Request, timeout_ms: u64) -> Result<Answer, CourierFailure> {
    tokio::select! {
        answer = birth.exchange(request, Duration::from_millis(timeout_ms)) => answer,
        interrupted = tokio::signal::ctrl_c() => {
            interrupted?;
            // Dropping the exchange closes its pipe. The Runtime releases the pending exact call on close.
            Err(CourierFailure::Interrupted)
        }
    }
}
