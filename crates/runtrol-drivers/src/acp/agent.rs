//! One ACP process per session, transported over NDJSON standard streams.

use core::time::Duration;
use std::collections::VecDeque;

use async_trait::async_trait;
use bytes::Bytes;
use runtrol_childproc::contain::{ChildGuard, TrackedChild, TrackedCommand};
use runtrol_childproc::{Containment, Program, SpawnError};
use runtrol_provider::{
    Agent, AgentCommand, Attached, CapabilitySet, CloseMode, ContentBlock, Declarant, Disposition,
    EventBody, Level, NativeSessionId, Notice, NoticeCode, Opaque, OpenIntent, Produced,
    ProviderError, ProviderId, SessionId, StopReason, TurnEvent, TurnId,
};
use tokio::io::AsyncWriteExt as _;
use tokio::process::ChildStdin;

use crate::acp::{map, wire};
use crate::framing::jsonrpc;
use crate::framing::{Incoming, LineError, Lines, RequestId};

/// How much of a handshake's pre-answer traffic is kept, as the most recent frames.
///
/// `session/load` replays the whole conversation as `session/update` notifications before it answers
/// (measured on grok 1.0.5: a two-screen conversation sent well over sixteen), and those notifications are
/// the history the reopened tab shows. A hard ceiling refused every conversation longer than a few turns
/// (measured 2026-08-21: "more than 16 frames or 131072 bytes arrived before the answer", on real
/// conversations a person had just had). So the bound keeps the tail and counts what it dropped, and the
/// session says so once before the first replayed frame. The subscriber ring downstream is 64 KiB, so a
/// tail of this size is already more than any reader receives.
const MAX_DEFERRED_FRAMES: usize = 256;
const MAX_DEFERRED_BYTES: usize = 768 * 1024;
const MAX_CAPABILITIES: usize = 64;
const MAX_CAPABILITY_BYTES: usize = 128;

/// A frame received while a handshake call was waiting for its answer.
struct DeferredFrame {
    line: Bytes,
    /// A provider question was already refused so the handshake could continue.
    question_answered: bool,
}

/// A live ACP session.
pub struct AcpAgent {
    provider: ProviderId,
    session: SessionId,
    native: String,
    child_guard: ChildGuard,
    child: TrackedChild,
    stdin: Option<ChildStdin>,
    lines: Lines<tokio::process::ChildStdout>,
    next_request: i64,
    prompt_request: Option<RequestId>,
    running: Option<TurnId>,
    next_turn: u32,
    interrupt_requested: bool,
    announced: VecDeque<Produced>,
    /// The mode identifiers this session announced at open, empty when it announced none.
    ///
    /// The gate for [`Self::switch_mode`]: measured, one agent answers `session/set_mode` with an empty
    /// success for any identifier while announcing no modes, so only announced identifiers are relayed.
    announced_mode_ids: Box<[Box<str>]>,
    /// Whether initialize announced `promptCapabilities.image: true`.
    ///
    /// The gate for sending an image block. Measured: cline announces true, grok announces false,
    /// and an unannounced capability reads as absent because absent support silently drops a prompt piece.
    accepts_images: bool,
    deferred: VecDeque<DeferredFrame>,
    deferred_bytes: usize,
    /// Replayed frames the bound let go of, oldest first, said once when the kept ones drain.
    deferred_dropped: usize,
    finished: bool,
}

