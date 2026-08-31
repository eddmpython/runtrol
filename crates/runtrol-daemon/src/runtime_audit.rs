//! Content-free bounded audit composition for public Runtime and local integration administration.
//!
//! Every row goes to the durable store. A draining generation has handed that store to its successor, so its
//! rows go to the relay the successor takes on its next handoff poll (`audit_relay`). Refusing to serve
//! instead, which is what a released store used to mean here, retired the whole public surface of every
//! draining generation the moment the store moved (measured 2026-08-29).

use runtrol_provider::WallMs;
use runtrol_runtime_protocol::{AppScope, MutationRequestId, RuntimeMethod};
use runtrol_store::{IntegrationAuditOutcome, IntegrationAuditRow, IntegrationKey, StoreError};
use tokio::sync::{mpsc, oneshot};

use crate::compose::Composed;

const PUBLIC_AUDIT_QUEUE_ROWS: usize = 64;
const PUBLIC_AUDIT_BATCH_ROWS: usize = 64;
const PUBLIC_AUDIT_ACTIVE_REQUESTS: usize = 32;
const ADMISSION_FRESH: u8 = 0;
const ADMISSION_ATTEMPTED: u8 = 1;
const ADMISSION_FINISHED: u8 = 2;

/// Cloneable public request journal backed by one bounded FIFO writer.
#[derive(Clone)]
pub(crate) struct AuditJournal {
    sender: mpsc::Sender<AuditWrite>,
    admissions: std::sync::Arc<AuditAdmissions>,
}

struct AuditAdmissions {
    slots: std::sync::Arc<tokio::sync::Semaphore>,
    active: std::sync::atomic::AtomicUsize,
    idle: tokio::sync::Notify,
}

/// One admitted request and the correlation shared by its attempted and terminal rows.
pub(crate) struct AuditAdmission {
    admissions: std::sync::Arc<AuditAdmissions>,
    request_id: Box<str>,
    stage: std::sync::atomic::AtomicU8,
    _slot: tokio::sync::OwnedSemaphorePermit,
}

/// Stable structural fields written at one request stage.
#[derive(Clone, Copy)]
pub(crate) struct AuditContext {
    integration: Option<IntegrationKey>,
    key_generation: Option<u64>,
    method: RuntimeMethod,
    scope: Option<AppScope>,
}

struct AuditWrite {
    row: IntegrationAuditRow,
    committed: oneshot::Sender<bool>,
}

#[derive(Debug)]
pub(crate) struct AuditUnavailable;

