//! Every identifier runtrol mints or relays. Defined here and nowhere else.
//!
//! Two families, and the split decides the representation:
//!
//! - **runtrol mints it** ([`SessionId`], [`TurnId`], [`ApprovalId`], [`OptionId`]). The format is
//!   runtrol's own promise, so each is a fixed-size `Copy` value with a documented layout.
//! - **a provider supplies it** ([`ProviderId`], [`NativeSessionId`], [`ToolCallId`]). The format
//!   belongs to the provider, so each is bounded, reference-counted, opaque text. runtrol compares
//!   these and never parses them for meaning.
//!
//! Each is a distinct newtype rather than an alias for `String`. Drivers correlate these values by
//! hand on every frame, so passing a tool call id where a native session id belongs is a mistake
//! worth spending the compiler's attention on.

use core::{fmt, str::FromStr};
use std::sync::Arc;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use uuid::Uuid;

/// Longest provider-supplied identifier runtrol accepts.
///
/// A provider emitting an id longer than this is broken or hostile, and runtrol will not hold an
/// unbounded string on its behalf. Both CLIs measured on this machine emit 36-byte session UUIDs
/// and roughly 30-byte tool call ids, so this ceiling is about seven times observed use.
pub const MAX_PROVIDER_TEXT: usize = 256;

/// A value offered as an identifier did not satisfy that identifier's format.
///
/// `what` names the identifier type in each variant so one error type serves every id here without
/// the message going vague. It is filled in by the constructor, never by a caller.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IdError {
    /// Empty text. An identifier that names nothing cannot correlate anything.
    #[error("{what} is empty")]
    Empty {
        /// Which identifier type rejected the value.
        what: &'static str,
    },

    /// Longer than the type allows.
    #[error("{what} is {len} bytes, over the {max} byte limit")]
    TooLong {
        /// Which identifier type rejected the value.
        what: &'static str,
        /// Length of the offered text, in bytes.
        len: usize,
        /// The limit that was exceeded.
        max: usize,
    },

    /// Contains a control character.
    ///
    /// Identifiers reach log lines, terminal output, and file names. A newline or an ANSI escape
    /// inside one is log forging and terminal injection, and provider output is untrusted by
    /// standing rule.
    #[error("{what} contains a control character at byte {at}")]
    Control {
        /// Which identifier type rejected the value.
        what: &'static str,
        /// Byte offset of the first control character.
        at: usize,
    },

    /// A character outside the type's permitted set.
    #[error("{what} may only contain {allowed}, found {found:?} at byte {at}")]
    Charset {
        /// Which identifier type rejected the value.
        what: &'static str,
        /// Human-readable description of the permitted set.
        allowed: &'static str,
        /// The offending character.
        found: char,
        /// Byte offset of the offending character.
        at: usize,
    },

    /// Correct characters, wrong shape.
    #[error("{what} is malformed: {why}")]
    Shape {
        /// Which identifier type rejected the value.
        what: &'static str,
        /// What the shape rule requires.
        why: &'static str,
    },
}

/// Shared validation for provider-supplied text identifiers.
fn check_provider_text(what: &'static str, text: &str) -> Result<(), IdError> {
    if text.is_empty() {
        return Err(IdError::Empty { what });
    }
    if text.len() > MAX_PROVIDER_TEXT {
        return Err(IdError::TooLong {
            what,
            len: text.len(),
            max: MAX_PROVIDER_TEXT,
        });
    }
    if let Some((at, _)) = text.char_indices().find(|(_, ch)| ch.is_control()) {
        return Err(IdError::Control { what, at });
    }
    Ok(())
}

