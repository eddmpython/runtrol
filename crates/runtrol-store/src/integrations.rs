//! Durable public Runtime integration grants and bounded enrollment decisions.
//!
//! Rows contain public verification keys, exact stable scope strings, approved paths, generations, and operational
//! timestamps. They have no field for private keys, caller input, provider output, events, or conversation content.

use redb::{ReadableDatabase as _, ReadableTable as _};
use runtrol_provider::WallMs;

use crate::error::StoreError;
use crate::open::Store;
use crate::schema::{ENROLLMENTS, EnrollmentKey, INTEGRATIONS, IntegrationKey, SCHEMA_VERSION};

const KEY_BYTES: usize = 32;
const DIGEST_BYTES: usize = 32;
const ROOT_IDENTITY_BYTES: usize = 24;
const MAX_SHORT_BYTES: usize = 512;
const MAX_PATH_BYTES: usize = 32 * 1024;
const MAX_SCOPES: usize = 32;
const MAX_ROOTS: usize = 32;

/// One canonical approved path bound to the exact directory present during approval.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IntegrationRootRow {
    /// Canonical operator-approved path.
    pub path: Box<str>,
    /// Platform filesystem identity. The store treats this as an opaque security value.
    pub identity: [u8; ROOT_IDENTITY_BYTES],
}

/// Durable authority for one public Runtime integration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IntegrationRow {
    /// Ed25519 verification key. Runtime never receives the private key.
    pub public_key: [u8; KEY_BYTES],
    /// Consumer-minted installed-instance label for diagnostics.
    pub client_instance_id: Box<str>,
    /// Operator-facing label approved locally.
    pub label: Box<str>,
    /// Digest of the exact closed enrollment manifest.
    pub manifest_digest: [u8; DIGEST_BYTES],
    /// Stable `AppScope` names. Store has no dependency on protocol authority types.
    pub scopes: Vec<Box<str>>,
    /// Canonical locally approved project paths and their exact filesystem identities.
    pub roots: Vec<IntegrationRootRow>,
    /// Key generation required in signed initialization.
    pub key_generation: u64,
    /// Grant generation checked again before each request.
    pub grant_generation: u64,
    /// Local approval time.
    pub approved_at: WallMs,
    /// Revocation time, if authority has been withdrawn.
    pub revoked_at: Option<WallMs>,
}

/// Terminal or pending enrollment state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EnrollmentState {
    /// Waiting for local approval.
    Pending,
    /// Approved into the named integration.
    Approved(IntegrationKey),
    /// Denied locally.
    Denied,
}

/// One bounded local enrollment request and its decision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnrollmentRow {
    /// Proposed Ed25519 verification key.
    pub public_key: [u8; KEY_BYTES],
    /// Consumer-minted installed-instance label.
    pub client_instance_id: Box<str>,
    /// Safe client display name.
    pub client_name: Box<str>,
    /// Safe client version text.
    pub client_version: Box<str>,
    /// Digest of the exact closed enrollment manifest.
    pub manifest_digest: [u8; DIGEST_BYTES],
    /// Requested stable scope names, locally narrowable only.
    pub scopes: Vec<Box<str>>,
    /// Requested project path strings, canonicalized only during local approval.
    pub roots: Vec<Box<str>>,
    /// Creation time.
    pub created_at: WallMs,
    /// Expiry time.
    pub expires_at: WallMs,
    /// Current local decision.
    pub state: EnrollmentState,
}

impl Store {
    /// Create one pending enrollment only when its key is absent.
    ///
    /// Returns false for an existing key, so retries cannot overwrite a terminal decision.
    ///
    /// # Errors
    ///
    /// [`StoreError::IntegrationCodec`] for fields outside bounded layout, or engine failure.
    pub fn create_enrollment(
        &self,
        key: EnrollmentKey,
        row: &EnrollmentRow,
    ) -> Result<bool, StoreError> {
        let encoded = encode_enrollment(row)?;
        let write = self.begin_durable_write("creating a Runtime integration enrollment")?;
        let inserted;
        {
            let mut table = write
                .open_table(ENROLLMENTS)
                .map_err(|error| engine("opening the integration enrollment table", error))?;
            if table
                .get(key)
                .map_err(|error| engine("checking an integration enrollment", error))?
                .is_some()
            {
                return Ok(false);
            }
            table
                .insert(key, encoded.as_slice())
                .map_err(|error| engine("writing an integration enrollment", error))?;
            inserted = true;
        }
        write
            .commit()
            .map_err(|error| engine("committing an integration enrollment", error))?;
        Ok(inserted)
    }

