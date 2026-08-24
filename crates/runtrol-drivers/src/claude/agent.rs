//! One process per session, driven over its own standard streams.
//!
//! # What the process actually is
//!
//! Measured: `--print --input-format stream-json --output-format stream-json --verbose` with the session
//! identifier runtrol minted. The process stays up across turns, reads one JSON object per line on its input, and
//! writes one per line on its output. No terminal is involved anywhere, which is a correctness decision rather
//! than a cost one: the platform's console layer hard-wraps at the terminal width and would split one long line
//! of JSON into two invalid ones.
//!
//! # Why the turn identifier lives here
//!
//! The mapping is pure and knows nothing about which turn is running. This does, because it is what sent the
//! prompt. So the ending the mapping recognises becomes a turn event here, stamped with the turn that was running
//! and with the provider as the one who declared it.
//!
//! The beginning is the same fact from the other end, and it comes from here for the same reason: nothing in the
//! stream announces a turn starting, so the only thing that knows one has is whatever wrote the prompt. Measured:
//! without it, runtrol showed a session as idle for the whole of a turn it was running, and an operator watching
//! a list would have seen nothing happening while the agent worked.
//!
//! A turn that was running when the process died gets an ending too, declared by the exit and **not** by the
//! provider, which is what keeps "the outcome is unknown" distinguishable from "it finished".
//!
//! # What the source boundary means
//!
//! `src_end` is a monotone boundary within this live provider stream. Complete content and provider-declared turn
//! endings advance it, while fragments and control events carry the current value. It is diagnostic ordering
//! metadata, not a transcript byte offset, a provider resume token, or a reconnect cursor.
//!
//! A subscriber reconnects with a `WatchCursor` over the bounded in-memory window. Content outside that window is
//! reported as an explicit gap. This driver never discovers, derives, or reads a provider transcript path.

use core::time::Duration;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

use async_trait::async_trait;
use runtrol_childproc::contain::{ChildGuard, TrackedChild, TrackedCommand};
use runtrol_childproc::{Containment, Program, SpawnError};
use runtrol_provider::{
    Agent, AgentCommand, ApprovalId, ApprovalRequest, Chunk, CloseMode, ContentBlock, Cost,
    Declarant, Disposition, EventBody, Level, MessageId, Notice, NoticeCode, Opaque, OpenIntent,
    Produced, ProviderError, ProviderId, SessionId, StopReason, TurnEvent, TurnId, Usage, WallMs,
    WithdrawnReason,
};
use tokio::io::AsyncWriteExt as _;
use tokio::process::ChildStdin;

use crate::claude::approval::{self, ApprovalBook, ApprovalBuildError, NativeAnswer};
use crate::claude::map::{self, Frame};
use crate::claude::store::Replay;
use crate::framing::{LineError, Lines};

/// A live session: one child process, its input, and its output.
pub struct ClaudeAgent {
    /// Which provider this is.
    ///
    /// Held because every variant of the error taxonomy names one: an operator reading a failure has to know which
    /// CLI it came from without inferring it from the wording.
    provider: ProviderId,
    /// runtrol's own name for the session.
    session: SessionId,
    /// The provider's name for it, once it has announced one.
    native: Option<String>,
    /// Explicit launch choices retained until the provider announces the attached session.
    model_requested: Option<Box<str>>,
    reasoning_effort_requested: Option<Box<str>>,
    /// The durable process-group record, dropped before the child handle so the live root can identify the group.
    child_guard: ChildGuard,
    /// The child.
    child: TrackedChild,
    /// Its input, taken so a command can be written.
    stdin: Option<ChildStdin>,
    /// Its output, one line at a time.
    lines: Lines<tokio::process::ChildStdout>,
    /// The turn that is running, if one is.
    ///
    /// Held here because this is what sent the prompt. The mapping is pure and cannot know it.
    running: Option<TurnId>,
    /// The message the provider is streaming right now, from the fragment that opened it.
    ///
    /// Held here for the same reason as the turn: the mapping is pure and sees one line at a time, and the
    /// real CLI names a message only on its opening fragment and on its whole (measured on 2.1.237). The
    /// deltas in between name nothing, and a subscriber shown nameless deltas cannot append them to one
    /// another, nor recognise the whole that follows as the same message. So each nameless delta is given
    /// the name of the message that is open. Correlation, not interpretation: nothing is read but the name.
    streaming_message: Option<MessageId>,
    /// Which turn number to use next.
    next_turn: u32,
    /// Which control-switch request number to use next, separate from turns so an interrupt and a switch sent
    /// in the same breath cannot mint the same control identity.
    next_switch: u64,
    /// Switches runtrol asked for and the CLI has not answered yet, by control request identity.
    ///
    /// Bounded by how fast an operator can click: each entry leaves on the CLI's reply, and a session that
    /// never replies is a protocol failure the read loop already promotes.
    pending_switches: BTreeMap<Box<str>, ControlSwitch>,
    /// The monotone source boundary in this live provider stream.
    ///
    /// A whole message or provider-declared ending advances it. Fragments and control events carry the current
    /// value. This is separate from the stream, epoch, and sequence in a subscriber's `WatchCursor`.
    src_end: u64,
    /// An event runtrol itself produced, waiting to be handed over.
    ///
    /// Bounded by one local turn announcement plus the bounded pending approval book.
    ///
    /// A terminal frame can withdraw every pending approval before it reports the turn ending. The queue cannot
    /// exceed that fixed set, and provider conversation content never enters it.
    announced: VecDeque<Produced>,
    /// Provider-native approval questions waiting for a human.
    approvals: ApprovalBook,
    /// Set once the stream is over, so nothing keeps reading a finished session.
    finished: bool,
}

impl ClaudeAgent {
    /// Start the process and bind to it.
    ///
    /// # Errors
    ///
    /// [`ProviderError::BinNotFound`] when the CLI is not installed, [`ProviderError::Spawn`] when it cannot be
    /// started, [`ProviderError::Unsupported`] when an argument cannot be passed at all.
    pub(crate) fn start(
        provider: ProviderId,
        program: &Program,
        intent: &OpenIntent,
        contained_by: &Containment,
        available_flags: &BTreeSet<Box<str>>,
        unavailable_flags: &BTreeMap<Box<str>, &'static str>,
        replay: Option<Replay>,
    ) -> Result<Self, ProviderError> {
        let args = argv(provider, intent, available_flags, unavailable_flags)?;
        runtrol_childproc::check_all(&args).map_err(|error| ProviderError::Unsupported {
            provider,
            what: error.to_string(),
            why: "this argument cannot be passed on a command line",
        })?;

        let mut command = TrackedCommand::new(program.path().as_std_path());
        command
            .args(program.leading())
            .args(&args)
            .current_dir(intent.workspace.as_std_path())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            // Left alone rather than captured. What the CLI writes there is its own diagnostics, and a pipe nobody
            // reads fills up and blocks the process it belongs to.
            .stderr(std::process::Stdio::inherit())
            .kill_on_drop(true);
        let (mut child, child_guard) = command
            .spawn(contained_by)
            .map_err(|error| spawn_error(provider, program, error))?;

        let missing = |what: &str| ProviderError::Spawn {
            provider,
            program: program.path().to_string(),
            source: std::io::Error::other(format!("the child has no {what} stream")),
        };
        let stdin = child.stdin.take().ok_or_else(|| missing("input"))?;
        let stdout = child.stdout.take().ok_or_else(|| missing("output"))?;

        // The stored conversation goes out first, before the CLI has said anything. Measured on 2.1.238: on
        // `--resume` this CLI prints its hello frame only after the first message it is sent (hook frames at
        // once, `init` thirteen seconds later right behind the input), so a replay queued behind the
        // attachment would show nothing until the operator spoke. The page paints what it is given; the
        // attachment arrives when the CLI gets round to it.
        let mut announced = VecDeque::new();
        if let Some(replay) = replay {
            announced.extend(
                replay_bodies(replay)
                    .into_iter()
                    .map(|body| Produced { src_end: 0, body }),
            );
        }

        Ok(Self {
            provider,
            session: intent.session,
            native: None,
            model_requested: intent.model.clone(),
            reasoning_effort_requested: intent.reasoning_effort.clone(),
            child,
            child_guard,
            stdin: Some(stdin),
            lines: Lines::new(stdout),
            running: None,
            streaming_message: None,
            next_turn: 0,
            next_switch: 0,
            pending_switches: BTreeMap::new(),
            src_end: 0,
            announced,
            approvals: ApprovalBook::new(),
            finished: false,
        })
    }

