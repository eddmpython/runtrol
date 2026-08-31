//! Bounded operational metadata for public Runtime authorization decisions.
//!
//! Rows have no generic payload, message, input, output, argument, environment, or transcript
//! field. Every admitted value is a structural identifier or a stable machine label.

#[cfg(test)]
use std::sync::atomic::{AtomicU64, Ordering};

use redb::{ReadableDatabase as _, ReadableTable as _, ReadableTableMetadata as _};
use runtrol_provider::WallMs;
use std::collections::BTreeSet;

use crate::error::StoreError;
use crate::open::Store;
use crate::schema::{
    INTEGRATION_AUDIT, INTEGRATION_AUDIT_RECEIPTS, IntegrationAuditKey, IntegrationAuditReceiptKey,
    IntegrationKey, SCHEMA_VERSION,
};

const MAX_LABEL_BYTES: usize = 128;
const MAX_SOURCE_GENERATION_BYTES: usize = 128;

type EncodedAuditRow = (WallMs, Vec<u8>);

/// Maximum retained authorization audit rows.
pub const INTEGRATION_AUDIT_MAX_ROWS: usize = 2_048;

/// Maximum encoded bytes in one authorization audit row.
pub const INTEGRATION_AUDIT_MAX_ROW_BYTES: usize = 2_048;

/// Maximum retained encoded authorization audit bytes.
pub const INTEGRATION_AUDIT_MAX_BYTES: usize =
    INTEGRATION_AUDIT_MAX_ROWS * INTEGRATION_AUDIT_MAX_ROW_BYTES;

/// Maximum retained draining-generation receipt epochs.
pub const INTEGRATION_AUDIT_RECEIPT_MAX_EPOCHS: usize = 64;

/// Structural outcome of one public authorization decision.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntegrationAuditOutcome {
    /// The operation entered evaluation after structural parsing.
    Attempted,
    /// Authority checks and the operation succeeded.
    Allowed,
    /// The operation did not produce a confirmed success. The reason distinguishes refusal from an
    /// indeterminate mutation outcome.
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
    /// UUIDv7 audit correlation identity, when an operation has one.
    pub request_id: Option<Box<str>>,
    /// Structural decision.
    pub outcome: IntegrationAuditOutcome,
    /// Stable machine reason, never raw provider or caller text.
    pub reason: Box<str>,
}

/// One sequenced row from a draining generation's replayable audit relay.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IntegrationAuditRelayEntry {
    /// Monotonic sequence within the relay epoch. Sequence zero is never valid.
    pub sequence: u64,
    /// Content-free authorization row.
    pub row: IntegrationAuditRow,
}

/// One stable marker covering relay rows lost before durable handoff.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IntegrationAuditRelayLoss {
    /// Highest contiguous sequence represented by the marker.
    pub through: u64,
    /// Content-free authorization row explaining the bounded loss.
    pub row: IntegrationAuditRow,
}

/// One replayable audit snapshot from a draining generation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IntegrationAuditRelayBatch {
    /// Process-unique UUIDv7 epoch of the draining generation.
    pub epoch: [u8; 16],
    /// Executable generation that owns the process epoch. This binds receipt retention to the live locator.
    pub source_generation: Box<str>,
    /// Stable loss marker when bounded relay retention evicted rows.
    pub loss: Option<IntegrationAuditRelayLoss>,
    /// Unacknowledged rows in ascending sequence order.
    pub entries: Vec<IntegrationAuditRelayEntry>,
}

struct RelayReceipt {
    watermark: u64,
    source_generation: Box<str>,
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

    /// Append one FIFO batch in one durable transaction and apply retention once.
    ///
    /// # Errors
    ///
    /// [`StoreError::IntegrationCodec`] when a field, row, or batch exceeds its bound, or engine failure.
    pub fn append_integration_audit_batch(
        &self,
        rows: &[IntegrationAuditRow],
    ) -> Result<(), StoreError> {
        self.append_integration_audit_batch_with_limit(rows, INTEGRATION_AUDIT_MAX_ROWS)
    }

