//! One notification in, one classification out. The envelope is read and the payload never is.
//!
//! Pure: bytes to a value, no process, no state, no clock. That is what lets the mapping be proved against
//! frames shaped like the ones the real CLI sends, which is the only way to know a mapping is right.
//!
//! # What is read, and what is not
//!
//! Read: the method, the provider's identifier for the turn, the turn's status, the item's kind and identifier
//! and status, the retry flag. Every one of those is something the supervisor takes a decision on.
//!
//! Not read: the message. What the agent said travels as a slice of the line it arrived on and is never opened.
//!
//! # An item is classified by name, not by a clever rule
//!
//! Eighteen kinds of item exist. A structural shortcut was available (everything carrying a status is an
//! action) and was not taken, because a shortcut that misfiles one kind misfiles it silently. The named ones
//! are mapped and the rest travel whole, which costs a subscriber nothing: an unmapped frame still carries its
//! entire body.

use bytes::Bytes;
use runtrol_provider::{
    Chunk, EventBody, Level, MessageId, Notice, NoticeCode, Opaque, RateLimit, StopReason,
    ToolCallFrame, ToolCallId, ToolCallStatus, ToolKind, Unmapped, Usage, Window,
};
use serde::Deserialize;
use serde_json::value::RawValue;

use crate::codex::bound::{STARTED, TERMINAL};

/// A notification could not be classified.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum MapError {
    /// The parameters are not a readable JSON object.
    ///
    /// The message names a kind and a position and never the text at that position, because the text at that
    /// position is somebody's conversation.
    #[error("not readable parameters: {detail}")]
    NotReadable {
        /// What kind of problem, and where.
        detail: String,
    },

    /// A notification that names a turn arrived without one.
    #[error("{method} carried no turn")]
    NoTurn {
        /// Which notification.
        method: Box<str>,
    },
}

/// What a notification turned out to be.
///
/// The driver completes the turn frames with what only it knows: which of its own turn numbers the provider's
/// identifier belongs to. Keeping that out of here is what keeps this pure.
#[derive(Clone, Debug)]
pub enum Frame {
    /// Work has demonstrably begun on a turn.
    Started {
        /// The provider's own identifier for the turn.
        native_turn: Box<str>,
    },
    /// The turn ended, by the provider's own declaration.
    Ended(Box<Ended>),
    /// A frame that is already an event.
    Body(EventBody),
    /// Nothing runtrol binds, carried through whole.
    Unbound(Unmapped),
}

/// How the turn ended.
#[derive(Clone, Debug)]
pub struct Ended {
    /// The provider's own identifier for the turn.
    pub native_turn: Box<str>,
    /// Why it stopped.
    pub stop: StopReason,
}

/// The notification fields runtrol decides on.
///
/// Unknown fields are the vendor's business, which is the opposite of a manifest and for the opposite reason: a
/// vendor is allowed to add things and an operator needs to be told about a typo.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Params<'line> {
    /// On a turn notification: the turn itself.
    #[serde(default, borrow)]
    turn: Option<&'line RawValue>,
    /// On a fragment: which turn it belongs to.
    #[serde(default)]
    turn_id: Option<&'line str>,
    /// On a fragment: which item it belongs to.
    #[serde(default)]
    item_id: Option<&'line str>,
    /// On an item notification: the item itself.
    #[serde(default, borrow)]
    item: Option<&'line RawValue>,
    /// On a usage notification: the counts.
    #[serde(default, borrow)]
    token_usage: Option<&'line RawValue>,
    /// On a quota notification: where the account stands.
    #[serde(default, borrow)]
    rate_limits: Option<&'line RawValue>,
    /// On a failure notification: whether the provider will try again.
    #[serde(default)]
    will_retry: Option<bool>,
}

/// The turn fields runtrol decides on.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Turn<'line> {
    /// The provider's own identifier.
    #[serde(default)]
    id: Option<&'line str>,
    /// Where the turn stands.
    #[serde(default)]
    status: Option<&'line str>,
}

/// The item fields runtrol decides on.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Item<'line> {
    /// Which kind of item.
    #[serde(rename = "type", default)]
    kind: Option<&'line str>,
    /// The provider's own identifier for it.
    #[serde(default)]
    id: Option<&'line str>,
    /// Where an action stands, on the items that are actions.
    #[serde(default)]
    status: Option<&'line str>,
}

