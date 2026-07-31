//! One question to the daemon, and the answer, for a window rather than a terminal.
//!
//! # Why this is not the command surface
//!
//! It asks the same daemon the same way, and the difference is what it does with a refusal. A terminal prints
//! it and exits with a status; a window has to put it on the screen and stay open. So the shapes differ at the
//! end even though the conversation is identical, and the conversation itself is not written twice: reaching a
//! daemon, and starting one when none is listening, is [`runtrol_cli::reach`].

use std::path::Path;

use runtrol_ipc::transport::Connection;
use runtrol_ipc::wire::{Request, Response};

/// A question could not be answered.
///
/// Every variant is something a window shows. There is no variant for a refusal: a daemon saying no is an
/// answer, and it arrives as [`Response::Failed`] like any other.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Failed {
    /// No daemon could be reached, and none could be started.
    #[error(transparent)]
    Unreachable(#[from] runtrol_cli::Unreachable),

    /// The connection failed while the question was in flight.
    #[error(transparent)]
    Transport(#[from] runtrol_ipc::transport::TransportError),

    /// The daemon went away without answering.
    ///
    /// Told apart from every other failure because it is the one that means the daemon itself is the problem
    /// rather than the question. Every request is answered, including the ones that cannot be carried out.
    #[error("the daemon stopped without answering")]
    NoAnswer,

    /// An answer arrived that this build cannot read.
    #[error("the daemon sent an answer this build cannot read: {detail}")]
    Unreadable {
        /// What went wrong reading it.
        detail: String,
    },

    /// The two builds do not speak the same wire format.
    #[error("this runtrol and its daemon do not speak the same wire format: {said}")]
    DifferentBuilds {
        /// What the daemon said about it.
        said: String,
    },
}

/// Open a connection and get the greeting out of the way.
///
/// # Errors
///
/// [`Failed`] when no daemon could be reached or the two builds disagree about the wire.
pub async fn greet(address: &str, runtrol: &Path) -> Result<Connection, Failed> {
    let mut connection = runtrol_cli::reach(address, runtrol).await?;
    // Agreed before anything else on the connection is acted on. A build that spoke without waiting would have
    // its real question refused and would have to work out why from a message about a format it never
    // mentioned.
    let welcome = exchange(
        &mut connection,
        &Request::Hello {
            wire: runtrol_ipc::WIRE_VERSION,
        },
    )
    .await?;
    if let Response::Failed(said) = &welcome {
        return Err(Failed::DifferentBuilds {
            said: said.message.to_string(),
        });
    }
    Ok(connection)
}

/// Ask one thing on a connection that has already greeted.
///
/// # Errors
///
/// [`Failed`] when the connection breaks or the answer cannot be read.
pub async fn exchange(connection: &mut Connection, request: &Request) -> Result<Response, Failed> {
    let frame = serde_json::to_vec(request).map_err(|error| Failed::Unreadable {
        detail: error.to_string(),
    })?;
    connection.send(&frame).await?;

    let answer = connection.recv().await?.ok_or(Failed::NoAnswer)?;
    serde_json::from_slice(&answer).map_err(|error| Failed::Unreadable {
        detail: error.to_string(),
    })
}

/// Open a connection, ask one thing, and let the connection go.
///
/// What every question except watching does. Watching keeps its connection, because it is a view rather than a
/// question.
///
/// # Errors
///
/// [`Failed`] in every case where no answer could be had.
pub async fn once(address: &str, runtrol: &Path, request: Request) -> Result<Response, Failed> {
    let mut connection = greet(address, runtrol).await?;
    exchange(&mut connection, &request).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_daemon_that_says_nothing_is_told_apart_from_one_that_refuses() {
        // Every request is answered, including the ones that cannot be carried out, so silence is a fact about
        // the daemon rather than about the request. A window that showed "refused" would send the operator
        // looking at what they asked for.
        assert_eq!(
            Failed::NoAnswer.to_string(),
            "the daemon stopped without answering"
        );
    }

    #[test]
    fn there_is_no_failure_variant_for_a_refusal() {
        // A refusal is an answer and has to reach the screen as one, with the daemon's own words. Making it an
        // error here would collapse "the provider said no" and "the connection broke" into one red box.
        let printed = format!(
            "{:?}",
            Failed::Unreadable {
                detail: "x".to_owned()
            }
        );
        assert!(!printed.contains("Refused"), "{printed}");
    }
}
