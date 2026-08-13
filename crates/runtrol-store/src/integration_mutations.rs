//! Durable bounded mutation intent metadata for the public Runtime.
//!
//! The row layout has no caller input, provider output, generic payload, or message field. Sensitive parameters are
//! represented only by a keyed authenticator whose key exists for one daemon boot.

use redb::{ReadableDatabase as _, ReadableTable as _, ReadableTableMetadata as _};
use runtrol_provider::WallMs;

use crate::error::StoreError;
use crate::open::Store;
use crate::schema::{INTEGRATION_MUTATIONS, IntegrationMutationKey, SCHEMA_VERSION};

const MAX_METHOD_BYTES: usize = 64;
const ROW_BYTES: usize = 1 + 16 + 8 + 1 + 32 + 1 + MAX_METHOD_BYTES;

/// Maximum retained public mutation identities across integrations.
pub const INTEGRATION_MUTATION_MAX_ROWS: usize = 2_048;

/// Durable state of a state-changing public Runtime request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntegrationMutationState {
    /// Recorded durably before an operation that may reach a provider.
    Pending,
    /// The operation reached a deterministic successful result.
    Completed,
    /// The operation reached a deterministic refusal.
    Denied,
}

/// Content-free durable mutation metadata.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IntegrationMutationRow {
    /// Ephemeral daemon boot identity. A different boot makes the outcome unknown.
    pub boot_id: [u8; 16],
    /// Admission time used for the bounded retention window.
    pub created_at: WallMs,
    /// Stable public method name.
    pub method: Box<str>,
    /// Keyed authenticator over exact request parameters.
    pub authenticator: [u8; 32],
    /// Current durable result class.
    pub state: IntegrationMutationState,
}

impl Store {
    /// Read one exact mutation record.
    ///
    /// # Errors
    ///
    /// Engine or closed codec failure.
    pub fn get_integration_mutation(
        &self,
        key: IntegrationMutationKey,
    ) -> Result<Option<IntegrationMutationRow>, StoreError> {
        let read = self
            .db()
            .begin_read()
            .map_err(|error| engine("starting a Runtime mutation read", error))?;
        let table = match read.open_table(INTEGRATION_MUTATIONS) {
            Ok(table) => table,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(None),
            Err(error) => return Err(engine("opening Runtime mutation metadata", error)),
        };
        table
            .get(key)
            .map_err(|error| engine("reading Runtime mutation metadata", error))?
            .map(|value| decode(value.value()))
            .transpose()
    }

    /// Durably insert a mutation intent only when its exact key is absent.
    ///
    /// Returns false for an existing identity or when the fixed global bound is full.
    ///
    /// # Errors
    ///
    /// Engine or closed codec failure.
    pub fn create_integration_mutation(
        &self,
        key: IntegrationMutationKey,
        row: &IntegrationMutationRow,
    ) -> Result<bool, StoreError> {
        let encoded = encode(row)?;
        let write = self.begin_durable_write("recording a Runtime mutation intent")?;
        let inserted;
        {
            let mut table = write
                .open_table(INTEGRATION_MUTATIONS)
                .map_err(|error| engine("opening Runtime mutation metadata", error))?;
            if table
                .get(key)
                .map_err(|error| engine("checking a Runtime mutation identity", error))?
                .is_some()
                || table
                    .len()
                    .map_err(|error| engine("measuring Runtime mutation metadata", error))?
                    >= u64::try_from(INTEGRATION_MUTATION_MAX_ROWS).map_err(|_| {
                        codec("mutation retention", "the row limit does not fit the table")
                    })?
            {
                return Ok(false);
            }
            table
                .insert(key, encoded.as_slice())
                .map_err(|error| engine("writing a Runtime mutation intent", error))?;
            inserted = true;
        }
        write
            .commit()
            .map_err(|error| engine("committing a Runtime mutation intent", error))?;
        Ok(inserted)
    }

    /// Move one mutation from pending to its deterministic result class.
    ///
    /// Returns false when the key is missing, belongs to another boot, or is no longer pending.
    ///
    /// # Errors
    ///
    /// Engine or closed codec failure.
    pub fn finish_integration_mutation(
        &self,
        key: IntegrationMutationKey,
        boot_id: [u8; 16],
        state: IntegrationMutationState,
    ) -> Result<bool, StoreError> {
        if state == IntegrationMutationState::Pending {
            return Err(codec(
                "mutation state",
                "finishing cannot write a pending result",
            ));
        }
        let write = self.begin_durable_write("finishing a Runtime mutation intent")?;
        let changed;
        {
            let mut table = write
                .open_table(INTEGRATION_MUTATIONS)
                .map_err(|error| engine("opening Runtime mutation metadata", error))?;
            let Some(stored) = table
                .get(key)
                .map_err(|error| engine("reading a Runtime mutation intent", error))?
            else {
                return Ok(false);
            };
            let mut row = decode(stored.value())?;
            if row.boot_id != boot_id || row.state != IntegrationMutationState::Pending {
                return Ok(false);
            }
            row.state = state;
            let encoded = encode(&row)?;
            drop(stored);
            table
                .insert(key, encoded.as_slice())
                .map_err(|error| engine("updating a Runtime mutation result", error))?;
            changed = true;
        }
        write
            .commit()
            .map_err(|error| engine("committing a Runtime mutation result", error))?;
        Ok(changed)
    }

