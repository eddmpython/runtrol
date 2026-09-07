//! Hold exact OS witnesses before requesting shutdown; EOF alone proves no child completion.

use runtrol_childproc::KeeperWait;
use runtrol_ipc::Response;
use runtrol_provider::ProcessIdentity;

use crate::{Failed, Outcome};

pub(crate) fn prepare(welcome: &Response) -> Result<KeeperWait, Failed> {
    let Response::Welcome {
        process_completion: Some(proof),
        ..
    } = welcome
    else {
        return Err(Failed::Completion(
            "the Runtime did not publish an exact process completion witness".to_owned(),
        ));
    };
    let runtime = ProcessIdentity::new(proof.runtime_pid, proof.runtime_started)
        .ok_or_else(|| Failed::Completion("the Runtime process identity is invalid".to_owned()))?;
    let keeper = ProcessIdentity::new(proof.keeper_pid, proof.keeper_started)
        .ok_or_else(|| Failed::Completion("the completion owner identity is invalid".to_owned()))?;
    KeeperWait::open(runtime, keeper).map_err(|error| Failed::Completion(error.to_string()))
}

pub(crate) fn deadline() -> tokio::time::Instant {
    tokio::time::Instant::now() + std::time::Duration::from_secs(30)
}

pub(crate) async fn within<T>(
    deadline: tokio::time::Instant,
    work: impl std::future::Future<Output = Result<T, Failed>>,
) -> Result<T, Failed> {
    tokio::time::timeout_at(deadline, work).await.map_err(|_| Failed::Completion(
        "process completion was not confirmed within the shutdown deadline; Runtime state was retained".to_owned(),
    ))?
}

pub(crate) async fn finish<Exchange, Write>(
    deadline: tokio::time::Instant,
    prepared: Result<KeeperWait, Failed>,
    exchange: Exchange,
    mut write: Write,
) -> Result<Outcome, Failed>
where
    Exchange: std::future::Future<Output = Result<Response, Failed>>,
    Write: FnMut(&str),
{
    within(deadline, async {
        // Send the stop even when a legacy generation cannot supply completion proof. Bound both the reply and
        // the process waits: a Runtime that leaves its connection open must not hang the uninstall hook forever.
        if let Ok(Response::Failed(said)) = exchange.await {
            write(&said.message);
            return Ok(Outcome::Refused);
        }
        // Transport failure is inconclusive during shutdown. The already-open exact OS witnesses below,
        // rather than a reply or EOF, decide whether the Runtime and its retained children finished.
        prepared?
            .wait()
            .await
            .map_err(|error| Failed::Completion(error.to_string()))?;
        write("the Runtime and every retained process have completed shutdown");
        Ok(Outcome::Carried)
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_stalled_reply_cannot_hide_the_process_completion_deadline() {
        let stopped = finish(
            tokio::time::Instant::now() + std::time::Duration::from_millis(10),
            Err(Failed::Completion("missing proof".to_owned())),
            std::future::pending(),
            |_| {},
        )
        .await;
        assert!(
            matches!(stopped, Err(Failed::Completion(message)) if message.contains("deadline"))
        );
    }

    #[tokio::test]
    async fn legacy_shutdown_is_sent_but_cannot_report_completion() {
        let sent = std::cell::Cell::new(false);
        let stopped = finish(
            deadline(),
            Err(Failed::Completion("missing proof".to_owned())),
            async {
                sent.set(true);
                Err(Failed::NoAnswer)
            },
            |_| {},
        )
        .await;
        assert!(sent.get());
        assert!(matches!(stopped, Err(Failed::Completion(_))));
    }
}
