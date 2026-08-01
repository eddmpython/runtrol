//! JSON-RPC over a child's standard streams: who asked what, and who is still waiting.
//!
//! One of these CLIs speaks a request and response protocol rather than a stream of updates, and it multiplexes
//! every session over one connection. That buys the memory win the tier design rests on (N sessions cost one
//! child) and it brings two problems this file owns.
//!
//! # Both sides ask questions
//!
//! Measured on the generated schema: 126 methods runtrol may call, 70 notifications it may receive, and **11
//! requests the provider makes of runtrol**. Those eleven are how an approval reaches a person. A reader that
//! only understood responses and notifications would silently drop every permission prompt, and the session
//! would appear to hang.
//!
//! So an incoming message is classified into three, and a message carrying an identifier and a method is a
//! question from the other side rather than an answer to ours.
//!
//! # Somebody is always waiting, and the waiting is what needs a bound
//!
//! Each request runtrol sends leaves a waiter behind until its answer arrives. A provider that stops answering
//! turns that into a map that grows forever, which is the same unbounded buffer the memory contract bans, wearing
//! different clothes. So the map is bounded and a request that would exceed it is refused by name.
//!
//! The other half of the same rule: when the connection goes away, **every waiter is told**. A caller awaiting a
//! response that will never come is the worst form of swallowed failure, because nothing anywhere reports it and
//! the session simply stops.
//!
//! # Unknown fields are the vendor's business
//!
//! Unlike a manifest, which refuses a key it does not know, an envelope from a provider is read for the fields
//! runtrol decides on and everything else is left alone. That asymmetry is deliberate: a manifest is written by
//! an operator who needs to be told about a typo, and a wire message is written by a vendor who is allowed to
//! add things. Ignoring what is not bound is what makes a vendor's new field a non-event instead of an outage.

use std::collections::BTreeMap;

use bytes::Bytes;
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use tokio::sync::oneshot;

/// How many answers runtrol will wait for at once.
///
/// Derived from the tier bound rather than chosen: at most eight sessions have a process, a session has at most a
/// small handful of questions outstanding, and the surface runtrol binds is about a dozen methods. Sixty-four is
/// generous against all of that. Its purpose is that a provider which stops answering cannot make this grow
/// without limit.
pub const MAX_PENDING: usize = 64;

/// The longest textual request identifier runtrol will accept.
///
/// The protocol permits a string identifier of any length. Accepting one without a bound would let a provider
/// decide how much memory the pending map costs per entry.
pub const MAX_ID_TEXT: usize = 128;

/// The protocol version runtrol writes.
///
/// Written even though the provider omits it in the other direction (measured: its outgoing frames have no such
/// field). The specification requires it, a server that omits it may still expect it, and emitting it costs
/// nothing. Its absence inbound is accepted for the same reason unknown fields are.
const VERSION: &str = "2.0";

/// A frame could not be read or written.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum FrameError {
    /// The line is not a JSON object.
    #[error("not a readable protocol frame: {detail}")]
    NotAFrame {
        /// What the reader said. Never the line itself, which may carry a conversation.
        detail: String,
    },

    /// The line is a JSON object and not any of the three shapes the protocol has.
    ///
    /// Told apart from an unreadable line because the fix is different: this is a provider sending something the
    /// protocol does not describe, and that is worth reporting as a protocol violation rather than as bad JSON.
    #[error("a frame with neither a method nor a result nor an error")]
    NotAMessage,

    /// A request identifier was neither a number nor a short string.
    #[error("a request identifier that is {what}")]
    BadId {
        /// What was wrong with it.
        what: &'static str,
    },

    /// Nothing that was sent can be written as a frame.
    #[error("cannot write a frame: {detail}")]
    NotWritable {
        /// What the writer said.
        detail: String,
    },
}

/// Which question an answer belongs to.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum RequestId {
    /// A number, which is what runtrol mints and what the supported provider uses.
    Number(i64),
    /// A string, which the protocol also permits.
    Text(Box<str>),
}

