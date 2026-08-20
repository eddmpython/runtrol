//! One command, from a connection to what a person reads.
//!
//! # The greeting is part of asking, not something the operator does
//!
//! Every connection agrees a wire format before anything else on it is acted on, and that rule belongs to the daemon.
//! What belongs here is that nobody has to know about it: a command greets, and if the answer is that the two builds
//! do not speak the same format, that is what the operator is told. It is never a request that silently did nothing.
//!
//! # The panic button ends the connection, and that is what success looks like
//!
//! Stopping every agent on the machine stops the daemon too. It is not a side effect to be tidied away: the daemon
//! holds its agents by being inside the same containment, which is what makes "everything stops" a guarantee the
//! kernel enforces rather than a loop over processes that might miss one.
//!
//! So this one request does not come back. The connection ends instead, and reporting that as a failure would tell
//! an operator their panic button broke at the moment it worked. What is written is what was observed and what it
//! means, not an outcome nobody saw, and the operator can confirm it with a listing: that starts a fresh daemon and
//! shows nothing running.
//!
//! # Watching is the same command with a different ending
//!
//! Every other command is one question and one answer. Watching turns the connection into a view of a session, so it
//! keeps reading until the session's stream ends or the operator stops it. The difference is where it stops, not what
//! it does, which is why it is one function and not two.

use runtrol_ipc::wire::{Request, Response};

use crate::link::Unreachable;

/// A command could not be carried out.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Failed {
    /// No daemon could be reached.
    #[error(transparent)]
    Unreachable(#[from] Unreachable),

    /// The connection failed while the command was in flight.
    #[error(transparent)]
    Transport(#[from] runtrol_ipc::transport::TransportError),

    /// The daemon stopped without answering.
    ///
    /// Told apart from every other failure, because it is the one that means the daemon itself is the problem rather
    /// than the request. Every request is answered, including the ones that cannot be carried out, so silence is a
    /// fact about the daemon.
    #[error("the daemon stopped without answering")]
    NoAnswer,

    /// An answer arrived that this build cannot read.
    #[error("the daemon sent an answer this build cannot read: {detail}")]
    Unreadable {
        /// What went wrong reading it.
        detail: String,
    },

    /// The two builds do not speak the same wire format.
    ///
    /// Named rather than folded into a general failure, because the operator's next move is to make the two the same
    /// version, which no other failure calls for.
    #[error("this runtrol and its daemon do not speak the same wire format: {said}")]
    DifferentBuilds {
        /// What the daemon said about it.
        said: String,
    },
}

/// What the daemon did about a request, once its answer has been written.
///
/// # Why a refusal is an answer here and a failure to whoever ran the command
///
/// A refusal is not a failure of the connection, of this build, or of the daemon: the request arrived, was
/// understood, and was declined, and the operator is told why in the daemon's own words. So it is not a
/// [`Failed`].
///
/// It is still not success. Whoever ran the command reads an exit status, and a command that did not do what
/// it was asked to do must not report that it did. Measured: a resume of a conversation the provider refuses
/// to continue printed the refusal and exited zero, so anything scripted around it saw a resume that worked.
/// # Why this is exhaustive
///
/// Every request is answered, and an answer either carries the request out or declines it. There is no third
/// thing at this seam, and leaving room for one would make every caller invent a policy for a case that cannot
/// arrive. If a third ever does exist, the one place that has to decide what it means should stop compiling
/// rather than quietly fold it into success.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Outcome {
    /// The daemon carried the request out.
    Carried,
    /// The daemon answered, and the answer was that it would not.
    Refused,
}

