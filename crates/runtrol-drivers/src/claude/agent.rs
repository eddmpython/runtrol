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
    Agent, AgentCommand, ApprovalId, ApprovalRequest, CloseMode, ContentBlock, Declarant,
    Disposition, EventBody, Level, Notice, NoticeCode, Opaque, OpenIntent, Produced, ProviderError,
    ProviderId, SessionId, StopReason, TurnEvent, TurnId, WallMs, WithdrawnReason,
};
use tokio::io::AsyncWriteExt as _;
use tokio::process::ChildStdin;

use crate::claude::approval::{self, ApprovalBook, ApprovalBuildError, NativeAnswer};
use crate::claude::map::{self, Frame};
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
    /// Which turn number to use next.
    next_turn: u32,
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
    pub fn start(
        provider: ProviderId,
        program: &Program,
        intent: &OpenIntent,
        contained_by: &Containment,
        available_flags: &BTreeSet<Box<str>>,
        unavailable_flags: &BTreeMap<Box<str>, &'static str>,
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
            next_turn: 0,
            src_end: 0,
            announced: VecDeque::new(),
            approvals: ApprovalBook::new(),
            finished: false,
        })
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

    /// Turn a classified frame into what leaves the driver.
    fn produce(&mut self, frame: Frame) -> Produced {
        match frame {
            Frame::Started(startup) => {
                self.native = Some(startup.native.as_str().to_owned());
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

            Frame::Ended(ended) => {
                // The provider's own word, which is the only thing that means the outcome is known.
                let turn = self.running.take().unwrap_or_else(|| self.mint_turn());
                self.src_end = self.src_end.saturating_add(1);
                Produced {
                    src_end: self.src_end,
                    body: EventBody::Turn(TurnEvent::Ended {
                        turn,
                        stop: ended.stop,
                        declared_by: Declarant::Provider,
                    }),
                }
            }

            Frame::Body(body) => {
                // A fragment belongs to the current source boundary. A complete body advances the boundary.
                if !body.is_fragment() {
                    self.src_end = self.src_end.saturating_add(1);
                }
                Produced {
                    src_end: self.src_end,
                    body,
                }
            }

            Frame::Approval(_)
            | Frame::ApprovalCancelled(_)
            | Frame::UnsupportedControl(_)
            | Frame::ControlResponse => Produced {
                src_end: self.src_end,
                body: EventBody::Notice(Box::new(Notice {
                    level: Level::Error,
                    code: NoticeCode::ProtocolViolation,
                    retryable: false,
                    payload: Opaque::owned(
                        r#"{"controlFrame":"was not handled at the control boundary"}"#.to_owned(),
                    ),
                })),
            },

            Frame::Unbound(unmapped) => Produced {
                src_end: self.src_end,
                body: EventBody::Unmapped(unmapped),
            },
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
                    why: "this driver can send text and a native block, and nothing else yet",
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
                    Ok(Frame::ControlResponse) => continue 'next_event,
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