impl AcpAgent {
    /// Start, initialize, and open one session with a manifest-declared ACP executable.
    ///
    /// # Errors
    ///
    /// Returns a named [`ProviderError`] when the manifest arguments are unsafe, the child cannot start, ACP v1
    /// negotiation fails, or the provider refuses the requested session operation.
    #[expect(
        clippy::too_many_lines,
        reason = "the linear handshake keeps child ownership, protocol negotiation, and session acknowledgement in failure order; splitting it would require partially initialized agent states"
    )]
    pub async fn start(
        provider: ProviderId,
        program: &Program,
        transport_argv: &[Box<str>],
        intent: &OpenIntent,
        contained_by: &Containment,
    ) -> Result<Self, ProviderError> {
        refuse_unsupported_open_overrides(provider, intent)?;

        let arguments: Vec<String> = transport_argv.iter().map(ToString::to_string).collect();
        let mut checked = program.leading().to_vec();
        checked.extend_from_slice(&arguments);
        runtrol_childproc::check_all(&checked).map_err(|error| ProviderError::Unsupported {
            provider,
            what: error.to_string(),
            why: "a manifest transport argument cannot be passed on a command line",
        })?;

        let mut command = TrackedCommand::new(program.path().as_std_path());
        command
            .args(program.leading())
            .args(&arguments)
            .current_dir(intent.workspace.as_std_path())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
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

        let mut agent = Self {
            provider,
            session: intent.session,
            native: String::new(),
            child,
            child_guard,
            stdin: Some(stdin),
            lines: Lines::new(stdout),
            next_request: 1,
            prompt_request: None,
            running: None,
            next_turn: 0,
            interrupt_requested: false,
            announced: VecDeque::with_capacity(2),
            announced_mode_ids: Box::new([]),
            accepts_images: false,
            deferred: VecDeque::new(),
            deferred_bytes: 0,
            deferred_dropped: 0,
            finished: false,
        };

        let initialized = agent
            .call(
                wire::INITIALIZE,
                &wire::Initialize {
                    protocol_version: 1,
                    client_capabilities: wire::Empty {},
                    client_info: wire::Implementation {
                        name: "runtrol",
                        version: env!("CARGO_PKG_VERSION"),
                    },
                },
                "initializing ACP",
            )
            .await?;
        let initialization: wire::Initialized =
            serde_json::from_slice(&initialized).map_err(|_| ProviderError::Protocol {
                provider,
                doing: "initializing ACP",
                detail: "the answer has no usable protocolVersion or agentCapabilities".to_owned(),
            })?;
        if initialization.protocol_version != 1 {
            return Err(ProviderError::Unsupported {
                provider,
                what: format!("ACP protocol version {}", initialization.protocol_version),
                why: "this build implements stable ACP v1",
            });
        }

        let can_load = initialization
            .agent_capabilities
            .get("loadSession")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        let caps = capabilities(provider, &initialization.agent_capabilities)?;
        agent.accepts_images = announces_image_support(&initialization.agent_capabilities);

        let opened = match &intent.disposition {
            Disposition::Fresh => {
                let cwd = intent.workspace.to_string();
                let answer = agent
                    .call(
                        wire::SESSION_NEW,
                        &wire::NewSession {
                            cwd: &cwd,
                            mcp_servers: [],
                        },
                        "creating an ACP session",
                    )
                    .await?;
                let result: wire::NewSessionResult<'_> =
                    serde_json::from_slice(&answer).map_err(|_| ProviderError::Protocol {
                        provider,
                        doing: "creating an ACP session",
                        detail: "the answer has no usable sessionId".to_owned(),
                    })?;
                let native = NativeSessionId::new(result.session_id).map_err(|error| {
                    ProviderError::Protocol {
                        provider,
                        doing: "creating an ACP session",
                        detail: format!("the session identifier is not usable: {error}"),
                    }
                })?;
                agent.native = result.session_id.to_owned();
                (answer, native)
            }
            Disposition::Resume { native } => {
                if !can_load {
                    return Err(ProviderError::Unsupported {
                        provider,
                        what: "resuming an ACP session".to_owned(),
                        why: "the agent did not announce the loadSession capability",
                    });
                }
                let cwd = intent.workspace.to_string();
                let answer = agent
                    .call(
                        wire::SESSION_LOAD,
                        &wire::LoadSession {
                            session_id: native,
                            cwd: &cwd,
                            mcp_servers: [],
                        },
                        "loading an ACP session",
                    )
                    .await?;
                let named =
                    NativeSessionId::new(native).map_err(|error| ProviderError::Protocol {
                        provider,
                        doing: "loading an ACP session",
                        detail: format!("the session identifier is not usable: {error}"),
                    })?;
                agent.native = native.to_string();
                (answer, named)
            }
            other => {
                return Err(ProviderError::Unsupported {
                    provider,
                    what: format!("{other:?}"),
                    why: "this ACP driver serves fresh and resumed sessions",
                });
            }
        };

        let payload = opaque_whole(&opened.0).ok_or_else(|| ProviderError::Protocol {
            provider,
            doing: "opening an ACP session",
            detail: "the provider answer is not a shareable JSON payload".to_owned(),
        })?;
        agent.announced.push_back(Produced {
            src_end: 0,
            body: EventBody::Attached(Box::new(Attached {
                native: opened.1,
                model_requested: None,
                reasoning_effort_requested: None,
                caps,
                payload,
            })),
        });
        if let Some(models) = announced_models(&opened.0) {
            agent.announced.push_back(Produced {
                src_end: 0,
                body: models,
            });
        }
        if let Some((modes, ids)) = announced_modes(&opened.0) {
            agent.announced_mode_ids = ids;
            agent.announced.push_back(Produced {
                src_end: 0,
                body: modes,
            });
        }
        Ok(agent)
    }

    /// Relay the operator's mode choice through the standard switch call, for announced modes only.
    ///
    /// The announcement is the gate (see the field), and the confirmation event repeats the announced set so
    /// a surface keeps its options.
    async fn switch_mode(&mut self, mode: Box<str>) -> Result<(), ProviderError> {
        if !self.announced_mode_ids.iter().any(|id| **id == *mode) {
            return Err(ProviderError::Unsupported {
                provider: self.provider,
                what: format!("switching to mode {mode:?}"),
                why: "this session announced no such mode, and an unannounced switch cannot be confirmed",
            });
        }
        let session = self.native.clone();
        let answer = self
            .call(
                wire::SESSION_SET_MODE,
                &wire::SetMode {
                    session_id: &session,
                    mode_id: &mode,
                },
                "switching the mode",
            )
            .await?;
        let payload = opaque_whole(&answer).ok_or_else(|| ProviderError::Protocol {
            provider: self.provider,
            doing: "switching the mode",
            detail: "the provider answer is not a shareable JSON payload".to_owned(),
        })?;
        self.announced.push_back(Produced {
            src_end: 0,
            body: EventBody::CurrentModeUpdate {
                mode_id: mode,
                available_ids: Some(self.announced_mode_ids.clone()),
                payload,
            },
        });
        Ok(())
    }

    /// Relay the operator's model choice through the agent's own switch call.
    ///
    /// The vendor-extension method rather than a probe: an agent that does not ship it answers
    /// method-not-found, and that refusal propagates as the loud error instead of being guessed around.
    async fn switch_model(
        &mut self,
        model: Box<str>,
        reasoning_effort: Option<Box<str>>,
    ) -> Result<(), ProviderError> {
        if reasoning_effort.is_some() {
            return Err(ProviderError::Unsupported {
                provider: self.provider,
                what: "switching the reasoning effort mid-session".to_owned(),
                why: "no ACP surface announces one to switch",
            });
        }
        let session = self.native.clone();
        let answer = self
            .call(
                wire::SESSION_SET_MODEL,
                &wire::SetModel {
                    session_id: &session,
                    model_id: &model,
                },
                "switching the model",
            )
            .await?;
        let payload = opaque_whole(&answer).ok_or_else(|| ProviderError::Protocol {
            provider: self.provider,
            doing: "switching the model",
            detail: "the provider answer is not a shareable JSON payload".to_owned(),
        })?;
        // The agent accepted, which is its word that the model moved; the event carries its answer.
        self.announced.push_back(Produced {
            src_end: 0,
            body: EventBody::CurrentModelUpdate {
                model_id: model,
                available_ids: None,
                payload,
            },
        });
        Ok(())
    }

    async fn call<P: serde::Serialize>(
        &mut self,
        method: &str,
        params: &P,
        doing: &'static str,
    ) -> Result<Bytes, ProviderError> {
        let id = self.issue();
        let line = jsonrpc::write_question(&id, method, params).map_err(|error| {
            ProviderError::Protocol {
                provider: self.provider,
                doing,
                detail: error.to_string(),
            }
        })?;
        self.write_line(&line).await?;
        self.wait_for(id, doing).await
    }

    async fn wait_for(
        &mut self,
        expected: RequestId,
        doing: &'static str,
    ) -> Result<Bytes, ProviderError> {
        loop {
            let line = self
                .read_line(doing)
                .await?
                .ok_or_else(|| ProviderError::Protocol {
                    provider: self.provider,
                    doing,
                    detail: "the provider stream ended before answering".to_owned(),
                })?;
            match jsonrpc::read(&line).map_err(|error| ProviderError::Protocol {
                provider: self.provider,
                doing,
                detail: error.to_string(),
            })? {
                Incoming::Answer { id, outcome } if id == expected => {
                    return outcome.map_err(|error| ProviderError::NativeRefused {
                        provider: self.provider,
                        doing,
                        detail: format!("{} ({})", error.message, error.code),
                    });
                }
                Incoming::Question { id, .. } => {
                    let refusal = jsonrpc::write_error(
                        &id,
                        -32601,
                        "runtrol does not serve this client method",
                    );
                    self.write_line(&refusal).await?;
                    self.defer(line, true, doing)?;
                }
                _ => self.defer(line, false, doing)?,
            }
        }
    }

    fn issue(&mut self) -> RequestId {
        let id = RequestId::Number(self.next_request);
        self.next_request = self.next_request.saturating_add(1);
        id
    }

    fn defer(
        &mut self,
        line: Bytes,
        question_answered: bool,
        doing: &'static str,
    ) -> Result<(), ProviderError> {
        if line.len() > MAX_DEFERRED_BYTES {
            // One frame larger than the whole tail budget is not history, it is a transport fault.
            return Err(ProviderError::Protocol {
                provider: self.provider,
                doing,
                detail: format!("a frame of {} bytes arrived before the answer", line.len()),
            });
        }
        // The tail is kept, the head is let go of and counted. A question the provider asked was already
        // refused on arrival, so dropping its frame loses nothing a subscriber could act on.
        while self.deferred.len() >= MAX_DEFERRED_FRAMES
            || self.deferred_bytes.saturating_add(line.len()) > MAX_DEFERRED_BYTES
        {
            let Some(oldest) = self.deferred.pop_front() else {
                break;
            };
            self.deferred_bytes = self.deferred_bytes.saturating_sub(oldest.line.len());
            self.deferred_dropped = self.deferred_dropped.saturating_add(1);
        }
        self.deferred_bytes = self.deferred_bytes.saturating_add(line.len());
        self.deferred.push_back(DeferredFrame {
            line,
            question_answered,
        });
        Ok(())
    }

    async fn write_line(&mut self, text: &str) -> Result<(), ProviderError> {
        let stdin = self.stdin.as_mut().ok_or_else(|| ProviderError::Protocol {
            provider: self.provider,
            doing: "sending an ACP frame",
            detail: "the session input has already been closed".to_owned(),
        })?;
        let mut framed = String::with_capacity(text.len() + 1);
        framed.push_str(text);
        framed.push('\n');
        stdin
            .write_all(framed.as_bytes())
            .await
            .map_err(|error| ProviderError::Io {
                provider: self.provider,
                doing: "writing an ACP frame",
                source: error,
            })?;
        stdin.flush().await.map_err(|error| ProviderError::Io {
            provider: self.provider,
            doing: "flushing an ACP frame",
            source: error,
        })
    }

    async fn read_line(&mut self, doing: &'static str) -> Result<Option<Bytes>, ProviderError> {
        self.lines
            .next()
            .await
            .map_err(|error| line_error(self.provider, doing, &error))
    }

    fn mint_turn(&mut self) -> TurnId {
        let turn = TurnId {
            epoch: 0,
            index: self.next_turn,
        };
        self.next_turn = self.next_turn.saturating_add(1);
        turn
    }

    async fn produce_line(
        &mut self,
        line: Bytes,
        question_answered: bool,
    ) -> Result<Produced, ProviderError> {
        let incoming = jsonrpc::read(&line).map_err(|error| ProviderError::Protocol {
            provider: self.provider,
            doing: "reading an ACP frame",
            detail: error.to_string(),
        })?;
        let body = match incoming {
            Incoming::Report { method, params } if &*method == wire::SESSION_UPDATE => {
                let params = params.ok_or_else(|| ProviderError::Protocol {
                    provider: self.provider,
                    doing: "reading an ACP session update",
                    detail: "the notification has no params".to_owned(),
                })?;
                map::update(&line, &params, &self.native).map_err(|error| {
                    ProviderError::Protocol {
                        provider: self.provider,
                        doing: "reading an ACP session update",
                        detail: error.to_string(),
                    }
                })?
            }
            Incoming::Report { method, .. } => {
                map::unmapped(&line, &method).map_err(|error| ProviderError::Protocol {
                    provider: self.provider,
                    doing: "carrying an unbound ACP notification",
                    detail: error.to_string(),
                })?
            }
            Incoming::Question { id, method, .. } => {
                if !question_answered {
                    let refusal = jsonrpc::write_error(
                        &id,
                        -32601,
                        "runtrol does not serve this client method",
                    );
                    self.write_line(&refusal).await?;
                }
                map::unmapped(&line, &method).map_err(|error| ProviderError::Protocol {
                    provider: self.provider,
                    doing: "carrying an unbound ACP request",
                    detail: error.to_string(),
                })?
            }
            Incoming::Answer { id, outcome } if self.prompt_request.as_ref() == Some(&id) => {
                self.prompt_request = None;
                let turn = self.running.take().unwrap_or_else(|| self.mint_turn());
                let answer = outcome.map_err(|error| ProviderError::NativeRefused {
                    provider: self.provider,
                    doing: "running an ACP prompt",
                    detail: format!("{} ({})", error.message, error.code),
                })?;
                let result: wire::PromptResult<'_> =
                    serde_json::from_slice(&answer).map_err(|_| ProviderError::Protocol {
                        provider: self.provider,
                        doing: "finishing an ACP prompt",
                        detail: "the answer has no usable stopReason".to_owned(),
                    })?;
                let stop = stop_reason(result.stop_reason);
                let declared_by = if self.interrupt_requested && stop == StopReason::Cancelled {
                    Declarant::InterruptAcked
                } else {
                    Declarant::Provider
                };
                self.interrupt_requested = false;
                EventBody::Turn(TurnEvent::Ended {
                    turn,
                    stop,
                    declared_by,
                })
            }
            Incoming::Answer { .. } => {
                map::unmapped(&line, "jsonrpc/response").map_err(|error| {
                    ProviderError::Protocol {
                        provider: self.provider,
                        doing: "carrying an unbound ACP response",
                        detail: error.to_string(),
                    }
                })?
            }
        };
        Ok(Produced { src_end: 0, body })
    }
}

