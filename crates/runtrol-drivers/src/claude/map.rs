//! One frame in, one classification out. The envelope is read and the payload never is.
//!
//! Pure: bytes to a value, no process, no state, no clock. That is what lets the whole mapping be proved against
//! frames recorded from the real CLI, which is the only way to know a mapping is right.
//!
//! # What is read, and what is not
//!
//! Read: `type`, `subtype`, the nested fragment kind, the session identifier, the capability list, the stop
//! reason, whether it failed. Every one of those is something the supervisor takes a decision on.
//!
//! Not read: the message. Not once, anywhere. What the agent said travels as a slice of the line it arrived on
//! and is never opened, which is why a mapping change can never start leaking a conversation.
//!
//! # Nothing is dropped
//!
//! A frame with no binding becomes an unmapped event carrying its own tag and its whole body. A vendor shipping
//! something new is then a frame a subscriber can render or ignore, rather than an outage. That is the direct
//! answer to how the last project in this space died.

use bytes::Bytes;
use runtrol_provider::{
    CapabilitySet, Chunk, EventBody, Level, MessageId, NativeSessionId, Notice, NoticeCode, Opaque,
    RateLimit, StopReason, Unmapped,
};
use serde::Deserialize;
use serde_json::value::RawValue;

use crate::claude::bound::TERMINAL;

/// A frame could not be classified.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum MapError {
    /// The line is not a readable JSON object.
    ///
    /// The message names a kind and a position and never the text at that position, because the text at that
    /// position is somebody's conversation.
    #[error("not a readable frame: {detail}")]
    NotAFrame {
        /// What kind of problem, and where.
        detail: String,
    },

    /// The frame has no `type`, which every frame this CLI sends has.
    #[error("a frame with no type")]
    NoType,

    /// The session identifier in the frame is not one runtrol will hold.
    #[error("the frame's session identifier is not usable: {detail}")]
    BadSessionId {
        /// Why it was refused.
        detail: String,
    },
}

/// What a frame turned out to be.
///
/// The driver completes two of these with what only it knows: which turn is running, and where a subscriber
/// recovers older content from. Keeping those out of here is what keeps this pure.
#[derive(Clone, Debug)]
pub enum Frame {
    /// The session started.
    Started(Box<Startup>),
    /// A frame that is already an event.
    Body(EventBody),
    /// The turn ended, by the provider's own declaration.
    Ended(Box<Ended>),
    /// Nothing runtrol binds, carried through whole.
    Unbound(Unmapped),
}

/// What the startup frame said.
#[derive(Clone, Debug)]
pub struct Startup {
    /// The identifier the CLI is using, which is the one runtrol issued.
    pub native: NativeSessionId,
    /// The capability tokens it announced.
    pub caps: CapabilitySet,
    /// Its own version, which it reports so nobody has to infer it from a version string.
    pub version: Option<Box<str>>,
    /// The model that will answer.
    ///
    /// Not the model runtrol asked for. Measured: those differ, and only the requested one is runtrol's decision.
    pub answering_with: Option<Box<str>>,
    /// The whole startup object, unread.
    pub payload: Opaque,
}

/// How the turn ended.
#[derive(Clone, Debug)]
pub struct Ended {
    /// Why it stopped.
    pub stop: StopReason,
    /// The provider reported a failure.
    pub failed: bool,
    /// The whole terminal frame, unread.
    ///
    /// Carries the cost, the usage, the timings and the permission denials. runtrol reads none of them here; a
    /// subscriber that wants them has them.
    pub payload: Opaque,
}