impl RequestId {
    /// Read an identifier out of an envelope.
    ///
    /// # Errors
    ///
    /// [`FrameError::BadId`] for anything that is not a number or a string within [`MAX_ID_TEXT`].
    pub fn parse(raw: &RawValue) -> Result<Self, FrameError> {
        let text = raw.get().trim();
        if let Some(inner) = text
            .strip_prefix('"')
            .and_then(|rest| rest.strip_suffix('"'))
        {
            if inner.len() > MAX_ID_TEXT {
                return Err(FrameError::BadId {
                    what: "a string longer than runtrol will hold",
                });
            }
            // Decoded rather than taken raw, so an escape inside the identifier means the same thing to runtrol
            // as it does to the provider.
            let decoded: Box<str> = serde_json::from_str::<String>(text)
                .map_err(|_| FrameError::BadId {
                    what: "a string runtrol cannot decode",
                })?
                .into();
            return Ok(Self::Text(decoded));
        }
        text.parse::<i64>()
            .map(Self::Number)
            .map_err(|_| FrameError::BadId {
                what: "neither a number nor a string",
            })
    }

    /// The identifier as it goes on the wire.
    fn to_wire(&self) -> serde_json::Value {
        match self {
            Self::Number(number) => serde_json::Value::from(*number),
            Self::Text(text) => serde_json::Value::from(text.to_string()),
        }
    }
}

impl core::fmt::Display for RequestId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Number(number) => write!(f, "{number}"),
            Self::Text(text) => f.write_str(text),
        }
    }
}

/// What the provider said went wrong.
///
/// The code and the message are read because runtrol decides on them: whether the session is degraded, whether
/// anything is worth retrying, whether a person has to act. The rest of the object is carried unread.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct WireError {
    /// The provider's own code.
    pub code: i64,
    /// The provider's own message.
    pub message: Box<str>,
    /// Everything else the provider attached.
    pub data: Option<Bytes>,
}

/// A message that arrived.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Incoming {
    /// An answer to something runtrol asked.
    Answer {
        /// Which question.
        id: RequestId,
        /// What came back.
        outcome: Result<Bytes, WireError>,
    },
    /// A question the provider is asking runtrol.
    ///
    /// How an approval reaches a person. A reader that treated every identified frame as an answer would drop
    /// these, and the session would appear to hang with nothing anywhere saying why.
    Question {
        /// The identifier to answer with.
        id: RequestId,
        /// Which method.
        method: Box<str>,
        /// The provider's own parameters, unread.
        params: Option<Bytes>,
    },
    /// Something the provider is telling runtrol, expecting no answer.
    Report {
        /// Which method.
        method: Box<str>,
        /// The provider's own parameters, unread.
        params: Option<Bytes>,
    },
}

/// The fields runtrol reads out of an envelope.
///
/// Unknown fields are left alone on purpose: see the module documentation.
#[derive(Deserialize)]
struct Envelope<'line> {
    /// Present on a question and on an answer, absent on a report.
    #[serde(default, borrow, deserialize_with = "present")]
    id: Option<&'line RawValue>,
    /// Present on a question and on a report.
    #[serde(default)]
    method: Option<&'line str>,
    /// A question's or a report's parameters.
    #[serde(default, borrow, deserialize_with = "present")]
    params: Option<&'line RawValue>,
    /// A successful answer.
    #[serde(default, borrow, deserialize_with = "present")]
    result: Option<&'line RawValue>,
    /// A failed answer.
    #[serde(default, borrow, deserialize_with = "present")]
    error: Option<&'line RawValue>,
}

/// Read a field that is there, keeping `null` distinct from missing.
///
/// The default reading of an optional field turns both an absent field and one set to `null` into nothing, and
/// here those mean opposite things: **`{"id":1,"result":null}` is a successful answer** to a method that returns
/// nothing, which several of them do. Collapsing the two made this reader refuse real answers, which is how this
/// function came to exist.
fn present<'de, D>(de: D) -> Result<Option<&'de RawValue>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    <&RawValue as Deserialize>::deserialize(de).map(Some)
}

/// The fields runtrol reads out of an error object.
#[derive(Deserialize)]
struct ErrorBody<'line> {
    /// The provider's own code.
    code: i64,
    /// The provider's own message.
    #[serde(default)]
    message: Option<&'line str>,
    /// Everything else.
    #[serde(default, borrow)]
    data: Option<&'line RawValue>,
}

