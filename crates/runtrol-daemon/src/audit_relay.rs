//! Authorization records of a draining generation, kept for the successor that owns the store.
//!
//! A draining generation has handed its durable store to the successor, and every public request still
//! records two authorization rows. Refusing to serve because those rows had nowhere to go retired the whole
//! public surface of every draining generation the moment the store moved: measured 2026-08-29 on the
//! operator machine, five draining generations refused every attach with "audit storage is unavailable" and
//! the eight conversations they hosted could be neither opened nor stopped. The rows are kept here instead,
//! bounded, and the successor takes them on its next handoff poll and appends them to the one store.
//! Durability is deferred by at most that poll. An eviction, should the successor never come, is itself a row,
//! so the gap shows in the audit rather than passing in silence.

use std::collections::VecDeque;
use std::sync::{PoisonError, RwLock};

use runtrol_ipc::wire::{GenerationAuditLine, GenerationAuditOutcome};
use runtrol_provider::WallMs;
use runtrol_store::{
    IntegrationAuditOutcome, IntegrationAuditRow, IntegrationKey, Store, StoreError,
};

/// How many rows a draining generation keeps for a successor that has not polled yet. The successor polls
/// every second, so a full queue means it has been gone for a while, and the oldest rows leave with a note.
const RELAY_ROW_CEILING: usize = 512;

/// The method name of the row that stands for rows lost to the ceiling.
const EVICTION_METHOD: &str = "generation/auditRelay";
const EVICTION_REASON: &str = "evictedBeforeRelay";

/// Held only across a push or a take, never across an await: both callers are synchronous, so the
/// standard lock is the right one, as it is for the claim registry beside it.
#[derive(Default)]
pub(crate) struct AuditRelay {
    state: RwLock<Pending>,
}

#[derive(Default)]
struct Pending {
    rows: VecDeque<IntegrationAuditRow>,
    evicted: bool,
}

impl AuditRelay {
    /// Keep one row for the successor.
    pub(crate) fn push(&self, row: IntegrationAuditRow) {
        let mut pending = self.state.write().unwrap_or_else(PoisonError::into_inner);
        if pending.rows.len() >= RELAY_ROW_CEILING {
            pending.rows.pop_front();
            pending.evicted = true;
        }
        pending.rows.push_back(row);
    }

    /// Everything kept since the last take, oldest first, led by an eviction note when rows were lost.
    pub(crate) fn take(&self) -> Vec<GenerationAuditLine> {
        let mut pending = self.state.write().unwrap_or_else(PoisonError::into_inner);
        let mut lines = Vec::with_capacity(pending.rows.len() + 1);
        if pending.evicted {
            pending.evicted = false;
            lines.push(line(&eviction_note()));
        }
        lines.extend(pending.rows.drain(..).map(|row| line(&row)));
        lines
    }
}

/// Append rows a draining peer relayed into this generation's store, which is the one durable store now.
///
/// A row this store cannot hold is skipped rather than fatal: the rows came from the same binary family and
/// passed the same bounds once, so a refusal here is the store itself failing, and this generation's own next
/// audit row will refuse its own request for that. Stopping the poll over it would strand the peer's grants.
pub(crate) fn persist(store: &Store, lines: Vec<GenerationAuditLine>) -> Result<(), StoreError> {
    for relayed in lines {
        store.append_integration_audit(&row(relayed))?;
    }
    Ok(())
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
    fn rows_come_back_oldest_first_and_only_once() {
        let relay = AuditRelay::default();
        relay.push(a_row("terminals/attach", 1));
        relay.push(a_row("terminals/write", 2));

        let taken = relay.take();
        assert_eq!(
            taken.iter().map(|line| &*line.method).collect::<Vec<_>>(),
            ["terminals/attach", "terminals/write"]
        );
        assert!(relay.take().is_empty(), "a second take has nothing left");
    }

    #[test]
    fn a_row_survives_the_wire_shape_exactly() {
        let original = a_row("terminals/attach", 42);
        assert_eq!(row(line(&original)), original);
    }

    #[test]
    fn relayed_rows_land_in_the_successor_store_as_its_own() {
        let root = std::env::temp_dir().join("runtrol-audit-relay-persist");
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
    fn the_ceiling_drops_the_oldest_and_says_so_once() {
        let relay = AuditRelay::default();
        for at in 0..=RELAY_ROW_CEILING as u64 {
            relay.push(a_row("terminals/write", at));
        }

        let taken = relay.take();
        assert_eq!(
            taken.len(),
            RELAY_ROW_CEILING + 1,
            "the ceiling of rows plus one note"
        );
        let note = taken.first().expect("the note leads");
        assert_eq!(&*note.method, EVICTION_METHOD);
        assert_eq!(&*note.reason, EVICTION_REASON);
        assert_eq!(note.outcome, GenerationAuditOutcome::Denied);
        let oldest_kept = taken.get(1).expect("the oldest surviving row");
        assert_eq!(
            oldest_kept.occurred_at_ms, 1,
            "row zero was the one evicted"
        );
        assert!(relay.take().is_empty(), "the note is not repeated");
    }
}