/// Why the single audit writer could no longer provide durable acknowledgements.
#[derive(Debug, thiserror::Error)]
pub(crate) enum AuditWriterError {
    #[error("the blocking audit commit task stopped: {0}")]
    Blocking(#[source] tokio::task::JoinError),
    #[error("the durable audit commit failed: {0}")]
    Store(#[source] StoreError),
}

/// Build one journal and its generation-scoped writer task.
pub(crate) fn journal(
    composed: std::sync::Arc<Composed>,
) -> (
    AuditJournal,
    impl std::future::Future<Output = Result<(), AuditWriterError>> + Send + 'static,
) {
    let (sender, receiver) = mpsc::channel(PUBLIC_AUDIT_QUEUE_ROWS);
    (
        AuditJournal {
            sender,
            admissions: std::sync::Arc::new(AuditAdmissions {
                slots: std::sync::Arc::new(tokio::sync::Semaphore::new(
                    PUBLIC_AUDIT_ACTIVE_REQUESTS,
                )),
                active: std::sync::atomic::AtomicUsize::new(0),
                idle: tokio::sync::Notify::new(),
            }),
        },
        write_journal(composed, receiver),
    )
}

impl AuditJournal {
    /// Reserve one request through both durable stages without creating an unbounded queue of send waiters.
    pub(crate) fn try_admit(&self) -> Result<AuditAdmission, AuditUnavailable> {
        self.admissions
            .active
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        let Ok(slot) = std::sync::Arc::clone(&self.admissions.slots).try_acquire_owned() else {
            self.admissions.leave();
            return Err(AuditUnavailable);
        };
        Ok(AuditAdmission {
            admissions: std::sync::Arc::clone(&self.admissions),
            request_id: MutationRequestId::now().as_str().into(),
            stage: std::sync::atomic::AtomicU8::new(ADMISSION_FRESH),
            _slot: slot,
        })
    }

    /// Refuse new requests while already admitted requests finish their two durable stages.
    pub(crate) fn begin_shutdown(&self) {
        self.admissions.slots.close();
    }

    /// Wait until every request admitted before shutdown has completed its terminal audit row.
    pub(crate) async fn wait_until_idle(&self) {
        loop {
            let notified = self.admissions.idle.notified();
            if self
                .admissions
                .active
                .load(std::sync::atomic::Ordering::Acquire)
                == 0
            {
                return;
            }
            notified.await;
        }
    }

    /// Durably record the one attempted row allowed for this admitted request.
    pub(crate) async fn attempt(
        &self,
        admission: &AuditAdmission,
        context: AuditContext,
    ) -> Result<(), AuditUnavailable> {
        admission.transition(ADMISSION_FRESH, ADMISSION_ATTEMPTED)?;
        self.append(row(
            context.integration,
            context.key_generation,
            context.method.as_str(),
            context.scope.map(AppScope::as_str),
            Some(admission.request_id.clone()),
            IntegrationAuditOutcome::Attempted,
            "attempted",
        ))
        .await
    }

    /// Durably record the one terminal outcome paired with this request's attempted row.
    pub(crate) async fn finish(
        &self,
        admission: &AuditAdmission,
        context: AuditContext,
        outcome: IntegrationAuditOutcome,
        reason: &'static str,
    ) -> Result<(), AuditUnavailable> {
        if outcome == IntegrationAuditOutcome::Attempted {
            return Err(AuditUnavailable);
        }
        admission.transition(ADMISSION_ATTEMPTED, ADMISSION_FINISHED)?;
        self.append(row(
            context.integration,
            context.key_generation,
            context.method.as_str(),
            context.scope.map(AppScope::as_str),
            Some(admission.request_id.clone()),
            outcome,
            reason,
        ))
        .await
    }

    /// Durably record one structurally invalid envelope as the request's only terminal row.
    pub(crate) async fn deny_structural(
        &self,
        admission: &AuditAdmission,
        method: &'static str,
        reason: &'static str,
    ) -> Result<(), AuditUnavailable> {
        admission.transition(ADMISSION_FRESH, ADMISSION_FINISHED)?;
        self.append(row(
            None,
            None,
            method,
            None,
            Some(admission.request_id.clone()),
            IntegrationAuditOutcome::Denied,
            reason,
        ))
        .await
    }

    async fn append(&self, row: IntegrationAuditRow) -> Result<(), AuditUnavailable> {
        let (committed, completion) = oneshot::channel();
        self.sender
            .send(AuditWrite { row, committed })
            .await
            .map_err(|_| AuditUnavailable)?;
        match completion.await {
            Ok(true) => Ok(()),
            Ok(false) | Err(_) => Err(AuditUnavailable),
        }
    }
}

impl AuditContext {
    pub(crate) const fn new(
        integration: Option<IntegrationKey>,
        key_generation: Option<u64>,
        method: RuntimeMethod,
        scope: Option<AppScope>,
    ) -> Self {
        Self {
            integration,
            key_generation,
            method,
            scope,
        }
    }
}

impl AuditAdmissions {
    fn leave(&self) {
        if self
            .active
            .fetch_sub(1, std::sync::atomic::Ordering::AcqRel)
            == 1
        {
            self.idle.notify_waiters();
        }
    }
}

impl Drop for AuditAdmission {
    fn drop(&mut self) {
        self.admissions.leave();
    }
}

impl AuditAdmission {
    fn transition(&self, from: u8, to: u8) -> Result<(), AuditUnavailable> {
        self.stage
            .compare_exchange(
                from,
                to,
                std::sync::atomic::Ordering::AcqRel,
                std::sync::atomic::Ordering::Acquire,
            )
            .map(|_| ())
            .map_err(|_| AuditUnavailable)
    }
}

async fn write_journal(
    composed: std::sync::Arc<Composed>,
    mut receiver: mpsc::Receiver<AuditWrite>,
) -> Result<(), AuditWriterError> {
    while let Some(first) = receiver.recv().await {
        let mut batch = Vec::with_capacity(PUBLIC_AUDIT_BATCH_ROWS);
        batch.push(first);
        // Give requests already runnable on sibling connections one scheduler turn to join this transaction.
        tokio::task::yield_now().await;
        while batch.len() < PUBLIC_AUDIT_BATCH_ROWS {
            let Ok(next) = receiver.try_recv() else {
                break;
            };
            batch.push(next);
        }
        let mut rows = Vec::with_capacity(batch.len());
        let mut completions = Vec::with_capacity(batch.len());
        for write in batch {
            rows.push(write.row);
            completions.push(write.committed);
        }
        let target = std::sync::Arc::clone(&composed);
        let committed = match tokio::task::spawn_blocking(move || append_batch(&target, rows)).await
        {
            Ok(result) => result.map_err(AuditWriterError::Store),
            Err(error) => Err(AuditWriterError::Blocking(error)),
        };
        for completion in completions {
            let _sent = completion.send(committed.is_ok());
        }
        if let Err(error) = committed {
            receiver.close();
            return Err(error);
        }
    }
    Ok(())
}

fn append_batch(composed: &Composed, rows: Vec<IntegrationAuditRow>) -> Result<(), StoreError> {
    match composed.store.append_integration_audit_batch(&rows) {
        // Draining: the successor owns the store now and takes these rows on its next poll.
        Err(StoreError::Released { .. }) => {
            composed.audit_relay.push_batch(rows);
            Ok(())
        }
        other => other,
    }
}

/// Record one public Runtime authorization stage.
#[cfg(test)]
pub(crate) fn public(
    composed: &Composed,
    integration: Option<IntegrationKey>,
    key_generation: Option<u64>,
    method: RuntimeMethod,
    scope: Option<AppScope>,
    outcome: IntegrationAuditOutcome,
    reason: &'static str,
) -> Result<(), StoreError> {
    append(
        composed,
        row(
            integration,
            key_generation,
            method.as_str(),
            scope.map(AppScope::as_str),
            None,
            outcome,
            reason,
        ),
    )
}

/// Record one private local integration administration stage.
pub(crate) fn local(
    composed: &Composed,
    integration: Option<IntegrationKey>,
    key_generation: Option<u64>,
    method: &'static str,
    outcome: IntegrationAuditOutcome,
    reason: &'static str,
) -> Result<(), StoreError> {
    append(
        composed,
        row(
            integration,
            key_generation,
            method,
            None,
            None,
            outcome,
            reason,
        ),
    )
}

/// Record a structurally invalid public envelope without retaining any caller-supplied method text.
#[cfg(test)]
pub(crate) fn structural(
    composed: &Composed,
    method: &'static str,
    reason: &'static str,
) -> Result<(), StoreError> {
    append(
        composed,
        row(
            None,
            None,
            method,
            None,
            None,
            IntegrationAuditOutcome::Denied,
            reason,
        ),
    )
}

fn append(composed: &Composed, row: IntegrationAuditRow) -> Result<(), StoreError> {
    match composed.store.append_integration_audit(&row) {
        // Draining: the successor owns the store now and takes this row on its next poll.
        Err(StoreError::Released { .. }) => {
            composed.audit_relay.push(row);
            Ok(())
        }
        other => other,
    }
}

fn row(
    integration: Option<IntegrationKey>,
    key_generation: Option<u64>,
    method: &'static str,
    scope: Option<&'static str>,
    request_id: Option<Box<str>>,
    outcome: IntegrationAuditOutcome,
    reason: &'static str,
) -> IntegrationAuditRow {
    IntegrationAuditRow {
        occurred_at: WallMs::now(),
        integration,
        key_generation,
        method: method.into(),
        scope: scope.map(Into::into),
        project: None,
        session: None,
        request_id,
        outcome,
        reason: reason.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn composed_for(name: &str) -> (Composed, String) {
        let root =
            std::env::temp_dir().join(format!("runtrol-audit-{}-{name}", std::process::id()));
        if root.exists() {
            std::fs::remove_dir_all(&root).expect("clear the previous run");
        }
        let text = root.to_str().expect("the scratch path is UTF-8").to_owned();
        // Composing without establishing containment: doing that in a test terminates the runner on one platform.
        let composed = Composed::for_tests(&text, runtrol_drivers::builtin())
            .expect("assemble a scratch home");
        (composed, text)
    }

    fn clean(composed: Composed, path: &str) {
        // The store owns an exclusive file handle. Release it before removing the scratch home.
        drop(composed);
        std::fs::remove_dir_all(path).expect("remove the scratch home");
    }

    #[test]
    fn a_row_goes_to_the_store_while_this_generation_holds_it() {
        let (composed, path) = composed_for("held");
        public(
            &composed,
            None,
            None,
            RuntimeMethod::TerminalsList,
            None,
            IntegrationAuditOutcome::Attempted,
            "attempted",
        )
        .expect("recorded");
        let rows = composed.store.list_integration_audit().expect("listed");
        assert!(
            rows.iter()
                .any(|row| &*row.method == RuntimeMethod::TerminalsList.as_str())
        );
        assert!(
            composed.audit_relay.is_empty(),
            "nothing waits for a successor"
        );
        clean(composed, &path);
    }

    #[test]
    fn a_draining_generation_keeps_the_row_for_its_successor_and_serves_on() {
        let (composed, path) = composed_for("drained");
        assert!(composed.store.release(), "the store is handed over once");

        public(
            &composed,
            None,
            None,
            RuntimeMethod::TerminalsAttach,
            None,
            IntegrationAuditOutcome::Attempted,
            "attempted",
        )
        .expect("a released store is not a refusal");

        let relayed = composed.audit_relay.snapshot();
        assert_eq!(relayed.entries.len(), 1);
        let kept = relayed.entries.first().expect("the one row kept");
        assert_eq!(&*kept.row.method, RuntimeMethod::TerminalsAttach.as_str());
        clean(composed, &path);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[expect(
        clippy::too_many_lines,
        reason = "one concurrency test keeps admission, paired correlation, durable acknowledgement, shutdown, and writer drain in the same race"
    )]
    async fn journal_keeps_concurrent_requests_and_acks_visible_commits() {
        const STAGES_PER_REQUEST: usize = 2;

        let (composed, path) = composed_for("journal");
        let composed = std::sync::Arc::new(composed);
        let (journal, writer) = journal(std::sync::Arc::clone(&composed));
        let writer = tokio::spawn(writer);
        let start =
            std::sync::Arc::new(tokio::sync::Barrier::new(PUBLIC_AUDIT_ACTIVE_REQUESTS + 1));
        let mut requests = Vec::with_capacity(PUBLIC_AUDIT_ACTIVE_REQUESTS);

        for request in 0..PUBLIC_AUDIT_ACTIVE_REQUESTS {
            let admission = journal.try_admit().expect("request admitted");
            let request_journal = journal.clone();
            let request_composed = std::sync::Arc::clone(&composed);
            let request_start = std::sync::Arc::clone(&start);
            requests.push(tokio::spawn(async move {
                request_start.wait().await;
                let terminal = if request % 2 == 0 {
                    IntegrationAuditOutcome::Allowed
                } else {
                    IntegrationAuditOutcome::Denied
                };
                let context = AuditContext::new(
                    None,
                    Some(u64::try_from(request).expect("request fits u64")),
                    RuntimeMethod::TerminalsList,
                    None,
                );
                for outcome in [IntegrationAuditOutcome::Attempted, terminal] {
                    let recorded = if outcome == IntegrationAuditOutcome::Attempted {
                        request_journal.attempt(&admission, context).await
                    } else {
                        request_journal
                            .finish(
                                &admission,
                                context,
                                outcome,
                                match outcome {
                                    IntegrationAuditOutcome::Attempted => "attempted",
                                    IntegrationAuditOutcome::Allowed => "allowed",
                                    IntegrationAuditOutcome::Denied => "denied",
                                },
                            )
                            .await
                    };
                    recorded.expect("journal append acknowledged");
                    assert!(
                        request_composed
                            .store
                            .list_integration_audit()
                            .expect("list after acknowledgement")
                            .iter()
                            .any(|row| {
                                row.request_id.as_deref() == Some(&*admission.request_id)
                                    && row.outcome == outcome
                            }),
                        "an acknowledgement must follow the durable commit"
                    );
                }
                assert!(
                    request_journal
                        .finish(&admission, context, terminal, "duplicateTerminalOutcome",)
                        .await
                        .is_err(),
                    "one admission cannot record a second terminal outcome"
                );
                (admission.request_id.clone(), terminal)
            }));
        }
        assert!(
            journal.try_admit().is_err(),
            "the active request bound is exact"
        );
        journal.begin_shutdown();
        assert!(
            journal.try_admit().is_err(),
            "shutdown closes admission permanently"
        );
        let waiting = tokio::spawn({
            let journal = journal.clone();
            async move { journal.wait_until_idle().await }
        });
        tokio::task::yield_now().await;
        assert!(
            !waiting.is_finished(),
            "shutdown waits for admitted terminal rows"
        );

        start.wait().await;
        let mut expected = std::collections::BTreeMap::new();
        for request in requests {
            let (request_id, outcome) = request.await.expect("concurrent audit request joined");
            assert!(
                expected.insert(request_id, outcome).is_none(),
                "every admitted request has one correlation"
            );
        }
        waiting.await.expect("idle waiter joined");
        drop(journal);
        tokio::time::timeout(std::time::Duration::from_secs(5), writer)
            .await
            .expect("journal writer stopped")
            .expect("journal writer joined")
            .expect("journal writer remained healthy");

        let rows = composed
            .store
            .list_integration_audit()
            .expect("list journal");
        assert_eq!(
            rows.len(),
            PUBLIC_AUDIT_ACTIVE_REQUESTS * STAGES_PER_REQUEST,
            "the journal neither loses nor duplicates admitted stages"
        );
        let mut correlated =
            std::collections::BTreeMap::<Box<str>, Vec<IntegrationAuditOutcome>>::new();
        for row in rows {
            correlated
                .entry(row.request_id.expect("public row correlation"))
                .or_default()
                .push(row.outcome);
        }
        assert_eq!(correlated.len(), PUBLIC_AUDIT_ACTIVE_REQUESTS);
        for (request_id, terminal) in expected {
            assert_eq!(
                correlated.remove(&request_id),
                Some(vec![IntegrationAuditOutcome::Attempted, terminal]),
                "one correlation joins exactly one attempted and one terminal row"
            );
        }
        assert!(correlated.is_empty());

        let Ok(composed) = std::sync::Arc::try_unwrap(composed) else {
            panic!("journal released the composed state");
        };
        clean(composed, &path);
    }
}