/// The envelope fields runtrol decides on.
///
/// Unknown fields are the vendor's business, which is the opposite of a manifest and for the opposite reason: a
/// vendor is allowed to add things and an operator needs to be told about a typo.
#[derive(Deserialize)]
struct Envelope<'line> {
    /// What kind of frame this is.
    #[serde(rename = "type")]
    kind: Option<&'line str>,
    /// The narrower kind, on the frames that have one.
    #[serde(default)]
    subtype: Option<&'line str>,
    /// The CLI's identifier for the session.
    #[serde(default)]
    session_id: Option<&'line str>,
    /// A fragment's real kind, nested one level down.
    #[serde(default, borrow)]
    event: Option<&'line RawValue>,
    /// A whole message.
    #[serde(default, borrow)]
    message: Option<&'line RawValue>,
    /// The tool call a subagent's output belongs under.
    #[serde(default)]
    parent_tool_use_id: Option<&'line str>,
    /// The CLI's own identifier for the exchange, used as the message identifier for fragments.
    #[serde(default)]
    request_id: Option<&'line str>,
    /// On the terminal frame: whether it failed.
    #[serde(default)]
    is_error: Option<bool>,
    /// On the terminal frame: why it stopped.
    #[serde(default)]
    stop_reason: Option<&'line str>,
    /// On the startup frame: what it can do.
    #[serde(default)]
    capabilities: Option<Vec<&'line str>>,
    /// On the startup frame: its own version.
    #[serde(default)]
    claude_code_version: Option<&'line str>,
    /// On the startup frame: the model that will answer.
    #[serde(default)]
    model: Option<&'line str>,
    /// On the quota frame: where the account stands.
    #[serde(default, borrow)]
    rate_limit_info: Option<&'line RawValue>,
}

/// A fragment's nested kind.
#[derive(Deserialize)]
struct Fragment<'line> {
    /// The nested kind, which is the one that matters.
    #[serde(rename = "type")]
    kind: Option<&'line str>,
}

/// Classify one frame.
///
/// # Errors
///
/// [`MapError::NotAFrame`] when the line is not a readable JSON object, [`MapError::NoType`] when it has no kind,
/// [`MapError::BadSessionId`] when the startup frame's identifier is not one runtrol will hold.
pub fn read(line: &Bytes) -> Result<Frame, MapError> {
    let envelope: Envelope<'_> =
        serde_json::from_slice(line).map_err(|error| MapError::NotAFrame {
            detail: format!("{} at column {}", kind_of(&error), error.column()),
        })?;
    let Some(kind) = envelope.kind else {
        return Err(MapError::NoType);
    };

    match (kind, envelope.subtype) {
        ("system", Some("init")) => Ok(Frame::Started(Box::new(startup(line, &envelope)?))),

        (terminal, Some(sub)) if terminal == TERMINAL.kind && Some(sub) == TERMINAL.subtype => {
            Ok(Frame::Ended(Box::new(ended(line, &envelope))))
        }

        ("assistant" | "user", _) => Ok(Frame::Body(whole_message(line, kind, &envelope))),

        ("stream_event", _) => Ok(Frame::Body(fragment(line, &envelope))),

        ("rate_limit_event", _) => Ok(Frame::Body(EventBody::RateLimitUpdate(Box::new(
            RateLimit {
                // The windows are the provider's own shape and runtrol does not model them yet. What it decides
                // on is only whether something is blocking, and nothing in this frame says so on its own.
                primary: None,
                secondary: None,
                reached: false,
                detail: payload_of(line, envelope.rate_limit_info),
            },
        )))),

        // Progress that is not conversation. Reported so a subscriber can show that something is happening,
        // without runtrol claiming to know what.
        ("system", Some(_)) => Ok(Frame::Body(EventBody::Notice(Box::new(Notice {
            level: Level::Info,
            code: NoticeCode::Other,
            retryable: false,
            payload: whole_line(line),
        })))),

        _ => Ok(Frame::Unbound(Unmapped {
            tag: tag_of(kind, envelope.subtype),
            turn: None,
            payload: whole_line(line),
            unknown_to_binding: true,
        })),
    }
}

/// The startup frame.
fn startup(line: &Bytes, envelope: &Envelope<'_>) -> Result<Startup, MapError> {
    let native = envelope
        .session_id
        .ok_or_else(|| MapError::BadSessionId {
            detail: "the startup frame carried no identifier".to_owned(),
        })
        .and_then(|text| {
            NativeSessionId::new(text).map_err(|error| MapError::BadSessionId {
                detail: error.to_string(),
            })
        })?;

    Ok(Startup {
        native,
        caps: CapabilitySet::from_tokens(envelope.capabilities.clone().unwrap_or_default()),
        version: envelope.claude_code_version.map(Into::into),
        answering_with: envelope.model.map(Into::into),
        payload: whole_line(line),
    })
}