/// Classify one line, keeping every payload as a slice of it.
///
/// # Errors
///
/// [`FrameError::NotAFrame`] when the line is not a readable JSON object, [`FrameError::NotAMessage`] when it is
/// an object of no shape the protocol has, [`FrameError::BadId`] for an identifier runtrol will not hold.
pub fn read(line: &Bytes) -> Result<Incoming, FrameError> {
    let envelope: Envelope<'_> =
        serde_json::from_slice(line).map_err(|error| FrameError::NotAFrame {
            // The reader's message names a position and a kind, never the content at that position. A frame that
            // failed to parse may still carry a conversation.
            detail: format!(
                "{} at line {}, column {}",
                kind_of(&error),
                error.line(),
                error.column()
            ),
        })?;

    let params = envelope.params.map(|raw| slice_of(line, raw));

    match (envelope.id, envelope.method) {
        // Both: the provider is asking runtrol something.
        (Some(id), Some(method)) => Ok(Incoming::Question {
            id: RequestId::parse(id)?,
            method: method.into(),
            params,
        }),
        // A method alone: the provider is telling runtrol something.
        (None, Some(method)) => Ok(Incoming::Report {
            method: method.into(),
            params,
        }),
        // An identifier alone: an answer, successful or not.
        (Some(id), None) => {
            let id = RequestId::parse(id)?;
            if let Some(raw) = envelope.error {
                return Ok(Incoming::Answer {
                    id,
                    outcome: Err(read_error(line, raw)?),
                });
            }
            match envelope.result {
                Some(raw) => Ok(Incoming::Answer {
                    id,
                    outcome: Ok(slice_of(line, raw)),
                }),
                // An identifier with neither. The protocol does not describe this, and guessing which of the two
                // it meant would be inventing an outcome.
                None => Err(FrameError::NotAMessage),
            }
        }
        (None, None) => Err(FrameError::NotAMessage),
    }
}

/// Read the two fields runtrol decides on out of an error object.
fn read_error(line: &Bytes, raw: &RawValue) -> Result<WireError, FrameError> {
    let body: ErrorBody<'_> =
        serde_json::from_str(raw.get()).map_err(|error| FrameError::NotAFrame {
            detail: format!("error object: {}", kind_of(&error)),
        })?;
    Ok(WireError {
        code: body.code,
        message: body.message.unwrap_or_default().into(),
        data: body.data.map(|data| slice_of(line, data)),
    })
}

/// What went wrong with a frame, as a phrase, and never the content it went wrong on.
///
/// The reader's own message can quote the text around the problem, and the text around the problem in a frame is
/// somebody's conversation. A category and a position say everything an operator needs and carry none of it.
fn kind_of(error: &serde_json::Error) -> &'static str {
    match error.classify() {
        serde_json::error::Category::Io => "the stream failed",
        serde_json::error::Category::Syntax => "invalid JSON",
        serde_json::error::Category::Data => "a value of the wrong shape",
        serde_json::error::Category::Eof => "the frame ended early",
    }
}

/// The part of `line` that `raw` occupies, shared rather than copied.
///
/// The raw value's text lives inside the line's own buffer, because that is where it was parsed from. Sharing it
/// is what keeps a payload relayed to several watchers costing pointers instead of copies.
fn slice_of(line: &Bytes, raw: &RawValue) -> Bytes {
    let text = raw.get();
    // A raw value taken from this line is inside this line. Copying rather than failing when it somehow is not
    // keeps a correct payload flowing instead of dropping a message over an accounting detail.
    if is_inside(line, text.as_bytes()) {
        line.slice_ref(text.as_bytes())
    } else {
        Bytes::copy_from_slice(text.as_bytes())
    }
}

/// Whether `part` points inside `whole`.
fn is_inside(whole: &Bytes, part: &[u8]) -> bool {
    let start = whole.as_ptr() as usize;
    let inner = part.as_ptr() as usize;
    match (
        start.checked_add(whole.len()),
        inner.checked_add(part.len()),
    ) {
        (Some(end), Some(inner_end)) => inner >= start && inner_end <= end,
        _ => false,
    }
}

/// What happened to an answer that arrived.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Delivered {
    /// It reached the caller that was waiting.
    ToTheCaller,
    /// Nobody was waiting for that identifier.
    ///
    /// Reported rather than dropped. It means the provider answered something runtrol did not ask, or answered
    /// twice, and either is worth a notice.
    NobodyAsked,
    /// The caller that was waiting has gone away.
    CallerGone,
}

/// Runtrol asked more questions than it will wait for at once.
#[derive(Clone, Copy, PartialEq, Eq, Debug, thiserror::Error)]
#[error("{waiting} answers are already outstanding, which is the limit")]
pub struct TooManyPending {
    /// How many are outstanding.
    pub waiting: usize,
}

/// Which questions runtrol is still waiting on.
#[derive(Debug)]
pub struct Pending {
    /// The next identifier to mint.
    next: i64,
    /// One waiter per outstanding question.
    waiting: BTreeMap<RequestId, oneshot::Sender<Result<Bytes, WireError>>>,
}