/// Ask a daemon one thing, and hand back what a person reads.
///
/// `runtrol` is the executable a daemon is started from when none is running. Named by the caller and never inferred:
/// see [`crate::link`].
///
/// Every line it produces is written as it is produced rather than collected, so a watch shows a conversation as it
/// happens instead of at the end. That is what `write` is for: it is called once per line, and what it does with the
/// line is the caller's business.
///
/// # Errors
///
/// [`Failed`] when no daemon could be reached, when the connection broke, or when the daemon said nothing. A request
/// the daemon refused is not a failure here: a refusal is an answer, it goes to `write` like any other, and it comes
/// back as [`Outcome::Refused`] so that whoever ran the command can exit accordingly.
pub async fn ask<Write>(
    address: &str,
    runtrol: &std::path::Path,
    request: Request,
    mut write: Write,
) -> Result<Outcome, Failed>
where
    Write: FnMut(&str),
{
    let mut connection = crate::link::reach(address, runtrol).await?;

    // The greeting first, and its answer read before anything else goes out. A build that spoke without waiting would
    // have its real request refused and would have to work out why from a message about a format it never mentioned.
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

    let watching = matches!(request, Request::Watch { .. });
    let stopping_everything = matches!(request, Request::StopEverything);

    let answer = match exchange(&mut connection, &request).await {
        Ok(answer) => answer,
        // The daemon went away without answering. For every other request that is a fact about the daemon; for
        // this one it is the request having been carried out, because what was stopped includes the daemon.
        Err(Failed::NoAnswer) if stopping_everything => {
            write("the daemon stopped, which is what stopping everything on this machine does");
            write("run any command to start a fresh one, and a listing will show nothing running");
            return Ok(Outcome::Carried);
        }
        Err(other) => return Err(other),
    };
    for line in crate::lines::render(&answer) {
        write(&line);
    }

    // Written first and reported second. The operator reads the daemon's own words either way; what changes is
    // the status the command exits with, and a refusal must not exit as though the work was done.
    if matches!(answer, Response::Failed(_)) {
        return Ok(Outcome::Refused);
    }

    if !watching {
        return Ok(Outcome::Carried);
    }

    // A view of the session from here on. It ends when the session's stream does, or when the operator stops this
    // command, and neither of those is a failure.
    loop {
        let Some(frame) = connection.recv().await? else {
            return Ok(Outcome::Carried);
        };
        let event: Response =
            serde_json::from_slice(&frame).map_err(|error| Failed::Unreadable {
                detail: error.to_string(),
            })?;
        for line in crate::lines::render(&event) {
            write(&line);
        }
        if let Some(outcome) = streaming_outcome(&event) {
            return Ok(outcome);
        }
    }
}

/// A stream failure ends a watch as a refused command after its diagnostic has been written.
fn streaming_outcome(response: &Response) -> Option<Outcome> {
    matches!(response, Response::Failed(_)).then_some(Outcome::Refused)
}

