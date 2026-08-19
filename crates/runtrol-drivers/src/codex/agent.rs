//! One conversation on a shared daemon.
//!
//! # What this owns that the mapping cannot
//!
//! Which turn is running. The provider names its turns with its own strings and runtrol numbers them, so the
//! pairing lives here, in the thing that sent the prompt. The mapping is pure and hands back the provider's
//! string; this is what turns it into a turn number.
//!
//! # A turn begins with an acknowledgement, and that is not a beginning
//!
//! Measured: `turn/start` answers in **two milliseconds** with a turn that is in progress and carries no work,
//! and the turn then runs for eight seconds. A probe that read that answer as the result reported the turn as
//! finished instantly. So the answer produces [`TurnEvent::Accepted`], the notification produces
//! [`TurnEvent::Started`], and only [`crate::codex::bound::TERMINAL`] ends anything.
//!
//! # Closing a session does not delete the conversation
//!
//! The provider owns the durable conversation and its native resume surface. runtrol retains only the native
//! identifier needed to ask that surface. Closing here means runtrol stops following the live stream; it neither
//! deletes nor reads the provider's transcript.

use core::time::Duration;
use std::collections::VecDeque;
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use runtrol_provider::{
    Agent, AgentCommand, ApprovalId, ApprovalRequest, Attached, CapabilitySet, CloseMode,
    ContentBlock, Declarant, Disposition, EventBody, Level, NativeSessionId, Notice, NoticeCode,
    Opaque, OpenIntent, Produced, ProviderError, ProviderId, SessionId, StopReason, TurnEvent,
    TurnId, WallMs, WithdrawnReason,
};
use serde::Deserialize;
use serde_json::value::RawValue;

use crate::codex::approval::{ApprovalBook, ApprovalBuildError};
use crate::codex::bound::{Answer, DECLINE_RESULT, INTERRUPT};
use crate::codex::conn::{Connection, Delivery, Inbox};
use crate::codex::map::{self, Frame};

/// The turn that is running.
#[derive(Clone, Debug)]
struct Running {
    /// runtrol's number for it.
    turn: TurnId,
    /// The provider's own name for it, which is what an interrupt has to quote.
    native: Box<str>,
}

/// One conversation, driven over a connection it shares with every other session.
pub struct CodexAgent {
    /// Which provider this is.
    provider: ProviderId,
    /// runtrol's own name for the session.
    session: SessionId,
    /// The provider's name for the conversation.
    native: String,
    /// The connection.
    ///
    /// Held rather than borrowed, because holding it is what keeps the daemon alive. When the last session
    /// lets go, the process stops.
    conn: Arc<Connection>,
    /// This conversation's frames.
    inbox: Inbox,
    /// The turn that is running, if one is.
    running: Option<Running>,
    /// Which turn number to use next.
    next_turn: u32,
    /// The monotone source boundary in this live provider stream.
    ///
    /// A completed item or provider-declared turn advances it. Fragments and control events carry the current
    /// value. This is separate from the stream, epoch, and sequence in a subscriber's `WatchCursor`.
    src_end: u64,
    /// Events runtrol itself produced, waiting to be handed over.
    ///
    /// Bounded by construction: what goes in is an attach, or a turn acknowledgement plus at most one model
    /// confirmation from the same receipt, and none of those can happen twice without the queue being drained
    /// in between. Provider conversation content never enters it.
    announced: VecDeque<Produced>,
    /// The model and effort the operator chose for the turns from here on, waiting for the next turn to carry
    /// them. This CLI's switch surface is the turn itself: `turn/start` documents both fields as overriding
    /// "this turn and subsequent turns", so the driver holds the words until it has a turn to put them on.
    pending_model: Option<Box<str>>,
    pending_effort: Option<Box<str>>,
    /// How many lines the connection had failed to read when this session last said so.
    said_unreadable: u64,
    /// Provider questions still waiting for a human, plus the bounded file payload join.
    approvals: ApprovalBook,
    /// Set once the stream is over, so nothing keeps reading a finished session.
    finished: bool,
}

/// The fields runtrol reads out of the answer that opens a conversation.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Opened<'line> {
    /// The conversation.
    #[serde(default, borrow)]
    thread: Option<ThreadRef<'line>>,
}