/// The quota fields runtrol decides on.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Limits<'line> {
    /// The shorter window.
    #[serde(default, borrow)]
    primary: Option<&'line RawValue>,
    /// The longer one.
    #[serde(default, borrow)]
    secondary: Option<&'line RawValue>,
    /// Present when a limit has actually been reached, and it names which.
    #[serde(default, borrow)]
    rate_limit_reached_type: Option<&'line RawValue>,
}

/// One quota window.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LimitWindow {
    /// How much of it is used.
    #[serde(default)]
    used_percent: Option<i64>,
    /// How long the window is.
    #[serde(default)]
    window_duration_mins: Option<i64>,
}

/// The context counts runtrol decides on.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TokenUsage {
    /// How large the model's context is.
    #[serde(default)]
    model_context_window: Option<u64>,
    /// The running totals.
    #[serde(default)]
    total: Option<Breakdown>,
}

/// The one total runtrol reads out of a usage breakdown.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Breakdown {
    /// Everything sent, which is what fills a context window.
    #[serde(default)]
    input_tokens: Option<u64>,
}

/// Classify one notification.
///
/// `params` is the provider's own parameter object, as a slice of the line it arrived on.
///
/// # Errors
///
/// [`MapError::NotReadable`] when the parameters are not a readable JSON object, [`MapError::NoTurn`] when a
/// turn notification carries no turn to name.
pub fn read(method: &str, params: Option<&Bytes>) -> Result<Frame, MapError> {
    let Some(body) = params else {
        // A bound notification with no parameters at all. Nothing to read, so it travels whole rather than
        // being invented into an event.
        return Ok(Frame::Unbound(Unmapped {
            tag: method.into(),
            turn: None,
            payload: Opaque::none(),
            unknown_to_binding: false,
        }));
    };

    let parsed: Params<'_> = read_object(body)?;

    match method {
        STARTED => Ok(Frame::Started {
            native_turn: turn_id(method, &parsed)?,
        }),

        TERMINAL => Ok(Frame::Ended(Box::new(Ended {
            native_turn: turn_id(method, &parsed)?,
            stop: stop_of(parsed.turn),
        }))),

        "item/agentMessage/delta" => Ok(Frame::Body(EventBody::AgentMessageChunk(Chunk {
            message_id: message_id(parsed.item_id),
            delta: true,
            parent: None,
            content: whole(body),
        }))),

        // Thinking arrives on its own methods, and a subscriber renders it somewhere else entirely.
        "item/reasoning/textDelta" | "item/reasoning/summaryTextDelta" => {
            Ok(Frame::Body(EventBody::AgentThoughtChunk(Chunk {
                message_id: message_id(parsed.item_id),
                delta: true,
                parent: None,
                content: whole(body),
            })))
        }

        "item/started" => Ok(item_frame(method, body, parsed.item, false)),
        "item/completed" => Ok(item_frame(method, body, parsed.item, true)),
        "item/fileChange/patchUpdated" => Ok(match tool_call_id(parsed.item_id) {
            Some(tool_call_id) => Frame::Body(EventBody::ToolCallUpdate(ToolCallFrame {
                tool_call_id,
                kind: Some(ToolKind::Edit),
                status: Some(ToolCallStatus::InProgress),
                delta: true,
                payload: whole(body),
            })),
            None => Frame::Unbound(Unmapped {
                tag: method.into(),
                turn: None,
                payload: whole(body),
                unknown_to_binding: false,
            }),
        }),

        "thread/tokenUsage/updated" => Ok(Frame::Body(EventBody::UsageUpdate(Box::new(usage(
            body,
            parsed.token_usage,
        ))))),

        "account/rateLimits/updated" => Ok(Frame::Body(EventBody::RateLimitUpdate(Box::new(
            limits(body, parsed.rate_limits),
        )))),

        // A failure the provider reported. **Never an ending**: the retry flag is what says whether the turn
        // is still running, and treating a retryable failure as terminal ends turns that are still going.
        "error" => Ok(Frame::Body(EventBody::Notice(Box::new(Notice {
            level: Level::Error,
            code: NoticeCode::Other,
            retryable: parsed.will_retry.unwrap_or(false),
            payload: whole(body),
        })))),

        _ => Ok(Frame::Unbound(Unmapped {
            tag: method.into(),
            turn: None,
            payload: whole(body),
            unknown_to_binding: true,
        })),
    }
}

