//! One frame in, one classification out. The envelope is read and the payload never is.
//!
//! Pure: bytes to a value, no process, no state, no clock. That is what lets the whole mapping be proved against
//! frames recorded from the real CLI, which is the only way to know a mapping is right.
//!
//! # What is read, and what is not
//!
//! Read: `type`, `subtype`, the nested fragment kind, the session identifier, the capability list, the stop
//! reason, whether it failed, and within a whole message the list of content blocks: which kind each block is,
//! and the identifiers that bind a tool call to its result. Every one of those is something the supervisor or a
//! subscriber takes a decision on.
//!
//! Not read: what is inside a block. Not once, anywhere. A block's text, a tool's input, a tool's output: each
//! travels as a slice of the line it arrived on and is never opened. Reading a block's `type` is the same act as
//! reading the envelope's `type`, one level further in, and it is what lets a tool call be shown as a tool call
//! instead of as prose. A mapping change still cannot start leaking a conversation, because nothing here ever
//! looks inside the slice it cuts.
//!
//! # Nothing is dropped
//!
//! A frame with no binding becomes an unmapped event carrying its own tag and its whole body. A vendor shipping
//! something new is then a frame a subscriber can render or ignore, rather than an outage. That is the direct
//! answer to how the last project in this space died.

use std::collections::BTreeMap;

use bytes::Bytes;
use runtrol_provider::{
    CapabilitySet, Chunk, EventBody, Level, MessageId, NativeSessionId, Notice, NoticeCode, Opaque,
    RateLimit, StopReason, ToolCallFrame, ToolCallId, ToolCallStatus, Unmapped, WallMs, Window,
};
use serde::Deserialize;
use serde_json::value::RawValue;

use crate::claude::approval::{self, IncomingApproval};
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

    /// A control request could not be correlated or safely represented.
    #[error("the control request is not usable: {detail}")]
    BadControl {
        /// Why it was refused, without any provider payload.
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
    /// One frame that is several events, in the order the provider laid them out.
    ///
    /// A whole message is a list of content blocks, and the blocks are different kinds of thing: what the agent
    /// said, what it thought, and the tool calls it made. Relaying that list as a single chunk shows a tool call
    /// as if it were prose, which is the opposite of showing the conversation the way the CLI shows it.
    ///
    /// A separate variant rather than a list on [`Frame::Body`] because the streaming path is one event per
    /// frame and by far the hottest, and it keeps allocating nothing. Split into a first and a rest so the list
    /// is non-empty by construction and no consumer has to invent behaviour for a frame that is no events.
    Bodies {
        /// The first event, which the driver returns.
        first: EventBody,
        /// The others, in order, which the driver queues behind it.
        rest: Vec<EventBody>,
    },
    /// The turn ended, by the provider's own declaration.
    Ended(Box<Ended>),
    /// The provider is waiting for a human choice.
    Approval(Box<IncomingApproval>),
    /// The provider withdrew a human choice.
    ApprovalCancelled(ControlCancellation),
    /// A control request runtrol cannot serve, which must receive an error rather than hang.
    UnsupportedControl(UnsupportedControl),
    /// A reply to a control request runtrol sent, consumed by the stateful driver boundary.
    ///
    /// Carried with its identity and outcome because the boundary correlates it: an interrupt reply says
    /// nothing a turn event does not, but a model-switch reply is the CLI's word on whether the model moved.
    ControlResponse(ControlOutcome),
    /// Nothing runtrol binds, carried through whole.
    Unbound(Unmapped),
}

/// The provider's reply to a control request runtrol sent.
#[derive(Clone, Debug)]
pub struct ControlOutcome {
    /// Which request this answers, exactly as runtrol minted it.
    pub(super) request_id: Box<str>,
    /// The provider's own sentence when it refused. `None` is success.
    pub(super) error: Option<Box<str>>,
    /// The whole reply, untouched.
    pub(super) payload: Opaque,
}

/// A provider cancellation addressed by its private request handle.
#[derive(Clone, Debug)]
pub struct ControlCancellation {
    pub(super) native_request: Box<str>,
    pub(super) payload: Opaque,
}

/// A provider question outside the control surface runtrol serves.
#[derive(Clone, Debug)]
pub struct UnsupportedControl {
    pub(super) native_request: Box<str>,
    pub(super) subtype: Box<str>,
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
    /// The permission mode in force at attachment, by the CLI's own name for it.
    pub starting_mode: Option<Box<str>>,
    /// Whether the frame names the CLI's slash commands.
    ///
    /// This CLI announces them only here, inside the frame it says hello with (measured: no separate
    /// update follows). The flag lets the agent re-emit the whole frame as the one dedicated
    /// commands event every service shares.
    pub announces_commands: bool,
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
    /// The running cost the terminal frame reported, in USD (the frame states the unit in the field name).
    ///
    /// The CLI's own single stated figure, read as it is. `None` when the frame carried none, and never a
    /// number runtrol arrived at by arithmetic.
    pub cost: Option<f64>,
    /// The CLI's own token breakdown from the terminal frame, verbatim, for a subscriber that wants the tokens.
    ///
    /// Its own bytes, not a shape runtrol invented. `null` when the frame carried none.
    pub usage_detail: Opaque,
    /// The whole terminal frame, unread beyond the cost and the usage above.
    ///
    /// Carries the timings and the permission denials too; a subscriber that wants them has them.
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
    /// A question on the stdio control channel.
    #[serde(default, borrow)]
    request: Option<&'line RawValue>,
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
    /// On the terminal frame: the running cost in USD, as the CLI states it.
    #[serde(default)]
    total_cost_usd: Option<f64>,
    /// On the terminal frame: the CLI's own token breakdown. Presence and bytes only; the numbers inside stay
    /// the vendor's words and travel verbatim rather than being lifted into typed fields.
    #[serde(default, borrow)]
    usage: Option<&'line RawValue>,
    /// On the startup frame: what it can do.
    #[serde(default)]
    capabilities: Option<Vec<&'line str>>,
    /// On the startup frame: its own version.
    #[serde(default)]
    claude_code_version: Option<&'line str>,
    /// On the startup frame: the model that will answer.
    #[serde(default)]
    model: Option<&'line str>,
    /// On the startup and status frames: the permission mode in force.
    #[serde(default, rename = "permissionMode")]
    permission_mode: Option<&'line str>,
    /// On the startup frame: whether it names its slash commands.
    ///
    /// Presence only. The list itself stays in the payload, which travels whole; lifting the names
    /// would be a second copy of the vendor's own words.
    #[serde(default, borrow)]
    slash_commands: Option<&'line RawValue>,
    /// On the quota frame: where the account stands.
    #[serde(default, borrow)]
    rate_limit_info: Option<&'line RawValue>,
}