/// Defines a provider-supplied text identifier: bounded, reference-counted, opaque.
///
/// `Arc<str>` rather than `Box<str>` because every event is cloned once per subscriber, and a
/// refcount bump is the difference between fan-out costing pointers and fan-out costing bytes.
macro_rules! provider_text_id {
    ($(#[$doc:meta])* $name:ident = $what:literal) => {
        $(#[$doc])*
        #[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(Arc<str>);

        impl $name {
            /// Name of this identifier type, as it appears in [`IdError`] messages.
            pub const WHAT: &'static str = $what;

            /// Validate provider-supplied text and take ownership of it.
            ///
            /// # Errors
            ///
            /// [`IdError::Empty`] for empty text, [`IdError::TooLong`] past
            /// [`MAX_PROVIDER_TEXT`], [`IdError::Control`] for a control character.
            pub fn new(text: &str) -> Result<Self, IdError> {
                check_provider_text($what, text)?;
                Ok(Self(Arc::from(text)))
            }

            /// The provider's own text, unchanged.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}({})", $what, &self.0)
            }
        }

        impl FromStr for $name {
            type Err = IdError;

            fn from_str(text: &str) -> Result<Self, IdError> {
                Self::new(text)
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
                ser.serialize_str(&self.0)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            /// Decoding runs the same validation as [`Self::new`]. A wire frame cannot smuggle in
            /// an identifier that the constructor would have refused.
            fn deserialize<D: Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
                let text = String::deserialize(de)?;
                Self::new(&text).map_err(serde::de::Error::custom)
            }
        }
    };
}

provider_text_id! {
    /// The provider's own identifier for a session.
    ///
    /// A Codex `threadId`, a Claude session UUID, or whatever a third provider uses. runtrol keeps
    /// it so it can hand the session back to the CLI that owns it, and compares it for equality.
    /// It never assumes a format: the Claude case where this happens to equal [`SessionId`] is a
    /// convenience the driver may exploit, not a rule the core may rely on.
    NativeSessionId = "native session id"
}

provider_text_id! {
    /// The provider's own identifier for one logical message.
    ///
    /// ACP calls it `messageId`. It groups a run of incremental fragments into the message they build
    /// up, which is the only reason runtrol looks at it: a subscriber needs to know whether a fragment
    /// continues the previous one or starts a new one.
    MessageId = "message id"
}

provider_text_id! {
    /// The provider's own identifier for one tool call.
    ///
    /// A Codex `itemId` or a Claude `tool_use_id`. Used to attach later updates to the call they
    /// belong to, and to express subagent nesting through a parent link. Relayed to subscribers
    /// verbatim, because their view of the call is the provider's payload and the two must agree.
    ToolCallId = "tool call id"
}

/// Which coding CLI a session belongs to.
///
/// Fixed size and `Copy`: it is part of a database key, it rides inside permission scopes, and it
/// is compared on every list operation, so an allocation here would be an allocation everywhere.
///
/// The character set is deliberately narrow. This value appears in file names under
/// `providers/<id>.toml`, in command lines, and in a database key, and a provider id that can
/// contain a path separator or a leading dash is a bug waiting for a hostile manifest.
///
/// Case is rejected rather than folded. Two normalization rules for one identifier is how a user
/// manifest silently fails to shadow a built-in provider and the same CLI shows up twice in one
/// list, so there is exactly one rule: lowercase or nothing.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProviderId {
    /// Number of meaningful bytes in `bytes`.
    len: u8,
    /// Left-aligned ASCII, zero-padded. Total size of the struct is 32 bytes.
    bytes: [u8; Self::MAX_LEN],
}

impl ProviderId {
    /// Longest provider id runtrol accepts.
    pub const MAX_LEN: usize = 31;

    /// Name of this identifier type, as it appears in [`IdError`] messages.
    pub const WHAT: &'static str = "provider id";

    /// Description of the permitted character set, for error messages.
    const ALLOWED: &'static str = "lowercase ascii letters, digits, and '-'";