fn spawn_error(provider: ProviderId, program: &Program, error: SpawnError) -> ProviderError {
    ProviderError::Spawn {
        provider,
        program: program.path().to_string(),
        source: std::io::Error::other(error),
    }
}

#[async_trait]
impl Agent for AcpAgent {
    fn session(&self) -> SessionId {
        self.session
    }

    fn native(&self) -> Option<&str> {
        Some(&self.native)
    }

    async fn send(&mut self, command: AgentCommand) -> Result<(), ProviderError> {
        match command {
            AgentCommand::Prompt(blocks) => {
                if self.running.is_some() {
                    return Err(ProviderError::Unsupported {
                        provider: self.provider,
                        what: "a second prompt while one is running".to_owned(),
                        why: "ACP permits one active prompt per session",
                    });
                }
                let prompt = prompt_blocks(self.provider, self.accepts_images, &blocks)?;
                let id = self.issue();
                let frame = jsonrpc::write_question(
                    &id,
                    wire::SESSION_PROMPT,
                    &wire::Prompt {
                        session_id: &self.native,
                        prompt,
                    },
                )
                .map_err(|error| ProviderError::Protocol {
                    provider: self.provider,
                    doing: "building an ACP prompt",
                    detail: error.to_string(),
                })?;
                let turn = self.mint_turn();
                self.write_line(&frame).await?;
                self.prompt_request = Some(id);
                self.running = Some(turn);
                self.announced.push_back(Produced {
                    src_end: 0,
                    body: EventBody::Turn(TurnEvent::Started { turn }),
                });
                Ok(())
            }
            AgentCommand::Interrupt => {
                let frame = jsonrpc::write_report(
                    wire::SESSION_CANCEL,
                    &wire::Cancel {
                        session_id: &self.native,
                    },
                )
                .map_err(|error| ProviderError::Protocol {
                    provider: self.provider,
                    doing: "building an ACP cancellation",
                    detail: error.to_string(),
                })?;
                self.write_line(&frame).await?;
                self.interrupt_requested = true;
                Ok(())
            }
            AgentCommand::Native(payload) => self.write_line(payload.as_str()).await,
            AgentCommand::SetMode { mode } => self.switch_mode(mode).await,

            AgentCommand::SetModel {
                model,
                reasoning_effort,
            } => self.switch_model(model, reasoning_effort).await,
            AgentCommand::Answer { .. } => Err(ProviderError::Unsupported {
                provider: self.provider,
                what: "answering an ACP client request".to_owned(),
                why: "this driver defaults unbound client methods to deny",
            }),
            other => Err(ProviderError::Unsupported {
                provider: self.provider,
                what: format!("{other:?}"),
                why: "this ACP driver has no binding for that command",
            }),
        }
    }