    /// Read one pending or terminal enrollment.
    ///
    /// # Errors
    ///
    /// Engine or closed codec failure.
    pub fn get_enrollment(&self, key: EnrollmentKey) -> Result<Option<EnrollmentRow>, StoreError> {
        let read = self
            .db()
            .begin_read()
            .map_err(|error| engine("starting an integration enrollment read", error))?;
        let table = match read.open_table(ENROLLMENTS) {
            Ok(table) => table,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(None),
            Err(error) => return Err(engine("opening the integration enrollment table", error)),
        };
        let stored = table
            .get(key)
            .map_err(|error| engine("reading an integration enrollment", error))?;
        stored
            .map(|value| decode_enrollment(value.value()))
            .transpose()
    }

    /// Every enrollment in key order. Damaged authority stops the read instead of being silently omitted.
    ///
    /// # Errors
    ///
    /// Engine or closed codec failure.
    pub fn list_enrollments(&self) -> Result<Vec<(EnrollmentKey, EnrollmentRow)>, StoreError> {
        let read = self
            .db()
            .begin_read()
            .map_err(|error| engine("starting an integration enrollment scan", error))?;
        let table = match read.open_table(ENROLLMENTS) {
            Ok(table) => table,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(Vec::new()),
            Err(error) => return Err(engine("opening the integration enrollment table", error)),
        };
        let mut result = Vec::new();
        let entries = table
            .range(EnrollmentKey::FIRST..=EnrollmentKey::LAST)
            .map_err(|error| engine("scanning integration enrollments", error))?;
        for entry in entries {
            let (key, value) =
                entry.map_err(|error| engine("reading an integration enrollment row", error))?;
            result.push((key.value(), decode_enrollment(value.value())?));
        }
        Ok(result)
    }

    /// Remove decisions whose disclosed watch lifetime has elapsed.
    ///
    /// Returns the number of removed rows. Calling this before admission keeps attacker-created pending state bounded.
    ///
    /// # Errors
    ///
    /// Engine or closed codec failure.
    pub fn purge_expired_enrollments(&self, now: WallMs) -> Result<usize, StoreError> {
        let write = self.begin_durable_write("purging expired Runtime integration enrollments")?;
        let removed;
        {
            let mut table = write
                .open_table(ENROLLMENTS)
                .map_err(|error| engine("opening the integration enrollment table", error))?;
            let entries = table
                .range(EnrollmentKey::FIRST..=EnrollmentKey::LAST)
                .map_err(|error| engine("scanning integration enrollments for expiry", error))?;
            let mut expired = Vec::new();
            for entry in entries {
                let (key, value) = entry.map_err(|error| {
                    engine("reading an integration enrollment for expiry", error)
                })?;
                if decode_enrollment(value.value())?.expires_at < now {
                    expired.push(key.value());
                }
            }
            for key in &expired {
                let previous = table
                    .remove(*key)
                    .map_err(|error| engine("removing an expired integration enrollment", error))?;
                if previous.is_none() {
                    return Err(integration_codec(
                        "enrollment expiry",
                        "a selected row disappeared during one write transaction",
                    ));
                }
            }
            removed = expired.len();
        }
        write
            .commit()
            .map_err(|error| engine("committing integration enrollment expiry", error))?;
        Ok(removed)
    }

