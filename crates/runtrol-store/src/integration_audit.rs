//! Bounded operational metadata for public Runtime authorization decisions.
//!
//! Rows have no generic payload, message, input, output, argument, environment, or transcript
//! field. Every admitted value is a structural identifier or a stable machine label.

use std::sync::atomic::{AtomicU64, Ordering};

use redb::{ReadableDatabase as _, ReadableTable as _, ReadableTableMetadata as _};
use runtrol_provider::WallMs;

use crate::error::StoreError;
use crate::open::Store;
use crate::schema::{INTEGRATION_AUDIT, IntegrationAuditKey, IntegrationKey, SCHEMA_VERSION};

const MAX_LABEL_BYTES: usize = 128;

/// Maximum retained authorization audit rows.
pub const INTEGRATION_AUDIT_MAX_ROWS: usize = 2_048;

/// Maximum encoded bytes in one authorization audit row.
pub const INTEGRATION_AUDIT_MAX_ROW_BYTES: usize = 2_048;

/// Maximum retained encoded authorization audit bytes.
pub const INTEGRATION_AUDIT_MAX_BYTES: usize =
    INTEGRATION_AUDIT_MAX_ROWS * INTEGRATION_AUDIT_MAX_ROW_BYTES;

static AUDIT_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Structural outcome of one public authorization decision.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntegrationAuditOutcome {
    /// The operation entered evaluation after structural parsing.
    Attempted,
    /// Authority checks and the operation succeeded.
    Allowed,
    /// The operation was refused with the recorded machine reason.
    Denied,
}

/// One bounded operational authorization record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IntegrationAuditRow {
    /// Decision time.
    pub occurred_at: WallMs,
    /// Approved integration when one was authenticated.
    pub integration: Option<IntegrationKey>,
    /// Signing-key generation used for the request.
    pub key_generation: Option<u64>,
    /// Stable public or private administration method name.
    pub method: Box<str>,
    /// Stable required app scope, when the method has one.
    pub scope: Option<Box<str>>,
    /// Opaque approved project identity, never an unapproved caller path.
    pub project: Option<Box<str>>,
    /// Opaque Runtime session identity, when an operation has one.
    pub session: Option<Box<str>>,
    /// UUIDv7 mutation identity, when an operation has one.
    pub request_id: Option<Box<str>>,
    /// Structural decision.
    pub outcome: IntegrationAuditOutcome,
    /// Stable machine reason, never raw provider or caller text.
    pub reason: Box<str>,
}

impl Store {
    /// Append one authorization record and evict oldest rows at the frozen retention ceiling.
    ///
    /// # Errors
    ///
    /// [`StoreError::IntegrationCodec`] when a field or row exceeds its bound, or engine failure.
    pub fn append_integration_audit(&self, row: &IntegrationAuditRow) -> Result<(), StoreError> {
        self.append_integration_audit_with_limit(row, INTEGRATION_AUDIT_MAX_ROWS)
    }

    fn append_integration_audit_with_limit(
        &self,
        row: &IntegrationAuditRow,
        max_rows: usize,
    ) -> Result<(), StoreError> {
        if max_rows == 0 || max_rows > INTEGRATION_AUDIT_MAX_ROWS {
            return Err(codec(
                "retention",
                "the row limit is outside the frozen bound",
            ));
        }
        let encoded = encode(row)?;
        let write = self
            .db()
            .begin_write()
            .map_err(|error| engine("starting a public Runtime audit write", error))?;
        {
            let mut table = write
                .open_table(INTEGRATION_AUDIT)
                .map_err(|error| engine("opening public Runtime audit metadata", error))?;
            let mut chosen = None;
            for _ in 0..16 {
                let key = audit_key(
                    row.occurred_at,
                    AUDIT_SEQUENCE.fetch_add(1, Ordering::Relaxed),
                );
                if table
                    .get(key)
                    .map_err(|error| engine("checking a public Runtime audit identity", error))?
                    .is_none()
                {
                    chosen = Some(key);
                    break;
                }
            }
            let key = chosen.ok_or_else(|| {
                codec(
                    "audit identity",
                    "a unique bounded identity could not be minted",
                )
            })?;
            table
                .insert(key, encoded.as_slice())
                .map_err(|error| engine("appending public Runtime audit metadata", error))?;
            let max_rows = u64::try_from(max_rows)
                .map_err(|_| codec("retention", "the row limit does not fit the table"))?;
            while table
                .len()
                .map_err(|error| engine("measuring public Runtime audit retention", error))?
                > max_rows
            {
                let oldest = {
                    let mut rows = table
                        .range(IntegrationAuditKey::FIRST..=IntegrationAuditKey::LAST)
                        .map_err(|error| {
                            engine("scanning public Runtime audit retention", error)
                        })?;
                    rows.next()
                        .transpose()
                        .map_err(|error| engine("reading public Runtime audit retention", error))?
                        .map(|(key, _)| key.value())
                };
                let Some(oldest) = oldest else {
                    return Err(codec(
                        "retention",
                        "the audit table length was inconsistent",
                    ));
                };
                drop(table.remove(oldest).map_err(|error| {
                    engine("evicting old public Runtime audit metadata", error)
                })?);
            }
        }
        write
            .commit()
            .map_err(|error| engine("committing public Runtime audit metadata", error))
    }