/// The terminal frame.
///
/// A missing stop reason is [`StopReason::Unknown`] and never success. Not understanding why something stopped is
/// not the same as being told it finished, and rendering the second for the first is how an operator comes to
/// trust a completion that never happened.
fn ended(line: &Bytes, envelope: &Envelope<'_>) -> Ended {
    let failed = envelope.is_error.unwrap_or(false);
    let stop = match envelope.stop_reason {
        Some("end_turn" | "stop_sequence") => StopReason::EndTurn,
        Some("max_tokens") => StopReason::MaxTokens,
        Some("refusal") => StopReason::Refusal,
        _ if failed => StopReason::Failed,
        // Measured: on a successful turn this field can be absent, and the frame's own subtype is what says it
        // succeeded. Treating the absence as failure would report every clean turn as broken.
        None => StopReason::EndTurn,
        // A token runtrol has no binding for. Named as not understood rather than guessed at.
        Some(_) => StopReason::Unknown,
    };
    Ended {
        stop,
        failed,
        payload: whole_line(line),
    }
}

/// A whole message, from either side.
fn whole_message(line: &Bytes, kind: &str, envelope: &Envelope<'_>) -> EventBody {
    let chunk = Chunk {
        message_id: message_id(envelope.request_id),
        delta: false,
        parent: parent_call(envelope.parent_tool_use_id),
        content: payload_of(line, envelope.message),
    };
    if kind == "user" {
        EventBody::UserMessageChunk(chunk)
    } else {
        EventBody::AgentMessageChunk(chunk)
    }
}

/// A fragment of a message.
///
/// The kind that matters is nested one level down, and a fragment whose nested kind says nothing about content
/// (the start and stop markers) is still relayed: a subscriber assembling a message needs the boundaries.
fn fragment(line: &Bytes, envelope: &Envelope<'_>) -> EventBody {
    let nested = nested_kind(envelope.event);

    let chunk = Chunk {
        message_id: message_id(envelope.request_id),
        delta: true,
        parent: parent_call(envelope.parent_tool_use_id),
        content: payload_of(line, envelope.event),
    };

    match nested.as_deref() {
        // Thinking arrives as its own nested kind, and a subscriber renders it somewhere else entirely.
        Some("thinking_delta") => EventBody::AgentThoughtChunk(chunk),
        _ => EventBody::AgentMessageChunk(chunk),
    }
}

/// The message identifier a frame belongs to, when the provider gave one runtrol can hold.
///
/// An identifier outside the vocabulary's bounds is not used for routing, and that is the entire consequence: the
/// frame still travels with its payload intact and a subscriber still renders it, it simply does not get attached
/// to a message. Refusing the frame instead would drop somebody's message over a name.
fn message_id(text: Option<&str>) -> Option<MessageId> {
    let Ok(id) = MessageId::new(text?) else {
        return None;
    };
    Some(id)
}

/// The tool call a subagent's output belongs under, when there is a usable one.
///
/// Same reasoning as [`message_id`]: without it the content is shown at the top level instead of nested, which is
/// a worse rendering and not a lost message.
fn parent_call(text: Option<&str>) -> Option<runtrol_provider::ToolCallId> {
    let Ok(id) = runtrol_provider::ToolCallId::new(text?) else {
        return None;
    };
    Some(id)
}

/// A fragment's nested kind, when it has a readable one.
///
/// Unreadable means the fragment is relayed as ordinary content rather than as thinking. The payload is untouched
/// either way, so the cost is which pane a subscriber puts it in.
fn nested_kind(raw: Option<&RawValue>) -> Option<String> {
    let Ok(fragment) = serde_json::from_str::<Fragment<'_>>(raw?.get()) else {
        return None;
    };
    fragment.kind.map(str::to_owned)
}

/// A frame's tag, for the unmapped case.
fn tag_of(kind: &str, subtype: Option<&str>) -> Box<str> {
    match subtype {
        Some(sub) => format!("{kind}/{sub}").into(),
        None => kind.into(),
    }
}