/// One item, started or completed.
///
/// A named kind becomes the event runtrol has an opinion about, and everything else travels whole. `durable`
/// records that the provider has persisted this item, which is what a cursor advances on.
fn item_frame(method: &str, body: &Bytes, raw: Option<&RawValue>, durable: bool) -> Frame {
    let relay = |tag: Box<str>| {
        Frame::Unbound(Unmapped {
            tag,
            turn: None,
            payload: whole(body),
            unknown_to_binding: false,
        })
    };

    let Some(item) = raw.and_then(read_nested::<Item<'_>>) else {
        return relay(method.into());
    };

    let chunk = || Chunk {
        message_id: message_id(item.id),
        delta: false,
        parent: None,
        content: whole(body),
    };

    match item.kind {
        Some("userMessage") => Frame::Body(EventBody::UserMessageChunk(chunk())),
        Some("agentMessage") => Frame::Body(EventBody::AgentMessageChunk(chunk())),
        Some("reasoning") => Frame::Body(EventBody::AgentThoughtChunk(chunk())),
        Some("plan") => Frame::Body(EventBody::Plan {
            payload: whole(body),
        }),

        Some(
            kind @ ("commandExecution" | "fileChange" | "mcpToolCall" | "dynamicToolCall"
            | "webSearch"),
        ) => {
            let Some(id) = tool_call_id(item.id) else {
                // An action runtrol cannot address is relayed rather than dropped. Without a usable identifier
                // a subscriber cannot pair the start with the finish, and inventing one would pair the wrong
                // two.
                return relay(kind.into());
            };
            let frame = ToolCallFrame {
                tool_call_id: id,
                kind: tool_kind(kind),
                status: status_of(item.status),
                delta: false,
                payload: whole(body),
            };
            if durable {
                Frame::Body(EventBody::ToolCallUpdate(frame))
            } else {
                Frame::Body(EventBody::ToolCall(frame))
            }
        }

        // A kind runtrol has no opinion about. It travels whole, which costs a subscriber nothing and is the
        // reason a vendor adding an item kind is not an outage.
        Some(other) => relay(other.into()),
        None => relay(method.into()),
    }
}

/// Which kind of action an item is, for a subscriber choosing how to render it.
fn tool_kind(kind: &str) -> Option<ToolKind> {
    match kind {
        "commandExecution" => Some(ToolKind::Execute),
        "fileChange" => Some(ToolKind::Edit),
        "webSearch" => Some(ToolKind::Fetch),
        // A tool the provider is hosting for somebody else. runtrol knows it is a call and not what it does.
        "mcpToolCall" | "dynamicToolCall" => Some(ToolKind::Other),
        _ => None,
    }
}

/// Where an action stands.
///
/// A token runtrol has no binding for becomes `None` rather than a guess. A wrong status is worse than an
/// absent one, because a phone decides whether to buzz on a failure.
fn status_of(status: Option<&str>) -> Option<ToolCallStatus> {
    match status? {
        "inProgress" => Some(ToolCallStatus::InProgress),
        "completed" => Some(ToolCallStatus::Completed),
        "failed" => Some(ToolCallStatus::Failed),
        // Refused by a rule or by a person. Not a failure of the tool, so it must not read as one.
        "declined" => Some(ToolCallStatus::Cancelled),
        _ => None,
    }
}

/// Why the turn stopped.
///
/// A status runtrol has no binding for, or none at all, is [`StopReason::Unknown`] and never success. Not
/// understanding why something stopped is not the same as being told it finished.
fn stop_of(raw: Option<&RawValue>) -> StopReason {
    let Some(turn) = raw.and_then(read_nested::<Turn<'_>>) else {
        return StopReason::Unknown;
    };
    match turn.status {
        Some("completed") => StopReason::EndTurn,
        Some("interrupted") => StopReason::Cancelled,
        Some("failed") => StopReason::Failed,
        // Either the provider used a token runtrol has no binding for, or it says the turn is over and still
        // running at the same time. Neither is something to render as a finished turn.
        Some(_) | None => StopReason::Unknown,
    }
}