    /// Give a nameless streamed fragment the name of the message that is open, and remember the name an
    /// opening fragment carries. A whole message closes the stream.
    fn correlate_stream(&mut self, body: EventBody) -> EventBody {
        match body {
            EventBody::AgentMessageChunk(chunk) if chunk.delta => {
                EventBody::AgentMessageChunk(self.named(chunk))
            }
            EventBody::AgentThoughtChunk(chunk) if chunk.delta => {
                EventBody::AgentThoughtChunk(self.named(chunk))
            }
            EventBody::AgentMessageChunk(_) | EventBody::AgentThoughtChunk(_) => {
                self.streaming_message = None;
                body
            }
            other => other,
        }
    }

    /// The fragment with the open message's name, remembering a name the fragment itself carries.
    fn named(&mut self, mut chunk: Chunk) -> Chunk {
        match &chunk.message_id {
            Some(id) => self.streaming_message = Some(id.clone()),
            None => chunk.message_id.clone_from(&self.streaming_message),
        }
        chunk
    }

    /// Write one line to the child's input.
    async fn write_line(&mut self, text: &str) -> Result<(), ProviderError> {
        let provider = self.provider;
        let stdin = self.stdin.as_mut().ok_or_else(|| ProviderError::Protocol {
            provider,
            doing: "sending a command",
            detail: "this session's input has already been closed".to_owned(),
        })?;

        // The newline is what makes it a frame. Written with the body in one call so a frame cannot be half sent.
        let mut framed = String::with_capacity(text.len() + 1);
        framed.push_str(text);
        framed.push('\n');

        stdin
            .write_all(framed.as_bytes())
            .await
            .map_err(|error| ProviderError::Protocol {
                provider,
                doing: "writing to the session",
                detail: error.to_string(),
            })?;
        stdin
            .flush()
            .await
            .map_err(|error| ProviderError::Protocol {
                provider,
                doing: "flushing the input of the session",
                detail: error.to_string(),
            })
    }

    /// What a control reply means, when it answers a model switch runtrol sent.
    ///
    /// Interrupt replies say nothing a turn event does not and stay dropped. A switch reply is the CLI's word
    /// on whether the model or mode moved: success becomes the matching current-state event, and a refusal
    /// becomes a loud notice carrying the CLI's own sentence.
    fn switch_outcome(&mut self, outcome: map::ControlOutcome) -> Option<Produced> {
        let asked = self.pending_switches.remove(&outcome.request_id)?;
        Some(Produced {
            src_end: self.src_end,
            body: switch_event(asked, outcome),
        })
    }

    /// Turn a classified frame into what leaves the driver.
    fn produce(&mut self, frame: Frame) -> Produced {
        match frame {
            Frame::Started(startup) => self.attached(*startup),
            Frame::Ended(ended) => self.ended(&ended),
            Frame::Body(body) => {
                let body = self.correlate_stream(body);
                // A fragment belongs to the current source boundary. A complete body advances the boundary.
                if !body.is_fragment() {
                    self.src_end = self.src_end.saturating_add(1);
                }
                Produced {
                    src_end: self.src_end,
                    body,
                }
            }
            Frame::Bodies { first, rest } => self.bodies(first, rest),
            Frame::Unbound(unmapped) => self.unbound(unmapped),
            Frame::ControlResponse(_)
            | Frame::Approval(_)
            | Frame::ApprovalCancelled(_)
            | Frame::UnsupportedControl(_) => self.not_a_body(),
        }
    }

    /// The attachment, with what the CLI said hello with queued to follow it: the mode and model in force
    /// and its slash commands.
    fn attached(&mut self, startup: map::Startup) -> Produced {
        self.native = Some(startup.native.as_str().to_owned());
        if let Some(mode) = startup.starting_mode.clone() {
            // The permission mode in force at attachment, by the CLI's own word, queued the same way
            // as the answering model below.
            self.announced.push_back(Produced {
                src_end: self.src_end,
                body: EventBody::CurrentModeUpdate {
                    mode_id: mode,
                    available_ids: None,
                    payload: startup.payload.clone(),
                },
            });
        }
        if let Some(model) = startup.answering_with.clone() {
            // The CLI's own word on which model answers, queued to follow the attachment. Requested and
            // answering differ (measured), and only this one is the provider's.
            self.announced.push_back(Produced {
                src_end: self.src_end,
                body: EventBody::CurrentModelUpdate {
                    model_id: model,
                    available_ids: None,
                    payload: startup.payload.clone(),
                },
            });
        }
        if startup.announces_commands {
            // This CLI names its slash commands only inside the frame it says hello with. Re-emitted
            // whole as the one dedicated commands event every service shares, so a surface reads a
            // single vocabulary instead of digging through per-dialect attachment payloads.
            self.announced.push_back(Produced {
                src_end: self.src_end,
                body: EventBody::AvailableCommandsUpdate {
                    payload: startup.payload.clone(),
                },
            });
        }
        Produced {
            src_end: self.src_end,
            body: EventBody::Attached(Box::new(runtrol_provider::Attached {
                native: startup.native,
                // What runtrol asked for, not what will answer. The answering model stays in the payload.
                model_requested: self.model_requested.clone(),
                reasoning_effort_requested: self.reasoning_effort_requested.clone(),
                caps: startup.caps,
                payload: startup.payload,
            })),
        }
    }

    /// The provider's own word that the turn ended, which is the only thing that means the outcome is known.
    ///
    /// The terminal frame is also where this CLI states the running cost and its token breakdown. That is
    /// emitted as the one usage frame every provider shares (codex and acp already send it), so a surface reads
    /// spend the same way for all of them. Queued ahead of the end, and off the same source line, so the number
    /// is recorded while this is still the current turn and the end stays the last word.
    fn ended(&mut self, ended: &map::Ended) -> Produced {
        self.streaming_message = None;
        let turn = self.running.take().unwrap_or_else(|| self.mint_turn());
        self.src_end = self.src_end.saturating_add(1);
        if let Some(amount) = ended.cost {
            self.announced.push_back(Produced {
                src_end: self.src_end,
                body: EventBody::UsageUpdate(Box::new(Usage {
                    // This CLI states money, not a context-window figure, on this frame. A used/size gauge would
                    // have to be invented, so it is left unsaid rather than guessed.
                    used: None,
                    size: None,
                    // The field is `total_cost_usd`; the provider names the unit, runtrol does not convert it.
                    cost: Some(Cost {
                        amount,
                        currency: "USD".into(),
                    }),
                    detail: ended.usage_detail.clone(),
                })),
            });
        }
        Produced {
            src_end: self.src_end,
            body: EventBody::Turn(TurnEvent::Ended {
                turn,
                stop: ended.stop,
                declared_by: Declarant::Provider,
            }),
        }
    }

