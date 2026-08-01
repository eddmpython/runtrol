//! One ACP process per session, transported over NDJSON standard streams.

use core::time::Duration;
use std::collections::VecDeque;

use async_trait::async_trait;
use bytes::Bytes;
use runtrol_childproc::{Containment, Program};
use runtrol_provider::{
    Agent, AgentCommand, Attached, CapabilitySet, CloseMode, ContentBlock, Declarant, Disposition,
    EventBody, NativeSessionId, Opaque, OpenIntent, Produced, ProviderError, ProviderId,
    ReplaySource, SessionId, StopReason, TurnEvent, TurnId,
};
use tokio::io::AsyncWriteExt as _;
use tokio::process::{Child, ChildStdin};

use crate::acp::{map, wire};
use crate::framing::jsonrpc;
use crate::framing::{Incoming, LineError, Lines, RequestId};

const MAX_DEFERRED_FRAMES: usize = 16;
const MAX_DEFERRED_BYTES: usize = 128 * 1024;
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
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Lines<tokio::process::ChildStdout>,
    next_request: i64,
    prompt_request: Option<RequestId>,
    running: Option<TurnId>,
    next_turn: u32,
    interrupt_requested: bool,
    announced: VecDeque<Produced>,
    deferred: VecDeque<DeferredFrame>,
    deferred_bytes: usize,
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
        if intent.model.is_some() || intent.permission.is_some() {
            return Err(ProviderError::Unsupported {
                provider,
                what: "a model or permission override at session creation".to_owned(),
                why: "ACP v1 does not carry either field in session/new or session/load",
            });
        }

        let arguments: Vec<String> = transport_argv.iter().map(ToString::to_string).collect();
        let mut checked = program.leading().to_vec();
        checked.extend_from_slice(&arguments);
        runtrol_childproc::check_all(&checked).map_err(|error| ProviderError::Unsupported {
            provider,
            what: error.to_string(),
            why: "a manifest transport argument cannot be passed on a command line",
        })?;

        let mut command = tokio::process::Command::new(program.path().as_std_path());
        command
            .args(program.leading())
            .args(&arguments)
            .current_dir(intent.workspace.as_std_path())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit())
            .kill_on_drop(true);
        runtrol_childproc::hide_console_window(command.as_std_mut());
        contained_by.prepare(command.as_std_mut());

        let mut child = command.spawn().map_err(|source| ProviderError::Spawn {
            provider,
            program: program.path().to_string(),
            source,
        })?;
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
            stdin: Some(stdin),
            lines: Lines::new(stdout),
            next_request: 1,
            prompt_request: None,
            running: None,
            next_turn: 0,
            interrupt_requested: false,
            announced: VecDeque::with_capacity(2),
            deferred: VecDeque::new(),
            deferred_bytes: 0,
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
                replay: ReplaySource::None,
                model_requested: None,
                caps,
                payload,
            })),
        });
        Ok(agent)
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
        if self.deferred.len() >= MAX_DEFERRED_FRAMES
            || self.deferred_bytes.saturating_add(line.len()) > MAX_DEFERRED_BYTES
        {
            return Err(ProviderError::Protocol {
                provider: self.provider,
                doing,
                detail: format!(
                    "more than {MAX_DEFERRED_FRAMES} frames or {MAX_DEFERRED_BYTES} bytes arrived before the answer"
                ),
            });
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
                let mut prompt = Vec::with_capacity(blocks.len());
                for block in &blocks {
                    match block {
                        ContentBlock::Text(text) => {
                            prompt.push(wire::PromptBlock::Text(wire::TextBlock {
                                type_: "text",
                                text,
                            }));
                        }
                        ContentBlock::Native(payload) => {
                            prompt.push(wire::PromptBlock::Native(payload));
                        }
                        other => {
                            return Err(ProviderError::Unsupported {
                                provider: self.provider,
                                what: format!("{other:?}"),
                                why: "this ACP v1 binding can send text and native content blocks",
                            });
                        }
                    }
                }
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

    async fn close(mut self: Box<Self>, how: CloseMode) -> Result<(), ProviderError> {
        drop(self.stdin.take());
        let grace = match how {
            CloseMode::Graceful { grace_ms } => Duration::from_millis(grace_ms),
            _ => Duration::ZERO,
        };
        if !grace.is_zero()
            && let Ok(Ok(_status)) = tokio::time::timeout(grace, self.child.wait()).await
        {
            return Ok(());
        }
        self.child.kill().await.map_err(|error| ProviderError::Io {
            provider: self.provider,
            doing: "stopping an ACP session",
            source: error,
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

#[cfg(test)]
mod tests {
    use super::*;

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