impl Pending {
    /// Nothing outstanding.
    #[must_use]
    pub fn new() -> Self {
        Self {
            next: 1,
            waiting: BTreeMap::new(),
        }
    }

    /// How many answers runtrol is waiting for.
    #[must_use]
    pub fn len(&self) -> usize {
        self.waiting.len()
    }

    /// Whether nothing is outstanding.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.waiting.is_empty()
    }

    /// Mint an identifier and take the waiting end for its answer.
    ///
    /// Identifiers count upwards and are never reused within a connection. Reuse would let a late answer to an
    /// abandoned question be delivered to whoever holds that number now.
    ///
    /// # Errors
    ///
    /// [`TooManyPending`] at [`MAX_PENDING`]. Refused rather than queued: a provider that has stopped answering
    /// must not be able to decide how much memory runtrol holds.
    pub fn issue(
        &mut self,
    ) -> Result<(RequestId, oneshot::Receiver<Result<Bytes, WireError>>), TooManyPending> {
        if self.waiting.len() >= MAX_PENDING {
            return Err(TooManyPending {
                waiting: self.waiting.len(),
            });
        }
        let id = RequestId::Number(self.next);
        self.next = self.next.wrapping_add(1);
        let (tx, rx) = oneshot::channel();
        self.waiting.insert(id.clone(), tx);
        Ok((id, rx))
    }

    /// Hand an answer to whoever was waiting for it.
    pub fn resolve(&mut self, id: &RequestId, outcome: Result<Bytes, WireError>) -> Delivered {
        match self.waiting.remove(id) {
            None => Delivered::NobodyAsked,
            Some(waiter) => match waiter.send(outcome) {
                Ok(()) => Delivered::ToTheCaller,
                // The caller stopped waiting. Not a failure of anything: it happens when a session closes while a
                // question is outstanding, and the answer has nowhere useful to go.
                Err(_) => Delivered::CallerGone,
            },
        }
    }

    /// Tell every waiter that no answer is coming.
    ///
    /// Called when the connection goes away, and the reason this exists rather than letting the map drop: a caller
    /// awaiting an answer that will never arrive is a failure nothing reports and nobody can see. Returns how
    /// many were waiting, so the caller can say so.
    pub fn abandon_all(&mut self, code: i64, why: &str) -> usize {
        let waiting = core::mem::take(&mut self.waiting);
        let count = waiting.len();
        for (_, sender) in waiting {
            // The receiver may already be gone, which is the same outcome from here: nobody is left waiting.
            drop(sender.send(Err(WireError {
                code,
                message: why.into(),
                data: None,
            })));
        }
        count
    }
}

impl Default for Pending {
    fn default() -> Self {
        Self::new()
    }
}

/// One outgoing frame, as a line of JSON with no newline of its own.
///
/// The caller writes it followed by a newline. Kept separate so nothing has to know whether the transport wants
/// one, and so a frame cannot be half-written with a newline already sent.
#[derive(Clone, PartialEq, Eq, Debug, Serialize)]
struct OutgoingRequest<'call, P> {
    /// The protocol version.
    jsonrpc: &'static str,
    /// Which question.
    id: serde_json::Value,
    /// Which method.
    method: &'call str,
    /// The parameters.
    params: &'call P,
}

/// One outgoing answer to a question the provider asked.
#[derive(Clone, PartialEq, Eq, Debug, Serialize)]
struct OutgoingAnswer<'call, R: ?Sized> {
    /// The protocol version.
    jsonrpc: &'static str,
    /// Which question is being answered.
    id: serde_json::Value,
    /// The answer.
    result: &'call R,
}

/// One outgoing report, which expects no answer.
#[derive(Clone, PartialEq, Eq, Debug, Serialize)]
struct OutgoingReport<'call, P> {
    /// The protocol version.
    jsonrpc: &'static str,
    /// Which method.
    method: &'call str,
    /// The parameters.
    params: &'call P,
}

/// Write a question for the provider.
///
/// Parameters are serialized rather than interpolated. Building a frame by pasting text together is how a value
/// from somewhere else becomes structure, and every value here came from somewhere else.
///
/// # Errors
///
/// [`FrameError::NotWritable`] when the parameters cannot be written as JSON.
pub fn write_question<P: Serialize>(
    id: &RequestId,
    method: &str,
    params: &P,
) -> Result<String, FrameError> {
    to_line(&OutgoingRequest {
        jsonrpc: VERSION,
        id: id.to_wire(),
        method,
        params,
    })
}