    /// Validate text as a provider id.
    ///
    /// # Errors
    ///
    /// [`IdError::Empty`] for empty text, [`IdError::TooLong`] past [`Self::MAX_LEN`],
    /// [`IdError::Charset`] for a character outside `[a-z0-9-]`, [`IdError::Shape`] for a
    /// leading or trailing `-` or a repeated `-`.
    pub fn parse(text: &str) -> Result<Self, IdError> {
        if text.is_empty() {
            return Err(IdError::Empty { what: Self::WHAT });
        }
        if text.len() > Self::MAX_LEN {
            return Err(IdError::TooLong {
                what: Self::WHAT,
                len: text.len(),
                max: Self::MAX_LEN,
            });
        }
        for (at, ch) in text.char_indices() {
            if !matches!(ch, 'a'..='z' | '0'..='9' | '-') {
                return Err(IdError::Charset {
                    what: Self::WHAT,
                    allowed: Self::ALLOWED,
                    found: ch,
                    at,
                });
            }
        }
        if text.starts_with('-') || text.ends_with('-') {
            return Err(IdError::Shape {
                what: Self::WHAT,
                why: "must not start or end with '-'",
            });
        }
        if text.contains("--") {
            return Err(IdError::Shape {
                what: Self::WHAT,
                why: "must not contain a repeated '-'",
            });
        }

        let mut bytes = [0_u8; Self::MAX_LEN];
        for (slot, byte) in bytes.iter_mut().zip(text.as_bytes()) {
            *slot = *byte;
        }
        // The length fits in a u8 because MAX_LEN is 31, checked above.
        #[expect(
            clippy::cast_possible_truncation,
            reason = "text.len() <= MAX_LEN == 31, enforced immediately above"
        )]
        let len = text.len() as u8;
        Ok(Self { len, bytes })
    }

    /// The id as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        let len = usize::from(self.len);
        match self.bytes.get(..len).map(core::str::from_utf8) {
            Some(Ok(text)) => text,
            // Unreachable by construction: `parse` is the only constructor and it admits ASCII
            // only, which is always valid UTF-8. An empty string rather than a panic keeps a
            // corrupted value visible in output instead of taking down the daemon.
            Some(Err(_)) | None => "",
        }
    }
}

impl fmt::Display for ProviderId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl fmt::Debug for ProviderId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ProviderId({})", self.as_str())
    }
}

impl FromStr for ProviderId {
    type Err = IdError;

    fn from_str(text: &str) -> Result<Self, IdError> {
        Self::parse(text)
    }
}

impl Serialize for ProviderId {
    fn serialize<S: Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        ser.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ProviderId {
    /// Decoding runs the same validation as [`ProviderId::parse`], so a manifest cannot introduce
    /// an id the constructor would have refused.
    fn deserialize<D: Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        let text = String::deserialize(de)?;
        Self::parse(&text).map_err(serde::de::Error::custom)
    }
}

/// runtrol's identifier for one hosted terminal: a provider's own terminal interface on a pseudo terminal
/// the daemon owns, which any number of viewers attach to.
///
/// Distinct from [`SessionId`] on purpose. A session is a structured conversation the daemon relays event by
/// event; a terminal is a screen the daemon carries byte by byte and never reads. Naming them with one type
/// would let a request about one land on the other.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TerminalId(Uuid);

impl TerminalId {
    /// Mint a new terminal id from the current time.
    #[must_use]
    pub fn now() -> Self {
        Self(Uuid::now_v7())
    }
}

impl fmt::Display for TerminalId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0.as_hyphenated())
    }
}

impl fmt::Debug for TerminalId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "TerminalId({})", self.0.as_hyphenated())
    }
}

impl FromStr for TerminalId {
    type Err = IdError;

    fn from_str(text: &str) -> Result<Self, IdError> {
        Uuid::parse_str(text).map(Self).map_err(|_| IdError::Shape {
            what: "terminal id",
            why: "must be a UUID",
        })
    }
}

impl Serialize for TerminalId {
    fn serialize<S: Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        ser.serialize_str(&self.0.as_hyphenated().to_string())
    }
}