    fn append_integration_audit_with_limit(
        &self,
        row: &IntegrationAuditRow,
        max_rows: usize,
    ) -> Result<(), StoreError> {
        self.append_integration_audit_batch_with_limit(std::slice::from_ref(row), max_rows)
    }

    fn append_integration_audit_batch_with_limit(
        &self,
        rows: &[IntegrationAuditRow],
        max_rows: usize,
    ) -> Result<(), StoreError> {
        if max_rows == 0 || max_rows > INTEGRATION_AUDIT_MAX_ROWS {
            return Err(codec(
                "retention",
                "the row limit is outside the frozen bound",
            ));
        }
        if rows.len() > INTEGRATION_AUDIT_MAX_ROWS {
            return Err(codec(
                "batch",
                "the audit batch exceeds the frozen row bound",
            ));
        }
        let encoded = rows
            .iter()
            .map(|row| encode(row).map(|encoded| (row.occurred_at, encoded)))
            .collect::<Result<Vec<_>, _>>()?;
        if encoded.is_empty() {
            return Ok(());
        }
        let max_rows = u64::try_from(max_rows)
            .map_err(|_| codec("retention", "the row limit does not fit the table"))?;
        let write = self
            .db()?
            .begin_write()
            .map_err(|error| engine("starting a public Runtime audit write", error))?;
        append_encoded_audit_rows(&write, &encoded, max_rows)?;
        write
            .commit()
            .map_err(|error| engine("committing public Runtime audit metadata", error))
    }

    /// Atomically append one replayable generation batch and advance its durable receipt.
    ///
    /// Rows at or below the stored receipt are ignored. Every new entry must immediately follow the
    /// durable watermark unless a loss marker explicitly covers the missing contiguous range. The returned
    /// value is the highest sequence committed with the rows in the same transaction.
    ///
    /// # Errors
    ///
    /// [`StoreError::IntegrationCodec`] for a zero epoch, malformed sequence order, a sequence gap, or a row
    /// outside its bound. Engine failures commit neither audit rows nor the receipt.
    pub fn append_integration_audit_relay_batch(
        &self,
        batch: &IntegrationAuditRelayBatch,
    ) -> Result<u64, StoreError> {
        validate_relay_batch(batch)?;
        let receipt_key = IntegrationAuditReceiptKey::from_bytes(batch.epoch);
        let receipt_limit = u64::try_from(INTEGRATION_AUDIT_RECEIPT_MAX_EPOCHS)
            .map_err(|_| codec("relay receipt", "the epoch limit does not fit the table"))?;
        let write = self
            .db()?
            .begin_write()
            .map_err(|error| engine("starting a generation audit relay write", error))?;
        let current = read_relay_receipt(&write, receipt_key)?;
        if current.as_ref().is_some_and(|receipt| {
            receipt.source_generation.as_ref() != batch.source_generation.as_ref()
        }) {
            return Err(codec(
                "relay receipt",
                "the process epoch is already bound to another Runtime generation",
            ));
        }
        let current_watermark = current.as_ref().map_or(0, |receipt| receipt.watermark);
        let (watermark, encoded) = encode_relay_rows(batch, current_watermark)?;
        if current_watermark == watermark && encoded.is_empty() {
            return Ok(watermark);
        }

        append_encoded_audit_rows(
            &write,
            &encoded,
            u64::try_from(INTEGRATION_AUDIT_MAX_ROWS)
                .map_err(|_| codec("retention", "the row limit does not fit the table"))?,
        )?;
        advance_relay_receipt(
            &write,
            receipt_key,
            watermark,
            &batch.source_generation,
            receipt_limit,
        )?;
        write
            .commit()
            .map_err(|error| engine("committing a generation audit relay receipt", error))?;
        Ok(watermark)
    }