/// Write an answer to a question the provider asked.
///
/// # Errors
///
/// [`FrameError::NotWritable`] when the answer cannot be written as JSON.
pub fn write_answer<R: Serialize + ?Sized>(
    id: &RequestId,
    result: &R,
) -> Result<String, FrameError> {
    to_line(&OutgoingAnswer {
        jsonrpc: VERSION,
        id: id.to_wire(),
        result,
    })
}

/// One outgoing refusal of a question the provider asked.
#[derive(Clone, PartialEq, Eq, Debug, Serialize)]
struct OutgoingRefusal<'call> {
    /// The protocol version.
    jsonrpc: &'static str,
    /// Which question is being refused.
    id: serde_json::Value,
    /// Why.
    error: RefusalBody<'call>,
}

/// The body of a refusal.
#[derive(Clone, PartialEq, Eq, Debug, Serialize)]
struct RefusalBody<'call> {
    /// runtrol's own code.
    code: i64,
    /// runtrol's own message.
    message: &'call str,
}

/// Refuse a question the provider asked.
///
/// # Why this exists rather than leaving the question alone
///
/// A provider that multiplexes every session over one connection waits on each question it asks. One left
/// unanswered stalls that daemon, and therefore every session on it, with nothing anywhere saying why. So a
/// question runtrol will not serve is refused out loud.
///
/// Infallible on purpose: this is the answer of last resort, and a refusal that could itself fail to be
/// written would leave the caller with nothing to send. Both fields are runtrol's own, so there is nothing
/// here that can fail to serialize.
#[must_use]
pub fn write_error(id: &RequestId, code: i64, message: &str) -> String {
    let refusal = OutgoingRefusal {
        jsonrpc: VERSION,
        id: id.to_wire(),
        error: RefusalBody { code, message },
    };
    // A structure of a number, a short string and an identifier runtrol already validated. `to_string` on it
    // can only fail on a type that refuses to serialize, and none of these is one.
    serde_json::to_string(&refusal).unwrap_or_else(|_| {
        // Built by hand rather than left empty, so the provider still receives a well formed refusal for the
        // question it asked. The identifier is a number or a string runtrol bounded, and the message is a
        // literal, so this is a valid frame.
        format!(
            r#"{{"jsonrpc":"{VERSION}","id":{},"error":{{"code":{code},"message":"refused"}}}}"#,
            id.to_wire()
        )
    })
}

/// Write a report for the provider, expecting no answer.
///
/// # Errors
///
/// [`FrameError::NotWritable`] when the parameters cannot be written as JSON.
pub fn write_report<P: Serialize>(method: &str, params: &P) -> Result<String, FrameError> {
    to_line(&OutgoingReport {
        jsonrpc: VERSION,
        method,
        params,
    })
}