impl<'de> Deserialize<'de> for TerminalId {
    fn deserialize<D: Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        let text = String::deserialize(de)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

/// runtrol's identifier for a session.
///
/// UUIDv7, so the 16 bytes sort chronologically. That is not decoration: sessions live in a
/// key-ordered database, and a time-ordered key turns "the sessions I touched recently" into a
/// bounded scan from one end instead of a walk over everything.
///
/// A real UUID rather than a counter because at least one CLI accepts a caller-chosen session id
/// in exactly this format, which lets a driver make the native id equal this one and skips a
/// mapping table.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SessionId(Uuid);

impl SessionId {
    /// Mint a new session id from the current time.
    #[must_use]
    pub fn now() -> Self {
        Self(Uuid::now_v7())
    }

    /// Rebuild from stored bytes.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(Uuid::from_bytes(bytes))
    }

    /// The 16 bytes, big-endian, as stored in the database key.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 16] {
        self.0.as_bytes()
    }
}

impl fmt::Display for SessionId {
    /// Lowercase hyphenated, which is the form the CLIs accept on their own command lines.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0.as_hyphenated())
    }
}

impl fmt::Debug for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SessionId({})", self.0.as_hyphenated())
    }
}

impl FromStr for SessionId {
    type Err = IdError;

    /// Accepts every spelling the underlying UUID parser accepts (hyphenated, simple, braced,
    /// urn), because a human types this on a command line and the round trip is what matters.
    fn from_str(text: &str) -> Result<Self, IdError> {
        Uuid::parse_str(text).map(Self).map_err(|_| IdError::Shape {
            what: "session id",
            why: "must be a UUID",
        })
    }
}

impl Serialize for SessionId {
    fn serialize<S: Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        ser.serialize_str(&self.0.as_hyphenated().to_string())
    }
}

impl<'de> Deserialize<'de> for SessionId {
    fn deserialize<D: Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        let text = String::deserialize(de)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

/// One live session hub incarnation.
///
/// A fresh value is minted whenever a hub is created. It is distinct from [`SessionId`] because it names a volatile
/// event stream, not a conversation or storage key.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StreamId(Uuid);

impl StreamId {
    /// Mint a new stream incarnation.
    #[must_use]
    pub fn now() -> Self {
        Self(Uuid::now_v7())
    }
}

impl fmt::Display for StreamId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0.as_hyphenated())
    }
}

impl fmt::Debug for StreamId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "StreamId({})", self.0.as_hyphenated())
    }
}

impl FromStr for StreamId {
    type Err = IdError;

    fn from_str(text: &str) -> Result<Self, IdError> {
        Uuid::parse_str(text).map(Self).map_err(|_| IdError::Shape {
            what: "stream id",
            why: "must be a UUID",
        })
    }
}

impl Serialize for StreamId {
    fn serialize<S: Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        ser.serialize_str(&self.0.as_hyphenated().to_string())
    }
}

impl<'de> Deserialize<'de> for StreamId {
    fn deserialize<D: Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        let text = String::deserialize(de)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

/// runtrol's identifier for one turn of a conversation.
///
/// Minted by runtrol rather than taken from the provider because the providers disagree: one hands
/// out a turn identifier, the other hands out nothing at all. Correlating a submission with its
/// outcome has to work on both, so the correlation identifier is runtrol's and each driver keeps
/// the provider's own token privately.
///
/// The epoch is part of the id, not context around it. A subscriber can quote a turn id back after
/// having been disconnected across a reattach, and without the epoch, turn 3 of the previous attach
/// and turn 3 of the current one are the same eight bytes.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
pub struct TurnId {
    /// Which driver attach this turn belongs to. Increments on every attach to the session.
    pub epoch: u32,
    /// Position within the epoch, counting from zero in submission order.
    pub index: u32,
}

impl TurnId {
    /// The first turn of an epoch.
    #[must_use]
    pub const fn first(epoch: u32) -> Self {
        Self { epoch, index: 0 }
    }

    /// The next turn in the same epoch.
    ///
    /// Saturates rather than wrapping. Four billion turns in one attach is not reachable, and a
    /// wrapped id would silently alias an earlier turn, which is worse than a stuck one.
    #[must_use]
    pub const fn next(self) -> Self {
        Self {
            epoch: self.epoch,
            index: self.index.saturating_add(1),
        }
    }
}

impl fmt::Display for TurnId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.epoch, self.index)
    }
}

