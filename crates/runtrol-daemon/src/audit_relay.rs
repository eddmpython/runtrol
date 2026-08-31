//! Authorization records of a draining generation, kept for the successor that owns the store.
//!
//! A draining generation has handed its durable store to the successor. Admitted public control requests
//! still record their content-free authorization stages, while attached terminal bytes stay on the direct
//! data plane. Refusing control because those rows had nowhere to go retired every draining generation's
//! surface the moment the store moved. The rows therefore stay in this bounded relay under a process-unique
//! epoch and sequence until a successor proves their durable commit with a later ACK.

use std::collections::VecDeque;
use std::sync::{PoisonError, RwLock};

use runtrol_ipc::wire::{
    GenerationAuditAck, GenerationAuditBatch, GenerationAuditEntry, GenerationAuditLine,
    GenerationAuditLoss, GenerationAuditOutcome,
};
use runtrol_provider::WallMs;
use runtrol_store::{
    IntegrationAuditOutcome, IntegrationAuditRelayBatch, IntegrationAuditRelayEntry,
    IntegrationAuditRelayLoss, IntegrationAuditRow, IntegrationKey, Store, StoreError,
};

/// How many rows a draining generation keeps for a successor that has not polled yet. The successor polls
/// every second, so a full queue means it has been gone for a while, and the oldest rows leave with a note.
const RELAY_ROW_CEILING: usize = 512;

/// The method name of the row that stands for rows lost to the ceiling.
const EVICTION_METHOD: &str = "generation/auditRelay";
const EVICTION_REASON: &str = "evictedBeforeRelay";

/// Held only across a push, snapshot, or ACK, never across an await: all callers are synchronous, so the
/// standard lock is the right one, as it is for the claim registry beside it.
pub(crate) struct AuditRelay {
    state: RwLock<Pending>,
    empty: tokio::sync::Notify,
}

struct Pending {
    epoch: [u8; 16],
    next_sequence: u64,
    acked_through: u64,
    rows: VecDeque<SequencedRow>,
    loss: Option<RelayLoss>,
}

struct SequencedRow {
    sequence: u64,
    row: IntegrationAuditRow,
}

struct RelayLoss {
    through: u64,
    row: IntegrationAuditRow,
}

impl Default for AuditRelay {
    fn default() -> Self {
        Self {
            state: RwLock::new(Pending {
                epoch: *uuid::Uuid::now_v7().as_bytes(),
                next_sequence: 1,
                acked_through: 0,
                rows: VecDeque::new(),
                loss: None,
            }),
            empty: tokio::sync::Notify::new(),
        }
    }
}

impl AuditRelay {
    /// Keep one row for the successor.
    pub(crate) fn push(&self, row: IntegrationAuditRow) {
        self.push_batch(std::iter::once(row));
    }

    /// Keep one writer batch under consecutive sequences while taking the relay lock once.
    pub(crate) fn push_batch(&self, rows: impl IntoIterator<Item = IntegrationAuditRow>) {
        let mut pending = self.state.write().unwrap_or_else(PoisonError::into_inner);
        for row in rows {
            let sequence = pending.next_sequence;
            pending.next_sequence = pending.next_sequence.saturating_add(1);
            if pending.rows.len() >= RELAY_ROW_CEILING
                && let Some(removed) = pending.rows.pop_front()
            {
                match &mut pending.loss {
                    Some(loss) => loss.through = removed.sequence,
                    None => {
                        pending.loss = Some(RelayLoss {
                            through: removed.sequence,
                            row: eviction_note(),
                        });
                    }
                }
            }
            pending.rows.push_back(SequencedRow { sequence, row });
        }
    }