    async fn next(&mut self) -> Option<Result<Produced, ProviderError>> {
        if let Some(announced) = self.announced.pop_front() {
            return Some(Ok(announced));
        }
        if self.finished {
            return None;
        }
        if self.deferred_dropped > 0 {
            // Said once, before the kept tail: the reader sees where the replay starts and knows the
            // provider keeps the rest. A count and a sentence, never the dropped content.
            let dropped = self.deferred_dropped;
            self.deferred_dropped = 0;
            return Some(Ok(Produced {
                src_end: 0,
                body: EventBody::Notice(Box::new(Notice {
                    level: Level::Info,
                    code: NoticeCode::Other,
                    retryable: false,
                    payload: Opaque::owned(format!(
                        r#"{{"message":"{dropped} earlier updates replayed by this coding service were left out; it keeps the whole conversation"}}"#
                    )),
                })),
            }));
        }
        let line = if let Some(deferred) = self.deferred.pop_front() {
            self.deferred_bytes = self.deferred_bytes.saturating_sub(deferred.line.len());
            Some(Ok((deferred.line, deferred.question_answered)))
        } else {
            match self.lines.next().await {
                Ok(Some(line)) => Some(Ok((line, false))),
                Ok(None) => None,
                Err(error) => Some(Err(line_error(
                    self.provider,
                    "reading an ACP frame",
                    &error,
                ))),
            }
        };

        match line {
            Some(Ok((line, question_answered))) => {
                let produced = self.produce_line(line, question_answered).await;
                if produced.is_err() {
                    self.finished = true;
                }
                Some(produced)
            }
            Some(Err(error)) => {
                self.finished = true;
                Some(Err(error))
            }
            None => {
                self.finished = true;
                self.running.take().map(|turn| {
                    Ok(Produced {
                        src_end: 0,
                        body: EventBody::Turn(TurnEvent::Ended {
                            turn,
                            stop: StopReason::Unknown,
                            declared_by: Declarant::ProcessExit,
                        }),
                    })
                })
            }
        }
    }