/// The provider's identifier for the turn a notification concerns.
///
/// # Errors
///
/// [`MapError::NoTurn`] when neither the turn object nor the flat field names one. A turn frame that named no
/// turn would have to be attached to whichever turn happened to be running, and that is how an ending lands on
/// the wrong one.
fn turn_id(method: &str, parsed: &Params<'_>) -> Result<Box<str>, MapError> {
    if let Some(id) = parsed
        .turn
        .and_then(read_nested::<Turn<'_>>)
        .and_then(|turn| turn.id.map(Box::<str>::from))
    {
        return Ok(id);
    }
    match parsed.turn_id {
        Some(id) => Ok(id.into()),
        None => Err(MapError::NoTurn {
            method: method.into(),
        }),
    }
}

/// How much of the context window is in use.
fn usage(body: &Bytes, raw: Option<&RawValue>) -> Usage {
    let counts = raw.and_then(read_nested::<TokenUsage>);
    Usage {
        used: counts
            .as_ref()
            .and_then(|counts| counts.total.as_ref())
            .and_then(|total| total.input_tokens),
        size: counts
            .as_ref()
            .and_then(|counts| counts.model_context_window),
        // The provider reports tokens here and money elsewhere. Reading a price out of a count would be
        // inventing one.
        cost: None,
        detail: payload(body, raw),
    }
}

/// Where the account stands against its limits.
fn limits(body: &Bytes, raw: Option<&RawValue>) -> RateLimit {
    let snapshot = raw.and_then(read_nested::<Limits<'_>>);
    RateLimit {
        primary: snapshot.as_ref().and_then(|snap| window(snap.primary)),
        secondary: snapshot.as_ref().and_then(|snap| window(snap.secondary)),
        // The field names which limit was reached and exists only when one was. Its presence is the answer to
        // the one question the supervisor asks of this frame.
        reached: snapshot
            .as_ref()
            .is_some_and(|snap| snap.rate_limit_reached_type.is_some()),
        detail: payload(body, raw),
    }
}

/// One quota window, when the provider reported a usable one.
fn window(raw: Option<&RawValue>) -> Option<Window> {
    let parsed = raw.and_then(read_nested::<LimitWindow>)?;
    let used = parsed.used_percent?;
    let minutes = parsed.window_duration_mins.and_then(|minutes| {
        // A window length that does not fit is reported as absent rather than clamped. A wrong number here
        // would be rendered as a real one.
        let Ok(minutes) = u32::try_from(minutes) else {
            return None;
        };
        Some(minutes)
    });
    Some(Window {
        used_percent: u8::try_from(used.clamp(0, i64::from(u8::MAX))).unwrap_or(u8::MAX),
        // The schema gives this as a bare integer and does not say whether it counts seconds or milliseconds.
        // Reading it either way puts a reset time in the wrong century half the time, and the whole snapshot
        // is in the payload for anyone who knows.
        resets_at: None,
        window_minutes: minutes,
    })
}

/// The message identifier a frame belongs to, when the provider gave one runtrol can hold.
///
/// An identifier outside the vocabulary's bounds is not used for routing, and that is the entire consequence:
/// the frame still travels with its payload intact.
fn message_id(text: Option<&str>) -> Option<MessageId> {
    let Ok(id) = MessageId::new(text?) else {
        return None;
    };
    Some(id)
}

/// The identifier of an action, when the provider gave one runtrol can hold.
fn tool_call_id(text: Option<&str>) -> Option<ToolCallId> {
    let Ok(id) = ToolCallId::new(text?) else {
        return None;
    };
    Some(id)
}

/// Read the parameter object.
fn read_object<'line, T: Deserialize<'line>>(body: &'line Bytes) -> Result<T, MapError> {
    serde_json::from_slice(body).map_err(|error| MapError::NotReadable {
        detail: format!("{} at column {}", kind_of(&error), error.column()),
    })
}

/// Read something nested, where being unreadable costs a field rather than the frame.
///
/// The frame is relayed with this field absent rather than refused. A vendor changing the shape of something
/// nested costs a rendering detail here, never somebody's message.
fn read_nested<'value, T: Deserialize<'value>>(raw: &'value RawValue) -> Option<T> {
    let Ok(value) = serde_json::from_str(raw.get()) else {
        return None;
    };
    Some(value)
}

/// A nested value as a payload, sharing the line's allocation, or the whole object when there is none.
fn payload(body: &Bytes, raw: Option<&RawValue>) -> Opaque {
    match raw {
        Some(value) => Opaque::borrowed_from(body, value.get()).unwrap_or_else(|| whole(body)),
        None => whole(body),
    }
}

/// The whole parameter object as a payload.
fn whole(body: &Bytes) -> Opaque {
    match core::str::from_utf8(body) {
        Ok(text) => Opaque::borrowed_from(body, text).unwrap_or_else(Opaque::none),
        // Parameters that are not UTF-8 could not have parsed as JSON, so nothing reaches here from `read`.
        // Answering with nothing rather than panicking keeps one bad frame from taking down a supervisor.
        Err(_) => Opaque::none(),
    }
}

/// What went wrong, as a phrase runtrol owns, and never the content it went wrong on.
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
    use super::*;
    use crate::codex::bound::NOTICES;

    /// The identifiers the frames below are shaped with.
    const THREAD: &str = "01999b4c-0000-7000-8000-000000000001";
    const TURN: &str = "turn_01";

    fn params(text: &str) -> Bytes {
        Bytes::copy_from_slice(text.as_bytes())
    }

    #[test]
    fn the_notification_that_ends_a_turn_is_read_as_an_ending() {
        let frame = params(&format!(
            r#"{{"threadId":"{THREAD}","turn":{{"id":"{TURN}","status":"completed","items":[],"durationMs":8020}}}}"#
        ));
        match read(TERMINAL, Some(&frame)).expect("readable") {
            Frame::Ended(ended) => {
                assert_eq!(&*ended.native_turn, TURN);
                assert_eq!(ended.stop, StopReason::EndTurn);
                assert!(ended.stop.is_success());
            }
            other => panic!("expected an ending, got {other:?}"),
        }
    }

    #[test]
    fn the_two_millisecond_acknowledgement_is_not_an_ending() {
        // The probe bug, as a test. This is the body `turn/start` answers with: a turn that is in progress and
        // carries no work. Reading it as an ending reported an eight second turn as finished instantly.
        let acknowledged = params(&format!(
            r#"{{"turn":{{"id":"{TURN}","status":"inProgress","items":[]}}}}"#
        ));
        match read(STARTED, Some(&acknowledged)).expect("readable") {
            Frame::Started { native_turn } => assert_eq!(&*native_turn, TURN),
            other => panic!("a beginning must not be an ending: {other:?}"),
        }
        // The same body under the terminal method still refuses to claim success, because its own status says
        // the turn is running.
        match read(TERMINAL, Some(&acknowledged)).expect("readable") {
            Frame::Ended(ended) => {
                assert_eq!(ended.stop, StopReason::Unknown);
                assert!(!ended.stop.is_success());
            }
            other => panic!("expected an ending, got {other:?}"),
        }
    }

    #[test]
    fn a_turn_that_ended_some_other_way_still_ends_and_is_not_success() {
        for (status, expected) in [
            ("interrupted", StopReason::Cancelled),
            ("failed", StopReason::Failed),
            ("something_new_from_the_vendor", StopReason::Unknown),
        ] {
            let frame = params(&format!(
                r#"{{"threadId":"{THREAD}","turn":{{"id":"{TURN}","status":"{status}","items":[]}}}}"#
            ));
            match read(TERMINAL, Some(&frame)).expect("readable") {
                Frame::Ended(ended) => {
                    assert_eq!(ended.stop, expected, "{status} was read wrongly");
                    assert!(
                        !ended.stop.is_success(),
                        "{status} must not read as success"
                    );
                }
                other => panic!("{status} has to end the turn: {other:?}"),
            }
        }
    }

    #[test]
    fn a_turn_notification_that_names_no_turn_is_refused() {
        // Attaching it to whichever turn happened to be running is how an ending lands on the wrong one.
        let anonymous = params(&format!(r#"{{"threadId":"{THREAD}"}}"#));
        assert!(matches!(
            read(TERMINAL, Some(&anonymous)),
            Err(MapError::NoTurn { .. })
        ));
    }

    #[test]
    fn a_fragment_is_told_apart_from_a_whole_message() {
        // A subscriber appends one and replaces the other. Getting it backwards duplicates every message.
        let piece = params(&format!(
            r#"{{"threadId":"{THREAD}","turnId":"{TURN}","itemId":"item_01","delta":"ok"}}"#
        ));
        match read("item/agentMessage/delta", Some(&piece)).expect("readable") {
            Frame::Body(body) => {
                assert!(body.is_fragment());
                assert!(body.is_content());
                assert_eq!(
                    body.message_id().map(|id| id.as_str().to_owned()),
                    Some("item_01".to_owned())
                );
            }
            other => panic!("expected a fragment, got {other:?}"),
        }

        let whole_message = params(&format!(
            r#"{{"threadId":"{THREAD}","turnId":"{TURN}","completedAtMs":1,"item":{{"type":"agentMessage","id":"item_01","text":"ok"}}}}"#
        ));
        match read("item/completed", Some(&whole_message)).expect("readable") {
            Frame::Body(body) => {
                assert!(!body.is_fragment());
                assert!(body.is_content());
            }
            other => panic!("expected a whole message, got {other:?}"),
        }
    }

    #[test]
    fn thinking_goes_somewhere_other_than_the_conversation() {
        for method in [
            "item/reasoning/textDelta",
            "item/reasoning/summaryTextDelta",
        ] {
            let thought = params(&format!(
                r#"{{"threadId":"{THREAD}","turnId":"{TURN}","itemId":"item_02","contentIndex":0,"delta":"..."}}"#
            ));
            assert!(
                matches!(
                    read(method, Some(&thought)).expect("readable"),
                    Frame::Body(EventBody::AgentThoughtChunk(_))
                ),
                "{method} is not read as thinking"
            );
        }
    }

    #[test]
    fn an_action_pairs_its_start_with_its_finish() {
        // Without a shared identifier a subscriber cannot pair them, and it renders the same command twice.
        let started = params(&format!(
            r#"{{"threadId":"{THREAD}","turnId":"{TURN}","startedAtMs":1,"item":{{"type":"commandExecution","id":"call_01","command":"ls","commandActions":[],"cwd":"C:\\work","status":"inProgress"}}}}"#
        ));
        match read("item/started", Some(&started)).expect("readable") {
            Frame::Body(EventBody::ToolCall(frame)) => {
                assert_eq!(frame.tool_call_id.as_str(), "call_01");
                assert_eq!(frame.kind, Some(ToolKind::Execute));
                assert_eq!(frame.status, Some(ToolCallStatus::InProgress));
            }
            other => panic!("expected an action beginning, got {other:?}"),
        }

        let finished = params(&format!(
            r#"{{"threadId":"{THREAD}","turnId":"{TURN}","completedAtMs":2,"item":{{"type":"commandExecution","id":"call_01","command":"ls","commandActions":[],"cwd":"C:\\work","status":"failed"}}}}"#
        ));
        match read("item/completed", Some(&finished)).expect("readable") {
            Frame::Body(body) => {
                assert!(
                    body.deserves_a_notification(),
                    "a failed action is worth telling somebody about"
                );
                match body {
                    EventBody::ToolCallUpdate(frame) => {
                        assert_eq!(frame.tool_call_id.as_str(), "call_01");
                        assert_eq!(frame.status, Some(ToolCallStatus::Failed));
                    }
                    other => panic!("expected an action update, got {other:?}"),
                }
            }
            other => panic!("expected an action update, got {other:?}"),
        }
    }

    #[test]
    fn an_action_refused_by_a_rule_does_not_read_as_a_failure() {
        // A declined command is a policy outcome. Rendering it as a failure sends somebody debugging a tool
        // that worked exactly as configured.
        let declined = params(&format!(
            r#"{{"threadId":"{THREAD}","turnId":"{TURN}","completedAtMs":2,"item":{{"type":"commandExecution","id":"call_02","command":"rm","commandActions":[],"cwd":"C:\\work","status":"declined"}}}}"#
        ));
        match read("item/completed", Some(&declined)).expect("readable") {
            Frame::Body(body) => {
                assert!(!body.deserves_a_notification());
                match body {
                    EventBody::ToolCallUpdate(frame) => {
                        assert_eq!(frame.status, Some(ToolCallStatus::Cancelled));
                    }
                    other => panic!("expected an action update, got {other:?}"),
                }
            }
            other => panic!("expected an action update, got {other:?}"),
        }
    }

    #[test]
    fn an_item_kind_runtrol_has_no_opinion_about_travels_whole() {
        // Eighteen kinds exist and runtrol maps a handful. The rest must reach a subscriber intact, which is
        // what makes a vendor adding one a non-event.
        let novel = params(&format!(
            r#"{{"threadId":"{THREAD}","turnId":"{TURN}","startedAtMs":1,"item":{{"type":"imageGeneration","id":"item_09","result":{{"deep":[1,2]}},"status":"inProgress"}}}}"#
        ));
        match read("item/started", Some(&novel)).expect("readable") {
            Frame::Unbound(unmapped) => {
                assert_eq!(&*unmapped.tag, "imageGeneration");
                assert!(
                    unmapped.payload.as_str().contains("deep"),
                    "the whole frame has to survive"
                );
                assert!(
                    !unmapped.unknown_to_binding,
                    "the notification was bound. it is the item kind runtrol has no opinion about"
                );
            }
            other => panic!("expected an unmapped item, got {other:?}"),
        }
    }

    #[test]
    fn a_notification_nobody_bound_is_carried_through_whole() {
        // The direct answer to how the last project in this space died. Seventy notifications exist and this
        // build binds ten.
        let novel = params(r#"{"somethingTheVendorAdded":{"deep":[1,2,3]}}"#);
        match read("thread/realtime/sdp", Some(&novel)).expect("readable") {
            Frame::Unbound(unmapped) => {
                assert_eq!(&*unmapped.tag, "thread/realtime/sdp");
                assert!(unmapped.unknown_to_binding);
                assert!(unmapped.payload.as_str().contains("deep"));
            }
            other => panic!("expected an unmapped notification, got {other:?}"),
        }
    }

    #[test]
    fn a_reported_failure_is_never_an_ending() {
        // A retryable failure arrives mid-turn and the turn is still running. Ending it would report an
        // outcome the provider never declared.
        let transient = params(&format!(
            r#"{{"threadId":"{THREAD}","turnId":"{TURN}","willRetry":true,"error":{{"message":"upstream hiccup"}}}}"#
        ));
        match read("error", Some(&transient)).expect("readable") {
            Frame::Body(EventBody::Notice(notice)) => {
                assert!(notice.retryable);
                assert_eq!(notice.level, Level::Error);
            }
            other => panic!("a failure is a notice, not an ending: {other:?}"),
        }
    }

    #[test]
    fn the_quota_gauge_arrives_without_being_asked_for() {
        // Measured: it comes on every turn for free. That is what makes "you are waiting on a limit" showable
        // instead of a spinner, at the cost of no extra call.
        let quota = params(
            r#"{"rateLimits":{"primary":{"usedPercent":87,"windowDurationMins":300,"resetsAt":1799999999},"secondary":{"usedPercent":12,"windowDurationMins":10080}}}"#,
        );
        match read("account/rateLimits/updated", Some(&quota)).expect("readable") {
            Frame::Body(EventBody::RateLimitUpdate(limit)) => {
                let primary = limit.primary.expect("the shorter window was reported");
                assert_eq!(primary.used_percent, 87);
                assert_eq!(primary.window_minutes, Some(300));
                assert!(
                    primary.resets_at.is_none(),
                    "the schema does not say what unit that integer is in, and guessing puts the reset in the wrong century"
                );
                assert_eq!(limit.secondary.map(|w| w.used_percent), Some(12));
                assert!(!limit.reached, "nothing here says a limit was reached");
            }
            other => panic!("expected a quota gauge, got {other:?}"),
        }
    }

    #[test]
    fn a_limit_that_was_actually_reached_says_so() {
        // The one decision the supervisor takes on this frame: is the account blocked.
        let blocked = params(
            r#"{"rateLimits":{"primary":{"usedPercent":100},"rateLimitReachedType":"primary"}}"#,
        );
        match read("account/rateLimits/updated", Some(&blocked)).expect("readable") {
            Frame::Body(EventBody::RateLimitUpdate(limit)) => assert!(limit.reached),
            other => panic!("expected a quota gauge, got {other:?}"),
        }
    }

    #[test]
    fn context_usage_reports_what_it_was_given_and_invents_no_price() {
        let counts = params(&format!(
            r#"{{"threadId":"{THREAD}","turnId":"{TURN}","tokenUsage":{{"modelContextWindow":272000,"total":{{"inputTokens":1024,"outputTokens":32}},"last":{{"inputTokens":2}}}}}}"#
        ));
        match read("thread/tokenUsage/updated", Some(&counts)).expect("readable") {
            Frame::Body(EventBody::UsageUpdate(usage)) => {
                assert_eq!(usage.used, Some(1024));
                assert_eq!(usage.size, Some(272_000));
                assert!(
                    usage.cost.is_none(),
                    "reading a price out of a token count would be inventing one"
                );
            }
            other => panic!("expected a usage update, got {other:?}"),
        }
    }

    #[test]
    fn every_bound_notification_maps_to_something_other_than_unmapped() {
        // The list and the mapping have to agree. A bound notification that falls through to unmapped is a
        // binding that exists on paper and nowhere else.
        let body = params(&format!(
            r#"{{"threadId":"{THREAD}","turnId":"{TURN}","turn":{{"id":"{TURN}","status":"completed"}},"itemId":"item_01","item":{{"type":"agentMessage","id":"item_01"}},"tokenUsage":{{}},"rateLimits":{{}},"willRetry":false}}"#
        ));
        for notice in NOTICES {
            match read(notice.method, Some(&body)) {
                Ok(Frame::Unbound(unmapped)) => panic!(
                    "{:?} is on the bound list and fell through as {:?}",
                    notice, unmapped.tag
                ),
                Ok(_) => {}
                Err(error) => panic!("{notice:?} could not be read at all: {error}"),
            }
        }
    }

    #[test]
    fn nothing_the_agent_said_reaches_a_log_line() {
        // The one place a conversation could leak without anybody intending it is a debug format, so every
        // frame that carries content is checked for it.
        let secret = "my private question";
        let frames = [
            (
                "item/agentMessage/delta",
                format!(r#"{{"turnId":"{TURN}","itemId":"i1","delta":"{secret}"}}"#),
            ),
            (
                "item/completed",
                format!(
                    r#"{{"turnId":"{TURN}","completedAtMs":1,"item":{{"type":"agentMessage","id":"i1","text":"{secret}"}}}}"#
                ),
            ),
            (
                "error",
                format!(
                    r#"{{"turnId":"{TURN}","willRetry":false,"error":{{"message":"{secret}"}}}}"#
                ),
            ),
            ("thread/realtime/sdp", format!(r#"{{"text":"{secret}"}}"#)),
        ];
        for (method, text) in frames {
            let mapped = read(method, Some(&params(&text))).expect("readable");
            let printed = format!("{mapped:?}");
            assert!(!printed.contains(secret), "leaked from {method}: {printed}");
        }
    }

    #[test]
    fn unreadable_parameters_do_not_put_their_content_in_the_message() {
        let secret = r#"{"delta":"my private question",,}"#;
        match read("item/agentMessage/delta", Some(&params(secret))) {
            Err(MapError::NotReadable { detail }) => {
                assert!(!detail.contains("private"), "leaked: {detail}");
                assert!(
                    [
                        "invalid JSON",
                        "the frame ended early",
                        "a value of the wrong shape",
                        "the stream failed",
                    ]
                    .iter()
                    .any(|phrase| detail.starts_with(phrase)),
                    "the phrase has to be one runtrol owns: {detail}"
                );
            }
            other => panic!("expected unreadable parameters, got {other:?}"),
        }
    }

    #[test]
    fn a_bound_notification_with_no_parameters_is_relayed_rather_than_invented() {
        match read(TERMINAL, None).expect("readable") {
            Frame::Unbound(unmapped) => {
                assert_eq!(&*unmapped.tag, TERMINAL);
                assert!(!unmapped.unknown_to_binding);
            }
            other => panic!("expected a relayed frame, got {other:?}"),
        }
    }
}