    /// Apply a successor receipt only when it belongs to this exact process epoch and cannot skip unsent rows.
    pub(crate) fn acknowledge(&self, receipt: GenerationAuditAck) -> Result<(), ()> {
        if receipt.epoch == [0; 16] {
            return Ok(());
        }
        let mut pending = self.state.write().unwrap_or_else(PoisonError::into_inner);
        if receipt.epoch != pending.epoch || receipt.through >= pending.next_sequence {
            return Err(());
        }
        if receipt.through <= pending.acked_through {
            return Ok(());
        }
        pending.acked_through = receipt.through;
        while pending
            .rows
            .front()
            .is_some_and(|row| row.sequence <= receipt.through)
        {
            pending.rows.pop_front();
        }
        if pending
            .loss
            .as_ref()
            .is_some_and(|loss| loss.through <= receipt.through)
        {
            pending.loss = None;
        }
        let empty = pending.rows.is_empty() && pending.loss.is_none();
        drop(pending);
        if empty {
            self.empty.notify_waiters();
        }
        Ok(())
    }

    /// Repeat every unacknowledged row without changing ownership.
    pub(crate) fn snapshot(&self) -> GenerationAuditBatch {
        let pending = self.state.read().unwrap_or_else(PoisonError::into_inner);
        GenerationAuditBatch {
            epoch: pending.epoch,
            loss: pending.loss.as_ref().map(|loss| GenerationAuditLoss {
                through: loss.through,
                row: line(&loss.row),
            }),
            entries: pending
                .rows
                .iter()
                .map(|entry| GenerationAuditEntry {
                    sequence: entry.sequence,
                    row: line(&entry.row),
                })
                .collect(),
        }
    }

    /// Destructive compatibility projection for a successor that predates durable receipts.
    pub(crate) fn take_legacy(&self) -> Vec<GenerationAuditLine> {
        let mut pending = self.state.write().unwrap_or_else(PoisonError::into_inner);
        let mut lines =
            Vec::with_capacity(pending.rows.len() + usize::from(pending.loss.is_some()));
        if let Some(loss) = pending.loss.take() {
            pending.acked_through = pending.acked_through.max(loss.through);
            lines.push(line(&loss.row));
        }
        if let Some(last) = pending.rows.back() {
            pending.acked_through = last.sequence;
        }
        lines.extend(pending.rows.drain(..).map(|entry| line(&entry.row)));
        drop(pending);
        self.empty.notify_waiters();
        lines
    }

    /// Whether every row has a successor receipt or was handed to a legacy successor.
    pub(crate) fn is_empty(&self) -> bool {
        let pending = self.state.read().unwrap_or_else(PoisonError::into_inner);
        pending.rows.is_empty() && pending.loss.is_none()
    }

    /// Wait for the ACK that retires the last row. Cancellation is safe because the condition is rechecked.
    pub(crate) async fn wait_until_empty(&self) {
        loop {
            let notified = self.empty.notified();
            if self.is_empty() {
                return;
            }
            notified.await;
        }
    }
}

/// Append rows a draining peer relayed into this generation's store, which is the one durable store now.
///
/// A refusal is fatal to the successor service. Continuing without the durable rows would acknowledge live
/// authority state without its required audit trail.
pub(crate) fn persist(store: &Store, lines: Vec<GenerationAuditLine>) -> Result<(), StoreError> {
    let rows = lines.into_iter().map(row).collect::<Vec<_>>();
    store.append_integration_audit_batch(&rows)
}

/// Atomically persist a replayable generation batch and return the receipt sent on the next poll.
pub(crate) fn persist_batch(
    store: &Store,
    source_generation: &str,
    batch: GenerationAuditBatch,
) -> Result<GenerationAuditAck, StoreError> {
    let epoch = batch.epoch;
    let stored = IntegrationAuditRelayBatch {
        epoch,
        source_generation: source_generation.into(),
        loss: batch.loss.map(|loss| IntegrationAuditRelayLoss {
            through: loss.through,
            row: row(loss.row),
        }),
        entries: batch
            .entries
            .into_iter()
            .map(|entry| IntegrationAuditRelayEntry {
                sequence: entry.sequence,
                row: row(entry.row),
            })
            .collect(),
    };
    store
        .append_integration_audit_relay_batch(&stored)
        .map(|through| GenerationAuditAck { epoch, through })
}