/// The one field runtrol reads out of a conversation.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ThreadRef<'line> {
    /// The provider's own identifier.
    #[serde(default)]
    id: Option<&'line str>,
}

/// The fields runtrol reads out of the answer that submits a turn.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Submitted<'line> {
    /// The turn.
    #[serde(default, borrow)]
    turn: Option<TurnRef<'line>>,
}

/// What the acknowledgement says about the turn.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TurnRef<'line> {
    /// The provider's own identifier.
    #[serde(default)]
    id: Option<&'line str>,
    /// What it carries, which measured is nothing at all.
    #[serde(default, borrow)]
    items: Option<Vec<&'line RawValue>>,
}

impl CodexAgent {
    /// Open a conversation on the connection and follow it.
    ///
    /// # Errors
    ///
    /// Whatever [`Connection::call`] returns, plus [`ProviderError::Protocol`] when the answer names no
    /// conversation and [`ProviderError::Unsupported`] for a way of opening one this driver does not serve.
    pub async fn start(
        conn: Arc<Connection>,
        provider: ProviderId,
        intent: &OpenIntent,
    ) -> Result<Self, ProviderError> {
        let (method, params, doing) = open_call(provider, intent)?;
        let answer = conn.call(&method, &params, doing).await?;

        let native = thread_of(&answer).ok_or_else(|| ProviderError::Protocol {
            provider,
            doing,
            detail: "the answer named no conversation, so there is nothing to follow".to_owned(),
        })?;
        let named = NativeSessionId::new(&native).map_err(|error| ProviderError::Protocol {
            provider,
            doing,
            detail: format!("the conversation's identifier is not usable: {error}"),
        })?;

        // Registered from the answer that produced the identifier. Nothing addressed to this conversation can
        // have preceded it: every turn notification names a turn that exists only because runtrol asked, and
        // runtrol can only ask with the identifier this answer just carried.
        let inbox = conn.register(&native).await;

        Ok(Self {
            provider,
            session: intent.session,
            native: native.clone(),
            conn,
            inbox,
            running: None,
            next_turn: 0,
            src_end: 0,
            announced: VecDeque::from([Produced {
                src_end: 0,
                body: EventBody::Attached(Box::new(Attached {
                    native: named,
                    model_requested: intent.model.clone(),
                    reasoning_effort_requested: intent.reasoning_effort.clone(),
                    // This CLI declares what it can do once per connection rather than once per conversation,
                    // and a capability list copied onto every session would be the same fact repeated with
                    // nothing keeping the copies true.
                    caps: CapabilitySet::from_tokens(Vec::<&str>::new()),
                    payload: payload_of(&answer),
                })),
            }]),
            pending_model: None,
            pending_effort: None,
            said_unreadable: 0,
            approvals: ApprovalBook::new(),
            finished: false,
        })
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

    /// Turn a classified notification into what leaves the driver.
    fn produce(&mut self, frame: Frame) -> Produced {
        match frame {
            Frame::Started { native_turn } => {
                let turn = self.turn_for(&native_turn);
                Produced {
                    src_end: self.src_end,
                    body: EventBody::Turn(TurnEvent::Started { turn }),
                }
            }

            Frame::Ended(ended) => {
                self.approvals.clear_items();
                let turn = self.turn_for(&ended.native_turn);
                // The provider's own word, which is the only thing that means the outcome is known.
                if self
                    .running
                    .as_ref()
                    .is_some_and(|running| running.native == ended.native_turn)
                {
                    self.running = None;
                }
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

            Frame::Unbound(unmapped) => Produced {
                src_end: self.src_end,
                body: EventBody::Unmapped(unmapped),
            },
        }
    }

    /// Which of runtrol's turn numbers the provider's name for a turn belongs to.
    ///
    /// The running turn when the names match, and a fresh number otherwise. A frame about a turn this session
    /// never submitted still belongs to a turn, and giving it the running one would attach somebody else's
    /// ending to the work in progress.
    fn turn_for(&mut self, native: &str) -> TurnId {
        match &self.running {
            Some(running) if &*running.native == native => running.turn,
            _ => self.mint_turn(),
        }
    }

    /// What runtrol has to say about its own accounting, if anything.
    ///
    /// Two things can go wrong that are runtrol's to report rather than the provider's: this session's queue
    /// overflowed, or the connection could not read a line. Neither may pass in silence, because both mean a
    /// subscriber is missing output it will never be told about otherwise.
    fn own_report(&mut self) -> Option<Produced> {
        let dropped = self.inbox.dropped();
        let unreadable = self.conn.unreadable();
        let unread_now = unreadable.saturating_sub(self.said_unreadable);
        if dropped == 0 && unread_now == 0 {
            return None;
        }
        self.said_unreadable = unreadable;

        // runtrol's own frame, so its payload is runtrol's own words and carries nothing of the provider's.
        let payload = Opaque::owned(format!(
            r#"{{"droppedByRuntrol":{dropped},"unreadableLines":{unread_now}}}"#
        ));
        Some(Produced {
            src_end: self.src_end,
            body: EventBody::Notice(Box::new(Notice {
                level: Level::Warn,
                code: if unread_now > 0 {
                    // The provider wrote something on a protocol stream that is not a protocol frame.
                    NoticeCode::ProtocolViolation
                } else {
                    // runtrol could not keep up with its own queue. Nothing about the provider is wrong, and
                    // no existing code describes runtrol's own backlog, so the honest one is the catch-all
                    // rather than a code that would send a reader looking at the CLI.
                    NoticeCode::Other
                },
                retryable: false,
                payload,
            })),
        })
    }

    /// What to report when the connection goes away.
    ///
    /// A turn that was running gets an ending declared by the exit rather than by the provider. That
    /// distinction is the whole point: a subscriber renders "the outcome is unknown", and the next attach
    /// must not claim the turn succeeded.
    fn ending_at_exit(&mut self) -> Option<Produced> {
        let running = self.running.take()?;
        Some(Produced {
            src_end: self.src_end,
            body: EventBody::Turn(TurnEvent::Ended {
                turn: running.turn,
                stop: StopReason::Unknown,
                declared_by: Declarant::ProcessExit,
            }),
        })
    }

    /// Ask the running turn to stop.
    ///
    /// # Errors
    ///
    /// [`ProviderError::Protocol`] when no turn is running, because the provider's own interrupt takes the
    /// identifier of the turn to stop and runtrol has none to give.
    async fn interrupt(&self) -> Result<(), ProviderError> {
        let Some(running) = &self.running else {
            return Err(ProviderError::Protocol {
                provider: self.provider,
                doing: "interrupting a turn",
                detail: "no turn is running, and this provider's interrupt names the turn to stop"
                    .to_owned(),
            });
        };
        self.conn
            .call(
                INTERRUPT,
                &serde_json::json!({
                    "threadId": self.native,
                    "turnId": running.native.to_string(),
                }),
                "interrupting a turn",
            )
            .await
            .map(|_answer| ())
    }

    async fn expire_one(&mut self) -> Option<Result<Produced, ProviderError>> {
        let native = self.approvals.due(WallMs::now())?;
        if let Err(error) = self
            .conn
            .answer(&native.request, &native.result, "expiring an approval")
            .await
        {
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
}

/// Which call opens the conversation, and with what.
///
/// # Errors
///
/// [`ProviderError::Unsupported`] for a way of opening a session this driver does not know. The contract's
/// dispositions are open ended on purpose, so that adding one does not break a driver written elsewhere; the
/// cost is that each driver says which ones it serves rather than quietly treating a new one as something
/// else.
fn open_call(
    provider: ProviderId,
    intent: &OpenIntent,
) -> Result<(String, serde_json::Value, &'static str), ProviderError> {
    let mut params = serde_json::Map::new();
    params.insert(
        "cwd".to_owned(),
        serde_json::Value::String(intent.workspace.as_str().to_owned()),
    );
    if let Some(model) = &intent.model {
        params.insert(
            "model".to_owned(),
            serde_json::Value::String(model.to_string()),
        );
    }
    if let Some(reasoning_effort) = &intent.reasoning_effort {
        params.insert(
            "config".to_owned(),
            serde_json::json!({ "model_reasoning_effort": reasoning_effort }),
        );
    }

    match &intent.disposition {
        // A conversation the provider does not have yet. Its identifier comes back in the answer, unlike the
        // other supported CLI where runtrol issues one.
        Disposition::Fresh => {
            if let Some(permission) = &intent.permission {
                params.insert(
                    "approvalPolicy".to_owned(),
                    serde_json::Value::String(permission.to_string()),
                );
            }
            Ok((
                "thread/start".to_owned(),
                serde_json::Value::Object(params),
                "starting a conversation",
            ))
        }
        Disposition::Resume { native } => {
            params.insert(
                "threadId".to_owned(),
                serde_json::Value::String(native.to_string()),
            );
            Ok((
                "thread/resume".to_owned(),
                serde_json::Value::Object(params),
                "resuming a conversation",
            ))
        }
        other => Err(ProviderError::Unsupported {
            provider,
            what: format!("{other:?}"),
            why: "this driver serves a fresh conversation and a resume, and nothing else yet",
        }),
    }
}

/// One prompt, as the parameters this CLI reads.
///
/// The operator's text goes in as it was written. Serialized rather than pasted together, because pasting is
/// how a value from somewhere else becomes structure and every value here came from somewhere else.
///
/// # Errors
///
/// [`ProviderError::Unsupported`] for a block kind that arrived after this driver did. Refused rather than
/// skipped: a prompt missing one of its parts is a prompt the operator did not write, and they would never
/// know.
fn prompt_params(
    provider: ProviderId,
    thread: &str,
    blocks: &[ContentBlock],
    model: Option<&str>,
    effort: Option<&str>,
) -> Result<serde_json::Value, ProviderError> {
    let mut input: Vec<serde_json::Value> = Vec::with_capacity(blocks.len());
    for block in blocks {
        input.push(match block {
            ContentBlock::Text(text) => {
                serde_json::json!({"type": "text", "text": text.to_string()})
            }
            // Forwarded whole. A block shape runtrol has never heard of still reaches the provider, which is
            // what keeps runtrol a pipe rather than a gate on which features are reachable. A payload that is
            // not readable JSON goes as text rather than being dropped: the operator wrote it, and losing
            // part of a prompt silently is worse than sending it plainly.
            ContentBlock::Native(payload) => match serde_json::from_str(payload.as_str()) {
                Ok(value) => value,
                Err(_) => serde_json::json!({"type": "text", "text": payload.as_str()}),
            },
            other => {
                return Err(ProviderError::Unsupported {
                    provider,
                    what: format!("{other:?}"),
                    why: "this driver can send text and a native block, and nothing else yet",
                });
            }
        });
    }
    let mut params = serde_json::json!({"threadId": thread, "input": input});
    // The operator's pending switch rides the turn, because that is this CLI's own switch surface: turn/start
    // documents `model` and `effort` as overriding this turn and the ones after it.
    if let (Some(object), Some(model)) = (params.as_object_mut(), model) {
        object.insert("model".to_owned(), serde_json::json!(model));
    }
    if let (Some(object), Some(effort)) = (params.as_object_mut(), effort) {
        object.insert("effort".to_owned(), serde_json::json!(effort));
    }
    Ok(params)
}

/// The conversation an answer names.
fn thread_of(answer: &Bytes) -> Option<String> {
    let Ok(opened) = serde_json::from_slice::<Opened<'_>>(answer) else {
        return None;
    };
    opened
        .thread
        .and_then(|thread| thread.id)
        .map(str::to_owned)
}

/// The whole answer as a payload.
fn payload_of(answer: &Bytes) -> Opaque {
    match core::str::from_utf8(answer) {
        Ok(text) => Opaque::borrowed_from(answer, text).unwrap_or_else(Opaque::none),
        // An answer that is not UTF-8 could not have been read as JSON, so nothing reaches here having
        // succeeded. Answering with nothing keeps one bad frame from taking down a supervisor.
        Err(_) => Opaque::none(),
    }
}

/// What runtrol says about a question it answered on the operator's behalf.
fn answered_notice(method: &str, how: Answer) -> EventBody {
    let (level, code) = match how {
        // A capability was blocked by a rule rather than by a person, and the operator has to know: a decline
        // nobody hears is indistinguishable from the agent choosing not to act.
        Answer::Decline => (Level::Warn, NoticeCode::PermissionAutoDenied),
        // The provider asked runtrol for a credential. It holds none and will not proxy one.
        Answer::Refuse => (Level::Warn, NoticeCode::CredentialRequestRefused),
    };
    EventBody::Notice(Box::new(Notice {
        level,
        code,
        retryable: false,
        // runtrol's own frame. The method is the provider's word for what it asked and carries no content.
        payload: Opaque::owned(format!(r#"{{"answeredWithoutAsking":"{method}"}}"#)),
    }))
}

#[async_trait]
impl Agent for CodexAgent {
    fn session(&self) -> SessionId {
        self.session
    }

    fn native(&self) -> Option<&str> {
        Some(&self.native)
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
                let params = prompt_params(
                    self.provider,
                    &self.native,
                    &blocks,
                    self.pending_model.as_deref(),
                    self.pending_effort.as_deref(),
                )?;
                let answer = self
                    .conn
                    .call("turn/start", &params, "sending a turn")
                    .await?;

                // Measured: this answers in two milliseconds with a turn that is in progress and carries no
                // work. It is a receipt, and reading it as a result reported an eight second turn as finished
                // instantly.
                let submitted = match serde_json::from_slice::<Submitted<'_>>(&answer) {
                    Ok(submitted) => submitted,
                    Err(error) => {
                        return Err(ProviderError::Protocol {
                            provider: self.provider,
                            doing: "sending a turn",
                            detail: format!("the receipt could not be read: {error}"),
                        });
                    }
                };
                let Some(native_turn) = submitted
                    .turn
                    .as_ref()
                    .and_then(|turn| turn.id)
                    .map(Box::<str>::from)
                else {
                    return Err(ProviderError::Protocol {
                        provider: self.provider,
                        doing: "sending a turn",
                        detail:
                            "the receipt named no turn, so nothing could be interrupted or ended"
                                .to_owned(),
                    });
                };
                let ack_only = submitted
                    .turn
                    .as_ref()
                    .and_then(|turn| turn.items.as_ref())
                    .is_none_or(Vec::is_empty);

                let turn = self.mint_turn();
                self.running = Some(Running {
                    turn,
                    native: native_turn,
                });
                self.announced.push_back(Produced {
                    src_end: self.src_end,
                    body: EventBody::Turn(TurnEvent::Accepted { turn, ack_only }),
                });
                // The receipt is also the CLI accepting the pending override: turn/start documents the fields
                // as sticky from this turn on, so acceptance is the moment the switch became true.
                if let Some(model) = self.pending_model.take() {
                    self.announced.push_back(Produced {
                        src_end: self.src_end,
                        body: EventBody::CurrentModelUpdate {
                            model_id: model,
                            available_ids: None,
                            payload: payload_of(&answer),
                        },
                    });
                }
                self.pending_effort = None;
                Ok(())
            }

            AgentCommand::Interrupt => self.interrupt().await,

            AgentCommand::SetModel {
                model,
                reasoning_effort,
            } => {
                // This CLI's switch surface is the next turn itself (turn/start's own documentation: the
                // override applies to "this turn and subsequent turns"). Nothing is sent now; the choice waits
                // for the turn that will carry it, and the confirmation event follows that turn's receipt.
                self.pending_model = Some(model);
                self.pending_effort = reasoning_effort;
                Ok(())
            }

            // Forwarded byte for byte, never inspected and never rewritten. A surface driving a feature that
            // shipped after this binary reaches the provider through here. One consequence is worth knowing:
            // the answer to a request sent this way is not routed back, because runtrol did not issue its
            // identifier, and the connection reports it as an answer nobody asked for.
            AgentCommand::Native(payload) => self.conn.send_verbatim(payload.as_str()).await,

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
                self.conn
                    .answer(&native.request, &native.result, "answering an approval")
                    .await?;
                self.approvals.complete(native.approval);
                Ok(())
            }

            // A command that arrived after this driver did. Saying so is the whole point: a wildcard that
            // returned success would report a command as sent when nothing was sent, and the operator would
            // wait for an effect that is never coming.
            other => Err(ProviderError::Unsupported {
                provider: self.provider,
                what: format!("{other:?}"),
                why: "this driver has no binding for that command",
            }),
        }
    }

    async fn next(&mut self) -> Option<Result<Produced, ProviderError>> {
        // What runtrol itself has to say comes first, and taking it is not a wait, so a caller that sets this
        // aside partway through loses nothing.
        if let Some(announced) = self.announced.pop_front() {
            return Some(Ok(announced));
        }
        if let Some(report) = self.own_report() {
            return Some(Ok(report));
        }
        if self.finished {
            return None;
        }

        let delivery = loop {
            if let Some(expired) = self.expire_one().await {
                return Some(expired);
            }
            match self.approvals.wait(WallMs::now()) {
                Some(wait) => {
                    // The timeout is not a dropped failure. It is the approval deadline becoming due, and the
                    // next loop iteration sends the retained rejection and publishes the expiry.
                    if let Ok(delivery) = tokio::time::timeout(wait, self.inbox.next()).await {
                        break delivery;
                    }
                }
                None => break self.inbox.next().await,
            }
        };

        match delivery {
            Some(Delivery::Report { method, params }) => {
                if let Some(params) = &params {
                    self.approvals.observe(&method, params);
                }
                match map::read(&method, params.as_ref()) {
                    Ok(frame) => Some(Ok(self.produce(frame))),
                    // A frame runtrol cannot read is a protocol failure for this session, promoted to session
                    // state by the caller. The connection itself keeps going, because every other session on
                    // it is unaffected by one conversation's bad frame.
                    Err(error) => {
                        self.finished = true;
                        Some(Err(ProviderError::Protocol {
                            provider: self.provider,
                            doing: "reading a frame from the session",
                            detail: error.to_string(),
                        }))
                    }
                }
            }

            Some(Delivery::Answered { method, how }) => Some(Ok(Produced {
                src_end: self.src_end,
                body: answered_notice(&method, how),
            })),

            Some(Delivery::Question { id, method, params }) => {
                let turn = self.running.as_ref().map(|running| running.turn);
                match self.approvals.open(id.clone(), &method, params, turn) {
                    Ok(request) => Some(Ok(Produced {
                        src_end: self.src_end,
                        body: EventBody::ApprovalRequested(Box::new(request)),
                    })),
                    Err(error) => {
                        let answered = self
                            .conn
                            .answer(&id, DECLINE_RESULT, "declining an unreadable approval")
                            .await;
                        match answered {
                            Ok(()) => Some(Ok(Produced {
                                src_end: self.src_end,
                                body: approval_notice(&method, &error),
                            })),
                            Err(write) => Some(Err(write)),
                        }
                    }
                }
            }

            None => {
                self.finished = true;
                self.ending_at_exit().map(Ok)
            }
        }
    }

    async fn close(mut self: Box<Self>, how: CloseMode) -> Result<(), ProviderError> {
        let grace = match how {
            CloseMode::Kill => Duration::ZERO,
            CloseMode::Graceful { grace_ms } => Duration::from_millis(grace_ms),
            // A way of closing that arrived after this driver did. It stops now, like an outright kill, rather
            // than refusing: a close that returned an error would leave a session nobody can end. The two arms
            // read the same and mean different things, which is why this one is spelled out.
            #[expect(
                clippy::match_same_arms,
                reason = "an unknown close mode stopping now is a decision, not a duplicate of Kill"
            )]
            _ => Duration::ZERO,
        };