    /// One provider line that turned out to be several events.
    fn bodies(&mut self, first: EventBody, rest: Vec<EventBody>) -> Produced {
        // A whole message closes whatever was streaming.
        self.streaming_message = None;
        // One line of the provider's stream is one source position, however many events it turned out
        // to be. The boundary advances once and every event off that line carries the same one.
        self.src_end = self.src_end.saturating_add(1);
        // The queue is drained before the next line is read, so the blocks reach a subscriber in the
        // order the provider laid them out.
        for body in rest {
            self.announced.push_back(Produced {
                src_end: self.src_end,
                body,
            });
        }
        Produced {
            src_end: self.src_end,
            body: first,
        }
    }

    /// A frame nobody bound, relayed whole under its own tag.
    fn unbound(&self, unmapped: runtrol_provider::Unmapped) -> Produced {
        Produced {
            src_end: self.src_end,
            body: EventBody::Unmapped(unmapped),
        }
    }

    /// A control frame that reached the body path: the control boundary should have taken it.
    fn not_a_body(&self) -> Produced {
        Produced {
            src_end: self.src_end,
            body: EventBody::Notice(Box::new(Notice {
                level: Level::Error,
                code: NoticeCode::ProtocolViolation,
                retryable: false,
                payload: Opaque::owned(
                    r#"{"controlFrame":"was not handled at the control boundary"}"#.to_owned(),
                ),
            })),
        }
    }

    /// The next turn number.
    fn mint_turn(&mut self) -> TurnId {
        let turn = TurnId {
            epoch: 0,
            index: self.next_turn,
        };
        self.next_turn = self.next_turn.saturating_add(1);
        turn
    }

    /// What to report when the stream ends.
    ///
    /// A turn that was running when the process stopped gets an ending declared by the exit rather than by the
    /// provider. That distinction is the whole point: a subscriber renders "the outcome is unknown", and the next
    /// attach must not claim the turn succeeded.
    fn ending_at_exit(&mut self) -> Option<Produced> {
        let turn = self.running.take()?;
        Some(Produced {
            src_end: self.src_end,
            body: EventBody::Turn(TurnEvent::Ended {
                turn,
                stop: StopReason::Unknown,
                declared_by: Declarant::ProcessExit,
            }),
        })
    }

    /// Send one retained provider-native answer.
    async fn write_answer(
        &mut self,
        native: &NativeAnswer,
        doing: &'static str,
    ) -> Result<(), ProviderError> {
        let frame = native.frame().map_err(|error| ProviderError::Protocol {
            provider: self.provider,
            doing,
            detail: error.to_string(),
        })?;
        self.write_line(&frame).await
    }

    /// Refuse the oldest approval whose deadline has passed.
    async fn expire_one(&mut self) -> Option<Result<Produced, ProviderError>> {
        let native = self.approvals.due(WallMs::now())?;
        if let Err(error) = self.write_answer(&native, "expiring an approval").await {
            return Some(Err(error));
        }
        self.approvals.complete(native.approval);
        Some(Ok(Produced {
            src_end: self.src_end,
            body: EventBody::ApprovalWithdrawn {
                id: native.approval,
                why: WithdrawnReason::Expired,
            },
        }))
    }

    /// Remove every pending approval and queue an explicit withdrawal for each one.
    fn withdraw_all(&mut self, why: WithdrawnReason) {
        for id in self.approvals.take_all() {
            self.announced.push_back(Produced {
                src_end: self.src_end,
                body: EventBody::ApprovalWithdrawn { id, why },
            });
        }
    }

    /// Handle the stateful control frames around the otherwise pure event mapping.
    async fn handle_frame(&mut self, frame: Frame) -> Result<Produced, ProviderError> {
        match frame {
            Frame::Approval(incoming) => {
                let native_request = incoming.native_request().to_owned();
                let turn = self.running;
                match self.approvals.open(*incoming, turn) {
                    Ok(request) => Ok(Produced {
                        src_end: self.src_end,
                        body: EventBody::ApprovalRequested(Box::new(request)),
                    }),
                    Err(error) => {
                        let frame = approval::deny_frame(&native_request).map_err(|write| {
                            ProviderError::Protocol {
                                provider: self.provider,
                                doing: "declining an unreadable approval",
                                detail: write.to_string(),
                            }
                        })?;
                        self.write_line(&frame).await?;
                        Ok(Produced {
                            src_end: self.src_end,
                            body: approval_notice(&error),
                        })
                    }
                }
            }

            Frame::ApprovalCancelled(cancelled) => {
                match self.approvals.cancel(&cancelled.native_request) {
                    Some(id) => Ok(Produced {
                        src_end: self.src_end,
                        body: EventBody::ApprovalWithdrawn {
                            id,
                            why: WithdrawnReason::ProviderCancelled,
                        },
                    }),
                    None => Ok(Produced {
                        src_end: self.src_end,
                        body: EventBody::Unmapped(runtrol_provider::Unmapped {
                            tag: "control_cancel_request".into(),
                            turn: self.running,
                            payload: cancelled.payload,
                            unknown_to_binding: false,
                        }),
                    }),
                }
            }

            Frame::UnsupportedControl(control) => {
                let frame = approval::error_frame(&control.native_request).map_err(|error| {
                    ProviderError::Protocol {
                        provider: self.provider,
                        doing: "refusing an unsupported control request",
                        detail: error.to_string(),
                    }
                })?;
                self.write_line(&frame).await?;
                Ok(Produced {
                    src_end: self.src_end,
                    body: unsupported_control_notice(&control.subtype),
                })
            }

            Frame::Ended(ended) => {
                self.withdraw_all(WithdrawnReason::TurnGone);
                let ending = self.produce(Frame::Ended(ended));
                if let Some(withdrawn) = self.announced.pop_front() {
                    self.announced.push_back(ending);
                    Ok(withdrawn)
                } else {
                    Ok(ending)
                }
            }

            other => Ok(self.produce(other)),
        }
    }
}

fn spawn_error(provider: ProviderId, program: &Program, error: SpawnError) -> ProviderError {
    ProviderError::Spawn {
        provider,
        program: program.path().to_string(),
        source: std::io::Error::other(error),
    }
}