fn line(row: &IntegrationAuditRow) -> GenerationAuditLine {
    GenerationAuditLine {
        occurred_at_ms: row.occurred_at.as_millis(),
        integration_key: row.integration.map(IntegrationKey::to_bytes),
        key_generation: row.key_generation,
        method: row.method.clone(),
        scope: row.scope.clone(),
        project: row.project.clone(),
        session: row.session.clone(),
        request_id: row.request_id.clone(),
        outcome: match row.outcome {
            IntegrationAuditOutcome::Attempted => GenerationAuditOutcome::Attempted,
            IntegrationAuditOutcome::Allowed => GenerationAuditOutcome::Allowed,
            IntegrationAuditOutcome::Denied => GenerationAuditOutcome::Denied,
        },
        reason: row.reason.clone(),
    }
}

fn row(line: GenerationAuditLine) -> IntegrationAuditRow {
    IntegrationAuditRow {
        occurred_at: WallMs::from_millis(line.occurred_at_ms),
        integration: line.integration_key.map(IntegrationKey::from_bytes),
        key_generation: line.key_generation,
        method: line.method,
        scope: line.scope,
        project: line.project,
        session: line.session,
        request_id: line.request_id,
        outcome: match line.outcome {
            GenerationAuditOutcome::Attempted => IntegrationAuditOutcome::Attempted,
            GenerationAuditOutcome::Allowed => IntegrationAuditOutcome::Allowed,
            GenerationAuditOutcome::Denied => IntegrationAuditOutcome::Denied,
        },
        reason: line.reason,
    }
}