    fn pid(&self) -> Option<u32> {
        self.child.id()
    }

    async fn close(mut self: Box<Self>, how: CloseMode) -> Result<(), ProviderError> {
        drop(self.stdin.take());
        let grace = match how {
            CloseMode::Graceful { grace_ms } => Duration::from_millis(grace_ms),
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
                            doing: "completing ACP process containment",
                            source: std::io::Error::other(error),
                        });
                }
                Ok(Err(wait_error)) => {
                    return match self.child_guard.terminate(&mut self.child).await {
                        Ok(()) => Err(ProviderError::Io {
                            provider: self.provider,
                            doing: "waiting for an ACP session to stop",
                            source: wait_error,
                        }),
                        Err(cleanup) => Err(ProviderError::Io {
                            provider: self.provider,
                            doing: "waiting for and cleaning up an ACP session",
                            source: std::io::Error::other(format!(
                                "wait failed: {wait_error}; cleanup also failed: {cleanup}"
                            )),
                        }),
                    };
                }
                Err(_elapsed) => {}
            }
        }
        self.child_guard
            .terminate(&mut self.child)
            .await
            .map_err(|error| ProviderError::Io {
                provider: self.provider,
                doing: "stopping an ACP session and its process group",
                source: std::io::Error::other(error),
            })
    }
}