/// The arguments for one session.
///
/// Separated from the spawn so the whole command line can be checked without starting anything. Every flag here is
/// on the bound list, and the two that are not always present are the ones a missing flag degrades.
///
/// # Errors
///
/// [`ProviderError::Unsupported`] for a way of opening a session this driver does not know. The contract's
/// dispositions are open ended on purpose, so that adding one does not break a driver written elsewhere; the cost
/// is that each driver has to say which ones it serves rather than quietly treating a new one as something else.
fn argv(
    provider: ProviderId,
    intent: &OpenIntent,
    available_flags: &BTreeSet<Box<str>>,
    unavailable_flags: &BTreeMap<Box<str>, &'static str>,
) -> Result<Vec<String>, ProviderError> {
    let mut args: Vec<String> = vec![
        "--print".to_owned(),
        "--input-format".to_owned(),
        "stream-json".to_owned(),
        "--output-format".to_owned(),
        "stream-json".to_owned(),
        // Measured: without this the CLI refuses to stream structured output at all.
        "--verbose".to_owned(),
        // Measured on 2.1.220 by asking the parser: hidden from help, but this is the stdio approval channel.
        "--permission-prompt-tool".to_owned(),
        "stdio".to_owned(),
    ];
    if available_flags.contains("--include-partial-messages") {
        args.push("--include-partial-messages".to_owned());
    }
    // The operator's own messages come back through the provider, as `user` frames, and the page shows them on
    // the operator's side. Without the flag the conversation reads as replies to nothing.
    if available_flags.contains("--replay-user-messages") {
        args.push("--replay-user-messages".to_owned());
    }

    match &intent.disposition {
        // runtrol issues the identifier. Measured: it comes back unchanged and becomes the name the CLI's own
        // store knows the session by, which is what makes deleting runtrol's records harmless.
        Disposition::Fresh => {
            args.push("--session-id".to_owned());
            args.push(intent.session.to_string());
        }
        // A resume takes the provider's own name for the conversation, which for this CLI is the same value.
        Disposition::Resume { native } => {
            args.push("--resume".to_owned());
            args.push(native.to_string());
        }
        // A way of opening a session that arrived after this driver did. Refused by name: guessing which of the two
        // it resembles would either start a conversation the operator did not ask for or continue the wrong one.
        other => {
            return Err(ProviderError::Unsupported {
                provider,
                what: format!("{other:?}"),
                why: "this driver serves a fresh session and a resume, and nothing else yet",
            });
        }
    }

    if let Some(model) = &intent.model {
        require_optional(provider, "--model", available_flags, unavailable_flags)?;
        args.extend(["--model".to_owned(), model.to_string()]);
    }
    if let Some(reasoning_effort) = &intent.reasoning_effort {
        require_optional(provider, "--effort", available_flags, unavailable_flags)?;
        args.extend(["--effort".to_owned(), reasoning_effort.to_string()]);
    }
    if let Some(permission) = &intent.permission {
        require_optional(
            provider,
            "--permission-mode",
            available_flags,
            unavailable_flags,
        )?;
        args.extend(["--permission-mode".to_owned(), permission.to_string()]);
    }
    Ok(args)
}

fn require_optional(
    provider: ProviderId,
    flag: &'static str,
    available_flags: &BTreeSet<Box<str>>,
    unavailable_flags: &BTreeMap<Box<str>, &'static str>,
) -> Result<(), ProviderError> {
    if available_flags.contains(flag) {
        return Ok(());
    }
    Err(ProviderError::Unsupported {
        provider,
        what: format!("the requested session option needs {flag}"),
        why: unavailable_flags
            .get(flag)
            .copied()
            .unwrap_or("the installed CLI parser did not confirm that flag"),
    })
}

/// One prompt, as the frame this CLI reads.
///
/// The operator's text goes in as it was written. Serialized rather than pasted together, because pasting is how a
/// value from somewhere else becomes structure and every value here came from somewhere else.
fn prompt_frame(provider: ProviderId, blocks: &[ContentBlock]) -> Result<String, ProviderError> {
    let mut content: Vec<serde_json::Value> = Vec::with_capacity(blocks.len());
    for block in blocks {
        content.push(match block {
            ContentBlock::Text(text) => {
                serde_json::json!({"type": "text", "text": text.to_string()})
            }
            // The CLI's own base64 content-block shape (measured 2026-08-20: a stream-json turn
            // carrying one answered normally, and the frame is not echoed back on the stream).
            ContentBlock::Image { media_type, base64 } => serde_json::json!({
                "type": "image",
                "source": {
                    "type": "base64",
                    "media_type": media_type.to_string(),
                    "data": base64.to_string(),
                },
            }),
            // Forwarded whole. A block shape runtrol has never heard of still reaches the provider, which is what
            // keeps runtrol a pipe rather than a gate on which features are reachable. A payload that is not
            // readable JSON goes as text rather than being dropped: the operator wrote it, and losing part of a
            // prompt silently is worse than sending it plainly.
            ContentBlock::Native(payload) => match serde_json::from_str(payload.as_str()) {
                Ok(value) => value,
                Err(_) => serde_json::Value::String(payload.as_str().to_owned()),
            },
            // A block kind that arrived after this driver did. Refused rather than skipped: a prompt missing one of
            // its parts is a prompt the operator did not write, and they would never know.
            other => {
                return Err(ProviderError::Unsupported {
                    provider,
                    what: format!("{other:?}"),
                    why: "this driver can send text, images, and a native block, and nothing else yet",
                });
            }
        });
    }

    serde_json::to_string(&serde_json::json!({
        "type": "user",
        "message": {"role": "user", "content": content},
    }))
    .map_err(|error| ProviderError::Protocol {
        provider,
        doing: "building a prompt",
        detail: error.to_string(),
    })
}

/// The frame that asks for the running turn to stop.
///
/// A request and not an outcome: what ends the turn is still the provider's own word, arriving as an event.
fn interrupt_frame(provider: ProviderId, request: u64) -> Result<String, ProviderError> {
    serde_json::to_string(&serde_json::json!({
        "type": "control_request",
        "request_id": format!("runtrol-{request}"),
        "request": {"subtype": "interrupt"},
    }))
    .map_err(|error| ProviderError::Protocol {
        provider,
        doing: "building an interrupt",
        detail: error.to_string(),
    })
}

/// The events a stored conversation's tail becomes, through the same reader a live frame goes through.
///
/// A record the reader cannot place (a shape this driver never bound) is left out and said once, so the page
/// is never filled with noise and never silently short. Turn boundaries and control frames cannot come out of
/// a stored message record and are not looked for.
fn replay_bodies(replay: Replay) -> Vec<EventBody> {
    let mut bodies = Vec::with_capacity(replay.records.len());
    let mut unread = 0_usize;
    for record in &replay.records {
        match map::read(record) {
            Ok(Frame::Body(body)) => bodies.push(body),
            Ok(Frame::Bodies { first, rest }) => {
                bodies.push(first);
                bodies.extend(rest);
            }
            Ok(Frame::Unbound(unmapped)) => bodies.push(EventBody::Unmapped(unmapped)),
            Ok(_) | Err(_) => unread = unread.saturating_add(1),
        }
    }
    let mut problems = Vec::new();
    if let Some(problem) = replay.problem {
        problems.push(problem.to_string());
    }
    if unread > 0 {
        problems.push(format!(
            "{unread} stored records of this conversation could not be read back and are not shown"
        ));
    }
    if !problems.is_empty() {
        bodies.push(EventBody::Notice(Box::new(Notice {
            level: Level::Info,
            code: NoticeCode::Other,
            retryable: false,
            payload: Opaque::owned(
                serde_json::json!({ "message": problems.join("; ") }).to_string(),
            ),
        })));
    }
    bodies
}

/// What a pending control request will announce if the CLI says yes.
///
/// The kind is decided by which command sent the request, never inferred from the reply: the CLI's control
/// responses carry only success or an error sentence, so the correlation map is the one place that knows what
/// was asked.
enum ControlSwitch {
    /// A model switch, carrying the requested model.
    Model(Box<str>),
    /// A permission-mode switch, carrying the requested mode.
    Mode(Box<str>),
}