/// A nested value as a payload, sharing the line's allocation, or the whole line when there is none.
fn payload_of(line: &Bytes, raw: Option<&RawValue>) -> Opaque {
    match raw {
        Some(value) => Opaque::borrowed_from(line, value.get()).unwrap_or_else(|| whole_line(line)),
        None => whole_line(line),
    }
}

/// The whole frame as a payload.
fn whole_line(line: &Bytes) -> Opaque {
    match core::str::from_utf8(line) {
        Ok(text) => Opaque::borrowed_from(line, text).unwrap_or_else(Opaque::none),
        // A frame that is not UTF-8 could not have parsed as JSON, so nothing reaches here from `read`.
        // Answering with nothing rather than panicking keeps one bad frame from taking down a supervisor.
        Err(_) => Opaque::none(),
    }
}

/// What went wrong with a frame, as a phrase runtrol owns, and never the content it went wrong on.
fn kind_of(error: &serde_json::Error) -> &'static str {
    match error.classify() {
        serde_json::error::Category::Io => "the stream failed",
        serde_json::error::Category::Syntax => "invalid JSON",
        serde_json::error::Category::Data => "a value of the wrong shape",
        serde_json::error::Category::Eof => "the frame ended early",
    }
}

#[cfg(test)]
mod tests {
    use runtrol_provider::{Declarant, TurnEvent, TurnId};

    use super::*;

    /// The session identifier the recordings below were made with.
    const SESSION: &str = "e191fc1c-0f5c-445d-ada1-4dc061109519";

    fn line(text: &str) -> Bytes {
        Bytes::copy_from_slice(text.as_bytes())
    }

    /// The startup frame, with the field set observed on 2.1.220 and its content shortened.
    fn recorded_startup() -> Bytes {
        line(&format!(
            r#"{{"type":"system","subtype":"init","session_id":"{SESSION}","uuid":"a-uuid","cwd":"C:\\work","model":"claude-haiku-4-5-20251001","permissionMode":"default","apiKeySource":"none","claude_code_version":"2.1.220","capabilities":["streamingInput","hooks"],"tools":["Read","Bash"],"skills":[],"agents":[],"mcp_servers":[],"plugins":[],"slash_commands":["compact"],"memory_paths":[],"output_style":"default"}}"#
        ))
    }

    /// The terminal frame, with the field set observed on 2.1.220.
    fn recorded_terminal(extra: &str) -> Bytes {
        line(&format!(
            r#"{{"type":"message","subtype":"success","session_id":"{SESSION}","uuid":"b-uuid","is_error":false,"result":"ok","num_turns":1,"total_cost_usd":0.0012,"duration_ms":2100,"duration_api_ms":1800,"ttft_ms":700,"usage":{{"input_tokens":12,"output_tokens":3}},"modelUsage":{{}},"permission_denials":[],"terminal_reason":"end_turn"{extra}}}"#
        ))
    }

    #[test]
    fn the_startup_frame_carries_the_identifier_runtrol_issued_back_unchanged() {
        // Measured: the identifier handed to the CLI comes back in this frame. That equality is what makes
        // deleting everything runtrol stores lose nothing, because the session is still there under a name the
        // CLI itself knows.
        match read(&recorded_startup()).expect("readable") {
            Frame::Started(startup) => {
                assert_eq!(startup.native.as_str(), SESSION);
                assert_eq!(startup.version.as_deref(), Some("2.1.220"));
                assert!(startup.caps.has("streamingInput"));
                assert!(startup.caps.has("hooks"));
                assert!(
                    !startup.caps.has("streaming"),
                    "a prefix is not a capability"
                );
                assert_eq!(
                    startup.answering_with.as_deref(),
                    Some("claude-haiku-4-5-20251001")
                );
            }
            other => panic!("expected a startup, got {other:?}"),
        }
    }

