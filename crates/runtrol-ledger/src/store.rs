//! Separate redb ownership, bounded queries, compaction, and recovery snapshots.

use redb::{
    Database, DatabaseError, Durability, ReadableDatabase as _, ReadableTable as _, TableDefinition,
};
use runtrol_provider::AbsPath;
use serde::{Deserialize, Serialize};

use crate::{
    ArtifactRecord, GateRunRecord, MAX_MISSIONS, MAX_QUERY_MISSIONS, MissionId, MissionRecord,
    Receipt, ReceiptId, RunRecord, TaskRecord,
};

const SCHEMA_VERSION: u8 = 1;
const META: TableDefinition<'static, &str, &[u8]> = TableDefinition::new("mission_meta");
const META_SCHEMA: &str = "schema";
const MISSIONS: TableDefinition<'static, &str, &[u8]> = TableDefinition::new("missions");

/// Complete bounded recovery state for one Mission.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct LedgerSnapshot {
    /// Mission state.
    pub mission: MissionRecord,
    /// Task states.
    pub tasks: Vec<TaskRecord>,
    /// Run identities and outcomes.
    pub runs: Vec<RunRecord>,
    /// Deterministic Gate metadata.
    pub gate_runs: Vec<GateRunRecord>,
    /// Artifact manifest metadata.
    pub artifacts: Vec<ArtifactRecord>,
    /// Content-addressed Receipts.
    pub receipts: Vec<(ReceiptId, Receipt)>,
    /// Whether older terminal detail was explicitly compacted.
    pub compacted: bool,
}

impl LedgerSnapshot {
    /// Compact transition detail only after the Mission is terminal.
    pub fn compact(&mut self) {
        if !self.mission.state.is_terminal() {
            return;
        }
        self.mission.compact();
        for task in &mut self.tasks {
            task.compact();
        }
        self.compacted = true;
    }
}

/// One bounded Mission listing.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ListedMissions {
    /// At most [`MAX_QUERY_MISSIONS`] snapshots.
    pub missions: Vec<LedgerSnapshot>,
    /// More rows exist and require another explicit query.
    pub truncated: bool,
}

/// Exclusive owner of the separate Mission ledger file.
#[derive(Debug)]
pub struct Ledger {
    db: Database,
    path: AbsPath,
}

impl Ledger {
    /// Open or create the separate ledger and acquire its exclusive schema lock.
    ///
    /// # Errors
    /// Returns [`LedgerError`] for an existing owner, I/O damage, or unsupported schema.
    pub fn open(path: &AbsPath) -> Result<Self, LedgerError> {
        let mut builder = Database::builder();
        builder.set_cache_size(crate::CACHE_BYTES);
        let db = builder
            .create(path.as_std_path())
            .map_err(|error| match error {
                DatabaseError::DatabaseAlreadyOpen => LedgerError::AlreadyOpen,
                other => LedgerError::Engine(other.to_string()),
            })?;
        let ledger = Self {
            db,
            path: path.clone(),
        };
        ledger.check_schema()?;
        Ok(ledger)
    }

    /// Exact file owned by this ledger.
    #[must_use]
    pub const fn path(&self) -> &AbsPath {
        &self.path
    }

    /// Durably replace one complete bounded Mission snapshot.
    ///
    /// # Errors
    /// Returns [`LedgerError`] for quota, encoding, or database failures.
    pub fn put(&self, snapshot: &LedgerSnapshot) -> Result<(), LedgerError> {
        Self::validate_bounds(snapshot)?;
        let encoded =
            serde_json::to_vec(snapshot).map_err(|error| LedgerError::Codec(error.to_string()))?;
        let key = snapshot.mission.id.to_string();
        let mut write = self.db.begin_write().map_err(engine)?;
        write
            .set_durability(Durability::Immediate)
            .map_err(engine)?;
        {
            let mut table = write.open_table(MISSIONS).map_err(engine)?;
            table
                .insert(key.as_str(), encoded.as_slice())
                .map_err(engine)?;
        }
        write.commit().map_err(engine)
    }

    /// Read one Mission recovery snapshot.
    ///
    /// # Errors
    /// Returns [`LedgerError`] for database damage or malformed stored data.
    pub fn snapshot(&self, mission_id: MissionId) -> Result<Option<LedgerSnapshot>, LedgerError> {
        let read = self.db.begin_read().map_err(engine)?;
        let table = match read.open_table(MISSIONS) {
            Ok(table) => table,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(None),
            Err(error) => return Err(engine(error)),
        };
        let key = mission_id.to_string();
        let value = table.get(key.as_str()).map_err(engine)?;
        value
            .map(|stored| {
                serde_json::from_slice(stored.value())
                    .map_err(|error| LedgerError::Codec(error.to_string()))
            })
            .transpose()
    }