    /// Remove receipts only after their source generation is absent from a successfully read live locator.
    ///
    /// A receipt for a listed generation is never evicted for age or activity. This is what lets a terminal
    /// survive arbitrarily many upgrades and later emit another audited control request without duplicating its
    /// earlier rows. The live-generation ceiling keeps this table bounded while those processes exist.
    ///
    /// # Errors
    ///
    /// Damaged receipt metadata or an engine failure leaves the receipt set unchanged.
    pub fn retain_integration_audit_relay_receipts(
        &self,
        live_generations: &BTreeSet<Box<str>>,
    ) -> Result<usize, StoreError> {
        let write = self
            .db()?
            .begin_write()
            .map_err(|error| engine("starting generation audit receipt retention", error))?;
        let removed = {
            let mut receipts = write
                .open_table(INTEGRATION_AUDIT_RECEIPTS)
                .map_err(|error| engine("opening generation audit relay receipts", error))?;
            let stale = receipts
                .range(IntegrationAuditReceiptKey::FIRST..=IntegrationAuditReceiptKey::LAST)
                .map_err(|error| {
                    engine("scanning generation audit relay receipt retention", error)
                })?
                .map(|entry| {
                    let (key, value) = entry.map_err(|error| {
                        engine("reading generation audit relay receipt retention", error)
                    })?;
                    let receipt = decode_relay_receipt(value.value())?;
                    Ok((key.value(), receipt.source_generation))
                })
                .collect::<Result<Vec<_>, StoreError>>()?
                .into_iter()
                .filter_map(|(key, source)| (!live_generations.contains(&source)).then_some(key))
                .collect::<Vec<_>>();
            for key in &stale {
                drop(receipts.remove(*key).map_err(|error| {
                    engine("removing a retired generation audit relay receipt", error)
                })?);
            }
            stale.len()
        };
        write
            .commit()
            .map_err(|error| engine("committing generation audit receipt retention", error))?;
        Ok(removed)
    }