impl FromStr for TurnId {
    type Err = IdError;

    fn from_str(text: &str) -> Result<Self, IdError> {
        const MALFORMED: IdError = IdError::Shape {
            what: "turn id",
            why: "must be <epoch>.<index>, both non-negative integers",
        };
        let (epoch, index) = text.split_once('.').ok_or(MALFORMED)?;
        Ok(Self {
            epoch: epoch.parse().map_err(|_| MALFORMED)?,
            index: index.parse().map_err(|_| MALFORMED)?,
        })
    }
}

/// runtrol's identifier for one pending approval.
///
/// UUIDv7 and not epoch-scoped, because a phone must be able to answer a prompt across a
/// reconnect. The provider's own handle for the same prompt (a connection-scoped integer on one
/// CLI, a request string on the other) is never exposed, because neither survives a reconnect.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ApprovalId(Uuid);

impl ApprovalId {
    /// Mint a new approval id from the current time.
    #[must_use]
    pub fn now() -> Self {
        Self(Uuid::now_v7())
    }

    /// The 16 bytes, big-endian.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 16] {
        self.0.as_bytes()
    }
}

impl fmt::Display for ApprovalId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0.as_hyphenated())
    }
}

impl fmt::Debug for ApprovalId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ApprovalId({})", self.0.as_hyphenated())
    }
}

impl FromStr for ApprovalId {
    type Err = IdError;

    fn from_str(text: &str) -> Result<Self, IdError> {
        Uuid::parse_str(text).map(Self).map_err(|_| IdError::Shape {
            what: "approval id",
            why: "must be a UUID",
        })
    }
}

impl Serialize for ApprovalId {
    fn serialize<S: Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        ser.serialize_str(&self.0.as_hyphenated().to_string())
    }
}

impl<'de> Deserialize<'de> for ApprovalId {
    fn deserialize<D: Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        let text = String::deserialize(de)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

/// Which option of an approval request a human chose.
///
/// The option's position in the list that request offered, and meaningful only inside that
/// request. It is deliberately not a capability token: a response is bound to its request by the
/// approval id plus a digest of the subject, and every approval id is a fresh UUIDv7, so a stale
/// or guessed option id can only ever be submitted against an approval that no longer exists.
/// Dressing an index up as a secret would be theatre.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
pub struct OptionId(pub u32);

impl fmt::Display for OptionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_id_accepts_the_documented_shape() {
        for good in ["codex", "claude", "a", "gpt-5", "1password-cli", "x9"] {
            assert!(ProviderId::parse(good).is_ok(), "rejected {good}");
            assert_eq!(
                ProviderId::parse(good).map(|id| id.to_string()),
                Ok(good.to_owned())
            );
        }
    }

    #[test]
    fn provider_id_rejects_case_instead_of_folding_it() {
        // Folding would give two normalization rules for one identifier, which is how a user
        // manifest silently fails to shadow a built-in.
        let error = ProviderId::parse("Codex").expect_err("uppercase must be rejected");
        assert!(matches!(
            error,
            IdError::Charset {
                found: 'C',
                at: 0,
                ..
            }
        ));
    }

    #[test]
    fn provider_id_rejects_path_and_flag_shaped_text() {
        for bad in [
            "../codex",
            "codex/claude",
            "codex\\claude",
            "-codex",
            "codex-",
            "co--dex",
            "codex cli",
            "코덱스",
        ] {
            assert!(ProviderId::parse(bad).is_err(), "accepted {bad}");
        }
    }

    #[test]
    fn provider_id_rejects_empty_and_overlong() {
        assert!(matches!(ProviderId::parse(""), Err(IdError::Empty { .. })));
        let long = "a".repeat(ProviderId::MAX_LEN + 1);
        assert!(matches!(
            ProviderId::parse(&long),
            Err(IdError::TooLong { .. })
        ));
        let at_limit = "a".repeat(ProviderId::MAX_LEN);
        assert!(ProviderId::parse(&at_limit).is_ok());
    }