fn capability_is_present(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Null | serde_json::Value::Bool(false) => false,
        serde_json::Value::Array(items) => !items.is_empty(),
        serde_json::Value::Object(fields) => !fields.is_empty(),
        _ => true,
    }
}

fn capabilities(
    provider: ProviderId,
    values: &serde_json::Map<String, serde_json::Value>,
) -> Result<CapabilitySet, ProviderError> {
    if values.len() > MAX_CAPABILITIES {
        return Err(ProviderError::Protocol {
            provider,
            doing: "reading ACP capabilities",
            detail: format!(
                "the agent announced {} top-level capabilities, over the {MAX_CAPABILITIES} limit",
                values.len()
            ),
        });
    }
    let mut tokens = Vec::new();
    for (name, value) in values {
        if name.len() > MAX_CAPABILITY_BYTES || name.chars().any(char::is_control) {
            return Err(ProviderError::Protocol {
                provider,
                doing: "reading ACP capabilities",
                detail: "a capability name is too long or contains a control character".to_owned(),
            });
        }
        if capability_is_present(value) {
            tokens.push(name.as_str());
        }
    }
    Ok(CapabilitySet::from_tokens(tokens))
}

fn stop_reason(reason: &str) -> StopReason {
    match reason {
        "end_turn" => StopReason::EndTurn,
        "max_tokens" => StopReason::MaxTokens,
        "max_turn_requests" => StopReason::MaxTurnRequests,
        "refusal" => StopReason::Refusal,
        "cancelled" => StopReason::Cancelled,
        _ => StopReason::Unknown,
    }
}

/// Bounds on the lifted model identifiers, so a hostile catalogue cannot grow the bounded event ring.
const MAX_ANNOUNCED_MODELS: usize = 32;
const MAX_MODEL_ID_BYTES: usize = 200;
/// Modes share the model bounds: both are short vendor identifiers, and one cap keeps hostile
/// announcements from growing state in either place.
const MAX_ANNOUNCED_MODES: usize = 32;
const MAX_MODE_ID_BYTES: usize = 200;

/// The model state a session answer announces, when it announces one.
///
/// A vendor extension rather than the standard (measured 2026-08-19: the ACP schema has no model vocabulary),
/// so absence is the normal case and nothing here fails on it. Identifiers are the only thing lifted, bounded
/// in count and length; the whole announcement rides as the payload.
fn announced_models(answer: &Bytes) -> Option<EventBody> {
    let read: wire::ModelsAnnounced<'_> = match serde_json::from_slice(answer) {
        Ok(read) => read,
        // ok: an answer that does not parse as this vendor extension simply has no announcement. Session
        // creation already validated the same answer for everything that is actually required.
        Err(_) => return None,
    };
    let models = read.models?;
    if models.current_model_id.is_empty() || models.current_model_id.len() > MAX_MODEL_ID_BYTES {
        return None;
    }
    let available: Vec<Box<str>> = models
        .available_models
        .iter()
        .map(|model| model.model_id)
        .filter(|id| !id.is_empty() && id.len() <= MAX_MODEL_ID_BYTES)
        .take(MAX_ANNOUNCED_MODELS)
        .map(Into::into)
        .collect();
    Some(EventBody::CurrentModelUpdate {
        model_id: models.current_model_id.into(),
        available_ids: (!available.is_empty()).then(|| available.into_boxed_slice()),
        payload: opaque_whole(answer)?,
    })
}

/// The mode set a session answer announces, as the event plus the bare identifiers for the gate.
///
/// This one is the ACP standard shape (`modes: {currentModeId, availableModes: [{id, ...}]}`), read just as
/// leniently as the model announcement: `modes: null` is a measured, normal answer and means no surface.
fn announced_modes(answer: &Bytes) -> Option<(EventBody, Box<[Box<str>]>)> {
    let read: wire::ModesAnnounced<'_> = match serde_json::from_slice(answer) {
        Ok(read) => read,
        // ok: an answer that does not carry the standard mode state simply has no announcement. Session
        // creation already validated the same answer for everything that is actually required.
        Err(_) => return None,
    };
    let modes = read.modes?;
    if modes.current_mode_id.is_empty() || modes.current_mode_id.len() > MAX_MODE_ID_BYTES {
        return None;
    }
    let available: Vec<Box<str>> = modes
        .available_modes
        .iter()
        .map(|mode| mode.id)
        .filter(|id| !id.is_empty() && id.len() <= MAX_MODE_ID_BYTES)
        .take(MAX_ANNOUNCED_MODES)
        .map(Into::into)
        .collect();
    let ids: Box<[Box<str>]> = available.into_boxed_slice();
    let event = EventBody::CurrentModeUpdate {
        mode_id: modes.current_mode_id.into(),
        available_ids: (!ids.is_empty()).then(|| ids.clone()),
        payload: opaque_whole(answer)?,
    };
    Some((event, ids))
}

