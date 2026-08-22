//! Durable bounded Mission Flight Signals.
//!
//! A row is a structural wake destination produced at the local machine. It contains an opaque deduplication token,
//! one Mission identity and reviewed digest, one closed kind, and an optional Runtime session identity. It has no
//! prompt, output, path, provider event, push endpoint, notification body, or arbitrary payload field.

use std::ops::Bound;

use redb::{ReadableDatabase as _, ReadableTable as _, ReadableTableMetadata as _};
use runtrol_provider::SessionId;

use crate::error::StoreError;
use crate::open::Store;
use crate::schema::{MISSION_SIGNALS, MissionSignalKey, SCHEMA_VERSION};

const MAX_ID_BYTES: usize = 160;
const MAX_SESSION_ID_BYTES: usize = 64;
const MAX_ROW_BYTES: usize = 512;

/// Maximum retained structural wake rows across the machine.
pub const MISSION_SIGNAL_MAX_ROWS: usize = 64;

/// Closed Mission Flight Signal reason.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MissionSignalKind {
    /// A Mission-owned Runtime session is waiting for a person.
    Person,
    /// Local Auto Flight stopped before Receipt Landing.
    Stopped,
    /// Local Auto Flight reached explicit Receipt Landing.
    Landing,
}

/// One structural Mission wake destination.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MissionSignalRow {
    /// Producer-minted idempotency identity. It is structural randomness, not caller content.
    pub dedupe: [u8; 16],
    /// Exact Mission identity.
    pub mission_id: Box<str>,
    /// Exact reviewed Mission digest.
    pub mission_sha256: [u8; 32],
    /// Closed reason vocabulary.
    pub kind: MissionSignalKind,
    /// Exact Runtime session for a person wait, absent for Mission-level destinations.
    pub session_id: Option<Box<str>>,
}

/// Result of an idempotent append.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppendMissionSignal {
    /// A new row was committed and warrants one generic push wake.
    Inserted(MissionSignalKey),
    /// The exact producer transition was already committed.
    Duplicate(MissionSignalKey),
}

/// One bounded cursor page, oldest first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListedMissionSignals {
    /// Structural rows after the requested cursor.
    pub signals: Vec<(MissionSignalKey, MissionSignalRow)>,
    /// Current global tail, including rows later hidden by caller authority.
    pub latest: Option<MissionSignalKey>,
    /// The supplied cursor fell out of the retained bound or was otherwise absent below the tail.
    pub gap: bool,
}

impl Store {
    /// Append one idempotent signal and evict oldest rows at the fixed ceiling.
    ///
    /// # Errors
    ///
    /// A reused dedupe identity with different structural fields, malformed bounds, or storage failure.
    pub fn append_mission_signal(
        &self,
        row: &MissionSignalRow,
    ) -> Result<AppendMissionSignal, StoreError> {
        let encoded = encode(row)?;
        let write = self.begin_durable_write("saving a Mission Flight Signal")?;
        let outcome = {
            let mut table = write
                .open_table(MISSION_SIGNALS)
                .map_err(|error| engine("opening Mission Flight Signal metadata", error))?;
            let existing = table
                .range(MissionSignalKey::FIRST..=MissionSignalKey::LAST)
                .map_err(|error| engine("scanning Mission Flight Signal deduplication", error))?
                .find_map(|entry| match entry {
                    Ok((key, value)) => match decode(value.value()) {
                        Ok(candidate) if candidate.dedupe == row.dedupe => {
                            Some(Ok((key.value(), candidate)))
                        }
                        Ok(_) => None,
                        Err(error) => Some(Err(error)),
                    },
                    Err(error) => Some(Err(engine(
                        "reading Mission Flight Signal deduplication",
                        error,
                    ))),
                })
                .transpose()?;
            if let Some((key, existing)) = existing {
                if existing != *row {
                    return Err(codec(
                        "dedupe identity",
                        "it was reused for a different structural transition",
                    ));
                }
                AppendMissionSignal::Duplicate(key)
            } else {
                let key = fresh_key(&table)?;
                table
                    .insert(key, encoded.as_slice())
                    .map_err(|error| engine("appending Mission Flight Signal metadata", error))?;
                while table
                    .len()
                    .map_err(|error| engine("measuring Mission Flight Signal retention", error))?
                    > MISSION_SIGNAL_MAX_ROWS as u64
                {
                    let oldest = table
                        .range(MissionSignalKey::FIRST..=MissionSignalKey::LAST)
                        .map_err(|error| engine("scanning Mission Flight Signal retention", error))?
                        .next()
                        .transpose()
                        .map_err(|error| engine("reading Mission Flight Signal retention", error))?
                        .map(|(candidate, _)| candidate.value())
                        .ok_or_else(|| codec("retention", "the table length was inconsistent"))?;
                    drop(table.remove(oldest).map_err(|error| {
                        engine("evicting old Mission Flight Signal metadata", error)
                    })?);
                }
                AppendMissionSignal::Inserted(key)
            }
        };
        write
            .commit()
            .map_err(|error| engine("committing Mission Flight Signal metadata", error))?;
        Ok(outcome)
    }