        let agent = &mut *self;

        for native in agent.approvals.rejections() {
            agent
                .conn
                .answer(
                    &native.request,
                    &native.result,
                    "declining an approval while closing",
                )
                .await?;
            agent.approvals.complete(native.approval);
        }

        // A turn that is still running is given the time it was granted to end on its own, and asked to stop
        // when it does not. Unsubscribing while it runs would leave the daemon working with nobody watching.
        if agent.running.is_some() && !grace.is_zero() {
            let deadline = tokio::time::Instant::now() + grace;
            while agent.running.is_some() {
                match tokio::time::timeout_at(deadline, agent.next()).await {
                    Ok(Some(Ok(_produced))) => {}
                    // The stream ended, the session failed, or the grace ran out. All three mean waiting is
                    // over, and what is still running is dealt with below.
                    Ok(Some(Err(_)) | None) | Err(_) => break,
                }
            }
        }
        if agent.running.is_some() {
            agent.interrupt().await?;
        }

        // Stop following the conversation. **Not a delete**: the provider keeps it, which is what makes
        // removing everything runtrol holds lose nothing.
        let unsubscribed = agent
            .conn
            .call(
                "thread/unsubscribe",
                &serde_json::json!({"threadId": agent.native}),
                "leaving a conversation",
            )
            .await;
        agent.inbox.close().await;
        unsubscribed.map(|_answer| ())
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

fn approval_notice(method: &str, error: &ApprovalBuildError) -> EventBody {
    EventBody::Notice(Box::new(Notice {
        level: Level::Error,
        code: NoticeCode::ProtocolViolation,
        retryable: false,
        payload: Opaque::owned(format!(
            r#"{{"approvalDeclined":"{method}","why":"{error}"}}"#
        )),
    }))
}

#[cfg(test)]
mod tests {
    use runtrol_provider::AbsPath;