    /// Atomically approve a pending request and create its exact narrowed grant.
    ///
    /// # Errors
    ///
    /// Missing or terminal enrollment, bounded codec, or engine failure.
    pub fn approve_enrollment(
        &self,
        enrollment: EnrollmentKey,
        integration: IntegrationKey,
        grant: &IntegrationRow,
    ) -> Result<(), StoreError> {
        let encoded_grant = encode_integration(grant)?;
        let write = self.begin_durable_write("approving a Runtime integration enrollment")?;
        {
            let mut enrollments = write
                .open_table(ENROLLMENTS)
                .map_err(|error| engine("opening the integration enrollment table", error))?;
            let Some(stored) = enrollments
                .get(enrollment)
                .map_err(|error| engine("reading an integration enrollment for approval", error))?
            else {
                return Err(integration_codec(
                    "enrollment",
                    "the pending request does not exist",
                ));
            };
            let mut pending = decode_enrollment(stored.value())?;
            if pending.state != EnrollmentState::Pending {
                return Err(integration_codec(
                    "enrollment",
                    "the request is already terminal",
                ));
            }
            if pending.expires_at < grant.approved_at {
                return Err(integration_codec(
                    "enrollment",
                    "the pending request expired before approval",
                ));
            }
            if grant.public_key != pending.public_key
                || grant.client_instance_id != pending.client_instance_id
                || grant.manifest_digest != pending.manifest_digest
                || !grant
                    .scopes
                    .iter()
                    .all(|scope| pending.scopes.contains(scope))
                || grant.key_generation != 1
                || grant.grant_generation != 1
                || grant.revoked_at.is_some()
            {
                return Err(integration_codec(
                    "integration grant",
                    "the approved grant does not narrow the exact pending proposal",
                ));
            }
            pending.state = EnrollmentState::Approved(integration);
            let pending = encode_enrollment(&pending)?;
            drop(stored);
            enrollments
                .insert(enrollment, pending.as_slice())
                .map_err(|error| engine("recording enrollment approval", error))?;

            let mut integrations = write
                .open_table(INTEGRATIONS)
                .map_err(|error| engine("opening the integration grant table", error))?;
            if integrations
                .get(integration)
                .map_err(|error| engine("checking the integration grant identity", error))?
                .is_some()
            {
                return Err(integration_codec(
                    "integration",
                    "the minted grant already exists",
                ));
            }
            integrations
                .insert(integration, encoded_grant.as_slice())
                .map_err(|error| engine("writing the integration grant", error))?;
        }
        write
            .commit()
            .map_err(|error| engine("committing integration approval", error))
    }

    /// Mark one pending enrollment denied without granting anything.
    ///
    /// # Errors
    ///
    /// Missing or terminal enrollment, codec, or engine failure.
    pub fn deny_enrollment(&self, enrollment: EnrollmentKey) -> Result<(), StoreError> {
        self.change_enrollment_state(enrollment, EnrollmentState::Denied)
    }

    /// Read one integration grant, including revoked state for exact denial behavior.
    ///
    /// # Errors
    ///
    /// Engine or codec failure.
    pub fn get_integration(
        &self,
        key: IntegrationKey,
    ) -> Result<Option<IntegrationRow>, StoreError> {
        let read = self
            .db()
            .begin_read()
            .map_err(|error| engine("starting an integration grant read", error))?;
        let table = match read.open_table(INTEGRATIONS) {
            Ok(table) => table,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(None),
            Err(error) => return Err(engine("opening the integration grant table", error)),
        };
        let stored = table
            .get(key)
            .map_err(|error| engine("reading an integration grant", error))?;
        stored
            .map(|value| decode_integration(value.value()))
            .transpose()
    }

    /// Every integration grant in key order, including revoked rows for local administration.
    ///
    /// # Errors
    ///
    /// Engine or closed codec failure.
    pub fn list_integrations(&self) -> Result<Vec<(IntegrationKey, IntegrationRow)>, StoreError> {
        let read = self
            .db()
            .begin_read()
            .map_err(|error| engine("starting an integration grant scan", error))?;
        let table = match read.open_table(INTEGRATIONS) {
            Ok(table) => table,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(Vec::new()),
            Err(error) => return Err(engine("opening the integration grant table", error)),
        };
        let entries = table
            .range(IntegrationKey::FIRST..=IntegrationKey::LAST)
            .map_err(|error| engine("scanning integration grants", error))?;
        let mut result = Vec::new();
        for entry in entries {
            let (key, value) =
                entry.map_err(|error| engine("reading an integration grant row", error))?;
            result.push((key.value(), decode_integration(value.value())?));
        }
        Ok(result)
    }