/// The CLI's answer to one switch, as the event it earns.
///
/// Success becomes the matching current-state event carrying what was asked (the reply itself repeats it for
/// the mode and stays empty for the model, so the correlation entry is the one whole record). A refusal
/// becomes a loud notice whose payload is the CLI's own sentence.
fn switch_event(asked: ControlSwitch, outcome: map::ControlOutcome) -> EventBody {
    match (outcome.error, asked) {
        (None, ControlSwitch::Model(model)) => EventBody::CurrentModelUpdate {
            model_id: model,
            available_ids: None,
            payload: outcome.payload,
        },
        (None, ControlSwitch::Mode(mode)) => EventBody::CurrentModeUpdate {
            mode_id: mode,
            available_ids: None,
            payload: outcome.payload,
        },
        (Some(_), ControlSwitch::Model(_)) => EventBody::Notice(Box::new(Notice {
            level: Level::Warn,
            code: NoticeCode::ModelRerouted,
            retryable: false,
            payload: outcome.payload,
        })),
        (Some(_), ControlSwitch::Mode(_)) => EventBody::Notice(Box::new(Notice {
            level: Level::Warn,
            code: NoticeCode::ModeRefused,
            retryable: false,
            payload: outcome.payload,
        })),
    }
}

/// The frame that asks the CLI to answer with a different model from here on.
///
/// Measured on 2.1.235: the control channel accepts `set_model` mid-session, answers with a control response
/// carrying this request identity, and the switch also lands in the CLI's own transcript as a local command.
fn set_model_frame(
    provider: ProviderId,
    request: &str,
    model: &str,
) -> Result<String, ProviderError> {
    serde_json::to_string(&serde_json::json!({
        "type": "control_request",
        "request_id": request,
        "request": {"subtype": "set_model", "model": model},
    }))
    .map_err(|error| ProviderError::Protocol {
        provider,
        doing: "building a model switch",
        detail: error.to_string(),
    })
}

/// The frame that asks the CLI to run under a different permission mode from here on.
///
/// Measured on 2.1.x: the control channel accepts `set_permission_mode` mid-session, answers success with the
/// mode it now runs under, refuses an unknown name with a sentence enumerating its own vocabulary, and also
/// emits its own `system`/`status` announcement carrying the new mode.
fn set_mode_frame(
    provider: ProviderId,
    request: &str,
    mode: &str,
) -> Result<String, ProviderError> {
    serde_json::to_string(&serde_json::json!({
        "type": "control_request",
        "request_id": request,
        "request": {"subtype": "set_permission_mode", "mode": mode},
    }))
    .map_err(|error| ProviderError::Protocol {
        provider,
        doing: "building a mode switch",
        detail: error.to_string(),
    })
}

#[async_trait]
impl Agent for ClaudeAgent {
    fn session(&self) -> SessionId {
        self.session
    }

    fn native(&self) -> Option<&str> {
        self.native.as_deref()
    }

    fn approval(&self, id: ApprovalId) -> Option<&ApprovalRequest> {
        self.approvals.get(id)
    }

    fn approvals(&self) -> Vec<&ApprovalRequest> {
        self.approvals.all()
    }

    async fn send(&mut self, command: AgentCommand) -> Result<(), ProviderError> {
        match command {
            AgentCommand::Prompt(blocks) => {
                let frame = prompt_frame(self.provider, &blocks)?;
                // The turn is minted before the frame goes out, so an ending that arrives immediately has a turn
                // to belong to.
                let turn = self.mint_turn();
                self.running = Some(turn);
                self.write_line(&frame).await?;

                // Said out loud, because nothing in the stream announces a turn starting and this is the only
                // thing that knows one has. Announced after the write rather than before: a prompt that could not
                // be sent did not start anything, and reporting one would show an operator a turn running that
                // never was.
                self.announced.push_back(Produced {
                    src_end: self.src_end,
                    body: EventBody::Turn(TurnEvent::Started { turn }),
                });
                Ok(())
            }
            AgentCommand::Interrupt => {
                let frame = interrupt_frame(self.provider, u64::from(self.next_turn))?;
                self.write_line(&frame).await
            }
            AgentCommand::SetModel {
                model,
                reasoning_effort,
            } => {
                if reasoning_effort.is_some() {
                    // Measured on this CLI (2.1.235): set_effort and set_reasoning_effort are refused by the
                    // control channel, so a request carrying one must refuse rather than drop it.
                    return Err(ProviderError::Unsupported {
                        provider: self.provider,
                        what: "switching the reasoning effort mid-session".to_owned(),
                        why: "this CLI's control channel moves only the model",
                    });
                }
                let request = format!("runtrol-model-{}", self.next_switch);
                self.next_switch = self.next_switch.saturating_add(1);
                let frame = set_model_frame(self.provider, &request, &model)?;
                self.write_line(&frame).await?;
                // Remembered after the write: a frame that never left is not pending anything.
                self.pending_switches
                    .insert(request.into(), ControlSwitch::Model(model));
                Ok(())
            }
            AgentCommand::SetMode { mode } => {
                let request = format!("runtrol-mode-{}", self.next_switch);
                self.next_switch = self.next_switch.saturating_add(1);
                let frame = set_mode_frame(self.provider, &request, &mode)?;
                self.write_line(&frame).await?;
                // Remembered after the write: a frame that never left is not pending anything.
                self.pending_switches
                    .insert(request.into(), ControlSwitch::Mode(mode));
                Ok(())
            }
            // Forwarded byte for byte. Never inspected and never rewritten.
            AgentCommand::Native(payload) => {
                let text = payload.as_str().to_owned();
                self.write_line(&text).await
            }
            AgentCommand::Answer {
                id,
                option,
                subject_digest,
            } => {
                let native =
                    self.approvals
                        .answer(id, option, subject_digest)
                        .map_err(|error| {
                            approval_error(self.provider, "answering an approval", &error)
                        })?;
                self.write_answer(&native, "answering an approval").await?;
                self.approvals.complete(native.approval);
                Ok(())
            }
            // A command that arrived after this driver did. Saying so is the whole point: a wildcard that returned
            // success would report a command as sent when nothing was sent, and the operator would wait for an
            // effect that is never coming.
            other => Err(ProviderError::Unsupported {
                provider: self.provider,
                what: format!("{other:?}"),
                why: "this driver has no binding for that command",
            }),
        }
    }