fn opaque_whole(bytes: &Bytes) -> Option<Opaque> {
    match core::str::from_utf8(bytes) {
        Ok(text) => Opaque::borrowed_from(bytes, text),
        Err(_) => None,
    }
}

fn line_error(provider: ProviderId, doing: &'static str, error: &LineError) -> ProviderError {
    ProviderError::Protocol {
        provider,
        doing,
        detail: error.to_string(),
    }
}

/// Refuse open-time overrides ACP v1 cannot carry, before any process is spawned.
///
/// `session/new` and `session/load` have no field for a model, a reasoning effort, or a permission
/// mode. A silently dropped override would open a session that quietly ignores the operator's
/// explicit choice (the reasoning effort did exactly that until 2026-08-20), so every override is
/// refused loudly here.
/// The operator's blocks as ACP prompt content, or a loud refusal for a piece that cannot travel.
///
/// The image gate is the agent's own initialize announcement, because an agent that said
/// `promptCapabilities.image: false` (measured: grok) would take the prompt with a piece silently
/// missing, and a prompt missing one of its parts is a prompt the operator did not write.
fn prompt_blocks(
    provider: ProviderId,
    accepts_images: bool,
    blocks: &[ContentBlock],
) -> Result<Vec<wire::PromptBlock<'_>>, ProviderError> {
    let mut prompt = Vec::with_capacity(blocks.len());
    for block in blocks {
        match block {
            ContentBlock::Text(text) => {
                prompt.push(wire::PromptBlock::Text(wire::TextBlock {
                    type_: "text",
                    text,
                }));
            }
            ContentBlock::Image { media_type, base64 } => {
                if !accepts_images {
                    return Err(ProviderError::Unsupported {
                        provider,
                        what: "an image attachment".to_owned(),
                        why: "this agent announced no image support at initialize",
                    });
                }
                prompt.push(wire::PromptBlock::Image(wire::ImageBlock {
                    type_: "image",
                    mime_type: media_type,
                    data: base64,
                }));
            }
            ContentBlock::Native(payload) => {
                prompt.push(wire::PromptBlock::Native(payload));
            }
            other => {
                return Err(ProviderError::Unsupported {
                    provider,
                    what: format!("{other:?}"),
                    why: "this ACP v1 binding can send text, images, and native content blocks",
                });
            }
        }
    }
    Ok(prompt)
}