/// The limit fields this CLI reports, measured on 2.1.246 with a real turn.
///
/// The measured payload: `status`, `resetsAt`, `rateLimitType`, `utilization`, `surpassedThreshold`,
/// `isUsingOverage`, and `unifiedWindows` holding one entry per window the account has.
///
/// # A note about what was believed before
///
/// This driver recorded, from 2.1.235, that the CLI reported no utilisation number at all, and the whole product
/// was arranged around that: the usage strip drew no bar for this service and said so. The number is there, and
/// it was read again only because the operator pointed at his own editor showing "94% of your weekly limit"
/// (2026-08-26). A measurement is true of the version it was taken on, and this one names its version for the
/// next person who has to decide whether to trust it.
#[derive(Deserialize)]
struct RateLimitInfo<'line> {
    /// The provider's own word for whether requests pass.
    #[serde(default)]
    status: Option<&'line str>,
    /// When the governing window resets, in unix seconds.
    ///
    /// Seconds established by magnitude: the measured value read as milliseconds lands in January 1970, three
    /// weeks after the epoch, which is not a time this CLI resets anything.
    #[serde(default, rename = "resetsAt")]
    resets_at: Option<u64>,
    /// How full the governing window is, as a fraction from zero to one.
    #[serde(default)]
    utilization: Option<f64>,
    /// Every window the account has, by the provider's own names.
    #[serde(default, rename = "unifiedWindows")]
    unified_windows: Option<BTreeMap<String, RateLimitWindow>>,
}

/// One window inside `unifiedWindows`.
#[derive(Deserialize)]
struct RateLimitWindow {
    /// How full that window is, as a fraction from zero to one.
    #[serde(default)]
    utilization: Option<f64>,
    /// When that window resets, in unix seconds.
    #[serde(default, rename = "resetsAt")]
    resets_at: Option<u64>,
}

/// The window names this CLI uses, and how long each one is.
///
/// Named rather than parsed: the length is what the strip labels a bar with, and inventing a length from a name
/// this build has not seen would put a wrong label on a real number. An unknown name still draws its bar, with
/// no length claimed.
const WINDOW_MINUTES: &[(&str, u32)] = &[("five_hour", 300), ("seven_day", 10_080)];

/// Turn one reported window into the shape a gauge draws.
fn window_of(name: &str, window: &RateLimitWindow) -> Option<Window> {
    if window.utilization.is_none() && window.resets_at.is_none() {
        return None;
    }
    Some(Window {
        used_percent: window.utilization.map(percent_of),
        resets_at: window
            .resets_at
            .map(|seconds| WallMs::from_millis(seconds.saturating_mul(1_000))),
        window_minutes: WINDOW_MINUTES
            .iter()
            .find(|(known, _)| *known == name)
            .map(|(_, minutes)| *minutes),
    })
}

/// A fraction from zero to one as the whole percent a bar can draw.
///
/// Clamped rather than trusted: the bar is a proportion, and a value outside the range would either overflow the
/// cast or draw a bar longer than its track.
fn percent_of(fraction: f64) -> u8 {
    let scaled = (fraction * 100.0).round();
    if scaled.is_nan() {
        return 0;
    }
    scaled.clamp(0.0, 100.0) as u8
}

/// The account's limit position, from the fields this CLI reports.
fn rate_limit(line: &Bytes, envelope: &Envelope<'_>) -> RateLimit {
    // An unreadable report is not silence: both fields fall back to their conservative readings (no window, and
    // a status that is not known-good), and the whole payload still travels in `detail` for whoever wants the
    // vendor's own words.
    let read = match envelope
        .rate_limit_info
        .map(|raw| serde_json::from_str::<RateLimitInfo<'_>>(raw.get()))
    {
        Some(Ok(read)) => Some(read),
        Some(Err(_)) | None => None,
    };
    let status = read.as_ref().and_then(|read| read.status);
    // The shortest window first, because that is the one a person is about to hit. The provider names its
    // windows and this build knows how long two of them are; an unknown name still draws, unlabelled.
    let mut windows: Vec<Window> = read
        .as_ref()
        .and_then(|read| read.unified_windows.as_ref())
        .map(|reported| {
            let mut ordered: Vec<(u32, Window)> = reported
                .iter()
                .filter_map(|(name, window)| {
                    window_of(name, window)
                        .map(|drawn| (drawn.window_minutes.unwrap_or(u32::MAX), drawn))
                })
                .collect();
            ordered.sort_by_key(|(minutes, _)| *minutes);
            ordered.into_iter().map(|(_, drawn)| drawn).collect()
        })
        .unwrap_or_default();
    if windows.is_empty() {
        // Older builds of this CLI reported one governing window at the top level and no `unifiedWindows`. That
        // report is still a real limit position and still draws.
        if let Some(read) = read.as_ref() {
            if read.utilization.is_some() || read.resets_at.is_some() {
                windows.push(Window {
                    used_percent: read.utilization.map(percent_of),
                    resets_at: read
                        .resets_at
                        .map(|seconds| WallMs::from_millis(seconds.saturating_mul(1_000))),
                    window_minutes: None,
                });
            }
        }
    }
    let mut windows = windows.into_iter();
    RateLimit {
        primary: windows.next(),
        secondary: windows.next(),
        // Only the provider's own good word means requests pass. A status this build has never seen reads as
        // reached, which errs loudly in a warning colour rather than silently as "no limit exists": the same
        // rule stop reasons follow, where not understood is never rendered as success.
        reached: status != Some("allowed"),
        detail: payload_of(line, envelope.rate_limit_info),
    }
}

/// A fragment's nested kind.
#[derive(Deserialize)]
struct Fragment<'line> {
    /// The nested kind, which is the one that matters.
    #[serde(rename = "type")]
    kind: Option<&'line str>,
    /// On the fragment that opens a message: the message it opens, whose identifier binds every later delta.
    #[serde(default, borrow)]
    message: Option<&'line RawValue>,
}

