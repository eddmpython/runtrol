//! Process-scoped admission and one bounded command per connection.
//!
//! Frames are the Runtime's own length-prefixed frames; what is inside them is JSON of the types here. A
//! connection sends one invocation carrying a hello and an optional command. Welcome proves admission; the
//! following command answer contains a receipt or consumed envelope. Unknown input closes the connection.

use core::fmt;

use serde::{Deserialize, Serialize};

use crate::envelope::PROTOCOL_VERSION;
use crate::id::ManagedSessionId;

mod commands;
pub use commands::{
    Answer, COMMAND_SLOTS, Invocation, Request, SESSION_PAGE, SESSION_WAIT_SLOTS, Session,
    WAIT_SLOTS,
};

/// The largest frame the courier reads into a value.
///
/// JSON may expand each body byte into six ASCII escape bytes. Reserve that worst case plus the envelope.
/// The transport checks this ceiling against the length prefix before allocating or reading the payload.
pub const MAX_FRAME_BYTES: usize = crate::Limits::INITIAL.body_bytes * 6 + 4096;

/// The first frame on a connection: which managed session speaks, proved by the token it was born with.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    /// The envelope layout the sender speaks. Must equal [`PROTOCOL_VERSION`].
    pub protocol_version: u16,
    /// The managed session the sender claims to be.
    pub session: ManagedSessionId,
    /// The token from the sender's environment, base64url without padding.
    pub token: String,
}

impl Hello {
    /// A hello for `session` speaking this crate's layout.
    #[must_use]
    pub const fn new(session: ManagedSessionId, token: String) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            session,
            token,
        }
    }
}

/// The token stays out of diagnostics: a hello's `Debug` form names the session and the version only.
impl fmt::Debug for Hello {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Hello")
            .field("protocol_version", &self.protocol_version)
            .field("session", &self.session)
            .finish_non_exhaustive()
    }
}

/// The Runtime's answer to a hello.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "answer", rename_all = "snake_case")]
pub enum HelloAnswer {
    /// The connection now speaks for `session`.
    Welcome {
        /// The admitted session, echoed so the sender can check it was understood.
        session: ManagedSessionId,
    },
    /// The connection is not a managed session of this Runtime generation. No reason travels: the Runtime
    /// says why on its own error stream, and a process that is not managed learns nothing more here.
    Refused,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hello_and_its_answers_cross_as_json_with_canonical_identifiers() {
        let session = ManagedSessionId::now();
        let hello = Hello::new(session, "dG9rZW4".to_owned());
        let json = serde_json::to_string(&hello).expect("a hello serializes");
        assert!(
            json.contains(&format!("\"session\":\"{session}\"")),
            "{json}"
        );
        assert_eq!(
            serde_json::from_str::<Hello>(&json).expect("a hello parses"),
            hello
        );

        let welcome = HelloAnswer::Welcome { session };
        let json = serde_json::to_string(&welcome).expect("an answer serializes");
        assert!(json.starts_with("{\"answer\":\"welcome\""), "{json}");
        assert_eq!(
            serde_json::from_str::<HelloAnswer>(&json).expect("an answer parses"),
            welcome
        );
        assert_eq!(
            serde_json::to_string(&HelloAnswer::Refused).expect("refused serializes"),
            "{\"answer\":\"refused\"}"
        );
    }

    #[test]
    fn a_hello_with_a_non_canonical_session_does_not_parse() {
        let session = ManagedSessionId::now().to_string().to_uppercase();
        let json = format!("{{\"protocol_version\":1,\"session\":\"{session}\",\"token\":\"t\"}}");
        assert!(serde_json::from_str::<Hello>(&json).is_err());
    }

    #[test]
    fn a_hello_never_shows_its_token() {
        let hello = Hello::new(ManagedSessionId::now(), "secret-token".to_owned());
        let rendered = format!("{hello:?}");
        assert!(!rendered.contains("secret"), "{rendered}");
        assert!(rendered.contains("session"), "{rendered}");
    }
}