/// Whether the initialize answer announced `promptCapabilities.image: true`.
///
/// Anything else, including absence, reads as no: sending an image an agent never claimed to take
/// would drop a piece of the prompt silently (measured: grok announces false, cline true).
fn announces_image_support(values: &serde_json::Map<String, serde_json::Value>) -> bool {
    values
        .get("promptCapabilities")
        .and_then(|caps| caps.get("image"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

fn refuse_unsupported_open_overrides(
    provider: ProviderId,
    intent: &OpenIntent,
) -> Result<(), ProviderError> {
    if intent.model.is_some() || intent.reasoning_effort.is_some() || intent.permission.is_some() {
        return Err(ProviderError::Unsupported {
            provider,
            what: "a model, reasoning effort, or permission override at session creation"
                .to_owned(),
            why: "ACP v1 carries none of these fields in session/new or session/load",
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use runtrol_provider::AbsPath;

    use super::*;

    fn an_intent() -> OpenIntent {
        OpenIntent {
            session: SessionId::now(),
            workspace: AbsPath::new(if cfg!(windows) { r"C:\work" } else { "/work" })
                .expect("valid"),
            disposition: Disposition::Fresh,
            model: None,
            reasoning_effort: None,
            permission: None,
        }
    }

    #[test]
    fn image_support_is_read_from_the_initialize_announcement_and_absence_reads_as_no() {
        // Measured 2026-08-20: cline announces {"promptCapabilities": {"image": true}}, grok
        // announces false. Absence is no, because absent support silently drops a prompt piece.
        let says = |json: &str| -> bool {
            let values: serde_json::Map<String, serde_json::Value> =
                serde_json::from_str(json).expect("a readable capability object");
            announces_image_support(&values)
        };
        assert!(says(
            r#"{"promptCapabilities":{"image":true,"audio":false}}"#
        ));
        assert!(!says(r#"{"promptCapabilities":{"image":false}}"#));
        assert!(!says(r#"{"promptCapabilities":{}}"#));
        assert!(!says("{}"));
        assert!(
            !says(r#"{"promptCapabilities":{"image":"yes"}}"#),
            "a non-bool is not a yes"
        );
    }

    #[test]
    fn every_open_override_is_refused_loudly_and_none_passes() {
        // Regression: the guard once checked only model and permission, so a reasoning effort was
        // silently dropped and the session opened as if the operator had chosen nothing.
        let provider = ProviderId::parse("acp-test").expect("the test's own id must be valid");
        let plain = an_intent();
        assert!(refuse_unsupported_open_overrides(provider, &plain).is_ok());

        let mut with_model = an_intent();
        with_model.model = Some("m".into());
        let mut with_effort = an_intent();
        with_effort.reasoning_effort = Some("high".into());
        let mut with_permission = an_intent();
        with_permission.permission = Some("plan".into());
        for overridden in [with_model, with_effort, with_permission] {
            assert!(
                matches!(
                    refuse_unsupported_open_overrides(provider, &overridden),
                    Err(ProviderError::Unsupported { .. })
                ),
                "an open-time override must be refused, not dropped",
            );
        }
    }

    #[test]
    fn an_announced_mode_state_becomes_the_event_and_a_null_announcement_becomes_nothing() {
        // The standard shape (session/new `modes` with currentModeId + availableModes[].id). The null case is
        // the measured one: grok 1.0.4 answers `"modes": null` (2026-08-19 probe), and the same probe showed
        // its set_mode accepting nonsense with an empty success, which is why absence must gate the switch.
        let announced = Bytes::from_static(
            br#"{"sessionId":"s","modes":{"currentModeId":"default","availableModes":[{"id":"default","name":"Default"},{"id":"plan","name":"Plan"}]}}"#,
        );
        let Some((
            EventBody::CurrentModeUpdate {
                mode_id,
                available_ids,
                ..
            },
            ids,
        )) = announced_modes(&announced)
        else {
            panic!("a standard announcement did not become the event");
        };
        assert_eq!(mode_id.as_ref(), "default");
        assert_eq!(
            available_ids.as_deref().map(<[Box<str>]>::len),
            Some(2),
            "the event carries the switchable set"
        );
        assert_eq!(ids.len(), 2, "the gate holds the same set");

        let null_modes = Bytes::from_static(
            br#"{"sessionId":"s","models":{"currentModelId":"m"},"modes":null}"#,
        );
        assert!(
            announced_modes(&null_modes).is_none(),
            "the measured null announcement means no mode surface"
        );
    }

    #[test]
    fn an_announced_model_state_becomes_the_event_and_absence_becomes_nothing() {
        // The shape is the real one: grok 1.0.4's session/new answer, trimmed to the fields read
        // (measured 2026-08-19). The vendor nests reasoning efforts in _meta; only identifiers are lifted.
        let announced = Bytes::from_static(
            br#"{"sessionId":"01a018c1","models":{"currentModelId":"grok-4.6","availableModels":[{"modelId":"grok-4.6","name":"Grok 4.6"},{"modelId":"grok-4.5","name":"Grok 4.5"}]}}"#,
        );
        let Some(EventBody::CurrentModelUpdate {
            model_id,
            available_ids,
            ..
        }) = announced_models(&announced)
        else {
            panic!("a real announcement did not become the event");
        };
        assert_eq!(model_id.as_ref(), "grok-4.6");
        let ids: Vec<&str> = available_ids
            .as_deref()
            .expect("the announced set is lifted")
            .iter()
            .map(AsRef::as_ref)
            .collect();
        assert_eq!(ids, ["grok-4.6", "grok-4.5"]);

        // The standard's own answer carries no models at all, and that is the normal case, not an error.
        let plain = Bytes::from_static(br#"{"sessionId":"s-1"}"#);
        assert!(announced_models(&plain).is_none());
    }

    #[test]
    fn a_hostile_catalogue_cannot_grow_past_the_bounds() {
        use core::fmt::Write as _;
        let mut hostile =
            String::from(r#"{"sessionId":"s","models":{"currentModelId":"m","availableModels":["#);
        for index in 0..100 {
            if index > 0 {
                hostile.push(',');
            }
            write!(hostile, r#"{{"modelId":"model-{index}"}}"#).expect("writing into a String");
        }
        hostile.push_str("]}}");
        let bytes = Bytes::from(hostile);
        let Some(EventBody::CurrentModelUpdate { available_ids, .. }) = announced_models(&bytes)
        else {
            panic!("the announcement did not parse");
        };
        assert_eq!(
            available_ids.map(|ids| ids.len()),
            Some(MAX_ANNOUNCED_MODELS),
            "the lifted set is capped, whatever the provider sends"
        );
    }

    #[test]
    fn stable_stop_reasons_cross_without_reinterpretation() {
        assert_eq!(stop_reason("end_turn"), StopReason::EndTurn);
        assert_eq!(stop_reason("max_tokens"), StopReason::MaxTokens);
        assert_eq!(
            stop_reason("max_turn_requests"),
            StopReason::MaxTurnRequests
        );
        assert_eq!(stop_reason("refusal"), StopReason::Refusal);
        assert_eq!(stop_reason("cancelled"), StopReason::Cancelled);
        assert_eq!(stop_reason("something_new"), StopReason::Unknown);
    }

    #[test]
    fn false_and_empty_capabilities_are_not_announced() {
        assert!(!capability_is_present(&serde_json::Value::Null));
        assert!(!capability_is_present(&serde_json::Value::Bool(false)));
        assert!(!capability_is_present(&serde_json::json!({})));
        assert!(capability_is_present(&serde_json::Value::Bool(true)));
        assert!(capability_is_present(&serde_json::json!({"image": true})));
    }
}