/// The one thing runtrol reads about a whole message: whether its content is a list of blocks, and which
/// message the provider says it is.
#[derive(Deserialize)]
struct Message<'line> {
    /// The provider's own identifier for the message. Measured on the real CLI (2.1.237): the `assistant`
    /// frame and the `message_start` fragment both carry it, and the deltas in between carry nothing, so
    /// this is the one name that joins a streamed message to its whole.
    #[serde(default)]
    id: Option<&'line str>,
    /// The blocks, when the provider sent a list rather than a bare string.
    #[serde(default, borrow)]
    content: Option<Vec<&'line RawValue>>,
}

/// The identifier a whole message (or the fragment that opens one) carries, when it carries one.
fn message_identity(message: Option<&RawValue>) -> Option<&str> {
    match serde_json::from_str::<Message<'_>>(message?.get()) {
        Ok(message) => message.id,
        // Not a message object at all. The caller falls back to the exchange identifier, which is what every
        // frame carried before the message's own name was read; nothing is lost but the better name.
        Err(_) => None,
    }
}

/// A content block's kind, and the identifiers that bind a call to the result that answers it.
///
/// Every field here is a discriminator or a handle. What the block contains has no field on this struct, which
/// is the mechanical reason no later edit starts reading it by accident.
#[derive(Deserialize)]
struct Block<'line> {
    /// Which kind of block.
    #[serde(rename = "type")]
    kind: Option<&'line str>,
    /// On a call: the provider's identifier for it.
    #[serde(default)]
    id: Option<&'line str>,
    /// On a result: which call it answers.
    #[serde(default)]
    tool_use_id: Option<&'line str>,
    /// On a result: whether the tool failed.
    #[serde(default)]
    is_error: Option<bool>,
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

        // Whatever its subtype, this frame ends the turn. The subtype says how, which `ended` reads: binding the
        // pair instead would leave a turn that ended some other way running forever.
        (terminal, _) if terminal == TERMINAL.kind => {
            Ok(Frame::Ended(Box::new(ended(line, &envelope))))
        }

        ("assistant" | "user", _) => Ok(whole_message(line, kind, &envelope)),

        ("stream_event", _) => Ok(Frame::Body(fragment(line, &envelope))),

        ("rate_limit_event", _) => Ok(Frame::Body(EventBody::RateLimitUpdate(Box::new(
            rate_limit(line, &envelope),
        )))),

        ("control_request", _) => control_request(line, &envelope),

        ("control_cancel_request", _) => {
            let native_request =
                approval::bounded_request_id(envelope.request_id).map_err(|error| {
                    MapError::BadControl {
                        detail: error.to_string(),
                    }
                })?;
            Ok(Frame::ApprovalCancelled(ControlCancellation {
                native_request,
                payload: whole_line(line),
            }))
        }

        ("control_response", _) => control_response(line),

        // A status frame carrying `permissionMode` is the CLI's own announcement of which mode now
        // governs (measured: one follows every accepted `set_permission_mode`, and the CLI also announces
        // on its own schedule). The identifier is routing, not conversation. A status without one stays the
        // generic progress notice below.
        ("system", Some("status")) if envelope.permission_mode.is_some() => {
            match envelope.permission_mode {
                Some(mode) => Ok(Frame::Body(EventBody::CurrentModeUpdate {
                    mode_id: mode.into(),
                    available_ids: None,
                    payload: whole_line(line),
                })),
                // The guard above makes this unreachable; answered as the generic notice rather than
                // panicking, because a mapping layer never gets to crash the session.
                None => Ok(progress_notice(line)),
            }
        }

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

/// Bind one provider question on the hidden stdio control channel.
/// Read the reply to a control request runtrol sent: which request, and the CLI's own sentence if it refused.
/// Progress that is not conversation, reported without runtrol claiming to know what it is.
fn progress_notice(line: &Bytes) -> Frame {
    Frame::Body(EventBody::Notice(Box::new(Notice {
        level: Level::Info,
        code: NoticeCode::Other,
        retryable: false,
        payload: whole_line(line),
    })))
}

fn control_response(line: &Bytes) -> Result<Frame, MapError> {
    #[derive(serde::Deserialize)]
    struct Reply<'line> {
        #[serde(borrow)]
        response: ReplyBody<'line>,
    }
    #[derive(serde::Deserialize)]
    struct ReplyBody<'line> {
        subtype: &'line str,
        request_id: &'line str,
        error: Option<&'line str>,
    }
    let read: Reply<'_> =
        serde_json::from_slice(line.as_ref()).map_err(|error| MapError::BadControl {
            detail: format!("unreadable control response: {error}"),
        })?;
    let error = match (read.response.subtype, read.response.error) {
        ("success", _) => None,
        // A non-success reply without a sentence is still a refusal; the subtype itself is the word then.
        (_, sentence) => Some(sentence.unwrap_or(read.response.subtype).into()),
    };
    Ok(Frame::ControlResponse(ControlOutcome {
        request_id: read.response.request_id.into(),
        error,
        payload: whole_line(line),
    }))
}

fn control_request(line: &Bytes, envelope: &Envelope<'_>) -> Result<Frame, MapError> {
    let request = envelope.request.ok_or_else(|| MapError::BadControl {
        detail: "the frame carried no request body".to_owned(),
    })?;
    let parsed = serde_json::from_str::<ControlKind<'_>>(request.get()).map_err(|_| {
        MapError::BadControl {
            detail: "the request body was not readable".to_owned(),
        }
    })?;
    if parsed.subtype == Some("can_use_tool") {
        let incoming =
            IncomingApproval::read(line, envelope.request_id, Some(request)).map_err(|error| {
                MapError::BadControl {
                    detail: error.to_string(),
                }
            })?;
        return Ok(Frame::Approval(Box::new(incoming)));
    }

    let native_request = approval::bounded_request_id(envelope.request_id).map_err(|error| {
        MapError::BadControl {
            detail: error.to_string(),
        }
    })?;
    Ok(Frame::UnsupportedControl(UnsupportedControl {
        native_request,
        subtype: parsed.subtype.unwrap_or("unknown").into(),
    }))
}