    /// Revoke an integration and increment its generation before committing.
    ///
    /// Returns false when no such integration exists.
    ///
    /// # Errors
    ///
    /// Codec, generation exhaustion, or engine failure.
    pub fn revoke_integration(
        &self,
        key: IntegrationKey,
        revoked_at: WallMs,
    ) -> Result<bool, StoreError> {
        let write = self.begin_durable_write("revoking a Runtime integration")?;
        {
            let mut table = write
                .open_table(INTEGRATIONS)
                .map_err(|error| engine("opening the integration grant table", error))?;
            let Some(stored) = table
                .get(key)
                .map_err(|error| engine("reading an integration for revocation", error))?
            else {
                return Ok(false);
            };
            let mut row = decode_integration(stored.value())?;
            row.grant_generation = row
                .grant_generation
                .checked_add(1)
                .ok_or_else(|| integration_codec("grant generation", "it is exhausted"))?;
            row.revoked_at = Some(revoked_at);
            let encoded = encode_integration(&row)?;
            drop(stored);
            table
                .insert(key, encoded.as_slice())
                .map_err(|error| engine("writing integration revocation", error))?;
        }
        write
            .commit()
            .map_err(|error| engine("committing integration revocation", error))?;
        Ok(true)
    }

    fn change_enrollment_state(
        &self,
        key: EnrollmentKey,
        state: EnrollmentState,
    ) -> Result<(), StoreError> {
        let write = self.begin_durable_write("deciding a Runtime integration enrollment")?;
        {
            let mut table = write
                .open_table(ENROLLMENTS)
                .map_err(|error| engine("opening the integration enrollment table", error))?;
            let Some(stored) = table
                .get(key)
                .map_err(|error| engine("reading an integration enrollment for decision", error))?
            else {
                return Err(integration_codec(
                    "enrollment",
                    "the pending request does not exist",
                ));
            };
            let mut row = decode_enrollment(stored.value())?;
            if row.state != EnrollmentState::Pending {
                return Err(integration_codec(
                    "enrollment",
                    "the request is already terminal",
                ));
            }
            row.state = state;
            let encoded = encode_enrollment(&row)?;
            drop(stored);
            table
                .insert(key, encoded.as_slice())
                .map_err(|error| engine("writing an integration enrollment decision", error))?;
        }
        write
            .commit()
            .map_err(|error| engine("committing an integration enrollment decision", error))
    }
}

fn encode_integration(row: &IntegrationRow) -> Result<Vec<u8>, StoreError> {
    let mut out = Vec::new();
    out.push(SCHEMA_VERSION);
    out.extend_from_slice(&row.public_key);
    write_text(
        &mut out,
        "client instance",
        &row.client_instance_id,
        MAX_SHORT_BYTES,
    )?;
    write_text(&mut out, "integration label", &row.label, MAX_SHORT_BYTES)?;
    out.extend_from_slice(&row.manifest_digest);
    write_strings(&mut out, "scope", &row.scopes, MAX_SCOPES, MAX_SHORT_BYTES)?;
    write_integration_roots(&mut out, &row.roots)?;
    out.extend_from_slice(&row.key_generation.to_le_bytes());
    out.extend_from_slice(&row.grant_generation.to_le_bytes());
    out.extend_from_slice(&row.approved_at.as_millis().to_le_bytes());
    out.extend_from_slice(
        &row.revoked_at
            .map_or(u64::MAX, WallMs::as_millis)
            .to_le_bytes(),
    );
    Ok(out)
}

fn decode_integration(bytes: &[u8]) -> Result<IntegrationRow, StoreError> {
    let mut cursor = Cursor::new(bytes);
    cursor.version()?;
    let row = IntegrationRow {
        public_key: cursor.fixed("public key")?,
        client_instance_id: cursor.text("client instance", MAX_SHORT_BYTES)?.into(),
        label: cursor.text("integration label", MAX_SHORT_BYTES)?.into(),
        manifest_digest: cursor.fixed("manifest digest")?,
        scopes: cursor.strings("scope", MAX_SCOPES, MAX_SHORT_BYTES)?,
        roots: cursor.integration_roots()?,
        key_generation: cursor.u64("key generation")?,
        grant_generation: cursor.u64("grant generation")?,
        approved_at: WallMs::from_millis(cursor.u64("approved at")?),
        revoked_at: optional_wall(cursor.u64("revoked at")?),
    };
    cursor.finish()?;
    Ok(row)
}