    async fn next(&mut self) -> Option<Result<Produced, ProviderError>> {
        'next_event: loop {
            // What runtrol itself has to say comes first, and taking it is not a wait, so a caller that sets this
            // aside partway through loses nothing.
            if let Some(announced) = self.announced.pop_front() {
                return Some(Ok(announced));
            }
            if self.finished {
                return None;
            }

            let line = loop {
                if let Some(expired) = self.expire_one().await {
                    return Some(expired);
                }
                match self.approvals.wait(WallMs::now()) {
                    Some(wait) => {
                        if let Ok(line) = tokio::time::timeout(wait, self.lines.next()).await {
                            break line;
                        }
                        // The read is cancel-safe. The next pass sends the retained rejection before reading again.
                    }
                    None => break self.lines.next().await,
                }
            };

            // Every provider question receives either an exact native answer or a fail-closed error. Ordinary frames
            // still produce one event each, including frames nobody has bound yet.
            return match line {
                Ok(Some(line)) => match map::read(&line) {
                    Ok(Frame::ControlResponse(outcome)) => {
                        if let Some(produced) = self.switch_outcome(outcome) {
                            return Some(Ok(produced));
                        }
                        continue 'next_event;
                    }
                    Ok(frame) => Some(self.handle_frame(frame).await),
                    // A frame runtrol cannot read is a protocol failure, promoted to session state by the caller
                    // rather than logged and stepped over.
                    Err(error) => {
                        self.finished = true;
                        Some(Err(ProviderError::Protocol {
                            provider: self.provider,
                            doing: "reading a frame from the session",
                            detail: error.to_string(),
                        }))
                    }
                },

                Ok(None) => {
                    self.finished = true;
                    // A turn that was running when the stream ended gets an ending declared by the exit, never by the
                    // provider. That is what keeps "unknown" distinguishable from "finished".
                    self.withdraw_all(WithdrawnReason::TurnGone);
                    if let Some(ending) = self.ending_at_exit() {
                        self.announced.push_back(ending);
                    }
                    self.announced.pop_front().map(Ok)
                }

                Err(error) => {
                    self.finished = true;
                    let detail = error.to_string();
                    let doing = match error {
                        LineError::TooLong { .. } | LineError::Poisoned => {
                            "reading a frame past the limit of the transport"
                        }
                        LineError::Io { .. } => "reading from the session",
                    };
                    Some(Err(ProviderError::Protocol {
                        provider: self.provider,
                        doing,
                        detail,
                    }))
                }
            };
        }
    }

    async fn close(mut self: Box<Self>, how: CloseMode) -> Result<(), ProviderError> {
        for native in self.approvals.rejections() {
            self.write_answer(&native, "declining an approval while closing")
                .await?;
            self.approvals.complete(native.approval);
        }

        // Closing the input is how this CLI is asked to finish: it reads until its input ends.
        drop(self.stdin.take());

        let grace = match how {
            CloseMode::Kill => Duration::ZERO,
            CloseMode::Graceful { grace_ms } => Duration::from_millis(grace_ms),
            // A way of closing that arrived after this driver did. It stops now, like an outright kill, rather than
            // refusing: a close that returned an error would leave a session nobody can end, and a running agent
            // with nobody watching is the outcome the whole containment design exists to prevent. The two arms read
            // the same and mean different things, which is why this one is spelled out instead of merged.
            #[expect(
                clippy::match_same_arms,
                reason = "an unknown close mode stopping now is a decision, not a duplicate of Kill"
            )]
            _ => Duration::ZERO,
        };

        if !grace.is_zero() {
            match tokio::time::timeout(grace, self.child.wait()).await {
                Ok(Ok(_status)) => {
                    return self
                        .child_guard
                        .complete()
                        .map_err(|error| ProviderError::Io {
                            provider: self.provider,
                            doing: "completing session process containment",
                            source: std::io::Error::other(error),
                        });
                }
                Ok(Err(wait_error)) => {
                    return match self.child_guard.terminate(&mut self.child).await {
                        Ok(()) => Err(ProviderError::Io {
                            provider: self.provider,
                            doing: "waiting for a session to stop",
                            source: wait_error,
                        }),
                        Err(cleanup) => Err(ProviderError::Io {
                            provider: self.provider,
                            doing: "waiting for and cleaning up a session",
                            source: std::io::Error::other(format!(
                                "wait failed: {wait_error}; cleanup also failed: {cleanup}"
                            )),
                        }),
                    };
                }
                Err(_elapsed) => {}
            }
        }

        // Either the caller asked for it to stop now, or it did not finish in the time it was given. Reported
        // rather than swallowed: an operator who asked for a session to stop has to know whether it did.
        self.child_guard
            .terminate(&mut self.child)
            .await
            .map_err(|error| ProviderError::Io {
                provider: self.provider,
                doing: "stopping a session and its process group",
                source: std::io::Error::other(error),
            })
    }
}

fn approval_error(
    provider: ProviderId,
    doing: &'static str,
    error: &ApprovalBuildError,
) -> ProviderError {
    ProviderError::Protocol {
        provider,
        doing,
        detail: error.to_string(),
    }
}