    /// List bounded authorization metadata oldest first for local administration.
    ///
    /// # Errors
    ///
    /// Damaged rows or engine failure. Authority audit corruption is never silently omitted.
    pub fn list_integration_audit(&self) -> Result<Vec<IntegrationAuditRow>, StoreError> {
        let read = self
            .db()?
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
            .db()?
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

fn read_relay_receipt(
    write: &redb::WriteTransaction,
    receipt_key: IntegrationAuditReceiptKey,
) -> Result<Option<RelayReceipt>, StoreError> {
    let receipts = write
        .open_table(INTEGRATION_AUDIT_RECEIPTS)
        .map_err(|error| engine("opening generation audit relay receipts", error))?;
    let current = receipts
        .get(receipt_key)
        .map_err(|error| engine("reading a generation audit relay receipt", error))?;
    current
        .map(|value| decode_relay_receipt(value.value()))
        .transpose()
}

fn encode_relay_rows(
    batch: &IntegrationAuditRelayBatch,
    current: u64,
) -> Result<(u64, Vec<EncodedAuditRow>), StoreError> {
    let mut watermark = current;
    let mut encoded =
        Vec::with_capacity(batch.entries.len() + usize::from(batch.loss.as_ref().is_some()));
    if let Some(loss) = batch.loss.as_ref().filter(|loss| loss.through > watermark) {
        encoded.push((loss.row.occurred_at, encode(&loss.row)?));
        watermark = loss.through;
    }
    for entry in &batch.entries {
        if entry.sequence <= watermark {
            continue;
        }
        let expected = watermark
            .checked_add(1)
            .ok_or_else(|| codec("relay sequence", "the sequence space is exhausted"))?;
        if entry.sequence != expected {
            return Err(codec(
                "relay sequence",
                "the next sequence is not contiguous with the durable receipt",
            ));
        }
        encoded.push((entry.row.occurred_at, encode(&entry.row)?));
        watermark = entry.sequence;
    }
    Ok((watermark, encoded))
}

fn advance_relay_receipt(
    write: &redb::WriteTransaction,
    receipt_key: IntegrationAuditReceiptKey,
    watermark: u64,
    source_generation: &str,
    receipt_limit: u64,
) -> Result<(), StoreError> {
    let mut receipts = write
        .open_table(INTEGRATION_AUDIT_RECEIPTS)
        .map_err(|error| engine("opening generation audit relay receipts", error))?;
    let replaced_epochs = receipts
        .range(IntegrationAuditReceiptKey::FIRST..=IntegrationAuditReceiptKey::LAST)
        .map_err(|error| engine("scanning generation audit relay receipt sources", error))?
        .map(|row| {
            let (key, value) = row
                .map_err(|error| engine("reading generation audit relay receipt sources", error))?;
            let key = key.value();
            let receipt = decode_relay_receipt(value.value())?;
            Ok((key, receipt.source_generation))
        })
        .collect::<Result<Vec<_>, StoreError>>()?
        .into_iter()
        .filter_map(|(key, source)| {
            (key != receipt_key && source.as_ref() == source_generation).then_some(key)
        })
        .collect::<Vec<_>>();
    for key in replaced_epochs {
        drop(
            receipts.remove(key).map_err(|error| {
                engine("removing a replaced generation audit relay epoch", error)
            })?,
        );
    }
    let is_new = receipts
        .get(receipt_key)
        .map_err(|error| engine("checking a generation audit relay receipt", error))?
        .is_none();
    if is_new
        && receipts
            .len()
            .map_err(|error| engine("measuring generation audit relay receipts", error))?
            >= receipt_limit
    {
        return Err(codec(
            "relay receipt",
            "the live generation receipt ceiling is full",
        ));
    }
    let encoded = encode_relay_receipt(watermark, source_generation)?;
    receipts
        .insert(receipt_key, encoded.as_slice())
        .map_err(|error| engine("advancing a generation audit relay receipt", error))?;
    Ok(())
}

fn encode_relay_receipt(watermark: u64, source_generation: &str) -> Result<Vec<u8>, StoreError> {
    if source_generation.is_empty() || source_generation.len() > MAX_SOURCE_GENERATION_BYTES {
        return Err(codec(
            "relay source generation",
            "the source generation is empty or oversized",
        ));
    }
    let length = u16::try_from(source_generation.len()).map_err(|_| {
        codec(
            "relay source generation",
            "the source generation length cannot be represented",
        )
    })?;
    let mut encoded = Vec::with_capacity(1 + 8 + 2 + source_generation.len());
    encoded.push(SCHEMA_VERSION);
    encoded.extend_from_slice(&watermark.to_le_bytes());
    encoded.extend_from_slice(&length.to_le_bytes());
    encoded.extend_from_slice(source_generation.as_bytes());
    Ok(encoded)
}

fn decode_relay_receipt(bytes: &[u8]) -> Result<RelayReceipt, StoreError> {
    let Some((&version, rest)) = bytes.split_first() else {
        return Err(codec("relay receipt", "the row is empty"));
    };
    if version != SCHEMA_VERSION {
        return Err(codec("relay receipt", "the row belongs to another schema"));
    }
    let Some(watermark_bytes) = rest.get(..8) else {
        return Err(codec("relay receipt", "the watermark is truncated"));
    };
    let Some(length_bytes) = rest.get(8..10) else {
        return Err(codec("relay receipt", "the source length is truncated"));
    };
    let watermark = u64::from_le_bytes(
        watermark_bytes
            .try_into()
            .map_err(|_| codec("relay receipt", "the watermark width is invalid"))?,
    );
    let length =
        usize::from(u16::from_le_bytes(length_bytes.try_into().map_err(
            |_| codec("relay receipt", "the source length width is invalid"),
        )?));
    if length == 0 || length > MAX_SOURCE_GENERATION_BYTES {
        return Err(codec(
            "relay source generation",
            "the source generation is empty or oversized",
        ));
    }
    let source = rest
        .get(10..)
        .filter(|source| source.len() == length)
        .ok_or_else(|| {
            codec(
                "relay receipt",
                "the source generation length is inconsistent",
            )
        })?;
    let source_generation = std::str::from_utf8(source)
        .map_err(|_| codec("relay receipt", "the source generation is not UTF-8"))?;
    Ok(RelayReceipt {
        watermark,
        source_generation: source_generation.into(),
    })
}

fn validate_relay_batch(batch: &IntegrationAuditRelayBatch) -> Result<(), StoreError> {
    if batch.epoch == [0; 16] {
        return Err(codec(
            "relay epoch",
            "the all-zero capability sentinel is not a durable epoch",
        ));
    }
    if batch.source_generation.is_empty()
        || batch.source_generation.len() > MAX_SOURCE_GENERATION_BYTES
    {
        return Err(codec(
            "relay source generation",
            "the source generation is empty or oversized",
        ));
    }
    let row_count = batch
        .entries
        .len()
        .checked_add(usize::from(batch.loss.is_some()))
        .ok_or_else(|| codec("relay batch", "the row count overflows"))?;
    if row_count > INTEGRATION_AUDIT_MAX_ROWS {
        return Err(codec(
            "relay batch",
            "the audit batch exceeds the frozen row bound",
        ));
    }
    if let Some(loss) = &batch.loss {
        if loss.through == 0 {
            return Err(codec(
                "relay loss",
                "a loss marker must cover at least sequence one",
            ));
        }
        if batch
            .entries
            .first()
            .is_some_and(|entry| entry.sequence <= loss.through)
        {
            return Err(codec(
                "relay loss",
                "the loss marker overlaps a retained entry",
            ));
        }
    }
    let mut previous = 0;
    for entry in &batch.entries {
        if entry.sequence == 0 {
            return Err(codec(
                "relay sequence",
                "sequence zero is reserved for an absent receipt",
            ));
        }
        if entry.sequence <= previous {
            return Err(codec(
                "relay sequence",
                "entries are not in strictly ascending order",
            ));
        }
        previous = entry.sequence;
    }
    Ok(())
}

fn append_encoded_audit_rows(
    write: &redb::WriteTransaction,
    rows: &[EncodedAuditRow],
    max_rows: u64,
) -> Result<(), StoreError> {
    if rows.is_empty() {
        return Ok(());
    }
    let mut table = write
        .open_table(INTEGRATION_AUDIT)
        .map_err(|error| engine("opening public Runtime audit metadata", error))?;
    let mut previous_key = table
        .last()
        .map_err(|error| engine("reading the latest public Runtime audit identity", error))?
        .map(|(key, _)| key.value());
    for (occurred_at, encoded) in rows {
        let key = audit_key_after(previous_key, *occurred_at)?;
        table
            .insert(key, encoded.as_slice())
            .map_err(|error| engine("appending public Runtime audit metadata", error))?;
        previous_key = Some(key);
    }
    while table
        .len()
        .map_err(|error| engine("measuring public Runtime audit retention", error))?
        > max_rows
    {
        let oldest = {
            let mut rows = table
                .range(IntegrationAuditKey::FIRST..=IntegrationAuditKey::LAST)
                .map_err(|error| engine("scanning public Runtime audit retention", error))?;
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
        drop(
            table
                .remove(oldest)
                .map_err(|error| engine("evicting old public Runtime audit metadata", error))?,
        );
    }
    Ok(())
}

fn audit_key_after(
    previous: Option<IntegrationAuditKey>,
    occurred_at: WallMs,
) -> Result<IntegrationAuditKey, StoreError> {
    let wall_clock_floor = u128::from(occurred_at.as_millis()) << 64;
    let value = match previous {
        Some(previous) => u128::from_be_bytes(previous.to_bytes())
            .checked_add(1)
            .ok_or_else(|| codec("audit identity", "the ordered identity space is exhausted"))?
            .max(wall_clock_floor),
        None => wall_clock_floor,
    };
    Ok(IntegrationAuditKey::from_bytes(value.to_be_bytes()))
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

    static SCRATCH_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct Scratch {
        root: AbsPath,
        store: Store,
    }

    impl Scratch {
        fn make() -> Self {
            let sequence = SCRATCH_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let base = std::env::temp_dir().join(format!(
                "runtrol-integration-audit-{}-{sequence}",
                std::process::id()
            ));
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

    fn relay_entry(sequence: u64) -> IntegrationAuditRelayEntry {
        let mut audit = row(sequence, IntegrationAuditOutcome::Allowed);
        audit.method = format!("relay/{sequence}").into();
        IntegrationAuditRelayEntry {
            sequence,
            row: audit,
        }
    }

    fn relay_batch(epoch: u8, sequences: &[u64]) -> IntegrationAuditRelayBatch {
        IntegrationAuditRelayBatch {
            epoch: [epoch; 16],
            source_generation: format!("{epoch:064x}").into(),
            loss: None,
            entries: sequences.iter().copied().map(relay_entry).collect(),
        }
    }

    fn relay_receipt(store: &Store, epoch: u8) -> Option<(u64, Box<str>)> {
        let engine = store.db().expect("store engine");
        let read = engine.begin_read().expect("receipt read");
        let table = read
            .open_table(INTEGRATION_AUDIT_RECEIPTS)
            .expect("receipt table");
        let receipt = table
            .get(IntegrationAuditReceiptKey::from_bytes([epoch; 16]))
            .expect("read receipt")
            .map(|value| {
                let receipt = decode_relay_receipt(value.value()).expect("decode receipt");
                (receipt.watermark, receipt.source_generation)
            });
        drop(table);
        drop(read);
        drop(engine);
        receipt
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
    fn audit_batch_preserves_fifo_order_for_equal_timestamps() {
        let scratch = Scratch::make();
        let rows = (0..32)
            .map(|index| {
                let mut row = row(7, IntegrationAuditOutcome::Allowed);
                row.method = format!("batch/{index}").into();
                row
            })
            .collect::<Vec<_>>();

        scratch
            .store
            .append_integration_audit_batch_with_limit(&rows, 64)
            .expect("append batch");

        let methods = scratch
            .store
            .list_integration_audit()
            .expect("list batch")
            .into_iter()
            .map(|row| row.method)
            .collect::<Vec<_>>();
        assert_eq!(
            methods,
            (0..32)
                .map(|index| format!("batch/{index}").into())
                .collect::<Vec<Box<str>>>()
        );
    }

    #[test]
    fn audit_fifo_survives_wall_clock_regression_across_transactions() {
        let scratch = Scratch::make();
        let mut first = row(100, IntegrationAuditOutcome::Attempted);
        first.method = "clock/first".into();
        scratch
            .store
            .append_integration_audit(&first)
            .expect("append first row");

        let mut second = row(90, IntegrationAuditOutcome::Allowed);
        second.method = "clock/second".into();
        let mut third = row(80, IntegrationAuditOutcome::Denied);
        third.method = "clock/third".into();
        scratch
            .store
            .append_integration_audit_batch(&[second, third])
            .expect("append regressed rows");

        let methods = scratch
            .store
            .list_integration_audit()
            .expect("list regressed rows")
            .into_iter()
            .map(|row| row.method)
            .collect::<Vec<_>>();
        assert_eq!(
            methods,
            ["clock/first", "clock/second", "clock/third"]
                .map(Box::<str>::from)
                .to_vec()
        );
    }

    #[test]
    fn audit_batch_applies_retention_after_the_whole_batch() {
        let scratch = Scratch::make();
        for at in 1..=2 {
            scratch
                .store
                .append_integration_audit_with_limit(
                    &row(at, IntegrationAuditOutcome::Attempted),
                    3,
                )
                .expect("append existing row");
        }
        let rows = (3..=5)
            .map(|at| row(at, IntegrationAuditOutcome::Allowed))
            .collect::<Vec<_>>();

        scratch
            .store
            .append_integration_audit_batch_with_limit(&rows, 3)
            .expect("append retained batch");

        let retained = scratch
            .store
            .list_integration_audit()
            .expect("list retained");
        assert_eq!(
            retained
                .iter()
                .map(|row| row.occurred_at)
                .collect::<Vec<_>>(),
            [3, 4, 5].map(WallMs::from_millis)
        );
        assert!(
            retained
                .iter()
                .all(|row| row.outcome == IntegrationAuditOutcome::Allowed)
        );
    }

    #[test]
    fn an_oversized_row_rejects_the_entire_batch() {
        let scratch = Scratch::make();
        let existing = row(1, IntegrationAuditOutcome::Attempted);
        scratch
            .store
            .append_integration_audit(&existing)
            .expect("append existing row");
        let accepted = row(2, IntegrationAuditOutcome::Allowed);
        let mut oversized = row(3, IntegrationAuditOutcome::Denied);
        oversized.project = Some("x".repeat(INTEGRATION_AUDIT_MAX_ROW_BYTES).into());
        let trailing = row(4, IntegrationAuditOutcome::Allowed);

        assert!(
            scratch
                .store
                .append_integration_audit_batch(&[accepted, oversized, trailing])
                .is_err()
        );
        assert_eq!(
            scratch
                .store
                .list_integration_audit()
                .expect("list after refusal"),
            vec![existing]
        );
    }

    #[test]
    fn repeated_relay_batch_does_not_duplicate_rows() {
        let scratch = Scratch::make();
        let batch = relay_batch(1, &[1, 2]);

        assert_eq!(
            scratch
                .store
                .append_integration_audit_relay_batch(&batch)
                .expect("commit relay batch"),
            2
        );
        let first_receipt = relay_receipt(&scratch.store, 1);
        assert_eq!(
            scratch
                .store
                .append_integration_audit_relay_batch(&batch)
                .expect("repeat relay batch"),
            2
        );
        assert_eq!(
            relay_receipt(&scratch.store, 1),
            first_receipt,
            "an exact replay performs no receipt write"
        );
        assert_eq!(
            scratch
                .store
                .list_integration_audit()
                .expect("list repeated relay")
                .into_iter()
                .map(|row| row.method)
                .collect::<Vec<_>>(),
            ["relay/1", "relay/2"].map(Box::<str>::from).to_vec()
        );
    }

    #[test]
    fn relay_gap_refuses_the_whole_transaction() {
        let scratch = Scratch::make();
        let mut existing = row(7, IntegrationAuditOutcome::Attempted);
        existing.method = "existing".into();
        scratch
            .store
            .append_integration_audit(&existing)
            .expect("append existing row");

        assert!(
            scratch
                .store
                .append_integration_audit_relay_batch(&relay_batch(2, &[1, 3]))
                .is_err()
        );
        assert_eq!(
            scratch
                .store
                .list_integration_audit()
                .expect("list after relay gap"),
            vec![existing]
        );
        assert_eq!(
            scratch
                .store
                .append_integration_audit_relay_batch(&relay_batch(2, &[1, 2, 3]))
                .expect("commit contiguous relay after refusal"),
            3,
            "the refused transaction did not leave a receipt"
        );
    }

    #[test]
    fn relay_replay_appends_only_entries_after_the_receipt() {
        let scratch = Scratch::make();
        scratch
            .store
            .append_integration_audit_relay_batch(&relay_batch(3, &[1, 2]))
            .expect("commit first relay snapshot");

        assert_eq!(
            scratch
                .store
                .append_integration_audit_relay_batch(&relay_batch(3, &[1, 2, 3]))
                .expect("commit overlapping relay snapshot"),
            3
        );
        assert_eq!(
            scratch
                .store
                .list_integration_audit()
                .expect("list overlapping relay")
                .into_iter()
                .map(|row| row.method)
                .collect::<Vec<_>>(),
            ["relay/1", "relay/2", "relay/3"]
                .map(Box::<str>::from)
                .to_vec()
        );
    }

    #[test]
    fn relay_loss_marker_is_appended_exactly_once() {
        let scratch = Scratch::make();
        let mut loss_row = row(3, IntegrationAuditOutcome::Denied);
        loss_row.method = "relay/loss".into();
        loss_row.reason = "evictedBeforeRelay".into();
        let mut batch = relay_batch(4, &[4]);
        batch.loss = Some(IntegrationAuditRelayLoss {
            through: 3,
            row: loss_row,
        });

        assert_eq!(
            scratch
                .store
                .append_integration_audit_relay_batch(&batch)
                .expect("commit loss marker"),
            4
        );
        assert_eq!(
            scratch
                .store
                .append_integration_audit_relay_batch(&batch)
                .expect("repeat loss marker"),
            4
        );
        batch.entries.push(relay_entry(5));
        assert_eq!(
            scratch
                .store
                .append_integration_audit_relay_batch(&batch)
                .expect("extend after loss marker"),
            5
        );
        assert_eq!(
            scratch
                .store
                .list_integration_audit()
                .expect("list loss relay")
                .into_iter()
                .map(|row| row.method)
                .collect::<Vec<_>>(),
            ["relay/loss", "relay/4", "relay/5"]
                .map(Box::<str>::from)
                .to_vec()
        );
    }

    #[test]
    fn relay_receipt_survives_store_reopen() {
        let scratch = Scratch::make();
        let path = scratch.root.join("state.redb").expect("database path");
        scratch
            .store
            .append_integration_audit_relay_batch(&relay_batch(5, &[1, 2]))
            .expect("commit before reopen");
        assert!(scratch.store.release(), "release the first store handle");
        let reopened = Store::open(&path).expect("reopen store");

        assert_eq!(
            reopened
                .append_integration_audit_relay_batch(&relay_batch(5, &[1, 2, 3]))
                .expect("continue after reopen"),
            3
        );
        assert_eq!(
            reopened
                .list_integration_audit()
                .expect("list after reopen")
                .into_iter()
                .map(|row| row.method)
                .collect::<Vec<_>>(),
            ["relay/1", "relay/2", "relay/3"]
                .map(Box::<str>::from)
                .to_vec()
        );
    }

    #[test]
    fn a_live_generation_receipt_is_never_displaced_by_other_epochs() {
        let scratch = Scratch::make();
        for epoch in 1..=64 {
            scratch
                .store
                .append_integration_audit_relay_batch(&relay_batch(epoch, &[1]))
                .expect("commit live epoch receipt");
        }
        let error = scratch
            .store
            .append_integration_audit_relay_batch(&relay_batch(65, &[1]))
            .expect_err("a new source cannot displace a live receipt");
        assert!(error.to_string().contains("receipt ceiling is full"));
        let read = scratch.store.db().expect("store engine");
        let transaction = read.begin_read().expect("receipt read");
        let receipts = transaction
            .open_table(INTEGRATION_AUDIT_RECEIPTS)
            .expect("receipt table");
        assert_eq!(
            receipts.len().expect("receipt count"),
            INTEGRATION_AUDIT_RECEIPT_MAX_EPOCHS as u64
        );
        assert!(
            receipts
                .get(IntegrationAuditReceiptKey::from_bytes([1; 16]))
                .expect("read first live receipt")
                .is_some(),
            "the oldest live receipt remains durable"
        );
        assert!(
            receipts
                .get(IntegrationAuditReceiptKey::from_bytes([65; 16]))
                .expect("read refused receipt")
                .is_none(),
            "the rejected source leaves no receipt"
        );
    }

    #[test]
    fn receipt_retention_removes_only_generations_absent_from_the_locator() {
        let scratch = Scratch::make();
        scratch
            .store
            .append_integration_audit_relay_batch(&relay_batch(1, &[1]))
            .expect("commit first live receipt");
        scratch
            .store
            .append_integration_audit_relay_batch(&relay_batch(2, &[1]))
            .expect("commit second live receipt");
        let first_source = relay_batch(1, &[]).source_generation;
        let live = BTreeSet::from([first_source.clone()]);

        assert_eq!(
            scratch
                .store
                .retain_integration_audit_relay_receipts(&live)
                .expect("retain locator generations"),
            1
        );
        assert_eq!(relay_receipt(&scratch.store, 1), Some((1, first_source)));
        assert_eq!(relay_receipt(&scratch.store, 2), None);
    }

    #[test]
    fn a_restarted_generation_replaces_its_prior_process_epoch() {
        let scratch = Scratch::make();
        let first = relay_batch(1, &[1]);
        let mut restarted = relay_batch(2, &[1]);
        restarted.source_generation = first.source_generation.clone();
        scratch
            .store
            .append_integration_audit_relay_batch(&first)
            .expect("commit original process epoch");
        scratch
            .store
            .append_integration_audit_relay_batch(&restarted)
            .expect("commit replacement process epoch");

        assert_eq!(relay_receipt(&scratch.store, 1), None);
        assert_eq!(
            relay_receipt(&scratch.store, 2),
            Some((1, first.source_generation))
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