    use super::*;

    fn a_provider() -> ProviderId {
        ProviderId::parse("codex").expect("the test's own id must be valid")
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
    fn a_fresh_conversation_asks_the_provider_to_name_it() {
        // The opposite of the other supported CLI, where runtrol issues the identifier. Here it comes back in
        // the answer, so nothing may be sent that presumes a name.
        let (method, params, _doing) =
            open_call(a_provider(), &an_intent(Disposition::Fresh)).expect("served");
        assert_eq!(method, "thread/start");
        assert!(
            params.get("threadId").is_none(),
            "a fresh conversation cannot name itself: {params}"
        );
        assert!(params.get("cwd").is_some(), "it has to say where it works");
    }

    #[test]
    fn a_resume_names_the_conversation_the_provider_knows() {
        let (method, params, _doing) = open_call(
            a_provider(),
            &an_intent(Disposition::Resume {
                native: "thread_abc".into(),
            }),
        )
        .expect("served");
        assert_eq!(method, "thread/resume");
        assert_eq!(
            params.get("threadId").and_then(serde_json::Value::as_str),
            Some("thread_abc")
        );
    }

    #[test]
    fn no_model_and_no_permission_means_the_providers_own_settings_decide() {
        // Passing a default would override a choice the operator already made in the CLI's own configuration.
        let (_method, params, _doing) =
            open_call(a_provider(), &an_intent(Disposition::Fresh)).expect("served");
        assert!(params.get("model").is_none());
        assert!(params.get("config").is_none());
        assert!(params.get("approvalPolicy").is_none());
    }

    #[test]
    fn a_discovered_reasoning_choice_uses_the_provider_config_key() {
        let mut intent = an_intent(Disposition::Fresh);
        intent.model = Some("provider-model".into());
        intent.reasoning_effort = Some("provider-effort".into());
        let (_method, params, _doing) = open_call(a_provider(), &intent).expect("served");
        assert_eq!(
            params
                .get("config")
                .and_then(|config| config.get("model_reasoning_effort"))
                .and_then(serde_json::Value::as_str),
            Some("provider-effort")
        );
    }

    #[test]
    fn a_prompt_carries_what_the_operator_wrote_and_nothing_else() {
        // The thin rule where it is easiest to break: runtrol adds no system prompt, no preamble, and no
        // instructions of its own, and the frame is built by serializing rather than by pasting text together.
        let written = "do the thing";
        let params = prompt_params(
            a_provider(),
            "thread_abc",
            &[ContentBlock::Text(written.into())],
            None,
            None,
        )
        .expect("writable");

        let input = params
            .get("input")
            .and_then(serde_json::Value::as_array)
            .expect("the input is an array");
        assert_eq!(input.len(), 1, "runtrol added a block of its own: {params}");
        assert_eq!(
            input
                .first()
                .and_then(|block| block.get("text"))
                .and_then(serde_json::Value::as_str),
            Some(written)
        );
    }

    #[test]
    fn text_with_a_newline_in_it_still_produces_one_frame() {
        // A newline inside a frame would split it into two invalid ones, and on a shared connection that
        // corrupts every session rather than one. Serializing is what makes it impossible.
        let params = prompt_params(
            a_provider(),
            "thread_abc",
            &[ContentBlock::Text("first\nsecond\r\nthird".into())],
            None,
            None,
        )
        .expect("writable");
        let line = serde_json::to_string(&params).expect("writable");
        assert!(!line.contains('\n'), "{line}");
        assert!(!line.contains('\r'), "{line}");
    }

    #[test]
    fn a_pending_switch_rides_the_turn_and_absence_invents_nothing() {
        // The CLI's own schema documents turn/start `model` and `effort` as overriding this turn and the ones
        // after it (generated 2026-08-19), which is why the switch is carried here and nowhere else.
        let with = prompt_params(
            a_provider(),
            "thread_abc",
            &[ContentBlock::Text("hi".into())],
            Some("gpt-5.3-codex"),
            Some("high"),
        )
        .expect("writable");
        assert_eq!(
            with.pointer("/model").and_then(serde_json::Value::as_str),
            Some("gpt-5.3-codex")
        );
        assert_eq!(
            with.pointer("/effort").and_then(serde_json::Value::as_str),
            Some("high")
        );

        let without = prompt_params(
            a_provider(),
            "thread_abc",
            &[ContentBlock::Text("hi".into())],
            None,
            None,
        )
        .expect("writable");
        assert!(
            without.get("model").is_none() && without.get("effort").is_none(),
            "no pending switch must invent no override fields: {without}"
        );
    }

    #[test]
    fn a_block_runtrol_has_never_heard_of_reaches_the_provider_whole() {
        let native =
            Opaque::owned(r#"{"type":"image","url":"data:image/png;base64,AA"}"#.to_owned());
        let params = prompt_params(
            a_provider(),
            "thread_abc",
            &[ContentBlock::Native(native)],
            None,
            None,
        )
        .expect("writable");
        let block = params.pointer("/input/0").expect("the block survived");
        assert_eq!(
            block.get("type").and_then(serde_json::Value::as_str),
            Some("image")
        );
        assert_eq!(
            block.get("url").and_then(serde_json::Value::as_str),
            Some("data:image/png;base64,AA"),
            "the block has to arrive whole, not flattened"
        );
    }

    #[test]
    fn a_native_block_that_is_not_readable_json_is_sent_rather_than_dropped() {
        // The operator wrote it. Losing part of a prompt silently is worse than sending it plainly.
        let params = prompt_params(
            a_provider(),
            "thread_abc",
            &[ContentBlock::Native(Opaque::owned("not json".to_owned()))],
            None,
            None,
        )
        .expect("writable");
        assert_eq!(
            params
                .pointer("/input/0/text")
                .and_then(serde_json::Value::as_str),
            Some("not json")
        );
    }

    #[test]
    fn the_conversation_is_read_out_of_the_answer_that_opened_it() {
        let answer = Bytes::from_static(
            br#"{"thread":{"id":"thread_abc","status":{"type":"idle"}},"model":"gpt-5.4-mini"}"#,
        );
        assert_eq!(thread_of(&answer).as_deref(), Some("thread_abc"));
        // An answer that names none is refused by the caller rather than guessed at.
        assert_eq!(thread_of(&Bytes::from_static(b"{}")), None);
        assert_eq!(thread_of(&Bytes::from_static(b"not json")), None);
    }

    #[test]
    fn a_question_answered_without_asking_anybody_is_reported() {
        // A decline nobody hears is indistinguishable from the agent choosing not to act, and a refused
        // credential request has to send the operator to their own machine.
        match answered_notice("item/commandExecution/requestApproval", Answer::Decline) {
            EventBody::Notice(notice) => {
                assert_eq!(notice.code, NoticeCode::PermissionAutoDenied);
                assert!(!notice.code.needs_operator_at_the_machine());
            }
            other => panic!("expected a notice, got {other:?}"),
        }
        match answered_notice("account/chatgptAuthTokens/refresh", Answer::Refuse) {
            EventBody::Notice(notice) => {
                assert_eq!(notice.code, NoticeCode::CredentialRequestRefused);
                assert!(
                    notice.code.needs_operator_at_the_machine(),
                    "runtrol holds no credential, so the only honest answer is to go to the machine"
                );
            }
            other => panic!("expected a notice, got {other:?}"),
        }
    }

    #[test]
    fn every_argument_of_a_prompt_is_serialized_rather_than_pasted() {
        // The one place a value from somewhere else becomes structure. A quote in the operator's text must not
        // be able to end a string and start a field.
        let params = prompt_params(
            a_provider(),
            r#"thread","injected":"x"#,
            &[ContentBlock::Text(r#"say "hello""#.into())],
            None,
            None,
        )
        .expect("writable");
        assert!(
            params.get("injected").is_none(),
            "a value became structure: {params}"
        );
        assert_eq!(
            params.get("threadId").and_then(serde_json::Value::as_str),
            Some(r#"thread","injected":"x"#)
        );
    }
}