fn approval_notice(error: &ApprovalBuildError) -> EventBody {
    let payload = serde_json::to_string(&serde_json::json!({
        "approvalDeclined": true,
        "why": error.to_string(),
    }))
    .unwrap_or_else(|_| r#"{"approvalDeclined":true}"#.to_owned());
    EventBody::Notice(Box::new(Notice {
        level: Level::Error,
        code: NoticeCode::ProtocolViolation,
        retryable: false,
        payload: Opaque::owned(payload),
    }))
}

fn unsupported_control_notice(subtype: &str) -> EventBody {
    let credential = matches!(subtype, "oauth_token_refresh" | "host_auth_token_refresh");
    let payload = serde_json::to_string(&serde_json::json!({
        "controlRequestRefused": subtype,
    }))
    .unwrap_or_else(|_| r#"{"controlRequestRefused":true}"#.to_owned());
    EventBody::Notice(Box::new(Notice {
        level: if credential {
            Level::Warn
        } else {
            Level::Error
        },
        code: if credential {
            NoticeCode::CredentialRequestRefused
        } else {
            NoticeCode::ProtocolViolation
        },
        retryable: false,
        payload: Opaque::owned(payload),
    }))
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use runtrol_childproc::{SpawnError, resolve};
    use runtrol_provider::AbsPath;

    use super::*;

    fn a_provider() -> ProviderId {
        ProviderId::parse("claude").expect("the test's own id must be valid")
    }

    fn all_flags() -> BTreeSet<Box<str>> {
        crate::claude::FLAGS
            .iter()
            .map(|flag| Box::<str>::from(flag.flag))
            .collect()
    }

    fn an_intent(disposition: Disposition) -> OpenIntent {
        OpenIntent {
            session: SessionId::now(),
            workspace: AbsPath::new(if cfg!(windows) { r"C:\work" } else { "/work" })
                .expect("valid"),
            disposition,
            model: None,
            reasoning_effort: None,
            permission: None,
        }
    }

    #[test]
    fn a_fresh_session_is_given_the_identifier_runtrol_minted() {
        // Measured: the CLI takes it and hands it back, so runtrol's name and the provider's are the same value.
        // That equality is what makes deleting everything runtrol stores lose nothing.
        let intent = an_intent(Disposition::Fresh);
        let args = argv(a_provider(), &intent, &all_flags(), &BTreeMap::new())
            .expect("a fresh session is served");
        let at = args
            .iter()
            .position(|arg| arg == "--session-id")
            .expect("a fresh session issues an identifier");
        assert_eq!(args.get(at + 1), Some(&intent.session.to_string()));
        assert!(!args.iter().any(|arg| arg == "--resume"));
    }

    #[test]
    fn a_resume_is_given_the_name_the_provider_knows() {
        let args = argv(
            a_provider(),
            &an_intent(Disposition::Resume {
                native: "some-provider-name".into(),
            }),
            &all_flags(),
            &BTreeMap::new(),
        )
        .expect("a resume is served");
        let at = args
            .iter()
            .position(|arg| arg == "--resume")
            .expect("a resume names what to continue");
        assert_eq!(
            args.get(at + 1).map(String::as_str),
            Some("some-provider-name")
        );
        assert!(
            !args.iter().any(|arg| arg == "--session-id"),
            "issuing an identifier for a session that exists would ask for a different session"
        );
    }

    #[test]
    fn the_flags_the_cli_refuses_to_stream_without_are_always_there() {
        // Measured: without the print flag it runs its own interface, and without the verbose flag it refuses to
        // stream structured output at all. Both are on the bound list as required.
        let args = argv(
            a_provider(),
            &an_intent(Disposition::Fresh),
            &all_flags(),
            &BTreeMap::new(),
        )
        .expect("served");
        for required in crate::claude::bound::FLAGS
            .iter()
            .filter(|flag| flag.required)
        {
            // The resume flag is required as a capability and used only on a resume.
            if required.flag == "--resume" {
                continue;
            }
            assert!(
                args.iter().any(|arg| arg == required.flag),
                "{} is required and missing from {args:?}",
                required.flag
            );
        }
    }

    #[test]
    fn no_model_and_no_permission_means_nothing_is_passed() {
        // The operator's own configuration decides. Passing a default would override a choice they already made.
        let args = argv(
            a_provider(),
            &an_intent(Disposition::Fresh),
            &all_flags(),
            &BTreeMap::new(),
        )
        .expect("served");
        assert!(!args.iter().any(|arg| arg == "--model"));
        assert!(!args.iter().any(|arg| arg == "--effort"));
        assert!(!args.iter().any(|arg| arg == "--permission-mode"));
    }

    #[test]
    fn a_stored_conversation_tail_replays_through_the_same_reader_as_live_frames() {
        // What the store hands over is the CLI's own records, the same shape as its live frames; the reader
        // that maps live frames maps them, so the page shows a reopened conversation exactly as it would have
        // shown it while it happened. Measured shapes (2.1.238), shortened.
        let records = vec![
            Bytes::from_static(br#"{"parentUuid":null,"isSidechain":false,"type":"user","message":{"role":"user","content":[{"type":"text","text":"name this folder"}]},"uuid":"u1","cwd":"C:\\work","sessionId":"s"}"#),
            Bytes::from_static(br#"{"parentUuid":"u1","isSidechain":false,"type":"assistant","message":{"id":"msg_01","role":"assistant","content":[{"type":"tool_use","id":"toolu_01","name":"Bash","input":{"command":"ls"}}]},"uuid":"a1","cwd":"C:\\work","sessionId":"s"}"#),
            Bytes::from_static(br#"{"parentUuid":"a1","isSidechain":false,"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_01","content":"runtrol"}]},"uuid":"u2","cwd":"C:\\work","sessionId":"s","toolUseResult":{"stdout":"runtrol"}}"#),
            Bytes::from_static(br#"{"parentUuid":"u2","isSidechain":false,"type":"assistant","message":{"id":"msg_02","role":"assistant","content":[{"type":"text","text":"It is runtrol."}]},"uuid":"a2","cwd":"C:\\work","sessionId":"s"}"#),
        ];
        let bodies = replay_bodies(Replay {
            records,
            problem: None,
        });
        let kinds: Vec<&str> = bodies
            .iter()
            .map(|body| match body {
                EventBody::UserMessageChunk(_) => "user",
                EventBody::AgentMessageChunk(_) => "agent",
                EventBody::ToolCall(_) => "tool started",
                EventBody::ToolCallUpdate(_) => "tool finished",
                EventBody::Notice(_) => "notice",
                _ => "other",
            })
            .collect();
        assert_eq!(
            kinds,
            vec!["user", "tool started", "tool finished", "agent"],
            "{bodies:?}"
        );
    }

    #[test]
    fn an_unreadable_stored_record_is_left_out_and_said_once() {
        let bodies = replay_bodies(Replay {
            records: vec![
                Bytes::from_static(b"not json at all"),
                Bytes::from_static(br#"{"type":"assistant","message":{"id":"msg_1","content":[{"type":"text","text":"kept"}]}}"#),
            ],
            problem: Some("the stored conversation could not be read back: fake".into()),
        });
        assert_eq!(bodies.len(), 2, "{bodies:?}");
        assert!(matches!(
            bodies.first(),
            Some(EventBody::AgentMessageChunk(_))
        ));
        match bodies.last() {
            Some(EventBody::Notice(notice)) => {
                assert!(notice.payload.as_str().contains("1 stored records"));
                assert!(notice.payload.as_str().contains("fake"));
            }
            other => panic!("expected one notice, got {other:?}"),
        }
    }

    #[test]
    fn the_operator_own_messages_are_asked_back_when_the_cli_can_replay_them() {
        // Measured on 2.1.238: `--replay-user-messages` re-emits each stdin message as a `user` frame. Without
        // it nothing shows the operator's side of the conversation, so the flag rides whenever the parser
        // confirmed it and is simply absent when it did not (older CLI): degraded, not refused.
        let args = argv(
            a_provider(),
            &an_intent(Disposition::Fresh),
            &all_flags(),
            &BTreeMap::new(),
        )
        .expect("served");
        assert!(args.iter().any(|arg| arg == "--replay-user-messages"));

        let mut flags = all_flags();
        flags.remove("--replay-user-messages");
        let args = argv(
            a_provider(),
            &an_intent(Disposition::Fresh),
            &flags,
            &BTreeMap::new(),
        )
        .expect("served without the replay flag");
        assert!(!args.iter().any(|arg| arg == "--replay-user-messages"));
    }

    #[test]
    fn an_explicit_choice_is_refused_when_its_optional_flag_was_not_confirmed() {
        let mut flags = all_flags();
        flags.remove("--include-partial-messages");
        flags.remove("--model");
        flags.remove("--effort");
        flags.remove("--permission-mode");
        let unavailable = [
            (
                Box::<str>::from("--model"),
                "the requested model cannot be selected",
            ),
            (
                Box::<str>::from("--effort"),
                "the requested reasoning posture cannot be selected",
            ),
            (
                Box::<str>::from("--permission-mode"),
                "the requested permission posture cannot be selected",
            ),
        ]
        .into_iter()
        .collect();
        for (model, effort, permission, expected_flag) in [
            (Some("operator-choice"), None, None, "--model"),
            (None, Some("operator-effort"), None, "--effort"),
            (None, None, Some("operator-permission"), "--permission-mode"),
        ] {
            let mut intent = an_intent(Disposition::Fresh);
            intent.model = model.map(Into::into);
            intent.reasoning_effort = effort.map(Into::into);
            intent.permission = permission.map(Into::into);
            let error = argv(a_provider(), &intent, &flags, &unavailable)
                .expect_err("an explicit choice must not be silently dropped");
            assert!(error.to_string().contains(expected_flag), "{error}");
        }
    }

    #[test]
    fn every_argument_can_actually_be_passed_on_a_command_line() {
        // The one place a value from somewhere else reaches a process launch. Checked before the spawn, because the
        // platform's own refusal names neither the argument nor the character.
        let mut intent = an_intent(Disposition::Fresh);
        intent.model = Some("haiku".into());
        intent.reasoning_effort = Some("high".into());
        intent.permission = Some("plan".into());
        let args = argv(a_provider(), &intent, &all_flags(), &BTreeMap::new()).expect("served");
        runtrol_childproc::check_all(&args).expect("every argument is passable");
    }

    #[test]
    fn a_prompt_carries_what_the_operator_wrote_and_nothing_else() {
        // The thin rule where it is easiest to break: runtrol adds no system prompt, no preamble, no instructions
        // of its own, and the frame is built by serializing rather than by pasting text together.
        let written = "do the thing";
        let frame =
            prompt_frame(a_provider(), &[ContentBlock::Text(written.into())]).expect("writable");

        assert!(frame.contains(written), "{frame}");
        assert!(frame.contains(r#""role":"user""#), "{frame}");
        assert!(!frame.contains('\n'), "a frame is one line: {frame}");

        let parsed: serde_json::Value = serde_json::from_str(&frame).expect("readable");
        let content = parsed
            .pointer("/message/content")
            .and_then(|value| value.as_array())
            .expect("the content is an array");
        assert_eq!(
            content.len(),
            1,
            "runtrol added a block of its own: {frame}"
        );
    }

    #[test]
    fn text_with_a_newline_in_it_still_produces_one_frame() {
        // A newline inside a frame would split it into two invalid ones. Serializing is what makes that impossible.
        let frame = prompt_frame(
            a_provider(),
            &[ContentBlock::Text("first\nsecond\r\nthird".into())],
        )
        .expect("writable");
        assert!(!frame.contains('\n'), "{frame}");
        assert!(!frame.contains('\r'), "{frame}");
    }

    #[test]
    fn an_image_block_becomes_the_cli_base64_source_shape() {
        // The measured shape (2026-08-20): a stream-json turn carrying this content block answered
        // normally, and the user frame is not echoed back on the stream.
        let frame = prompt_frame(
            a_provider(),
            &[
                ContentBlock::Text("what color is this".into()),
                ContentBlock::Image {
                    media_type: "image/png".into(),
                    base64: "aGVsbG8=".into(),
                },
            ],
        )
        .expect("writable");
        let parsed: serde_json::Value = serde_json::from_str(&frame).expect("readable");
        let image = parsed
            .pointer("/message/content/1")
            .expect("the image block");
        assert_eq!(
            image.pointer("/type").and_then(|v| v.as_str()),
            Some("image")
        );
        assert_eq!(
            image.pointer("/source/type").and_then(|v| v.as_str()),
            Some("base64")
        );
        assert_eq!(
            image.pointer("/source/media_type").and_then(|v| v.as_str()),
            Some("image/png")
        );
        assert_eq!(
            image.pointer("/source/data").and_then(|v| v.as_str()),
            Some("aGVsbG8=")
        );
    }

    #[test]
    fn a_block_runtrol_has_never_heard_of_reaches_the_provider_whole() {
        let native = runtrol_provider::Opaque::owned(
            r#"{"type":"image","source":{"kind":"base64"}}"#.to_owned(),
        );
        let frame = prompt_frame(a_provider(), &[ContentBlock::Native(native)]).expect("writable");
        let parsed: serde_json::Value = serde_json::from_str(&frame).expect("readable");
        let block = parsed
            .pointer("/message/content/0")
            .expect("the block survived");
        assert_eq!(
            block.pointer("/type").and_then(|v| v.as_str()),
            Some("image")
        );
        assert_eq!(
            block.pointer("/source/kind").and_then(|v| v.as_str()),
            Some("base64"),
            "the block has to arrive whole, not flattened"
        );
    }

    #[test]
    fn a_model_switch_is_the_cli_control_request_and_carries_only_the_choice() {
        // Measured on 2.1.235: {"subtype":"set_model","model":...} succeeds mid-session. The frame carries the
        // operator's choice verbatim and asserts nothing about the outcome, which stays the CLI's word.
        let frame = set_model_frame(a_provider(), "runtrol-model-0", "sonnet").expect("writable");
        let parsed: serde_json::Value = serde_json::from_str(&frame).expect("readable");
        assert_eq!(
            parsed.pointer("/type").and_then(|v| v.as_str()),
            Some("control_request")
        );
        assert_eq!(
            parsed.pointer("/request/subtype").and_then(|v| v.as_str()),
            Some("set_model")
        );
        assert_eq!(
            parsed.pointer("/request/model").and_then(|v| v.as_str()),
            Some("sonnet")
        );
        assert_eq!(
            parsed.pointer("/request_id").and_then(|v| v.as_str()),
            Some("runtrol-model-0")
        );
    }

    #[test]
    fn a_mode_switch_is_the_cli_control_request_and_carries_only_the_choice() {
        // Measured on 2.1.x: {"subtype":"set_permission_mode","mode":...} succeeds mid-session with
        // {"response":{"mode":"acceptEdits"}}, and an unknown name is refused with the CLI's own vocabulary
        // sentence. The frame carries the operator's choice verbatim and asserts nothing about the outcome.
        let frame =
            set_mode_frame(a_provider(), "runtrol-mode-0", "acceptEdits").expect("writable");
        let parsed: serde_json::Value = serde_json::from_str(&frame).expect("readable");
        assert_eq!(
            parsed.pointer("/request/subtype").and_then(|v| v.as_str()),
            Some("set_permission_mode")
        );
        assert_eq!(
            parsed.pointer("/request/mode").and_then(|v| v.as_str()),
            Some("acceptEdits")
        );
        assert_eq!(
            parsed.pointer("/request_id").and_then(|v| v.as_str()),
            Some("runtrol-mode-0")
        );
    }

    #[test]
    fn a_mode_switch_reply_becomes_the_mode_event_and_a_refusal_becomes_a_loud_notice() {
        // Both payloads are the measured CLI answers (2026-08-19 probe): success repeats the mode, and the
        // refusal sentence enumerates the CLI's own vocabulary.
        let confirmed = switch_event(
            ControlSwitch::Mode("acceptEdits".into()),
            map::ControlOutcome {
                request_id: "mode-1".into(),
                error: None,
                payload: Opaque::owned(
                    r#"{"type":"control_response","response":{"subtype":"success","request_id":"mode-1","response":{"mode":"acceptEdits"}}}"#.to_owned(),
                ),
            },
        );
        match confirmed {
            EventBody::CurrentModeUpdate { mode_id, .. } => assert_eq!(&*mode_id, "acceptEdits"),
            other => panic!("expected the mode event, got {other:?}"),
        }

        let refused = switch_event(
            ControlSwitch::Mode("nonsense".into()),
            map::ControlOutcome {
                request_id: "mode-2".into(),
                error: Some(
                    "Cannot set permission mode: must be one of acceptEdits, auto, bypassPermissions, default, dontAsk, plan".into(),
                ),
                payload: Opaque::owned("{}".to_owned()),
            },
        );
        match refused {
            EventBody::Notice(notice) => {
                assert_eq!(notice.code, NoticeCode::ModeRefused);
                assert_eq!(notice.level, Level::Warn);
            }
            other => panic!("expected the loud refusal, got {other:?}"),
        }
    }

    #[test]
    fn an_interrupt_is_a_control_frame_and_says_nothing_about_an_outcome() {
        let frame = interrupt_frame(a_provider(), 3).expect("writable");
        let parsed: serde_json::Value = serde_json::from_str(&frame).expect("readable");
        assert_eq!(
            parsed.pointer("/type").and_then(|v| v.as_str()),
            Some("control_request")
        );
        assert_eq!(
            parsed.pointer("/request/subtype").and_then(|v| v.as_str()),
            Some("interrupt")
        );
        assert!(
            !frame.contains("stop_reason") && !frame.contains("success"),
            "an interrupt must not assert an outcome: {frame}"
        );
    }

    #[test]
    fn the_cli_is_installed_or_this_machine_cannot_run_a_session() {
        // Not an assertion about runtrol. It records whether the thing being driven is here, so a failure further
        // along can be told apart from an absent CLI.
        match resolve("claude") {
            Ok(program) => assert!(program.path().as_str().len() > 1),
            // A machine without it has nothing to be wrong about.
            Err(SpawnError::NotFound { .. }) => {}
            Err(other) => panic!("the CLI is here and unusable: {other}"),
        }
    }
}