    /// List bounded authorization metadata oldest first for local administration.
    ///
    /// # Errors
    ///
    /// Damaged rows or engine failure. Authority audit corruption is never silently omitted.
    pub fn list_integration_audit(&self) -> Result<Vec<IntegrationAuditRow>, StoreError> {
        let read = self
            .db()
            .begin_read()
            .map_err(|error| engine("starting a public Runtime audit read", error))?;
        let table = match read.open_table(INTEGRATION_AUDIT) {
            Ok(table) => table,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(Vec::new()),
            Err(error) => return Err(engine("opening public Runtime audit metadata", error)),
        };
        table
            .range(IntegrationAuditKey::FIRST..=IntegrationAuditKey::LAST)
            .map_err(|error| engine("scanning public Runtime audit metadata", error))?
            .map(|entry| {
                let (_, value) = entry
                    .map_err(|error| engine("reading public Runtime audit metadata", error))?;
                decode(value.value())
            })
            .collect()
    }

    /// Remove all authorization metadata without changing grants or enrollment state.
    ///
    /// # Errors
    ///
    /// Engine failure.
    pub fn purge_integration_audit(&self) -> Result<usize, StoreError> {
        let write = self
            .db()
            .begin_write()
            .map_err(|error| engine("starting a public Runtime audit purge", error))?;
        let removed = {
            let mut table = write
                .open_table(INTEGRATION_AUDIT)
                .map_err(|error| engine("opening public Runtime audit metadata", error))?;
            let keys = table
                .range(IntegrationAuditKey::FIRST..=IntegrationAuditKey::LAST)
                .map_err(|error| engine("scanning public Runtime audit purge", error))?
                .map(|entry| {
                    entry
                        .map(|(key, _)| key.value())
                        .map_err(|error| engine("reading public Runtime audit purge", error))
                })
                .collect::<Result<Vec<_>, _>>()?;
            for key in &keys {
                drop(
                    table
                        .remove(*key)
                        .map_err(|error| engine("purging public Runtime audit metadata", error))?,
                );
            }
            keys.len()
        };
        write
            .commit()
            .map_err(|error| engine("committing public Runtime audit purge", error))?;
        Ok(removed)
    }
}

fn audit_key(occurred_at: WallMs, sequence: u64) -> IntegrationAuditKey {
    let mut bytes = [0_u8; 16];
    bytes[..8].copy_from_slice(&occurred_at.as_millis().to_be_bytes());
    bytes[8..].copy_from_slice(&sequence.to_be_bytes());
    IntegrationAuditKey::from_bytes(bytes)
}

fn encode(row: &IntegrationAuditRow) -> Result<Vec<u8>, StoreError> {
    let mut out = Vec::new();
    out.push(SCHEMA_VERSION);
    out.extend_from_slice(&row.occurred_at.as_millis().to_le_bytes());
    write_optional_fixed(&mut out, row.integration.map(IntegrationKey::to_bytes));
    write_optional_u64(&mut out, row.key_generation);
    write_text(&mut out, "method", &row.method, MAX_LABEL_BYTES)?;
    write_optional_text(&mut out, "scope", row.scope.as_deref(), MAX_LABEL_BYTES)?;
    write_optional_text(&mut out, "project", row.project.as_deref(), MAX_LABEL_BYTES)?;
    write_optional_text(&mut out, "session", row.session.as_deref(), MAX_LABEL_BYTES)?;
    write_optional_text(
        &mut out,
        "request identity",
        row.request_id.as_deref(),
        MAX_LABEL_BYTES,
    )?;
    out.push(match row.outcome {
        IntegrationAuditOutcome::Attempted => 0,
        IntegrationAuditOutcome::Allowed => 1,
        IntegrationAuditOutcome::Denied => 2,
    });
    write_text(&mut out, "reason", &row.reason, MAX_LABEL_BYTES)?;
    if out.len() > INTEGRATION_AUDIT_MAX_ROW_BYTES {
        return Err(codec("audit row", "it exceeds the frozen byte ceiling"));
    }
    Ok(out)
}