    /// List a bounded cursor page without applying caller authority.
    ///
    /// # Errors
    ///
    /// Malformed retained rows or storage failure. The daemon applies current Mission and root authority afterward.
    pub fn list_mission_signals(
        &self,
        after: Option<MissionSignalKey>,
    ) -> Result<ListedMissionSignals, StoreError> {
        let read = self
            .db()
            .begin_read()
            .map_err(|error| engine("starting a Mission Flight Signal read", error))?;
        let table = match read.open_table(MISSION_SIGNALS) {
            Ok(table) => table,
            Err(redb::TableError::TableDoesNotExist(_)) => {
                return Ok(ListedMissionSignals {
                    signals: Vec::new(),
                    latest: None,
                    gap: false,
                });
            }
            Err(error) => return Err(engine("opening Mission Flight Signal metadata", error)),
        };
        let latest = table
            .range(MissionSignalKey::FIRST..=MissionSignalKey::LAST)
            .map_err(|error| engine("scanning the Mission Flight Signal tail", error))?
            .next_back()
            .transpose()
            .map_err(|error| engine("reading the Mission Flight Signal tail", error))?
            .map(|(key, _)| key.value());
        let gap = match (after, latest) {
            (Some(cursor), Some(tail)) if cursor <= tail => table
                .get(cursor)
                .map_err(|error| engine("checking a Mission Flight Signal cursor", error))?
                .is_none(),
            _ => false,
        };
        let bounds = after.map_or(
            (
                Bound::Included(MissionSignalKey::FIRST),
                Bound::Included(MissionSignalKey::LAST),
            ),
            |cursor| {
                (
                    Bound::Excluded(cursor),
                    Bound::Included(MissionSignalKey::LAST),
                )
            },
        );
        let signals = table
            .range(bounds)
            .map_err(|error| engine("scanning Mission Flight Signal metadata", error))?
            .map(|entry| {
                let (key, value) = entry
                    .map_err(|error| engine("reading Mission Flight Signal metadata", error))?;
                Ok((key.value(), decode(value.value())?))
            })
            .collect::<Result<Vec<_>, StoreError>>()?;
        Ok(ListedMissionSignals {
            signals,
            latest,
            gap,
        })
    }

    /// Remove retained signals for one exact Mission digest when a fresh local arm supersedes them.
    ///
    /// # Errors
    ///
    /// Malformed retained rows or storage failure.
    pub fn clear_mission_signals(
        &self,
        mission_id: &str,
        mission_sha256: &[u8; 32],
    ) -> Result<usize, StoreError> {
        let write = self.begin_durable_write("clearing Mission Flight Signals")?;
        let removed = {
            let mut table = write
                .open_table(MISSION_SIGNALS)
                .map_err(|error| engine("opening Mission Flight Signal metadata", error))?;
            let keys = table
                .range(MissionSignalKey::FIRST..=MissionSignalKey::LAST)
                .map_err(|error| engine("scanning Mission Flight Signal cleanup", error))?
                .filter_map(|entry| match entry {
                    Ok((key, value)) => match decode(value.value()) {
                        Ok(row)
                            if row.mission_id.as_ref() == mission_id
                                && row.mission_sha256 == *mission_sha256 =>
                        {
                            Some(Ok(key.value()))
                        }
                        Ok(_) => None,
                        Err(error) => Some(Err(error)),
                    },
                    Err(error) => Some(Err(engine("reading Mission Flight Signal cleanup", error))),
                })
                .collect::<Result<Vec<_>, StoreError>>()?;
            for key in &keys {
                drop(
                    table.remove(*key).map_err(|error| {
                        engine("removing Mission Flight Signal metadata", error)
                    })?,
                );
            }
            keys.len()
        };
        write
            .commit()
            .map_err(|error| engine("committing Mission Flight Signal cleanup", error))?;
        Ok(removed)
    }
}

fn fresh_key(
    table: &redb::Table<'_, MissionSignalKey, &[u8]>,
) -> Result<MissionSignalKey, StoreError> {
    for _ in 0..16 {
        let key = MissionSignalKey::from_bytes(*SessionId::now().as_bytes());
        if table
            .get(key)
            .map_err(|error| engine("checking a Mission Flight Signal identity", error))?
            .is_none()
        {
            return Ok(key);
        }
    }
    Err(codec(
        "signal identity",
        "a unique bounded identity could not be minted",
    ))
}

