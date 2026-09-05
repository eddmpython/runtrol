//! The courier command: a managed session's own line to the dialogue endpoint.
//!
//! For now it does one thing, and it is the thing every later verb stands on: it proves the connection is
//! admitted. It reads the four values the Runtime gave this process at birth, connects to the endpoint, says
//! hello as its session, and reports what the Runtime answered. A process the Runtime did not start as a managed
//! session has none of those values and cannot even ask. The verbs (list, tell, ask) arrive in later stamps.

use runtrol_courier::ManagedSessionId;
use runtrol_courier::env::{COURIER_ENDPOINT_ENV, COURIER_TOKEN_ENV, MANAGED_SESSION_ENV};
use runtrol_courier::wire::{Hello, HelloAnswer, MAX_FRAME_BYTES};

/// What the courier connection came back with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Admission {
    /// The Runtime admitted this connection as its session.
    Welcomed,
    /// The Runtime refused it. The reason stays on the Runtime's side; this process learns only that it was out.
    Refused,
}

impl Admission {
    /// The one word this outcome prints, which a journey reads back from the terminal.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Welcomed => "welcome",
            Self::Refused => "refused",
        }
    }
}

/// Why the courier could not even ask.
#[derive(Debug, thiserror::Error)]
pub enum CourierFailure {
    /// A birth value is missing: this process was not started by the Runtime as a managed session.
    #[error("{0} is not set: this process is not a Runtime-managed session")]
    NotManaged(&'static str),
    /// The managed session value is not a canonical identifier.
    #[error("the managed session value {0:?} is not a session identifier")]
    Session(String),
    /// The endpoint could not be reached, or the connection failed while asking.
    #[error(transparent)]
    Transport(#[from] runtrol_ipc::TransportError),
    /// The endpoint closed without answering.
    #[error("the courier endpoint sent no answer before it closed")]
    NoAnswer,
    /// The answer was not one this build understands.
    #[error("the courier endpoint's answer was not one this build understands")]
    Unintelligible,
}

fn birth_value(name: &'static str) -> Result<String, CourierFailure> {
    std::env::var(name).map_err(|_missing| CourierFailure::NotManaged(name))
}

/// Say hello on the courier endpoint and report whether this session was admitted.
///
/// # Errors
///
/// [`CourierFailure`] when this process is not a managed session, the endpoint cannot be reached, or its answer
/// is missing or unintelligible.
pub async fn courier() -> Result<Admission, CourierFailure> {
    let endpoint = birth_value(COURIER_ENDPOINT_ENV)?;
    let token = birth_value(COURIER_TOKEN_ENV)?;
    let session_text = birth_value(MANAGED_SESSION_ENV)?;
    let session: ManagedSessionId = session_text
        .parse()
        .map_err(|_not_canonical| CourierFailure::Session(session_text))?;

    let mut connection = runtrol_ipc::connect(&endpoint).await?;
    let hello = serde_json::to_vec(&Hello::new(session, token))
        .map_err(|_unencodable| CourierFailure::Unintelligible)?;
    connection.send(&hello).await?;
    let frame = connection
        .recv_bounded(MAX_FRAME_BYTES)
        .await?
        .ok_or(CourierFailure::NoAnswer)?;
    match serde_json::from_slice::<HelloAnswer>(&frame) {
        Ok(HelloAnswer::Welcome { session: admitted }) if admitted == session => {
            Ok(Admission::Welcomed)
        }
        Ok(HelloAnswer::Refused) => Ok(Admission::Refused),
        Ok(HelloAnswer::Welcome { .. }) | Err(_) => Err(CourierFailure::Unintelligible),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_outcome_prints_its_own_word() {
        assert_eq!(Admission::Welcomed.word(), "welcome");
        assert_eq!(Admission::Refused.word(), "refused");
    }

    // The not-a-managed-session path reads the process environment, which edition 2024 makes unsafe to change
    // and this crate forbids. The real journey proves it: a process the Runtime did not start, run outside the
    // managed tree, cannot ask and is refused.
}