    #[test]
    fn a_capability_is_never_inferred_from_a_version_string() {
        // The CLI declares what it can do. Nothing here reads its version to decide anything, which is why a new
        // release cannot silently change what runtrol believes it supports.
        let without = line(&format!(
            r#"{{"type":"system","subtype":"init","session_id":"{SESSION}","claude_code_version":"99.0.0"}}"#
        ));
        match read(&without).expect("readable") {
            Frame::Started(startup) => {
                assert!(
                    startup.caps.is_empty(),
                    "a version must not manufacture a capability"
                );
                assert_eq!(startup.version.as_deref(), Some("99.0.0"));
            }
            other => panic!("expected a startup, got {other:?}"),
        }
    }

    #[test]
    fn the_turn_ends_on_the_frame_the_cli_actually_sends() {
        // The measurement that refuted the design note. It said `result`; the CLI sends `message`/`success` and
        // `result` is a field inside it. A driver written from the note would never see a turn end and the
        // operator would watch a finished turn spin forever.
        match read(&recorded_terminal("")).expect("readable") {
            Frame::Ended(ended) => {
                assert!(!ended.failed);
                assert_eq!(ended.stop, StopReason::EndTurn);
                assert!(ended.stop.is_success());
            }
            other => panic!("expected an ending, got {other:?}"),
        }
    }

    #[test]
    fn the_frame_the_design_note_described_is_not_treated_as_an_ending() {
        // If it ever appears, it is something new and travels as unmapped. Binding it as well would mean two
        // frames could end one turn, and a late one would reopen a closed turn.
        let note_said = line(&format!(
            r#"{{"type":"result","subtype":"success","session_id":"{SESSION}","is_error":false}}"#
        ));
        match read(&note_said).expect("readable") {
            Frame::Unbound(unmapped) => {
                assert_eq!(&*unmapped.tag, "result/success");
                assert!(unmapped.unknown_to_binding);
            }
            other => panic!("expected an unmapped frame, got {other:?}"),
        }
    }

    #[test]
    fn a_missing_stop_reason_on_a_successful_ending_is_not_read_as_failure() {
        // Measured: the field can be absent on a clean turn, and the frame's own subtype is what says it
        // succeeded. Treating the absence as failure would report every clean turn as broken.
        match read(&recorded_terminal("")).expect("readable") {
            Frame::Ended(ended) => assert_eq!(ended.stop, StopReason::EndTurn),
            other => panic!("expected an ending, got {other:?}"),
        }
    }