fn encode(row: &MissionSignalRow) -> Result<Vec<u8>, StoreError> {
    if matches!(row.kind, MissionSignalKind::Person) != row.session_id.is_some() {
        return Err(codec(
            "session identity",
            "it must exist only for a person signal",
        ));
    }
    let mut out = Vec::with_capacity(256);
    out.push(SCHEMA_VERSION);
    out.extend_from_slice(&row.dedupe);
    write_text(&mut out, "Mission identity", &row.mission_id, MAX_ID_BYTES)?;
    out.extend_from_slice(&row.mission_sha256);
    out.push(match row.kind {
        MissionSignalKind::Person => 0,
        MissionSignalKind::Stopped => 1,
        MissionSignalKind::Landing => 2,
    });
    write_optional_text(
        &mut out,
        "session identity",
        row.session_id.as_deref(),
        MAX_SESSION_ID_BYTES,
    )?;
    if out.len() > MAX_ROW_BYTES {
        return Err(codec("row", "it exceeds the frozen byte ceiling"));
    }
    Ok(out)
}

fn decode(bytes: &[u8]) -> Result<MissionSignalRow, StoreError> {
    if bytes.len() > MAX_ROW_BYTES {
        return Err(codec("row", "it exceeds the frozen byte ceiling"));
    }
    let mut cursor = Cursor { bytes, at: 0 };
    if cursor.byte("row version")? != SCHEMA_VERSION {
        return Err(codec("row version", "it belongs to another schema"));
    }
    let dedupe = cursor.fixed("dedupe identity")?;
    let mission_id = cursor.text("Mission identity", MAX_ID_BYTES)?.into();
    let mission_sha256 = cursor.fixed("Mission digest")?;
    let kind = match cursor.byte("signal kind")? {
        0 => MissionSignalKind::Person,
        1 => MissionSignalKind::Stopped,
        2 => MissionSignalKind::Landing,
        _ => return Err(codec("signal kind", "it is not recognized")),
    };
    let session_id = cursor.optional_text("session identity", MAX_SESSION_ID_BYTES)?;
    if matches!(kind, MissionSignalKind::Person) != session_id.is_some() {
        return Err(codec(
            "session identity",
            "it must exist only for a person signal",
        ));
    }
    if cursor.at != bytes.len() {
        return Err(codec("end of row", "trailing bytes remain"));
    }
    Ok(MissionSignalRow {
        dedupe,
        mission_id,
        mission_sha256,
        kind,
        session_id,
    })
}

fn write_text(
    out: &mut Vec<u8>,
    field: &'static str,
    value: &str,
    max: usize,
) -> Result<(), StoreError> {
    if value.is_empty() || value.len() > max {
        return Err(codec(field, "it is empty or exceeds its byte limit"));
    }
    let length =
        u16::try_from(value.len()).map_err(|_| codec(field, "its length cannot be represented"))?;
    out.extend_from_slice(&length.to_le_bytes());
    out.extend_from_slice(value.as_bytes());
    Ok(())
}

fn write_optional_text(
    out: &mut Vec<u8>,
    field: &'static str,
    value: Option<&str>,
    max: usize,
) -> Result<(), StoreError> {
    if let Some(value) = value {
        out.push(1);
        write_text(out, field, value, max)
    } else {
        out.push(0);
        Ok(())
    }
}

struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, field: &'static str, count: usize) -> Result<&'a [u8], StoreError> {
        let end = self
            .at
            .checked_add(count)
            .ok_or_else(|| codec(field, "its length overflows the row"))?;
        let value = self
            .bytes
            .get(self.at..end)
            .ok_or_else(|| codec(field, "the row ended early"))?;
        self.at = end;
        Ok(value)
    }

    fn byte(&mut self, field: &'static str) -> Result<u8, StoreError> {
        self.take(field, 1)?
            .first()
            .copied()
            .ok_or_else(|| codec(field, "the row ended early"))
    }

    fn u16(&mut self, field: &'static str) -> Result<u16, StoreError> {
        Ok(u16::from_le_bytes(
            self.take(field, 2)?
                .try_into()
                .map_err(|_| codec(field, "the field has the wrong width"))?,
        ))
    }

    fn fixed<const N: usize>(&mut self, field: &'static str) -> Result<[u8; N], StoreError> {
        self.take(field, N)?
            .try_into()
            .map_err(|_| codec(field, "the field has the wrong width"))
    }

    fn text(&mut self, field: &'static str, max: usize) -> Result<&'a str, StoreError> {
        let length = usize::from(self.u16(field)?);
        if length == 0 || length > max {
            return Err(codec(field, "it is empty or exceeds its byte limit"));
        }
        std::str::from_utf8(self.take(field, length)?).map_err(|_| codec(field, "it is not UTF-8"))
    }

    fn optional_text(
        &mut self,
        field: &'static str,
        max: usize,
    ) -> Result<Option<Box<str>>, StoreError> {
        match self.byte(field)? {
            0 => Ok(None),
            1 => self.text(field, max).map(|value| Some(value.into())),
            _ => Err(codec(field, "its presence marker is not recognized")),
        }
    }
}