fn encode_enrollment(row: &EnrollmentRow) -> Result<Vec<u8>, StoreError> {
    let mut out = Vec::new();
    out.push(SCHEMA_VERSION);
    out.extend_from_slice(&row.public_key);
    write_text(
        &mut out,
        "client instance",
        &row.client_instance_id,
        MAX_SHORT_BYTES,
    )?;
    write_text(&mut out, "client name", &row.client_name, MAX_SHORT_BYTES)?;
    write_text(
        &mut out,
        "client version",
        &row.client_version,
        MAX_SHORT_BYTES,
    )?;
    out.extend_from_slice(&row.manifest_digest);
    write_strings(&mut out, "scope", &row.scopes, MAX_SCOPES, MAX_SHORT_BYTES)?;
    write_strings(&mut out, "root", &row.roots, MAX_ROOTS, MAX_PATH_BYTES)?;
    out.extend_from_slice(&row.created_at.as_millis().to_le_bytes());
    out.extend_from_slice(&row.expires_at.as_millis().to_le_bytes());
    match row.state {
        EnrollmentState::Pending => out.push(0),
        EnrollmentState::Approved(integration) => {
            out.push(1);
            out.extend_from_slice(&integration.to_bytes());
        }
        EnrollmentState::Denied => out.push(2),
    }
    Ok(out)
}

fn decode_enrollment(bytes: &[u8]) -> Result<EnrollmentRow, StoreError> {
    let mut cursor = Cursor::new(bytes);
    cursor.version()?;
    let public_key = cursor.fixed("public key")?;
    let client_instance_id = cursor.text("client instance", MAX_SHORT_BYTES)?.into();
    let client_name = cursor.text("client name", MAX_SHORT_BYTES)?.into();
    let client_version = cursor.text("client version", MAX_SHORT_BYTES)?.into();
    let manifest_digest = cursor.fixed("manifest digest")?;
    let scopes = cursor.strings("scope", MAX_SCOPES, MAX_SHORT_BYTES)?;
    let roots = cursor.strings("root", MAX_ROOTS, MAX_PATH_BYTES)?;
    let created_at = WallMs::from_millis(cursor.u64("created at")?);
    let expires_at = WallMs::from_millis(cursor.u64("expires at")?);
    let state = match cursor.byte("enrollment state")? {
        0 => EnrollmentState::Pending,
        1 => EnrollmentState::Approved(IntegrationKey::from_bytes(cursor.fixed("integration")?)),
        2 => EnrollmentState::Denied,
        _ => {
            return Err(integration_codec(
                "enrollment state",
                "it is not recognized",
            ));
        }
    };
    cursor.finish()?;
    Ok(EnrollmentRow {
        public_key,
        client_instance_id,
        client_name,
        client_version,
        manifest_digest,
        scopes,
        roots,
        created_at,
        expires_at,
        state,
    })
}

fn write_text(
    out: &mut Vec<u8>,
    field: &'static str,
    text: &str,
    max: usize,
) -> Result<(), StoreError> {
    if text.is_empty() || text.len() > max {
        return Err(integration_codec(
            field,
            "it is empty or exceeds its byte limit",
        ));
    }
    let length = u32::try_from(text.len())
        .map_err(|_| integration_codec(field, "its length cannot be represented"))?;
    out.extend_from_slice(&length.to_le_bytes());
    out.extend_from_slice(text.as_bytes());
    Ok(())
}

fn write_strings(
    out: &mut Vec<u8>,
    field: &'static str,
    values: &[Box<str>],
    max_count: usize,
    max_bytes: usize,
) -> Result<(), StoreError> {
    if values.len() > max_count {
        return Err(integration_codec(field, "there are too many values"));
    }
    out.extend_from_slice(
        &u16::try_from(values.len())
            .map_err(|_| integration_codec(field, "the value count cannot be represented"))?
            .to_le_bytes(),
    );
    for value in values {
        write_text(out, field, value, max_bytes)?;
    }
    Ok(())
}