    #[test]
    fn provider_id_is_copy_and_fits_in_one_word_pair() {
        // The struct is a database key part and a scope payload. If it grows past 32 bytes the
        // reason it exists (no allocation on the compare path) has quietly gone away.
        assert_eq!(size_of::<ProviderId>(), 32);
    }

    #[test]
    fn provider_text_ids_reject_control_characters() {
        // A newline inside an id forges log lines; an escape sequence rewrites a terminal.
        for bad in ["a\nb", "a\rb", "a\u{1b}[31m", "a\tb", "a\0b"] {
            assert!(
                matches!(NativeSessionId::new(bad), Err(IdError::Control { .. })),
                "accepted {bad:?}"
            );
        }
    }

    #[test]
    fn provider_text_ids_are_bounded() {
        let long = "a".repeat(MAX_PROVIDER_TEXT + 1);
        assert!(matches!(
            ToolCallId::new(&long),
            Err(IdError::TooLong { .. })
        ));
        assert!(ToolCallId::new(&"a".repeat(MAX_PROVIDER_TEXT)).is_ok());
        assert!(matches!(ToolCallId::new(""), Err(IdError::Empty { .. })));
    }

    #[test]
    fn provider_text_ids_keep_the_providers_own_spelling() {
        // runtrol relays these. Normalizing one would break correlation against the payload the
        // subscriber renders.
        let id = ToolCallId::new("toolu_01AbC-XyZ.9").expect("valid tool call id");
        assert_eq!(id.as_str(), "toolu_01AbC-XyZ.9");
    }

    #[test]
    fn session_ids_sort_chronologically() {
        let first = SessionId::now();
        let second = SessionId::now();
        assert!(first < second, "UUIDv7 must order by mint time");
    }

    #[test]
    fn session_id_round_trips_through_text_and_bytes() {
        let id = SessionId::now();
        let parsed: SessionId = id.to_string().parse().expect("display must be parseable");
        assert_eq!(id, parsed);
        assert_eq!(id, SessionId::from_bytes(*id.as_bytes()));
    }

    #[test]
    fn stream_ids_are_distinct_from_sessions_and_round_trip() {
        let stream = StreamId::now();
        let parsed: StreamId = stream
            .to_string()
            .parse()
            .expect("display must be parseable");
        assert_eq!(parsed, stream);
        assert_ne!(stream.to_string(), SessionId::now().to_string());
    }

    #[test]
    fn session_id_display_is_the_form_a_cli_accepts() {
        let id = SessionId::now();
        let text = id.to_string();
        assert_eq!(text.len(), 36, "hyphenated");
        assert!(
            !text.contains(|ch: char| ch.is_ascii_uppercase()),
            "lowercase"
        );
    }

    #[test]
    fn turn_id_carries_its_epoch() {
        let old = TurnId { epoch: 1, index: 3 };
        let new = TurnId { epoch: 2, index: 3 };
        assert_ne!(old, new, "an epoch change must produce a different turn id");
        assert!(old < new);
    }

    #[test]
    fn turn_id_round_trips_and_rejects_junk() {
        let id = TurnId {
            epoch: 7,
            index: 42,
        };
        assert_eq!(id.to_string(), "7.42");
        assert_eq!("7.42".parse::<TurnId>(), Ok(id));
        for bad in ["7", "7.", ".42", "7.42.1", "-1.0", "a.b", ""] {
            assert!(bad.parse::<TurnId>().is_err(), "accepted {bad}");
        }
    }

    #[test]
    fn turn_index_saturates_rather_than_aliasing() {
        let last = TurnId {
            epoch: 0,
            index: u32::MAX,
        };
        assert_eq!(last.next(), last, "a wrapped index would alias turn zero");
        assert_eq!(TurnId::first(4).next().index, 1);
    }
}