fn codec(field: &'static str, why: &'static str) -> StoreError {
    StoreError::MissionSignalCodec { field, why }
}

fn engine(doing: &'static str, error: impl Into<redb::Error>) -> StoreError {
    StoreError::Engine {
        doing,
        source: Box::new(error.into()),
    }
}

#[cfg(test)]
mod tests {
    use runtrol_provider::AbsPath;

    use super::*;

    struct Scratch {
        root: AbsPath,
        store: Store,
    }

    impl Scratch {
        fn make() -> Self {
            let base =
                std::env::temp_dir().join(format!("runtrol-mission-signals-{}", SessionId::now()));
            std::fs::create_dir_all(&base).expect("create scratch");
            let root = AbsPath::canonicalize(base.to_str().expect("UTF-8 scratch")).expect("root");
            let store =
                Store::open(&root.join("state.redb").expect("database path")).expect("open store");
            Self { root, store }
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            if let Err(error) = std::fs::remove_dir_all(self.root.as_std_path()) {
                eprintln!("could not clean Mission Flight Signal scratch: {error}");
            }
        }
    }

    fn row(index: u8, kind: MissionSignalKind) -> MissionSignalRow {
        MissionSignalRow {
            dedupe: [index; 16],
            mission_id: format!("msn_{index}").into(),
            mission_sha256: [index; 32],
            kind,
            session_id: matches!(kind, MissionSignalKind::Person)
                .then(|| format!("session-{index}").into()),
        }
    }

    #[test]
    fn signals_are_idempotent_bounded_and_cursor_addressable() {
        let scratch = Scratch::make();
        let first = row(1, MissionSignalKind::Landing);
        let inserted = scratch.store.append_mission_signal(&first).expect("append");
        let first_key = match inserted {
            AppendMissionSignal::Inserted(key) => key,
            AppendMissionSignal::Duplicate(_) => panic!("first insert was duplicate"),
        };
        assert_eq!(
            scratch.store.append_mission_signal(&first).expect("repeat"),
            AppendMissionSignal::Duplicate(first_key)
        );
        for index in 2..=u8::try_from(MISSION_SIGNAL_MAX_ROWS + 1).expect("bounded fixture") {
            scratch
                .store
                .append_mission_signal(&row(index, MissionSignalKind::Stopped))
                .expect("append bounded signal");
        }
        let listed = scratch.store.list_mission_signals(None).expect("list");
        assert_eq!(listed.signals.len(), MISSION_SIGNAL_MAX_ROWS);
        assert!(
            listed
                .signals
                .iter()
                .all(|(_, candidate)| candidate != &first)
        );
        let after_old = scratch
            .store
            .list_mission_signals(Some(first_key))
            .expect("old cursor");
        assert!(after_old.gap);
        assert_eq!(after_old.signals.len(), MISSION_SIGNAL_MAX_ROWS);
        let tail = listed.latest.expect("tail");
        let after_tail = scratch
            .store
            .list_mission_signals(Some(tail))
            .expect("tail cursor");
        assert!(!after_tail.gap);
        assert!(after_tail.signals.is_empty());
    }

    #[test]
    fn dedupe_conflicts_and_kind_session_mismatches_fail_closed() {
        let scratch = Scratch::make();
        let original = row(7, MissionSignalKind::Stopped);
        scratch
            .store
            .append_mission_signal(&original)
            .expect("append");
        let conflicting = MissionSignalRow {
            mission_id: "msn_other".into(),
            ..original.clone()
        };
        assert!(matches!(
            scratch.store.append_mission_signal(&conflicting),
            Err(StoreError::MissionSignalCodec { .. })
        ));
        let malformed = MissionSignalRow {
            session_id: Some("not-allowed".into()),
            ..row(8, MissionSignalKind::Landing)
        };
        assert!(scratch.store.append_mission_signal(&malformed).is_err());
    }

    #[test]
    fn a_fresh_arm_clears_only_its_exact_mission_digest() {
        let scratch = Scratch::make();
        let target = row(9, MissionSignalKind::Stopped);
        let other = row(10, MissionSignalKind::Landing);
        scratch
            .store
            .append_mission_signal(&target)
            .expect("target");
        scratch.store.append_mission_signal(&other).expect("other");
        assert_eq!(
            scratch
                .store
                .clear_mission_signals(&target.mission_id, &target.mission_sha256)
                .expect("clear"),
            1
        );
        let listed = scratch.store.list_mission_signals(None).expect("list");
        assert_eq!(listed.signals.len(), 1);
        assert_eq!(listed.signals.first().map(|(_, row)| row), Some(&other));
    }
}