fn write_integration_roots(
    out: &mut Vec<u8>,
    roots: &[IntegrationRootRow],
) -> Result<(), StoreError> {
    if roots.len() > MAX_ROOTS {
        return Err(integration_codec("root", "there are too many values"));
    }
    out.extend_from_slice(
        &u16::try_from(roots.len())
            .map_err(|_| integration_codec("root", "the value count cannot be represented"))?
            .to_le_bytes(),
    );
    for root in roots {
        write_text(out, "root", &root.path, MAX_PATH_BYTES)?;
        out.extend_from_slice(&root.identity);
    }
    Ok(())
}

fn optional_wall(value: u64) -> Option<WallMs> {
    (value != u64::MAX).then(|| WallMs::from_millis(value))
}

struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Cursor<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, at: 0 }
    }

    fn version(&mut self) -> Result<(), StoreError> {
        if self.byte("row version")? != SCHEMA_VERSION {
            return Err(integration_codec(
                "row version",
                "it belongs to another schema",
            ));
        }
        Ok(())
    }

    fn finish(&self) -> Result<(), StoreError> {
        if self.at != self.bytes.len() {
            return Err(integration_codec("end of row", "trailing bytes remain"));
        }
        Ok(())
    }

    fn byte(&mut self, field: &'static str) -> Result<u8, StoreError> {
        let value = self
            .bytes
            .get(self.at)
            .copied()
            .ok_or_else(|| integration_codec(field, "the row ended early"))?;
        self.at = self.at.saturating_add(1);
        Ok(value)
    }

    fn fixed<const N: usize>(&mut self, field: &'static str) -> Result<[u8; N], StoreError> {
        let bytes = self.take(field, N)?;
        bytes
            .try_into()
            .map_err(|_| integration_codec(field, "the fixed field has the wrong length"))
    }

    fn u16(&mut self, field: &'static str) -> Result<u16, StoreError> {
        Ok(u16::from_le_bytes(self.fixed(field)?))
    }

    fn u32(&mut self, field: &'static str) -> Result<u32, StoreError> {
        Ok(u32::from_le_bytes(self.fixed(field)?))
    }

    fn u64(&mut self, field: &'static str) -> Result<u64, StoreError> {
        Ok(u64::from_le_bytes(self.fixed(field)?))
    }

    fn text(&mut self, field: &'static str, max: usize) -> Result<&'a str, StoreError> {
        let length = usize::try_from(self.u32(field)?)
            .map_err(|_| integration_codec(field, "its length does not fit usize"))?;
        if length == 0 || length > max {
            return Err(integration_codec(
                field,
                "it is empty or exceeds its byte limit",
            ));
        }
        std::str::from_utf8(self.take(field, length)?)
            .map_err(|_| integration_codec(field, "it is not UTF-8"))
    }

    fn strings(
        &mut self,
        field: &'static str,
        max_count: usize,
        max_bytes: usize,
    ) -> Result<Vec<Box<str>>, StoreError> {
        let count = usize::from(self.u16(field)?);
        if count > max_count {
            return Err(integration_codec(field, "there are too many values"));
        }
        (0..count)
            .map(|_| self.text(field, max_bytes).map(Into::into))
            .collect()
    }

    fn integration_roots(&mut self) -> Result<Vec<IntegrationRootRow>, StoreError> {
        let count = usize::from(self.u16("root")?);
        if count > MAX_ROOTS {
            return Err(integration_codec("root", "there are too many values"));
        }
        (0..count)
            .map(|_| {
                Ok(IntegrationRootRow {
                    path: self.text("root", MAX_PATH_BYTES)?.into(),
                    identity: self.fixed("root identity")?,
                })
            })
            .collect()
    }

    fn take(&mut self, field: &'static str, count: usize) -> Result<&'a [u8], StoreError> {
        let end = self
            .at
            .checked_add(count)
            .ok_or_else(|| integration_codec(field, "its length overflows the row"))?;
        let result = self
            .bytes
            .get(self.at..end)
            .ok_or_else(|| integration_codec(field, "the row ended early"))?;
        self.at = end;
        Ok(result)
    }
}

fn integration_codec(field: &'static str, why: &'static str) -> StoreError {
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
    use super::*;
    use runtrol_provider::AbsPath;

    struct Scratch {
        root: AbsPath,
        store: Store,
    }

    impl Scratch {
        fn make(name: &str) -> Self {
            let base = std::env::temp_dir().join(format!("runtrol-integrations-{name}"));
            drop(std::fs::remove_dir_all(&base));
            std::fs::create_dir_all(&base).expect("create scratch");
            let root = AbsPath::canonicalize(base.to_str().expect("UTF-8 scratch")).expect("root");
            let path = root.join("state.redb").expect("database path");
            let store = Store::open(&path).expect("open store");
            Self { root, store }
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            if let Err(error) = std::fs::remove_dir_all(self.root.as_std_path()) {
                eprintln!("could not clean integration scratch: {error}");
            }
        }
    }

    fn enrollment() -> EnrollmentRow {
        EnrollmentRow {
            public_key: [1; 32],
            client_instance_id: "fixture-instance".into(),
            client_name: "Fixture".into(),
            client_version: "1.0.0".into(),
            manifest_digest: [2; 32],
            scopes: vec!["provider.read".into()],
            roots: vec!["C:/work".into()],
            created_at: WallMs::from_millis(1),
            expires_at: WallMs::from_millis(4),
            state: EnrollmentState::Pending,
        }
    }

    #[test]
    fn enrollment_approval_and_revocation_are_durable_and_exact() {
        let scratch = Scratch::make("lifecycle");
        let pending = EnrollmentKey::from_bytes([3; 16]);
        let integration = IntegrationKey::from_bytes([4; 16]);
        assert!(
            scratch
                .store
                .create_enrollment(pending, &enrollment())
                .expect("create")
        );
        assert!(
            !scratch
                .store
                .create_enrollment(pending, &enrollment())
                .expect("duplicate")
        );
        let grant = IntegrationRow {
            public_key: [1; 32],
            client_instance_id: "fixture-instance".into(),
            label: "Fixture".into(),
            manifest_digest: [2; 32],
            scopes: vec!["provider.read".into()],
            roots: vec![IntegrationRootRow {
                path: "C:/work".into(),
                identity: [7; ROOT_IDENTITY_BYTES],
            }],
            key_generation: 1,
            grant_generation: 1,
            approved_at: WallMs::from_millis(3),
            revoked_at: None,
        };
        scratch
            .store
            .approve_enrollment(pending, integration, &grant)
            .expect("approve");
        assert_eq!(
            scratch
                .store
                .get_enrollment(pending)
                .expect("pending")
                .map(|one| one.state),
            Some(EnrollmentState::Approved(integration))
        );
        assert_eq!(
            scratch.store.get_integration(integration).expect("grant"),
            Some(grant)
        );
        assert!(
            scratch
                .store
                .revoke_integration(integration, WallMs::from_millis(5))
                .expect("revoke")
        );
        let revoked = scratch
            .store
            .get_integration(integration)
            .expect("read revoked")
            .expect("exists");
        assert_eq!(revoked.grant_generation, 2);
        assert_eq!(revoked.revoked_at, Some(WallMs::from_millis(5)));
    }

    #[test]
    fn expired_enrollments_are_removed_without_touching_live_decisions() {
        let scratch = Scratch::make("expiry");
        let expired = EnrollmentKey::from_bytes([5; 16]);
        let live = EnrollmentKey::from_bytes([6; 16]);
        let mut expired_row = enrollment();
        expired_row.expires_at = WallMs::from_millis(10);
        let mut live_row = enrollment();
        live_row.expires_at = WallMs::from_millis(30);
        assert!(
            scratch
                .store
                .create_enrollment(expired, &expired_row)
                .expect("expired fixture")
        );
        assert!(
            scratch
                .store
                .create_enrollment(live, &live_row)
                .expect("live fixture")
        );

        assert_eq!(
            scratch
                .store
                .purge_expired_enrollments(WallMs::from_millis(20))
                .expect("purge"),
            1
        );
        assert_eq!(
            scratch.store.get_enrollment(expired).expect("expired"),
            None
        );
        assert!(scratch.store.get_enrollment(live).expect("live").is_some());
    }

    #[test]
    fn damaged_authority_is_never_silently_ignored() {
        assert!(decode_enrollment(&[SCHEMA_VERSION, 1]).is_err());
        assert!(decode_integration(&[SCHEMA_VERSION, 1]).is_err());
    }
}