#[derive(Deserialize)]
struct ControlKind<'line> {
    #[serde(default)]
    subtype: Option<&'line str>,
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
        starting_mode: envelope.permission_mode.map(Into::into),
        announces_commands: envelope.slash_commands.is_some(),
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
        // The frame declared success and did not say why it stopped. Its own subtype is the answer, and it is the
        // only thing here that is allowed to mean a turn finished.
        None if envelope.subtype == Some("success") => StopReason::EndTurn,
        // No reason given and nothing saying it succeeded. Not understanding why something stopped is not the same
        // as being told it finished, and rendering the second for the first is how an operator comes to trust a
        // completion that never happened.
        // These two read the same and mean different things: one is a frame that gave no reason at all, the other
        // gave one this build has no binding for. Spelled apart so that giving either its own answer later is an
        // edit rather than an untangling.
        #[expect(
            clippy::match_same_arms,
            reason = "no reason given and a reason not understood are different facts that share an answer today"
        )]
        None => StopReason::Unknown,
        Some(_) => StopReason::Unknown,
    };
    Ended {
        stop,
        failed,
        cost: envelope.total_cost_usd,
        // The usage object's own bytes, sliced out of the line without a copy. A slice that somehow does not
        // point inside the line answers as nothing rather than guessing, the same rule `whole_line` follows.
        usage_detail: envelope
            .usage
            .and_then(|usage| Opaque::borrowed_from(line, usage.get()))
            .unwrap_or_else(Opaque::none),
        payload: whole_line(line),
    }
}

/// A whole message, from either side, as the events its content blocks are.
fn whole_message(line: &Bytes, kind: &str, envelope: &Envelope<'_>) -> Frame {
    let user = kind == "user";
    // The message's own identifier first, the exchange identifier as the fallback. The deltas that streamed
    // this message carried the message identifier (from its opening fragment), never the exchange one, so
    // this is what lets a subscriber recognise the whole as the message it already showed piece by piece.
    let id =
        message_id(message_identity(envelope.message)).or_else(|| message_id(envelope.request_id));
    let parent = tool_call(envelope.parent_tool_use_id);

    let mut bodies = content_blocks(envelope.message)
        .into_iter()
        .map(|block| block_body(line, user, id.clone(), parent.clone(), block));

    let Some(first) = bodies.next() else {
        // A message whose content is not a readable list of blocks is relayed whole, which is what this did
        // before it read blocks at all. A vendor changing the container degrades to one chunk, not to silence.
        return Frame::Body(said(user, id, parent, payload_of(line, envelope.message)));
    };
    let rest: Vec<EventBody> = bodies.collect();
    if rest.is_empty() {
        Frame::Body(first)
    } else {
        Frame::Bodies { first, rest }
    }
}

/// The content blocks of a whole message, empty when there is no readable list of them.
fn content_blocks<'line>(message: Option<&'line RawValue>) -> Vec<&'line RawValue> {
    let Some(message) = message else {
        return Vec::new();
    };
    match serde_json::from_str::<Message<'line>>(message.get()) {
        // No list, either because `content` is absent or because it is the bare string the provider also sends.
        // Both mean the same thing to the caller, which relays the message whole, so nothing is lost by not
        // telling them apart here.
        Ok(message) => message.content.unwrap_or_default(),
        // A message that is not an object at all. Same answer and same reason: the caller relays it whole.
        Err(_) => Vec::new(),
    }
}

/// One content block as the event it is.
fn block_body(
    line: &Bytes,
    user: bool,
    id: Option<MessageId>,
    parent: Option<ToolCallId>,
    block: &RawValue,
) -> EventBody {
    let payload = Opaque::borrowed_from(line, block.get()).unwrap_or_else(|| whole_line(line));
    let read: Block<'_> = match serde_json::from_str(block.get()) {
        Ok(read) => read,
        // A block whose shape is not this one is still somebody's content. Shown as content it is visible;
        // refused it would be gone.
        Err(_) => return said(user, id, parent, payload),
    };

    match read.kind {
        // Thinking is rendered somewhere else entirely, which is why it is told apart here and not downstream.
        Some("thinking" | "redacted_thinking") => EventBody::AgentThoughtChunk(Chunk {
            message_id: id,
            delta: false,
            parent,
            content: payload,
        }),

        Some("tool_use" | "server_tool_use" | "mcp_tool_use") => match tool_call(read.id) {
            Some(tool_call_id) => EventBody::ToolCall(ToolCallFrame {
                tool_call_id,
                // This CLI does not classify its own tools. Inferring a kind from a tool name is the hardcoding
                // the discovery rule forbids, and it would be silently wrong the first time a tool is renamed.
                kind: None,
                status: Some(ToolCallStatus::InProgress),
                delta: false,
                payload,
            }),
            // Without a usable identifier no result could ever be bound to this call. Shown as content rather
            // than as a call that would sit unfinished forever.
            None => said(user, id, parent, payload),
        },

        Some("tool_result" | "mcp_tool_result") => match tool_call(read.tool_use_id) {
            Some(tool_call_id) => EventBody::ToolCallUpdate(ToolCallFrame {
                tool_call_id,
                kind: None,
                status: Some(if read.is_error == Some(true) {
                    ToolCallStatus::Failed
                } else {
                    ToolCallStatus::Completed
                }),
                delta: false,
                payload,
            }),
            None => said(user, id, parent, payload),
        },

        // Text, images, documents, and whatever a vendor adds next. All of it is content and all of it is shown.
        _ => said(user, id, parent, payload),
    }
}