    #[test]
    fn a_stop_reason_runtrol_does_not_know_is_named_as_not_understood() {
        // Not understanding a reason is not the same as being given one. Rendering an unknown token as success is
        // how an operator comes to trust a completion that never happened.
        let odd = recorded_terminal(r#","stop_reason":"something_new_from_the_vendor""#);
        match read(&odd).expect("readable") {
            Frame::Ended(ended) => {
                assert_eq!(ended.stop, StopReason::Unknown);
                assert!(!ended.stop.is_success());
            }
            other => panic!("expected an ending, got {other:?}"),
        }
    }

    #[test]
    fn a_failed_ending_says_so_and_is_not_success() {
        let failed = line(&format!(
            r#"{{"type":"message","subtype":"success","session_id":"{SESSION}","is_error":true}}"#
        ));
        match read(&failed).expect("readable") {
            Frame::Ended(ended) => {
                assert!(ended.failed);
                assert_eq!(ended.stop, StopReason::Failed);
                assert!(!ended.stop.is_success());
            }
            other => panic!("expected an ending, got {other:?}"),
        }
    }

    #[test]
    fn only_the_providers_own_word_can_end_a_turn() {
        // The mapping produces an ending only from the terminal frame. Everything else, including a frame that
        // mentions a stop reason, is content. The declarant a driver stamps on that ending is the provider's,
        // which is the only value that means the outcome is known.
        let mentions_stopping = line(&format!(
            r#"{{"type":"assistant","session_id":"{SESSION}","message":{{"stop_reason":"end_turn"}}}}"#
        ));
        assert!(matches!(
            read(&mentions_stopping).expect("readable"),
            Frame::Body(EventBody::AgentMessageChunk(_))
        ));

        let ending = TurnEvent::Ended {
            turn: TurnId::first(0),
            stop: StopReason::EndTurn,
            declared_by: Declarant::Provider,
        };
        match ending {
            TurnEvent::Ended { declared_by, .. } => assert!(declared_by.is_the_providers_word()),
            other => panic!("expected an ending, got {other:?}"),
        }
    }

    #[test]
    fn a_whole_message_and_a_fragment_are_told_apart() {
        // A subscriber appends one and replaces the other. Getting it backwards duplicates every message.
        let whole = line(&format!(
            r#"{{"type":"assistant","session_id":"{SESSION}","request_id":"req_01","message":{{"role":"assistant"}}}}"#
        ));
        match read(&whole).expect("readable") {
            Frame::Body(body) => {
                assert!(!body.is_fragment());
                assert!(body.is_content());
                assert_eq!(
                    body.message_id().map(|id| id.as_str().to_owned()),
                    Some("req_01".to_owned())
                );
            }
            other => panic!("expected content, got {other:?}"),
        }

        let piece = line(&format!(
            r#"{{"type":"stream_event","session_id":"{SESSION}","request_id":"req_01","event":{{"type":"content_block_delta","delta":{{"text":"ok"}}}}}}"#
        ));
        match read(&piece).expect("readable") {
            Frame::Body(body) => {
                assert!(body.is_fragment());
                assert!(body.is_content());
            }
            other => panic!("expected content, got {other:?}"),
        }
    }

    #[test]
    fn thinking_goes_somewhere_other_than_the_conversation() {
        // A subscriber renders it in a different place, and the nested kind is the only thing that says which.
        let thought = line(&format!(
            r#"{{"type":"stream_event","session_id":"{SESSION}","event":{{"type":"thinking_delta","thinking":"..."}}}}"#
        ));
        assert!(matches!(
            read(&thought).expect("readable"),
            Frame::Body(EventBody::AgentThoughtChunk(_))
        ));
    }

    #[test]
    fn a_subagents_output_stays_attached_to_the_call_it_came_from() {
        let nested = line(&format!(
            r#"{{"type":"assistant","session_id":"{SESSION}","parent_tool_use_id":"toolu_01","message":{{}}}}"#
        ));
        match read(&nested).expect("readable") {
            Frame::Body(EventBody::AgentMessageChunk(chunk)) => {
                assert_eq!(
                    chunk.parent.map(|id| id.as_str().to_owned()),
                    Some("toolu_01".to_owned())
                );
            }
            other => panic!("expected content, got {other:?}"),
        }
    }

    #[test]
    fn the_quota_gauge_arrives_without_being_asked_for() {
        // Measured: it comes on every turn for free. That is what makes "you are waiting on a limit" showable
        // instead of a spinner, at the cost of no extra call.
        let quota = line(&format!(
            r#"{{"type":"rate_limit_event","session_id":"{SESSION}","rate_limit_info":{{"used":10,"limit":100}}}}"#
        ));
        assert!(matches!(
            read(&quota).expect("readable"),
            Frame::Body(EventBody::RateLimitUpdate(_))
        ));
    }

    #[test]
    fn progress_that_is_not_conversation_is_not_rendered_as_conversation() {
        // Measured: four of these arrived in one short turn. Putting them in the conversation would fill it with
        // token counts.
        let progress = line(&format!(
            r#"{{"type":"system","subtype":"thinking_tokens","session_id":"{SESSION}","estimated_tokens":42,"estimated_tokens_delta":7}}"#
        ));
        match read(&progress).expect("readable") {
            Frame::Body(body) => {
                assert!(!body.is_content(), "a token estimate is not conversation");
                assert!(
                    !body.deserves_a_notification(),
                    "and it must not buzz a phone"
                );
            }
            other => panic!("expected a notice, got {other:?}"),
        }
    }

    #[test]
    fn a_frame_nobody_bound_is_carried_through_whole() {
        // The direct answer to how the last project in this space died. A vendor shipping something new is a frame
        // a subscriber can render or ignore, not an outage.
        let novel = line(&format!(
            r#"{{"type":"something_the_vendor_added","session_id":"{SESSION}","payload":{{"deep":[1,2,3]}}}}"#
        ));
        match read(&novel).expect("readable") {
            Frame::Unbound(unmapped) => {
                assert_eq!(&*unmapped.tag, "something_the_vendor_added");
                assert!(unmapped.unknown_to_binding);
                assert!(
                    unmapped.payload.as_str().contains("deep"),
                    "the whole frame has to survive"
                );
            }
            other => panic!("expected an unmapped frame, got {other:?}"),
        }
    }

    #[test]
    fn nothing_the_agent_said_reaches_a_log_line() {
        // The one place a conversation could leak without anybody intending it is a debug format, so every frame
        // that carries content is checked for it.
        let secret = "my private question";
        let frames = [
            line(&format!(
                r#"{{"type":"assistant","session_id":"{SESSION}","message":{{"text":"{secret}"}}}}"#
            )),
            line(&format!(
                r#"{{"type":"stream_event","session_id":"{SESSION}","event":{{"type":"content_block_delta","text":"{secret}"}}}}"#
            )),
            line(&format!(
                r#"{{"type":"whatever","session_id":"{SESSION}","text":"{secret}"}}"#
            )),
            recorded_startup(),
        ];
        for frame in frames {
            let mapped = read(&frame).expect("readable");
            let printed = format!("{mapped:?}");
            assert!(!printed.contains(secret), "leaked: {printed}");
        }
    }

    #[test]
    fn a_frame_with_no_kind_is_refused_by_name() {
        assert!(matches!(
            read(&line(r#"{"session_id":"x"}"#)),
            Err(MapError::NoType)
        ));
    }

    #[test]
    fn an_unreadable_frame_does_not_put_its_content_in_the_message() {
        let secret = r#"{"type":"assistant","message":{"text":"my private question"},,}"#;
        match read(&line(secret)) {
            Err(MapError::NotAFrame { detail }) => {
                assert!(!detail.contains("private"), "leaked: {detail}");
                assert!(
                    [
                        "invalid JSON",
                        "the frame ended early",
                        "a value of the wrong shape",
                        "the stream failed"
                    ]
                    .iter()
                    .any(|phrase| detail.starts_with(phrase)),
                    "the phrase has to be one runtrol owns: {detail}"
                );
            }
            other => panic!("expected an unreadable frame, got {other:?}"),
        }
    }

    #[test]
    fn a_startup_frame_with_no_identifier_is_refused() {
        // Without it there is nothing to tie the session to, and guessing would tie it to the wrong one.
        let anonymous = line(r#"{"type":"system","subtype":"init","capabilities":[]}"#);
        assert!(matches!(
            read(&anonymous),
            Err(MapError::BadSessionId { .. })
        ));
    }

    #[test]
    fn every_bound_frame_kind_maps_to_something_other_than_unmapped() {
        // The list and the mapping have to agree. A bound frame that falls through to unmapped is a binding that
        // exists on paper and nowhere else.
        for frame in crate::claude::bound::FRAMES {
            let subtype = frame
                .subtype
                .map(|sub| format!(r#","subtype":"{sub}""#))
                .unwrap_or_default();
            let text = format!(
                r#"{{"type":"{}"{subtype},"session_id":"{SESSION}"}}"#,
                frame.kind
            );
            match read(&line(&text)) {
                Ok(Frame::Unbound(unmapped)) => panic!(
                    "{:?} is on the bound list and fell through as {:?}",
                    frame, unmapped.tag
                ),
                Ok(_) => {}
                Err(error) => panic!("{frame:?} could not be read at all: {error}"),
            }
        }
    }

    #[test]
    fn the_control_channel_is_recorded_as_a_surface_and_not_as_a_mapping() {
        // Measured, so it belongs in the surface list. Not mapped into events yet, so it is kept apart: a list
        // claiming otherwise would be a binding that exists on paper and nowhere else.
        //
        // This goes red on the day the mapping learns them, which is the day they move up into the frame list.
        for frame in crate::claude::bound::CONTROL {
            let text = format!(r#"{{"type":"{}","session_id":"{SESSION}"}}"#, frame.kind);
            match read(&line(&text)).expect("readable") {
                Frame::Unbound(unmapped) => assert_eq!(&*unmapped.tag, frame.kind),
                other => panic!(
                    "{frame:?} is now mapped as {other:?}. move it into FRAMES and delete this test"
                ),
            }
        }
    }
}