/// Serialize one frame.
fn to_line<T: Serialize>(frame: &T) -> Result<String, FrameError> {
    serde_json::to_string(frame).map_err(|error| FrameError::NotWritable {
        detail: error.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(text: &str) -> Bytes {
        Bytes::copy_from_slice(text.as_bytes())
    }

    #[test]
    fn a_provider_asking_runtrol_something_is_not_read_as_an_answer() {
        // Eleven of these exist and they are how an approval reaches a person. Treating every identified frame as
        // an answer would drop all of them, and the session would appear to hang.
        let frame = line(
            r#"{"id":7,"method":"item/commandExecution/requestApproval","params":{"command":"rm -rf /"}}"#,
        );
        match read(&frame).expect("readable") {
            Incoming::Question { id, method, params } => {
                assert_eq!(id, RequestId::Number(7));
                assert_eq!(&*method, "item/commandExecution/requestApproval");
                assert!(params.is_some(), "the provider's parameters must survive");
            }
            other => panic!("expected a question, got {other:?}"),
        }
    }

    #[test]
    fn the_three_shapes_are_told_apart() {
        let answer = read(&line(r#"{"id":1,"result":{"threads":[]}}"#)).expect("readable");
        assert!(matches!(answer, Incoming::Answer { outcome: Ok(_), .. }));

        let report = read(&line(
            r#"{"method":"turn/completed","params":{"turn":"t1"}}"#,
        ))
        .expect("readable");
        assert!(matches!(report, Incoming::Report { .. }));

        let question = read(&line(r#"{"id":"abc","method":"fs/readTextFile"}"#)).expect("readable");
        match question {
            Incoming::Question { id, .. } => assert_eq!(id, RequestId::Text("abc".into())),
            other => panic!("expected a question, got {other:?}"),
        }
    }

    #[test]
    fn the_provider_omitting_the_version_field_is_accepted() {
        // Measured: its outgoing frames have no such field. Refusing them would refuse the whole protocol.
        assert!(read(&line(r#"{"id":1,"result":null}"#)).is_ok());
        assert!(read(&line(r#"{"jsonrpc":"2.0","id":1,"result":null}"#)).is_ok());
    }

    #[test]
    fn an_answer_of_nothing_is_an_answer() {
        // A method that returns nothing answers with a null result, and several of them do. Reading that as
        // "no result field" refuses a real answer, and the caller waits forever for one that already arrived.
        match read(&line(r#"{"id":1,"result":null}"#)).expect("readable") {
            Incoming::Answer {
                id,
                outcome: Ok(result),
            } => {
                assert_eq!(id, RequestId::Number(1));
                assert_eq!(
                    &*result, b"null",
                    "the answer is the JSON null, not nothing"
                );
            }
            other => panic!("expected a successful answer, got {other:?}"),
        }
    }

    #[test]
    fn a_question_with_no_parameters_is_told_apart_from_one_with_null_parameters() {
        // Both are legal and they are not the same frame. Guessing they are would make one method's absent
        // argument into another method's explicit null.
        match read(&line(r#"{"id":1,"method":"account/read"}"#)).expect("readable") {
            Incoming::Question { params, .. } => assert_eq!(params, None),
            other => panic!("expected a question, got {other:?}"),
        }
        match read(&line(r#"{"id":1,"method":"account/read","params":null}"#)).expect("readable") {
            Incoming::Question { params, .. } => {
                assert_eq!(params.as_deref(), Some(&b"null"[..]));
            }
            other => panic!("expected a question, got {other:?}"),
        }
    }

    #[test]
    fn a_field_runtrol_has_never_heard_of_is_left_alone() {
        // A vendor adding a field must be a non-event. This is the opposite of a manifest, which refuses a key it
        // does not know, and the difference is who wrote the file.
        let frame = line(
            r#"{"jsonrpc":"2.0","id":1,"result":{"a":1},"somethingNew":true,"another":{"nested":[1]}}"#,
        );
        assert!(matches!(
            read(&frame).expect("readable"),
            Incoming::Answer { outcome: Ok(_), .. }
        ));
    }

    #[test]
    fn a_failed_answer_carries_the_code_and_the_message_runtrol_decides_on() {
        let frame = line(
            r#"{"id":4,"error":{"code":-32000,"message":"quota exhausted","data":{"retryAfter":60}}}"#,
        );
        match read(&frame).expect("readable") {
            Incoming::Answer {
                outcome: Err(failure),
                ..
            } => {
                assert_eq!(failure.code, -32_000);
                assert_eq!(&*failure.message, "quota exhausted");
                assert!(failure.data.is_some());
            }
            other => panic!("expected a failed answer, got {other:?}"),
        }
    }

    #[test]
    fn an_object_of_no_shape_the_protocol_has_is_told_apart_from_bad_json() {
        // Two different reports for two different problems: a provider sending something undescribed, and a line
        // that is not JSON at all.
        assert_eq!(
            read(&line(r#"{"jsonrpc":"2.0"}"#)),
            Err(FrameError::NotAMessage)
        );
        assert_eq!(read(&line(r#"{"id":9}"#)), Err(FrameError::NotAMessage));
        assert!(matches!(
            read(&line("not json at all")),
            Err(FrameError::NotAFrame { .. })
        ));
    }

    #[test]
    fn a_frame_that_would_not_parse_does_not_put_its_content_in_the_message() {
        // The realistic leak: a malformed frame carrying a conversation, and a message that quotes it.
        // The reader's own message happens not to quote content today. What is asserted is stronger than that:
        // the phrase comes from a closed set runtrol writes, so no future change to the reader's wording can
        // start carrying a conversation into a log line.
        let secret = r#"{"method":"x","params":{"text":"my private question"},,}"#;
        match read(&line(secret)) {
            Err(FrameError::NotAFrame { detail }) => {
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
                    "the phrase has to be one runtrol owns, not one the reader chose: {detail}"
                );
                assert!(
                    detail.contains("column"),
                    "and it has to place the problem: {detail}"
                );
            }
            other => panic!("expected an unreadable frame, got {other:?}"),
        }
    }

    #[test]
    fn a_payload_is_a_slice_of_the_line_it_arrived_on() {
        // What keeps relaying one message to several watchers costing pointers rather than copies. The parent
        // buffer is built here, so this compares the payload against something other than itself.
        let text = r#"{"method":"session/update","params":{"text":"hello"}}"#;
        let frame = line(text);
        let start = frame.as_ptr() as usize;

        match read(&frame).expect("readable") {
            Incoming::Report { params, .. } => {
                let params = params.expect("parameters are there");
                let at = params.as_ptr() as usize;
                assert!(
                    at >= start && at < start + frame.len(),
                    "the payload was copied out of the line instead of shared with it"
                );
            }
            other => panic!("expected a report, got {other:?}"),
        }
    }

    #[test]
    fn an_identifier_runtrol_will_not_hold_is_refused() {
        // A string identifier of any length is permitted by the protocol, which would let a provider decide how
        // much memory each pending entry costs.
        let long = "x".repeat(MAX_ID_TEXT + 1);
        let frame = line(&format!(r#"{{"id":"{long}","result":null}}"#));
        assert!(matches!(read(&frame), Err(FrameError::BadId { .. })));

        let structured = line(r#"{"id":{"nested":true},"result":null}"#);
        assert!(matches!(read(&structured), Err(FrameError::BadId { .. })));
    }

    #[test]
    fn an_escaped_identifier_means_the_same_thing_to_both_sides() {
        let frame = line(r#"{"id":"a\"b","result":null}"#);
        match read(&frame).expect("readable") {
            Incoming::Answer { id, .. } => assert_eq!(id, RequestId::Text("a\"b".into())),
            other => panic!("expected an answer, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn an_answer_reaches_the_caller_that_asked() {
        let mut pending = Pending::new();
        let (id, waiting) = pending.issue().expect("room for one question");
        assert_eq!(pending.len(), 1);

        assert_eq!(
            pending.resolve(&id, Ok(Bytes::from_static(b"{}"))),
            Delivered::ToTheCaller
        );
        assert!(
            pending.is_empty(),
            "an answered question stops being pending"
        );
        assert_eq!(
            waiting.await.expect("the answer arrives"),
            Ok(Bytes::from_static(b"{}"))
        );
    }

    #[test]
    fn identifiers_are_never_reused_within_a_connection() {
        // Reuse would let a late answer to an abandoned question be delivered to whoever holds that number now.
        let mut pending = Pending::new();
        let mut seen = Vec::new();
        for _ in 0..8 {
            let (id, waiting) = pending.issue().expect("room");
            seen.push(id.clone());
            // Resolve immediately, so the map is empty again and only the counter decides.
            pending.resolve(&id, Ok(Bytes::new()));
            drop(waiting);
        }
        let unique: std::collections::BTreeSet<&RequestId> = seen.iter().collect();
        assert_eq!(
            unique.len(),
            seen.len(),
            "an identifier came back: {seen:?}"
        );
    }

    #[test]
    fn a_provider_that_stops_answering_cannot_decide_how_much_runtrol_holds() {
        // The unbounded buffer the memory contract bans, wearing different clothes.
        let mut pending = Pending::new();
        let mut held = Vec::new();
        for _ in 0..MAX_PENDING {
            let (_, waiting) = pending.issue().expect("room up to the limit");
            held.push(waiting);
        }
        assert_eq!(pending.len(), MAX_PENDING);

        match pending.issue() {
            Err(refusal) => assert_eq!(refusal.waiting, MAX_PENDING),
            Ok(_) => panic!("the bound did not hold"),
        }
    }

    #[tokio::test]
    async fn every_waiter_is_told_when_the_connection_goes_away() {
        // A caller awaiting an answer that will never arrive is a failure nothing reports and nobody can see. The
        // session simply stops, and that is the worst shape a swallowed failure takes.
        let mut pending = Pending::new();
        let mut waiting = Vec::new();
        for _ in 0..4 {
            let (_, one) = pending.issue().expect("room");
            waiting.push(one);
        }

        let told = pending.abandon_all(-1, "the provider went away");
        assert_eq!(told, 4);
        assert!(pending.is_empty());

        for one in waiting {
            match one.await.expect("a waiter is answered rather than dropped") {
                Err(failure) => assert!(failure.message.contains("went away"), "{failure:?}"),
                Ok(_) => panic!("an abandoned question must not report success"),
            }
        }
    }

    #[test]
    fn an_answer_nobody_asked_for_is_reported_rather_than_dropped() {
        // It means the provider answered something runtrol did not ask, or answered twice, and either is worth a
        // notice rather than silence.
        let mut pending = Pending::new();
        assert_eq!(
            pending.resolve(&RequestId::Number(999), Ok(Bytes::new())),
            Delivered::NobodyAsked
        );
    }

    #[test]
    fn an_answer_whose_caller_left_is_told_apart_from_one_nobody_asked_for() {
        let mut pending = Pending::new();
        let (id, waiting) = pending.issue().expect("room");
        drop(waiting);
        assert_eq!(
            pending.resolve(&id, Ok(Bytes::new())),
            Delivered::CallerGone
        );
    }

    #[test]
    fn an_outgoing_frame_is_one_line_whatever_is_in_it() {
        // A newline inside a frame would split it into two invalid ones. Serializing rather than pasting text is
        // what makes that impossible, including for a value that itself contains newlines.
        let params = serde_json::json!({
            "text": "first\nsecond\r\nthird",
            "nested": {"deep": ["a\nb"]},
        });
        let written =
            write_question(&RequestId::Number(3), "turn/start", &params).expect("writable");

        assert!(!written.contains('\n'), "{written}");
        assert!(!written.contains('\r'), "{written}");
        assert!(written.contains(r#""jsonrpc":"2.0""#), "{written}");
        assert!(written.contains(r#""id":3"#), "{written}");
        assert!(written.contains(r#""method":"turn/start""#), "{written}");
    }

    #[test]
    fn an_outgoing_frame_reads_back_as_what_it_was() {
        // The two directions have to agree, or a question runtrol asks is a question the provider cannot read.
        let written = write_question(
            &RequestId::Number(11),
            "thread/list",
            &serde_json::json!({}),
        )
        .expect("writable");
        match read(&line(&written)).expect("readable") {
            Incoming::Question { id, method, .. } => {
                assert_eq!(id, RequestId::Number(11));
                assert_eq!(&*method, "thread/list");
            }
            other => panic!("expected a question, got {other:?}"),
        }

        let answered = write_answer(
            &RequestId::Text("abc".into()),
            &serde_json::json!({"ok": true}),
        )
        .expect("writable");
        match read(&line(&answered)).expect("readable") {
            Incoming::Answer { id, outcome } => {
                assert_eq!(id, RequestId::Text("abc".into()));
                assert!(outcome.is_ok());
            }
            other => panic!("expected an answer, got {other:?}"),
        }

        let reported = write_report("initialized", &serde_json::json!({})).expect("writable");
        assert!(matches!(
            read(&line(&reported)).expect("readable"),
            Incoming::Report { .. }
        ));
    }

    #[test]
    fn a_question_runtrol_will_not_serve_is_refused_out_loud() {
        // A question left unanswered stalls a daemon that multiplexes every session, so there is no case
        // where saying nothing is acceptable. The refusal has to read back as an answer to the same question.
        let written = write_error(&RequestId::Number(7), -32_601, "runtrol has no binding");
        assert!(!written.contains('\n'), "{written}");
        match read(&line(&written)).expect("readable") {
            Incoming::Answer { id, outcome } => {
                assert_eq!(id, RequestId::Number(7));
                match outcome {
                    Err(failure) => {
                        assert_eq!(failure.code, -32_601);
                        assert!(failure.message.contains("no binding"));
                    }
                    Ok(_) => panic!("a refusal must not read as success"),
                }
            }
            other => panic!("expected an answer, got {other:?}"),
        }
    }

    #[test]
    fn a_refusal_of_a_textual_question_names_the_same_question() {
        let written = write_error(&RequestId::Text("req-42".into()), -1, "no");
        match read(&line(&written)).expect("readable") {
            Incoming::Answer { id, .. } => assert_eq!(id.to_string(), "req-42"),
            other => panic!("expected an answer, got {other:?}"),
        }
    }

    #[test]
    fn a_textual_identifier_survives_the_round_trip() {
        let written = write_answer(&RequestId::Text("req-42".into()), &serde_json::json!(null))
            .expect("writable");
        match read(&line(&written)).expect("readable") {
            Incoming::Answer { id, .. } => assert_eq!(id.to_string(), "req-42"),
            other => panic!("expected an answer, got {other:?}"),
        }
    }
}