/// The row that stands in for rows the ceiling pushed out: denied durability, at the moment it was noticed.
fn eviction_note() -> IntegrationAuditRow {
    IntegrationAuditRow {
        occurred_at: WallMs::now(),
        integration: None,
        key_generation: None,
        method: EVICTION_METHOD.into(),
        scope: None,
        project: None,
        session: None,
        request_id: None,
        outcome: IntegrationAuditOutcome::Denied,
        reason: EVICTION_REASON.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a_row(method: &str, at: u64) -> IntegrationAuditRow {
        IntegrationAuditRow {
            occurred_at: WallMs::from_millis(at),
            integration: Some(IntegrationKey::from_bytes([7; 16])),
            key_generation: Some(3),
            method: method.into(),
            scope: Some("session.list".into()),
            project: None,
            session: Some("sess_1".into()),
            request_id: None,
            outcome: IntegrationAuditOutcome::Allowed,
            reason: "allowed".into(),
        }
    }

    #[test]
    fn rows_repeat_oldest_first_until_their_epoch_receipt() {
        let relay = AuditRelay::default();
        relay.push(a_row("terminals/attach", 1));
        relay.push(a_row("terminals/write", 2));

        let first = relay.snapshot();
        assert_eq!(
            first
                .entries
                .iter()
                .map(|entry| &*entry.row.method)
                .collect::<Vec<_>>(),
            ["terminals/attach", "terminals/write"]
        );
        assert_eq!(relay.snapshot(), first, "a missing ACK repeats exactly");
        relay
            .acknowledge(GenerationAuditAck {
                epoch: first.epoch,
                through: 2,
            })
            .expect("matching durable receipt");
        assert!(relay.is_empty());
        assert!(relay.snapshot().entries.is_empty());
    }

    #[test]
    fn a_row_survives_the_wire_shape_exactly() {
        let original = a_row("terminals/attach", 42);
        assert_eq!(row(line(&original)), original);
    }

    #[test]
    fn relayed_rows_land_in_the_successor_store_as_its_own() {
        let root = std::env::temp_dir().join(format!(
            "runtrol-audit-relay-persist-{}-{}",
            std::process::id(),
            uuid::Uuid::now_v7()
        ));
        if root.exists() {
            std::fs::remove_dir_all(&root).expect("clear the previous run");
        }
        std::fs::create_dir_all(&root).expect("a scratch home");
        let file = root.join("runtrol.redb");
        let path =
            runtrol_provider::AbsPath::new(file.to_str().expect("the scratch path is UTF-8"))
                .expect("an absolute scratch path");
        let store = Store::open(&path).expect("a fresh store opens");

        persist(&store, vec![line(&a_row("terminals/attach", 5))]).expect("appended");

        let rows = store.list_integration_audit().expect("listed");
        assert!(
            rows.contains(&a_row("terminals/attach", 5)),
            "the relayed row is a row of this store"
        );
        // The store owns an exclusive file handle. Release it before removing the scratch home.
        drop(store);
        std::fs::remove_dir_all(&root).expect("remove the scratch home");
    }

    #[test]
    fn a_replayed_generation_batch_commits_once_and_only_its_receipt_removes_rows() {
        let root = std::env::temp_dir().join(format!(
            "runtrol-audit-relay-replay-{}-{}",
            std::process::id(),
            uuid::Uuid::now_v7()
        ));
        std::fs::create_dir_all(&root).expect("a scratch home");
        let file = root.join("runtrol.redb");
        let path =
            runtrol_provider::AbsPath::new(file.to_str().expect("the scratch path is UTF-8"))
                .expect("an absolute scratch path");
        let store = Store::open(&path).expect("a fresh store opens");
        let relay = AuditRelay::default();
        relay.push_batch([a_row("terminals/attach", 5), a_row("terminals/stop", 6)]);
        let snapshot = relay.snapshot();

        let first =
            persist_batch(&store, "generation-a", snapshot.clone()).expect("first durable receipt");
        let repeated = persist_batch(&store, "generation-a", snapshot.clone())
            .expect("replayed durable receipt");
        assert_eq!(first, repeated);
        assert_eq!(first.through, 2);
        assert_eq!(
            store.list_integration_audit().expect("listed").len(),
            2,
            "the replay did not duplicate rows"
        );
        assert_eq!(
            relay.snapshot(),
            snapshot,
            "persistence alone is not an ACK"
        );
        relay.acknowledge(first).expect("matching receipt ACK");
        assert!(relay.is_empty());

        drop(store);
        std::fs::remove_dir_all(&root).expect("remove the scratch home");
    }

    #[test]
    fn the_ceiling_drops_the_oldest_and_says_so_once() {
        let relay = AuditRelay::default();
        for at in 0..=RELAY_ROW_CEILING as u64 {
            relay.push(a_row("terminals/write", at));
        }

        let taken = relay.snapshot();
        assert_eq!(
            taken.entries.len(),
            RELAY_ROW_CEILING,
            "the row ceiling stays exact"
        );
        let loss = taken.loss.as_ref().expect("bounded loss marker");
        assert_eq!(loss.through, 1);
        assert_eq!(&*loss.row.method, EVICTION_METHOD);
        assert_eq!(&*loss.row.reason, EVICTION_REASON);
        assert_eq!(loss.row.outcome, GenerationAuditOutcome::Denied);
        let oldest_kept = taken.entries.first().expect("the oldest surviving row");
        assert_eq!(
            oldest_kept.row.occurred_at_ms, 1,
            "row zero was the one evicted"
        );
        assert_eq!(relay.snapshot(), taken, "loss and rows remain stable");
        relay
            .acknowledge(GenerationAuditAck {
                epoch: taken.epoch,
                through: u64::try_from(RELAY_ROW_CEILING).expect("ceiling fits") + 1,
            })
            .expect("receipt covers marker and rows");
        assert!(relay.is_empty());
    }

    #[test]
    fn an_ack_from_another_process_epoch_cannot_remove_rows() {
        let relay = AuditRelay::default();
        relay.push(a_row("terminals/attach", 1));
        let batch = relay.snapshot();
        assert!(
            relay
                .acknowledge(GenerationAuditAck {
                    epoch: [0xAA; 16],
                    through: 1,
                })
                .is_err()
        );
        assert_eq!(relay.snapshot(), batch);
    }
}