    /// List a bounded page of Mission snapshots in key order.
    ///
    /// # Errors
    /// Returns [`LedgerError`] for database damage or malformed stored data.
    pub fn list(&self, limit: usize) -> Result<ListedMissions, LedgerError> {
        let wanted = limit.clamp(1, MAX_QUERY_MISSIONS);
        let read = self.db.begin_read().map_err(engine)?;
        let table = match read.open_table(MISSIONS) {
            Ok(table) => table,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(ListedMissions::default()),
            Err(error) => return Err(engine(error)),
        };
        let mut listed = ListedMissions::default();
        for entry in table.iter().map_err(engine)? {
            let (_, value) = entry.map_err(engine)?;
            if listed.missions.len() == wanted {
                listed.truncated = true;
                break;
            }
            listed.missions.push(
                serde_json::from_slice(value.value())
                    .map_err(|error| LedgerError::Codec(error.to_string()))?,
            );
        }
        Ok(listed)
    }

    /// Event-driven terminal compaction for one Mission.
    ///
    /// # Errors
    /// Returns [`LedgerError`] for missing rows or database failures.
    pub fn compact(&self, mission_id: MissionId) -> Result<(), LedgerError> {
        let mut snapshot = self
            .snapshot(mission_id)?
            .ok_or(LedgerError::MissingMission)?;
        snapshot.compact();
        self.put(&snapshot)
    }

    fn validate_bounds(snapshot: &LedgerSnapshot) -> Result<(), LedgerError> {
        if snapshot.tasks.len() > crate::MAX_TASKS_PER_MISSION {
            return Err(LedgerError::Quota("tasks"));
        }
        if snapshot.mission.transitions.len() > crate::MAX_TRANSITIONS_PER_MISSION {
            return Err(LedgerError::Quota("transitions"));
        }
        Ok(())
    }

    fn check_schema(&self) -> Result<(), LedgerError> {
        let read = self.db.begin_read().map_err(engine)?;
        match read.open_table(META) {
            Ok(table) => {
                let version = table
                    .get(META_SCHEMA)
                    .map_err(engine)?
                    .and_then(|value| value.value().first().copied());
                match version {
                    Some(SCHEMA_VERSION) => return Ok(()),
                    Some(_) => return Err(LedgerError::UnsupportedSchema),
                    None => {}
                }
            }
            Err(redb::TableError::TableDoesNotExist(_)) => {}
            Err(error) => return Err(engine(error)),
        }
        drop(read);
        let mut write = self.db.begin_write().map_err(engine)?;
        write
            .set_durability(Durability::Immediate)
            .map_err(engine)?;
        {
            let mut table = write.open_table(META).map_err(engine)?;
            table
                .insert(META_SCHEMA, [SCHEMA_VERSION].as_slice())
                .map_err(engine)?;
        }
        write.commit().map_err(engine)
    }
}

fn engine(error: impl core::fmt::Display) -> LedgerError {
    LedgerError::Engine(error.to_string())
}

/// Ledger ownership, codec, schema, or quota failure.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum LedgerError {
    /// Another process owns the file lock.
    #[error("mission ledger is already open")]
    AlreadyOpen,
    /// Database engine failure.
    #[error("mission ledger engine failed: {0}")]
    Engine(String),
    /// Stored or supplied closed data was malformed.
    #[error("mission ledger codec failed: {0}")]
    Codec(String),
    /// This build cannot read the schema generation.
    #[error("mission ledger schema is unsupported")]
    UnsupportedSchema,
    /// A hard record quota was exceeded.
    #[error("mission ledger quota exceeded: {0}")]
    Quota(&'static str),
    /// The requested Mission does not exist.
    #[error("mission does not exist")]
    MissingMission,
}

const _: usize = MAX_MISSIONS;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MissionRecord;

    struct Scratch {
        root: AbsPath,
    }
    impl Scratch {
        fn make(name: &str) -> Self {
            let path = std::env::temp_dir()
                .join(format!("runtrol-ledger-{name}-{}", uuid::Uuid::now_v7()));
            std::fs::create_dir_all(&path).expect("scratch");
            Self {
                root: AbsPath::canonicalize(path.to_str().expect("UTF-8")).expect("canonical"),
            }
        }
        fn database(&self) -> AbsPath {
            self.root.join("mission.redb").expect("join")
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ignored = std::fs::remove_dir_all(self.root.as_std_path());
        }
    }

    #[test]
    fn restart_recovers_exact_metadata_snapshot() {
        let scratch = Scratch::make("restart");
        let path = scratch.database();
        let mission = MissionRecord::draft([7; 32], "project".into());
        let id = mission.id;
        let snapshot = LedgerSnapshot {
            mission,
            tasks: Vec::new(),
            runs: Vec::new(),
            gate_runs: Vec::new(),
            artifacts: Vec::new(),
            receipts: Vec::new(),
            compacted: false,
        };
        {
            let ledger = Ledger::open(&path).expect("open");
            ledger.put(&snapshot).expect("put");
        }
        let recovered = Ledger::open(&path)
            .expect("reopen")
            .snapshot(id)
            .expect("read")
            .expect("exists");
        assert_eq!(recovered, snapshot);
    }

    #[test]
    fn second_owner_is_refused() {
        let scratch = Scratch::make("lock");
        let path = scratch.database();
        let _first = Ledger::open(&path).expect("first");
        assert_eq!(
            Ledger::open(&path).expect_err("second must fail"),
            LedgerError::AlreadyOpen
        );
    }
}