    /// Remove mutation identities strictly older than the retention boundary.
    ///
    /// # Errors
    ///
    /// Engine or closed codec failure.
    pub fn purge_integration_mutations_before(&self, before: WallMs) -> Result<usize, StoreError> {
        let write = self.begin_durable_write("purging expired Runtime mutations")?;
        let removed;
        {
            let mut table = write
                .open_table(INTEGRATION_MUTATIONS)
                .map_err(|error| engine("opening Runtime mutation metadata", error))?;
            let mut expired = Vec::new();
            for entry in table
                .range(IntegrationMutationKey::FIRST..=IntegrationMutationKey::LAST)
                .map_err(|error| engine("scanning Runtime mutation retention", error))?
            {
                let (key, value) =
                    entry.map_err(|error| engine("reading Runtime mutation retention", error))?;
                if decode(value.value())?.created_at < before {
                    expired.push(key.value());
                }
            }
            for key in &expired {
                drop(
                    table
                        .remove(*key)
                        .map_err(|error| engine("purging Runtime mutation metadata", error))?,
                );
            }
            removed = expired.len();
        }
        write
            .commit()
            .map_err(|error| engine("committing Runtime mutation retention", error))?;
        Ok(removed)
    }
}

fn encode(row: &IntegrationMutationRow) -> Result<Vec<u8>, StoreError> {
    if row.method.is_empty() || row.method.len() > MAX_METHOD_BYTES {
        return Err(codec(
            "mutation method",
            "it is empty or exceeds its byte limit",
        ));
    }
    let method_length = u8::try_from(row.method.len())
        .map_err(|_| codec("mutation method", "its length cannot be represented"))?;
    let mut out = Vec::with_capacity(ROW_BYTES);
    out.push(SCHEMA_VERSION);
    out.extend_from_slice(&row.boot_id);
    out.extend_from_slice(&row.created_at.as_millis().to_le_bytes());
    out.push(method_length);
    out.extend_from_slice(row.method.as_bytes());
    out.extend_from_slice(&row.authenticator);
    out.push(match row.state {
        IntegrationMutationState::Pending => 0,
        IntegrationMutationState::Completed => 1,
        IntegrationMutationState::Denied => 2,
    });
    Ok(out)
}

fn decode(bytes: &[u8]) -> Result<IntegrationMutationRow, StoreError> {
    let mut cursor = Cursor { bytes, at: 0 };
    if cursor.byte("mutation row version")? != SCHEMA_VERSION {
        return Err(codec(
            "mutation row version",
            "it belongs to another schema",
        ));
    }
    let boot_id = cursor.fixed("mutation boot identity")?;
    let created_at = WallMs::from_millis(cursor.u64("mutation creation time")?);
    let method_length = usize::from(cursor.byte("mutation method length")?);
    if method_length == 0 || method_length > MAX_METHOD_BYTES {
        return Err(codec(
            "mutation method",
            "it is empty or exceeds its byte limit",
        ));
    }
    let method = std::str::from_utf8(cursor.take("mutation method", method_length)?)
        .map_err(|_| codec("mutation method", "it is not UTF-8"))?
        .into();
    let authenticator = cursor.fixed("mutation authenticator")?;
    let state = match cursor.byte("mutation state")? {
        0 => IntegrationMutationState::Pending,
        1 => IntegrationMutationState::Completed,
        2 => IntegrationMutationState::Denied,
        _ => return Err(codec("mutation state", "it is not recognized")),
    };
    if cursor.at != bytes.len() {
        return Err(codec("mutation row end", "trailing bytes remain"));
    }
    Ok(IntegrationMutationRow {
        boot_id,
        created_at,
        method,
        authenticator,
        state,
    })
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

    fn u64(&mut self, field: &'static str) -> Result<u64, StoreError> {
        Ok(u64::from_le_bytes(
            self.take(field, 8)?
                .try_into()
                .map_err(|_| codec(field, "the field has the wrong width"))?,
        ))
    }

    fn fixed<const N: usize>(&mut self, field: &'static str) -> Result<[u8; N], StoreError> {
        self.take(field, N)?
            .try_into()
            .map_err(|_| codec(field, "the field has the wrong width"))
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
    use crate::IntegrationKey;

    #[test]
    fn mutation_rows_survive_reopen_without_caller_content() {
        let base = std::env::temp_dir().join("runtrol-integration-mutations");
        drop(std::fs::remove_dir_all(&base));
        std::fs::create_dir_all(&base).expect("create scratch");
        let root = AbsPath::canonicalize(base.to_str().expect("UTF-8 scratch")).expect("root");
        let path = root.join("state.redb").expect("database path");
        let key = IntegrationMutationKey::new(IntegrationKey::from_bytes([4; 16]), [7; 16]);
        let row = IntegrationMutationRow {
            boot_id: [9; 16],
            created_at: WallMs::from_millis(100),
            method: "sessions/submitInput".into(),
            authenticator: [11; 32],
            state: IntegrationMutationState::Pending,
        };
        {
            let store = Store::open(&path).expect("open store");
            assert!(
                store
                    .create_integration_mutation(key, &row)
                    .expect("create")
            );
        }
        let store = Store::open(&path).expect("reopen store");
        assert_eq!(
            store.get_integration_mutation(key).expect("read"),
            Some(row)
        );
        assert!(
            store
                .finish_integration_mutation(key, [9; 16], IntegrationMutationState::Completed,)
                .expect("finish")
        );
        std::mem::drop(store);
        std::fs::remove_dir_all(base).expect("clean scratch");
    }
}