fn decode(bytes: &[u8]) -> Result<IntegrationAuditRow, StoreError> {
    if bytes.len() > INTEGRATION_AUDIT_MAX_ROW_BYTES {
        return Err(codec("audit row", "it exceeds the frozen byte ceiling"));
    }
    let mut cursor = Cursor { bytes, at: 0 };
    if cursor.byte("row version")? != SCHEMA_VERSION {
        return Err(codec("row version", "it belongs to another schema"));
    }
    let occurred_at = WallMs::from_millis(cursor.u64("occurred at")?);
    let integration = cursor
        .optional_fixed("integration")?
        .map(IntegrationKey::from_bytes);
    let key_generation = cursor.optional_u64("key generation")?;
    let method = cursor.text("method", MAX_LABEL_BYTES)?.into();
    let scope = cursor.optional_text("scope", MAX_LABEL_BYTES)?;
    let project = cursor.optional_text("project", MAX_LABEL_BYTES)?;
    let session = cursor.optional_text("session", MAX_LABEL_BYTES)?;
    let request_id = cursor.optional_text("request identity", MAX_LABEL_BYTES)?;
    let outcome = match cursor.byte("outcome")? {
        0 => IntegrationAuditOutcome::Attempted,
        1 => IntegrationAuditOutcome::Allowed,
        2 => IntegrationAuditOutcome::Denied,
        _ => return Err(codec("outcome", "it is not recognized")),
    };
    let reason = cursor.text("reason", MAX_LABEL_BYTES)?.into();
    if cursor.at != bytes.len() {
        return Err(codec("end of row", "trailing bytes remain"));
    }
    Ok(IntegrationAuditRow {
        occurred_at,
        integration,
        key_generation,
        method,
        scope,
        project,
        session,
        request_id,
        outcome,
        reason,
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

fn write_optional_fixed(out: &mut Vec<u8>, value: Option<[u8; 16]>) {
    match value {
        Some(value) => {
            out.push(1);
            out.extend_from_slice(&value);
        }
        None => out.push(0),
    }
}

fn write_optional_u64(out: &mut Vec<u8>, value: Option<u64>) {
    match value {
        Some(value) => {
            out.push(1);
            out.extend_from_slice(&value.to_le_bytes());
        }
        None => out.push(0),
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

    fn u64(&mut self, field: &'static str) -> Result<u64, StoreError> {
        Ok(u64::from_le_bytes(
            self.take(field, 8)?
                .try_into()
                .map_err(|_| codec(field, "the field has the wrong width"))?,
        ))
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

    fn optional_fixed(&mut self, field: &'static str) -> Result<Option<[u8; 16]>, StoreError> {
        match self.byte(field)? {
            0 => Ok(None),
            1 => self
                .take(field, 16)?
                .try_into()
                .map(Some)
                .map_err(|_| codec(field, "the field has the wrong width")),
            _ => Err(codec(field, "its presence marker is not recognized")),
        }
    }

    fn optional_u64(&mut self, field: &'static str) -> Result<Option<u64>, StoreError> {
        match self.byte(field)? {
            0 => Ok(None),
            1 => self.u64(field).map(Some),
            _ => Err(codec(field, "its presence marker is not recognized")),
        }
    }
}

fn codec(field: &'static str, why: &'static str) -> StoreError {
    StoreError::IntegrationCodec { field, why }
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
            let base = std::env::temp_dir().join("runtrol-integration-audit");
            drop(std::fs::remove_dir_all(&base));
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
                eprintln!("could not clean integration audit scratch: {error}");
            }
        }
    }

    fn row(at: u64, outcome: IntegrationAuditOutcome) -> IntegrationAuditRow {
        IntegrationAuditRow {
            occurred_at: WallMs::from_millis(at),
            integration: Some(IntegrationKey::from_bytes([3; 16])),
            key_generation: Some(2),
            method: "providers/list".into(),
            scope: Some("provider.read".into()),
            project: None,
            session: None,
            request_id: None,
            outcome,
            reason: "allowed".into(),
        }
    }

    #[test]
    fn audit_rows_round_trip_and_oldest_rows_are_evicted() {
        let scratch = Scratch::make();
        for at in 1..=4 {
            scratch
                .store
                .append_integration_audit_with_limit(&row(at, IntegrationAuditOutcome::Allowed), 3)
                .expect("append audit");
        }
        let rows = scratch.store.list_integration_audit().expect("list audit");
        assert_eq!(rows.len(), 3);
        assert_eq!(
            rows.first().map(|row| row.occurred_at),
            Some(WallMs::from_millis(2))
        );
        assert_eq!(
            rows.last().map(|row| row.occurred_at),
            Some(WallMs::from_millis(4))
        );
        assert_eq!(scratch.store.purge_integration_audit().expect("purge"), 3);
        assert!(
            scratch
                .store
                .list_integration_audit()
                .expect("empty")
                .is_empty()
        );
    }

    #[test]
    fn audit_rows_have_no_generic_content_field_and_enforce_the_byte_ceiling() {
        assert_eq!(
            INTEGRATION_AUDIT_MAX_BYTES,
            INTEGRATION_AUDIT_MAX_ROWS * INTEGRATION_AUDIT_MAX_ROW_BYTES
        );
        let mut oversized = row(1, IntegrationAuditOutcome::Denied);
        oversized.project = Some("x".repeat(INTEGRATION_AUDIT_MAX_ROW_BYTES).into());
        assert!(encode(&oversized).is_err());
    }
}