/// A block shown as something one of the two sides said.
fn said(
    user: bool,
    id: Option<MessageId>,
    parent: Option<ToolCallId>,
    content: Opaque,
) -> EventBody {
    let chunk = Chunk {
        message_id: id,
        delta: false,
        parent,
        content,
    };
    if user {
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

    // The opening fragment names the message; the deltas after it name nothing (measured on the real CLI),
    // and the stateful session fills them in from the opening it saw. The exchange identifier is kept as the
    // fallback for a CLI that does put it on fragments.
    let opened = envelope.event.and_then(|event| {
        match serde_json::from_str::<Fragment<'_>>(event.get()) {
            Ok(fragment) => fragment.message,
            // A fragment whose shape is not this one opens no message; it is relayed nameless and the open
            // message's name (if any) is supplied by the session, exactly like every other delta.
            Err(_) => None,
        }
    });
    let chunk = Chunk {
        message_id: message_id(message_identity(opened))
            .or_else(|| message_id(envelope.request_id)),
        delta: true,
        parent: tool_call(envelope.parent_tool_use_id),
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

/// A tool call named by the provider, when the name is one runtrol will hold.
///
/// Answers three questions with one rule: which call a block is, which call a result answers, and which call a
/// subagent's output belongs under.
///
/// Same reasoning as [`message_id`]: without it the content is shown at the top level instead of attached, which
/// is a worse rendering and not a lost message.
fn tool_call(text: Option<&str>) -> Option<ToolCallId> {
    let Ok(id) = ToolCallId::new(text?) else {
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

    #[test]
    fn a_status_frame_with_a_permission_mode_is_the_cli_announcing_its_mode() {
        // The measured announcement that follows an accepted set_permission_mode (2026-08-19 probe).
        let announced = read(&line(
            r#"{"type":"system","subtype":"status","status":null,"permissionMode":"acceptEdits","uuid":"107e9eb0-a37b-4b4e-80fc-5728e269d152","session_id":"dda14c4d-17ac-42b9-9e28-bb3fb6c36e92"}"#,
        ))
        .expect("the measured line maps");
        match announced {
            Frame::Body(EventBody::CurrentModeUpdate {
                mode_id,
                available_ids,
                ..
            }) => {
                assert_eq!(&*mode_id, "acceptEdits");
                assert!(
                    available_ids.is_none(),
                    "the status frame names only the mode in force"
                );
            }
            other => panic!("expected the mode event, got {other:?}"),
        }

        // A status frame without a mode stays the generic notice it always was.
        let plain = read(&line(
            r#"{"type":"system","subtype":"status","status":null}"#,
        ))
        .expect("the plain status maps");
        assert!(
            matches!(plain, Frame::Body(EventBody::Notice(_))),
            "a modeless status is progress, not a mode announcement"
        );
    }

    /// The startup frame, with the field set observed on 2.1.220 and its content shortened.
    fn recorded_startup() -> Bytes {
        line(&format!(
            r#"{{"type":"system","subtype":"init","session_id":"{SESSION}","uuid":"a-uuid","cwd":"C:\\work","model":"claude-haiku-4-5-20251001","permissionMode":"default","apiKeySource":"none","claude_code_version":"2.1.220","capabilities":["streamingInput","hooks"],"tools":["Read","Bash"],"skills":[],"agents":[],"mcp_servers":[],"plugins":[],"slash_commands":["compact"],"memory_paths":[],"output_style":"default"}}"#
        ))
    }

    /// The terminal frame, copied from one a session runtrol started and prompted actually produced.
    ///
    /// Copied rather than composed. A hand-written version of this frame named a kind the CLI does not send, every
    /// test here agreed with it, and the defect only surfaced when the product was run: the ending came out of the
    /// far end as something runtrol had no binding for. Shortened only where a field's content is long.
    fn recorded_terminal() -> Bytes {
        line(&format!(
            r#"{{"type":"result","subtype":"success","is_error":false,"duration_api_ms":4602,"num_turns":1,"stop_reason":"end_turn","session_id":"{SESSION}","total_cost_usd":0.0917765,"usage":{{"input_tokens":2,"output_tokens":5}},"modelUsage":{{}},"permission_denials":[],"terminal_reason":"completed","result":"PONG","ttft_ms":3459,"duration_ms":3640,"uuid":"9f17c389-eeb6-4a74-bc1f-850d45d6b9aa"}}"#
        ))
    }

    /// The captured frame with the three fields runtrol decides on set to something else.
    ///
    /// Written as a modification rather than as another frame, so that what a variant differs by is visible and
    /// nothing here can drift into being a second, invented shape.
    fn terminal_saying(subtype: &str, stop_reason: Option<&str>, is_error: bool) -> Bytes {
        let stop = stop_reason.map_or_else(String::new, |reason| {
            format!(r#","stop_reason":"{reason}""#)
        });
        line(&format!(
            r#"{{"type":"result","subtype":"{subtype}","session_id":"{SESSION}","is_error":{is_error},"result":"PONG"{stop}}}"#
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
                assert!(
                    startup.announces_commands,
                    "the recorded frame names slash_commands, and that presence is the flag"
                );
            }
            other => panic!("expected a startup, got {other:?}"),
        }
    }

    #[test]
    fn a_startup_without_slash_commands_announces_none() {
        // Presence is the only thing read: an init frame that says nothing about commands must not
        // produce a commands event, because an empty dedicated update means withdrawal to a reader.
        let bare = line(&format!(
            r#"{{"type":"system","subtype":"init","session_id":"{SESSION}","capabilities":[]}}"#
        ));
        match read(&bare).expect("readable") {
            Frame::Started(startup) => assert!(!startup.announces_commands),
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
    fn the_frame_the_product_received_is_the_one_that_ends_a_turn() {
        // The whole reason this file exists. Watched coming out of a real session: an ending that is not
        // recognised leaves the turn running forever and nothing anywhere says why.
        match read(&recorded_terminal()).expect("readable") {
            Frame::Ended(ended) => {
                assert_eq!(ended.stop, StopReason::EndTurn);
                assert!(!ended.failed);
                assert!(ended.stop.is_success());
            }
            other => panic!("expected an ending, got {other:?}"),
        }
    }

    #[test]
    fn the_ending_frame_carries_the_cost_and_the_usage_verbatim() {
        // This CLI states its running cost and its token breakdown on the ending frame. The cost is read as the
        // one number it is (compared with a tolerance, never bit-for-bit), and the usage object rides along as
        // the vendor's own bytes so a surface can show spend without runtrol summing anything.
        match read(&recorded_terminal()).expect("readable") {
            Frame::Ended(ended) => {
                let cost = ended.cost.expect("the frame stated a cost");
                assert!((cost - 0.091_776_5).abs() < 1e-9, "cost was {cost}");
                assert!(
                    ended.usage_detail.as_str().contains("input_tokens"),
                    "the vendor usage object travels verbatim: {}",
                    ended.usage_detail.as_str()
                );
            }
            other => panic!("expected an ending, got {other:?}"),
        }
    }

    #[test]
    fn an_ending_without_a_cost_stays_none_rather_than_becoming_a_zero() {
        // A frame that stated no cost must not read as a cost of zero: zero reads as "free", and inventing one is
        // exactly the quiet wrongness the usage line exists to avoid.
        match read(&terminal_saying("success", None, false)).expect("readable") {
            Frame::Ended(ended) => {
                assert!(ended.cost.is_none(), "no cost stated means no cost shown");
                assert_eq!(ended.usage_detail.as_str(), "null");
            }
            other => panic!("expected an ending, got {other:?}"),
        }
    }

    #[test]
    fn a_turn_that_ended_some_other_way_still_ends() {
        // Only `success` has been observed. Binding the pair rather than the kind would mean any other subtype
        // ends nothing, and the case where an operator most needs to be told would be the one that goes silent.
        let other_way = terminal_saying("error_during_execution", None, true);
        match read(&other_way).expect("readable") {
            Frame::Ended(ended) => {
                assert!(ended.failed);
                assert!(!ended.stop.is_success());
            }
            other => panic!("a turn that ended badly has to end: {other:?}"),
        }
    }

    #[test]
    fn a_missing_stop_reason_is_answered_by_the_subtype_and_never_assumed() {
        // A frame that declared success and did not say why it stopped is a finished turn. One that says neither
        // is not, and reading it as success is how an operator comes to trust a completion that never happened.
        match read(&terminal_saying("success", None, false)).expect("readable") {
            Frame::Ended(ended) => assert_eq!(ended.stop, StopReason::EndTurn),
            other => panic!("expected an ending, got {other:?}"),
        }
        match read(&terminal_saying("something_new", None, false)).expect("readable") {
            Frame::Ended(ended) => {
                assert_eq!(ended.stop, StopReason::Unknown);
                assert!(!ended.stop.is_success());
            }
            other => panic!("expected an ending, got {other:?}"),
        }
    }

    #[test]
    fn a_stop_reason_runtrol_does_not_know_is_named_as_not_understood() {
        // Not understanding a reason is not the same as being given one. Rendering an unknown token as success is
        // how an operator comes to trust a completion that never happened.
        let odd = terminal_saying("success", Some("something_new_from_the_vendor"), false);
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
        let failed = terminal_saying("success", None, true);
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
    fn a_streamed_message_and_its_whole_share_the_message_identifier() {
        // Measured on the real CLI (2.1.237): `message_start` carries `event.message.id`, the deltas carry
        // nothing, and the final `assistant` frame carries `message.id` (plus a request_id the fragments never
        // had). A subscriber joins the pieces to the whole by the message identifier, so that is the one used
        // wherever it exists; the exchange identifier is only the fallback.
        let opening = line(&format!(
            r#"{{"type":"stream_event","session_id":"{SESSION}","event":{{"type":"message_start","message":{{"id":"msg_01","role":"assistant","content":[]}}}}}}"#
        ));
        match read(&opening).expect("readable") {
            Frame::Body(body) => assert_eq!(
                body.message_id().map(|id| id.as_str().to_owned()),
                Some("msg_01".to_owned())
            ),
            other => panic!("expected content, got {other:?}"),
        }
        let delta = line(&format!(
            r#"{{"type":"stream_event","session_id":"{SESSION}","event":{{"type":"content_block_delta","index":0,"delta":{{"type":"text_delta","text":"h"}}}}}}"#
        ));
        match read(&delta).expect("readable") {
            Frame::Body(body) => {
                assert!(body.message_id().is_none(), "a bare delta names no message");
            }
            other => panic!("expected content, got {other:?}"),
        }
        let whole = line(&format!(
            r#"{{"type":"assistant","session_id":"{SESSION}","request_id":"req_01","message":{{"id":"msg_01","role":"assistant","content":[{{"type":"text","text":"hello"}}]}}}}"#
        ));
        match read(&whole).expect("readable") {
            Frame::Body(body) => assert_eq!(
                body.message_id().map(|id| id.as_str().to_owned()),
                Some("msg_01".to_owned()),
                "the whole message is named by the message, not the exchange"
            ),
            other => panic!("expected content, got {other:?}"),
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

    /// A whole assistant message with the block list the CLI actually sends.
    ///
    /// Copied from a session on 2.1.233 and shortened. Composed by hand it would have been a list of one text
    /// block, which is the shape that already worked and the reason a tool call went unshown for so long.
    fn recorded_speech_and_a_call() -> Bytes {
        line(&format!(
            r#"{{"type":"assistant","session_id":"{SESSION}","request_id":"req_01","message":{{"id":"msg_01","role":"assistant","content":[{{"type":"text","text":"Let me look at the tree."}},{{"type":"tool_use","id":"toolu_01","name":"Bash","input":{{"command":"git status","description":"Show working tree status"}}}}]}}}}"#
        ))
    }

    /// The result frame that answers it, which this CLI sends as a user message.
    fn recorded_result(is_error: &str) -> Bytes {
        line(&format!(
            r#"{{"type":"user","session_id":"{SESSION}","message":{{"role":"user","content":[{{"type":"tool_result","tool_use_id":"toolu_01","content":"On branch main"{is_error}}}]}}}}"#
        ))
    }

    #[test]
    fn a_tool_call_is_shown_as_a_tool_call_and_not_as_prose() {
        // The flagship case. One message is a list of blocks and the blocks are different kinds of thing.
        // Relayed as a single chunk, every tool this CLI ran arrived as prose, which is to say as nothing a
        // subscriber could fold, label, or attach a result to.
        match read(&recorded_speech_and_a_call()).expect("readable") {
            Frame::Bodies { first, rest } => {
                assert!(
                    matches!(first, EventBody::AgentMessageChunk(_)),
                    "what it said comes first, in the order the provider wrote it"
                );
                assert_eq!(rest.len(), 1, "and the call is its own event");
                match rest.first().expect("the rest is not empty") {
                    EventBody::ToolCall(frame) => {
                        assert_eq!(frame.tool_call_id.as_str(), "toolu_01");
                        assert_eq!(frame.status, Some(ToolCallStatus::InProgress));
                        assert!(
                            frame.kind.is_none(),
                            "this CLI does not classify its tools and runtrol does not guess"
                        );
                    }
                    other => panic!("expected a tool call, got {other:?}"),
                }
            }
            other => panic!("expected several events out of one message, got {other:?}"),
        }
    }

    #[test]
    fn a_result_completes_the_call_it_answers() {
        // The identifier is the whole point: without it a subscriber has a call that never finishes and a result
        // floating loose beside it.
        match read(&recorded_result("")).expect("readable") {
            Frame::Body(EventBody::ToolCallUpdate(frame)) => {
                assert_eq!(frame.tool_call_id.as_str(), "toolu_01");
                assert_eq!(frame.status, Some(ToolCallStatus::Completed));
            }
            other => panic!("expected an update to the call, got {other:?}"),
        }
    }

    #[test]
    fn a_tool_that_failed_says_so() {
        // A failed call is one of the few things worth waking a phone for, so it cannot be reported as finished.
        match read(&recorded_result(r#","is_error":true"#)).expect("readable") {
            Frame::Body(EventBody::ToolCallUpdate(frame)) => {
                assert_eq!(frame.status, Some(ToolCallStatus::Failed));
            }
            other => panic!("expected an update to the call, got {other:?}"),
        }
    }

    #[test]
    fn speech_and_thinking_in_one_message_are_told_apart() {
        // They arrive in the same block list and a subscriber renders them in different places.
        let both = line(&format!(
            r#"{{"type":"assistant","session_id":"{SESSION}","message":{{"content":[{{"type":"thinking","thinking":"weighing it up","signature":"sig"}},{{"type":"text","text":"here is the answer"}}]}}}}"#
        ));
        match read(&both).expect("readable") {
            Frame::Bodies { first, rest } => {
                assert!(matches!(first, EventBody::AgentThoughtChunk(_)));
                assert!(matches!(
                    rest.first().expect("the rest is not empty"),
                    EventBody::AgentMessageChunk(_)
                ));
            }
            other => panic!("expected two events, got {other:?}"),
        }
    }

    #[test]
    fn each_block_carries_its_own_slice_and_not_its_neighbours() {
        // A block is cut out of the line it arrived on and handed over unopened. Handing over the whole line
        // instead would show every block's content under every block.
        match read(&recorded_speech_and_a_call()).expect("readable") {
            Frame::Bodies { first, rest } => {
                let EventBody::AgentMessageChunk(said) = first else {
                    panic!("expected speech first");
                };
                assert!(said.content.as_str().contains("Let me look at the tree"));
                assert!(
                    !said.content.as_str().contains("git status"),
                    "what it said must not carry what it ran"
                );
                let Some(EventBody::ToolCall(call)) = rest.first() else {
                    panic!("expected a call second");
                };
                assert!(
                    call.payload.as_str().contains("Show working tree status"),
                    "the call's own input is what a subscriber renders it from"
                );
                assert!(
                    !call.payload.as_str().contains("Let me look at the tree"),
                    "and it must not carry what it said"
                );
            }
            other => panic!("expected several events, got {other:?}"),
        }
    }

    #[test]
    fn a_call_with_no_usable_identifier_is_shown_rather_than_left_unfinished() {
        // Nothing could ever be bound to it. Shown as content it is at least visible; shown as a call it would
        // sit spinning until the session ended.
        let nameless = line(&format!(
            r#"{{"type":"assistant","session_id":"{SESSION}","message":{{"content":[{{"type":"tool_use","name":"Bash","input":{{}}}}]}}}}"#
        ));
        match read(&nameless).expect("readable") {
            Frame::Body(EventBody::AgentMessageChunk(chunk)) => {
                assert!(chunk.content.as_str().contains("tool_use"));
            }
            other => panic!("expected content, got {other:?}"),
        }
    }

    #[test]
    fn a_block_kind_nobody_bound_is_still_shown() {
        // The same answer the frame level gives a frame nobody bound. An image today, whatever a vendor adds
        // next tomorrow: it is content and a subscriber gets it.
        let image = line(&format!(
            r#"{{"type":"user","session_id":"{SESSION}","message":{{"content":[{{"type":"image","source":{{"type":"base64","media_type":"image/png"}}}}]}}}}"#
        ));
        match read(&image).expect("readable") {
            Frame::Body(EventBody::UserMessageChunk(chunk)) => {
                assert!(chunk.content.as_str().contains("image/png"));
            }
            other => panic!("expected content, got {other:?}"),
        }
    }

    #[test]
    fn the_operator_own_message_replayed_by_the_cli_is_shown_on_the_operator_side() {
        // Measured on 2.1.238 with `--replay-user-messages`: the CLI re-emits the stdin message as a `user`
        // frame marked `isReplay`. It is the operator's own words coming back through the provider, and the
        // only way they are shown; a local echo would be runtrol authoring a line of the conversation.
        let replayed = line(&format!(
            r#"{{"type":"user","message":{{"role":"user","content":[{{"type":"text","text":"Reply with exactly: ok"}}]}},"session_id":"{SESSION}","parent_tool_use_id":null,"uuid":"386c249b-b892-4757-8fd5-0c5a36e2e4e9","timestamp":"2026-08-21T01:37:50.273Z","isReplay":true}}"#
        ));
        match read(&replayed).expect("readable") {
            Frame::Body(EventBody::UserMessageChunk(chunk)) => {
                assert!(chunk.content.as_str().contains("Reply with exactly: ok"));
                assert!(!chunk.delta);
            }
            other => panic!("expected the operator's message, got {other:?}"),
        }
    }

    #[test]
    fn a_message_whose_content_is_not_a_list_is_relayed_whole() {
        // This CLI also sends `content` as a bare string, and a vendor is allowed to change the container. Either
        // way the message is relayed as one chunk, which is what this did before it read blocks at all.
        let bare = line(&format!(
            r#"{{"type":"assistant","session_id":"{SESSION}","message":{{"content":"just a sentence"}}}}"#
        ));
        match read(&bare).expect("readable") {
            Frame::Body(EventBody::AgentMessageChunk(chunk)) => {
                assert!(chunk.content.as_str().contains("just a sentence"));
            }
            other => panic!("expected one chunk, got {other:?}"),
        }
    }

    #[test]
    fn the_measured_limit_report_becomes_a_window_and_not_a_guess() {
        // The payload a 2.1.235 turn produced: a governing window and a reset, with no utilisation anywhere.
        // That build is still readable, and its window draws with no percentage rather than an invented one.
        let measured = line(&format!(
            r#"{{"type":"rate_limit_event","session_id":"{SESSION}","rate_limit_info":{{"status":"allowed","resetsAt":1787131200,"rateLimitType":"five_hour","overageStatus":"rejected","overageDisabledReason":"org_level_disabled","isUsingOverage":false}}}}"#
        ));
        match read(&measured).expect("readable") {
            Frame::Body(EventBody::RateLimitUpdate(limit)) => {
                assert!(
                    !limit.reached,
                    "the provider's own word for passing is allowed"
                );
                let window = limit.primary.expect("the reset instant makes a window");
                assert_eq!(
                    window.used_percent, None,
                    "no number was reported, so none is shown"
                );
                assert_eq!(
                    window.resets_at.map(WallMs::as_millis),
                    Some(1_787_131_200_000),
                    "the provider counts seconds and the vocabulary counts milliseconds"
                );
                assert!(
                    limit.detail.as_str().contains("five_hour"),
                    "which window governs stays in the provider's own payload"
                );
            }
            other => panic!("expected a limit update, got {other:?}"),
        }
    }

    #[test]
    fn the_measured_windows_become_two_bars_shortest_first() {
        // The exact payload a real turn produced on 2.1.246, byte for byte from `--output-format stream-json`.
        // Both windows carry a utilisation, which is the number the strip draws, and the shorter window leads
        // because that is the one a person is about to hit.
        let measured = line(&format!(
            r#"{{"type":"rate_limit_event","session_id":"{SESSION}","rate_limit_info":{{"status":"allowed_warning","resetsAt":1787835600,"rateLimitType":"seven_day","utilization":0.94,"isUsingOverage":false,"surpassedThreshold":0.75,"unifiedWindows":{{"five_hour":{{"utilization":0.21,"resetsAt":1787749800}},"seven_day":{{"utilization":0.94,"resetsAt":1787835600}}}}}}}}"#
        ));
        match read(&measured).expect("readable") {
            Frame::Body(EventBody::RateLimitUpdate(limit)) => {
                let primary = limit.primary.expect("the five hour window");
                assert_eq!(primary.used_percent, Some(21));
                assert_eq!(primary.window_minutes, Some(300));
                assert_eq!(
                    primary.resets_at.map(WallMs::as_millis),
                    Some(1_787_749_800_000)
                );
                let secondary = limit.secondary.expect("the seven day window");
                assert_eq!(secondary.used_percent, Some(94));
                assert_eq!(secondary.window_minutes, Some(10_080));
                assert!(
                    limit.reached,
                    "a warning is not the provider's word for passing"
                );
            }
            other => panic!("expected a limit update, got {other:?}"),
        }
    }

    #[test]
    fn a_top_level_utilisation_without_named_windows_still_draws() {
        // A build that reports the governing window at the top level and nothing under `unifiedWindows` is
        // still reporting a real position. Reading only the newer shape would have thrown it away.
        let older = line(&format!(
            r#"{{"type":"rate_limit_event","session_id":"{SESSION}","rate_limit_info":{{"status":"allowed","resetsAt":1787131200,"utilization":0.5}}}}"#
        ));
        match read(&older).expect("readable") {
            Frame::Body(EventBody::RateLimitUpdate(limit)) => {
                let window = limit.primary.expect("one window");
                assert_eq!(window.used_percent, Some(50));
                assert_eq!(
                    window.window_minutes, None,
                    "no name, so no length is claimed"
                );
                assert!(limit.secondary.is_none());
            }
            other => panic!("expected a limit update, got {other:?}"),
        }
    }

    #[test]
    fn a_limit_status_this_build_has_never_seen_reads_as_reached() {
        // The same rule stop reasons follow: not understood is never rendered as success. Erring this way is a
        // warning colour on screen; erring the other way is "no limit exists" said precisely when the provider
        // is talking about one.
        let novel = line(&format!(
            r#"{{"type":"rate_limit_event","session_id":"{SESSION}","rate_limit_info":{{"status":"something_new","resetsAt":1787131200}}}}"#
        ));
        match read(&novel).expect("readable") {
            Frame::Body(EventBody::RateLimitUpdate(limit)) => assert!(limit.reached),
            other => panic!("expected a limit update, got {other:?}"),
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
    fn the_control_channel_has_a_fail_closed_mapping() {
        let approval = line(
            r#"{"type":"control_request","request_id":"req-1","request":{"subtype":"can_use_tool","input":{"command":"cargo test"}}}"#,
        );
        assert!(matches!(
            read(&approval).expect("readable"),
            Frame::Approval(_)
        ));

        let unsupported = line(
            r#"{"type":"control_request","request_id":"req-2","request":{"subtype":"oauth_token_refresh"}}"#,
        );
        assert!(matches!(
            read(&unsupported).expect("readable"),
            Frame::UnsupportedControl(_)
        ));

        let cancelled = line(r#"{"type":"control_cancel_request","request_id":"req-1"}"#);
        assert!(matches!(
            read(&cancelled).expect("readable"),
            Frame::ApprovalCancelled(_)
        ));

        let response = line(
            r#"{"type":"control_response","response":{"subtype":"success","request_id":"runtrol-1","response":{}}}"#,
        );
        assert!(matches!(
            read(&response).expect("readable"),
            Frame::ControlResponse(_)
        ));
    }

    #[test]
    fn a_control_reply_carries_its_identity_and_the_cli_own_sentence_when_refused() {
        // Both payloads are the real CLI's, measured on 2.1.235 (2026-08-19): a set_model success and a
        // set_effort refusal.
        let accepted = line(
            r#"{"type":"control_response","response":{"subtype":"success","request_id":"probe-1"}}"#,
        );
        let Frame::ControlResponse(outcome) = read(&accepted).expect("readable") else {
            panic!("a control response mapped as something else");
        };
        assert_eq!(outcome.request_id.as_ref(), "probe-1");
        assert!(outcome.error.is_none());

        let refused = line(
            r#"{"type":"control_response","response":{"subtype":"error","request_id":"p1","error":"Unsupported control request subtype: set_effort"}}"#,
        );
        let Frame::ControlResponse(outcome) = read(&refused).expect("readable") else {
            panic!("a control refusal mapped as something else");
        };
        assert_eq!(outcome.request_id.as_ref(), "p1");
        assert_eq!(
            outcome.error.as_deref(),
            Some("Unsupported control request subtype: set_effort")
        );
    }
}