/// Send one request and read the one answer to it.
async fn exchange(
    connection: &mut runtrol_ipc::transport::Connection,
    request: &Request,
) -> Result<Response, Failed> {
    let frame = serde_json::to_vec(request).map_err(|error| Failed::Unreadable {
        detail: error.to_string(),
    })?;
    connection.send(&frame).await?;

    let answer = connection.recv().await?.ok_or(Failed::NoAnswer)?;
    serde_json::from_slice(&answer).map_err(|error| Failed::Unreadable {
        detail: error.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failure_mid_watch_is_not_reported_as_a_successful_stream_end() {
        let failure = Response::Failed(runtrol_ipc::wire::WireError {
            message: "one event could not cross the wire".into(),
            retryable: false,
            needs_the_operator: false,
        });
        assert_eq!(streaming_outcome(&failure), Some(Outcome::Refused));
        assert_eq!(streaming_outcome(&Response::Done), None);
    }

    #[test]
    fn a_daemon_that_says_nothing_is_told_apart_from_one_that_refuses() {
        // Every request is answered, including the ones that cannot be carried out, so silence is a fact about the
        // daemon rather than about the request. An operator told "refused" would go looking at their command.
        assert_eq!(
            Failed::NoAnswer.to_string(),
            "the daemon stopped without answering"
        );
    }

    /// A daemon that greets and then goes away without answering anything else.
    ///
    /// Which is exactly what the real one does when it is told to stop everything, because what it stops includes
    /// itself. Started here so the rule is checked against the behaviour rather than against a matcher.
    async fn a_daemon_that_greets_then_vanishes(name: &str) -> String {
        let address = if cfg!(windows) {
            format!(r"\\.\pipe\runtrol-test-ask-{name}-{}", std::process::id())
        } else {
            let dir =
                std::env::temp_dir().join(format!("runtrol-ask-{name}-{}", std::process::id()));
            std::fs::create_dir_all(&dir).expect("create the scratch directory");
            dir.join("runtrol.sock").to_string_lossy().into_owned()
        };

        // Bound before this returns, so that a caller connecting straight away finds it rather than deciding no
        // daemon is running and trying to start one.
        let mut listener = runtrol_ipc::transport::Listener::bind(&address)
            .await
            .expect("the endpoint binds");
        drop(tokio::spawn(async move {
            while let Ok(mut connection) = listener.accept().await {
                // The greeting, and then nothing. Dropping the connection is the daemon going away.
                if connection.recv().await.is_ok() {
                    let welcome = serde_json::to_vec(&Response::Welcome {
                        wire: runtrol_ipc::WIRE_VERSION,
                        providers: Vec::new(),
                        device: None,
                        push_public_key: None,
                        build_digest: None,
                    })
                    .expect("writable");
                    drop(connection.send(&welcome).await);
                    drop(connection.recv().await);
                }
            }
        }));
        address
    }

    #[tokio::test]
    async fn silence_is_a_failure_for_every_request_except_the_one_that_stops_the_daemon() {
        // The exception is narrow on purpose. A daemon that stopped answering is a real failure and has to read
        // as one; the panic button is the single request whose success is the connection ending, because what it
        // stops includes the thing that would have answered.
        let address = a_daemon_that_greets_then_vanishes("silence").await;
        let unreachable = std::path::Path::new("this-program-does-not-exist-and-that-is-the-point");

        let mut said = Vec::new();
        ask(
            &address,
            unreachable,
            Request::StopEverything,
            |line: &str| said.push(line.to_owned()),
        )
        .await
        .expect("a connection that ends is what stopping everything looks like");
        assert!(
            said.iter().any(|line| line.contains("the daemon stopped")),
            "{said:?}"
        );

        match ask(&address, unreachable, Request::List, |_line: &str| {}).await {
            Err(Failed::NoAnswer) => {}
            other => panic!("silence about any other request is a failure, got {other:?}"),
        }
    }

    /// A daemon that greets and then refuses whatever it is asked, the way the real one answers a request a
    /// provider declined.
    async fn a_daemon_that_refuses(name: &str) -> String {
        let address = if cfg!(windows) {
            format!(r"\\.\pipe\runtrol-test-ask-{name}-{}", std::process::id())
        } else {
            let dir =
                std::env::temp_dir().join(format!("runtrol-ask-{name}-{}", std::process::id()));
            std::fs::create_dir_all(&dir).expect("create the scratch directory");
            dir.join("runtrol.sock").to_string_lossy().into_owned()
        };

        let mut listener = runtrol_ipc::transport::Listener::bind(&address)
            .await
            .expect("the endpoint binds");
        drop(tokio::spawn(async move {
            while let Ok(mut connection) = listener.accept().await {
                let welcome = serde_json::to_vec(&Response::Welcome {
                    wire: runtrol_ipc::WIRE_VERSION,
                    providers: Vec::new(),
                    device: None,
                    push_public_key: None,
                    build_digest: None,
                })
                .expect("writable");
                if connection.recv().await.is_ok() {
                    drop(connection.send(&welcome).await);
                }
                let refusal = serde_json::to_vec(&Response::Failed(runtrol_ipc::wire::WireError {
                    message: "no rollout found for that conversation".into(),
                    retryable: false,
                    needs_the_operator: false,
                }))
                .expect("writable");
                if connection.recv().await.is_ok() {
                    drop(connection.send(&refusal).await);
                }
            }
        }));
        address
    }

    #[tokio::test]
    async fn a_refusal_is_written_for_the_operator_and_reported_as_not_carried_out() {
        // Both halves matter and they are different audiences. A person reads the daemon's own words; whatever
        // ran the command reads a status. Measured before this existed: a resume the provider refused printed
        // the refusal and exited zero, so anything scripted around it saw a resume that had worked.
        let address = a_daemon_that_refuses("refusal").await;
        let unreachable = std::path::Path::new("this-program-does-not-exist-and-that-is-the-point");

        let mut said = Vec::new();
        let outcome = ask(&address, unreachable, Request::List, |line: &str| {
            said.push(line.to_owned());
        })
        .await
        .expect("a refusal is an answer, not a broken connection");

        assert_eq!(outcome, Outcome::Refused);
        assert!(
            said.iter().any(|line| line.contains("no rollout")),
            "the daemon's own words have to reach the operator: {said:?}"
        );
    }

    #[test]
    fn a_version_mismatch_says_what_it_is_rather_than_that_something_went_wrong() {
        // The one failure whose answer is "make the two the same version". Folded into a general failure, it would
        // read as a broken request and send the operator to look at what they typed.
        let said = Failed::DifferentBuilds {
            said: "this daemon speaks wire format 1 and the caller speaks 2".to_owned(),
        }
        .to_string();
        assert!(said.contains("wire format"), "{said}");
    }
}
