//! Durable public Runtime integration grants and bounded enrollment decisions.
//!
//! Rows contain public verification keys, exact stable scope strings, approved paths, generations, and operational
//! timestamps. They have no field for private keys, caller input, provider output, events, or conversation content.

use redb::{ReadableDatabase as _, ReadableTable as _};
use runtrol_provider::WallMs;

use crate::error::StoreError;
use crate::open::Store;
use crate::schema::{
    ENROLLMENTS, EnrollmentKey, INTEGRATION_AUTHORITY_STATE, INTEGRATION_TOMBSTONES, INTEGRATIONS,
    IntegrationKey, SCHEMA_VERSION,
};

const KEY_BYTES: usize = 32;
const DIGEST_BYTES: usize = 32;
const ROOT_IDENTITY_BYTES: usize = 24;
const MAX_SHORT_BYTES: usize = 512;
const MAX_PATH_BYTES: usize = 32 * 1024;
const MAX_SCOPES: usize = 32;
const MAX_ROOTS: usize = 32;
const AUTHORITY_STATE_KEY: &str = "authority";
// A fixed 64 KiB guard is 0.32 percent of the 20 MiB idle contract and avoids heap growth with revocation history.
const REVOCATION_GUARD_BYTES: usize = 64 * 1024;
const REVOCATION_GUARD_HASHES: u64 = 4;

/// Maximum number of active public Runtime integrations.
///
/// Integrations are operator-approved machine principals, not sessions. Sixty-four leaves substantial room for
/// real tools while keeping the resident projection and every authority scan small under the 20 MiB idle contract.
pub const INTEGRATION_ACTIVE_MAX_ROWS: usize = 64;

/// Maximum aggregate canonical bytes of active public Runtime authority.
///
/// This is the sum of the exact encoded integration values. It bounds path-heavy grants independently of row count.
pub const INTEGRATION_AUTHORITY_MAX_BYTES: usize = 4 * 1024 * 1024;

/// Maximum number of exact compact revocation tombstones retained on disk and in memory.
///
/// Older exact tombstones are folded into a fixed revocation guard. The guard may reject a fresh random identity as
/// a false positive, but it never permits a retired identity to be reused.
pub const INTEGRATION_REVOKED_MAX_ROWS: usize = 256;

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

/// Compact durable proof that one integration identity was retired.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IntegrationRevocation {
    /// Last committed public-key generation.
    pub key_generation: u64,
    /// Revocation generation. It is newer than the active grant it retired.
    pub grant_generation: u64,
    /// Local revocation time for diagnostics.
    pub revoked_at: WallMs,
    /// Monotonic durable order used for deterministic bounded retention despite clock rollback.
    pub order: u64,
}

/// Fixed-size, false-positive-only guard for every integration identity ever retired.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IntegrationRevocationGuard {
    bits: Box<[u8]>,
}

impl IntegrationRevocationGuard {
    /// Create an empty fixed guard for a new authority projection.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            bits: vec![0; REVOCATION_GUARD_BYTES].into_boxed_slice(),
        }
    }

    /// Whether this identity is permanently unavailable after a revocation.
    #[must_use]
    pub fn contains(&self, key: IntegrationKey) -> bool {
        guard_bits(key).all(|bit| {
            self.bits
                .get(bit / 8)
                .is_some_and(|byte| byte & (1 << (bit % 8)) != 0)
        })
    }

    /// Permanently retire one identity. Bits only move from zero to one.
    #[must_use]
    pub fn insert(&mut self, key: IntegrationKey) -> bool {
        for bit in guard_bits(key) {
            let Some(byte) = self.bits.get_mut(bit / 8) else {
                return false;
            };
            *byte |= 1 << (bit % 8);
        }
        true
    }
}

/// Bounded durable authority restored before a public listener starts.
pub struct IntegrationAuthoritySnapshot {
    active: Vec<(IntegrationKey, IntegrationRow)>,
    revoked: Vec<(IntegrationKey, IntegrationRevocation)>,
    revocation_guard: IntegrationRevocationGuard,
    active_bytes: usize,
}

/// Owned bounded fields used to construct a read-optimized authority projection.
pub struct IntegrationAuthorityParts {
    /// Exact active integration rows.
    pub active: Vec<(IntegrationKey, IntegrationRow)>,
    /// Recent exact compact revocation tombstones.
    pub revoked: Vec<(IntegrationKey, IntegrationRevocation)>,
    /// Fixed false-positive-only guard for all retired identities.
    pub revocation_guard: IntegrationRevocationGuard,
    /// Aggregate canonical encoded active authority bytes.
    pub active_bytes: usize,
}

impl IntegrationAuthoritySnapshot {
    /// Consume the snapshot into active rows, recent exact tombstones, the permanent guard, and encoded bytes.
    #[must_use]
    pub fn into_parts(self) -> IntegrationAuthorityParts {
        IntegrationAuthorityParts {
            active: self.active,
            revoked: self.revoked,
            revocation_guard: self.revocation_guard,
            active_bytes: self.active_bytes,
        }
    }
}

/// Durable result of one exact integration key rotation attempt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IntegrationKeyRotation {
    /// The old generation was current and the replacement committed.
    Rotated(IntegrationRow),
    /// The same replacement was already committed at the next generation.
    Replayed(IntegrationRow),
    /// The current generation or key does not match this request.
    Conflict,
    /// The integration identity does not exist.
    Missing,
    /// The integration was revoked before rotation.
    Revoked,
}

/// Durable result of one exact local integration authority replacement.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IntegrationGrantChange {
    /// The expected generation was current and the replacement committed.
    Changed(IntegrationRow),
    /// The supplied authority already matches the current row.
    Unchanged(IntegrationRow),
    /// The current generation no longer matches the local review.
    Conflict,
    /// The integration identity does not exist.
    Missing,
    /// The integration was revoked before the replacement.
    Revoked,
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

struct AuthorityState {
    active_rows: usize,
    active_bytes: usize,
    next_revocation_order: u64,
    revocation_guard: IntegrationRevocationGuard,
}

struct AuthorityData {
    state: AuthorityState,
    active: Vec<(IntegrationKey, IntegrationRow)>,
    revoked: Vec<(IntegrationKey, IntegrationRevocation)>,
}

struct ScannedIntegrations {
    active: Vec<(IntegrationKey, IntegrationRow)>,
    legacy_revoked: Vec<(IntegrationKey, IntegrationRevocation)>,
    active_bytes: usize,
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
            .db()?
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
            .db()?
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
        let mut authority = normalize_authority(&write)?;
        if authority.active.iter().any(|(key, _)| *key == integration)
            || authority.state.revocation_guard.contains(integration)
        {
            return Err(StoreError::IntegrationIdentityUnavailable);
        }
        let next_rows = authority.state.active_rows.saturating_add(1);
        let next_bytes = authority
            .state
            .active_bytes
            .saturating_add(encoded_grant.len());
        ensure_authority_capacity(next_rows, next_bytes)?;
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
        authority.state.active_rows = next_rows;
        authority.state.active_bytes = next_bytes;
        write_authority_state(&write, &authority.state)?;
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

    /// Read one active integration grant.
    ///
    /// # Errors
    ///
    /// Engine or codec failure.
    pub fn get_integration(
        &self,
        key: IntegrationKey,
    ) -> Result<Option<IntegrationRow>, StoreError> {
        let read = self
            .db()?
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

    /// Restore bounded active authority and compact revocation state in one durable normalization transaction.
    ///
    /// Legacy revoked full rows are compacted here. Legacy active authority above either fixed ceiling fails closed,
    /// and an existing singleton usage row must exactly match the canonical active table.
    ///
    /// # Errors
    ///
    /// Engine, authority capacity, or closed codec failure.
    pub fn load_integration_authority(&self) -> Result<IntegrationAuthoritySnapshot, StoreError> {
        let write = self.begin_durable_write("restoring Runtime integration authority")?;
        let data = normalize_authority(&write)?;
        write_authority_state(&write, &data.state)?;
        write
            .commit()
            .map_err(|error| engine("committing Runtime integration authority restore", error))?;
        Ok(IntegrationAuthoritySnapshot {
            active: data.active,
            revoked: data.revoked,
            revocation_guard: data.state.revocation_guard,
            active_bytes: data.state.active_bytes,
        })
    }

    /// Every active integration grant in key order.
    ///
    /// # Errors
    ///
    /// Engine, authority capacity, or closed codec failure.
    pub fn list_integrations(&self) -> Result<Vec<(IntegrationKey, IntegrationRow)>, StoreError> {
        Ok(self.load_integration_authority()?.active)
    }

    /// Revoke an integration, compact it, and increment its generation before committing.
    ///
    /// Returns the exact committed tombstone, or `None` when no active integration exists.
    ///
    /// # Errors
    ///
    /// Codec, generation exhaustion, or engine failure.
    pub fn revoke_integration(
        &self,
        key: IntegrationKey,
        revoked_at: WallMs,
    ) -> Result<Option<IntegrationRevocation>, StoreError> {
        let write = self.begin_durable_write("revoking a Runtime integration")?;
        let mut authority = normalize_authority(&write)?;
        let revoked;
        {
            let mut table = write
                .open_table(INTEGRATIONS)
                .map_err(|error| engine("opening the integration grant table", error))?;
            let Some(stored) = table
                .get(key)
                .map_err(|error| engine("reading an integration for revocation", error))?
            else {
                return Ok(None);
            };
            let mut row = decode_integration(stored.value())?;
            if row.revoked_at.is_some() {
                return Err(integration_codec(
                    "integration revocation",
                    "an active authority table contained a revoked row after normalization",
                ));
            }
            let active_bytes = stored.value().len();
            row.grant_generation = row
                .grant_generation
                .checked_add(1)
                .ok_or_else(|| integration_codec("grant generation", "it is exhausted"))?;
            let order = authority
                .state
                .next_revocation_order
                .checked_add(1)
                .ok_or_else(|| integration_codec("revocation order", "it is exhausted"))?;
            revoked = IntegrationRevocation {
                key_generation: row.key_generation,
                grant_generation: row.grant_generation,
                revoked_at,
                order,
            };
            drop(stored);
            let removed = table
                .remove(key)
                .map_err(|error| engine("removing revoked active integration authority", error))?;
            if removed.is_none() {
                return Err(integration_codec(
                    "integration revocation",
                    "the selected active row disappeared during one write transaction",
                ));
            }
            authority.state.active_rows =
                authority.state.active_rows.checked_sub(1).ok_or_else(|| {
                    integration_codec("authority usage", "active row count underflowed")
                })?;
            authority.state.active_bytes = authority
                .state
                .active_bytes
                .checked_sub(active_bytes)
                .ok_or_else(|| {
                integration_codec("authority usage", "active bytes underflowed")
            })?;
            authority.state.next_revocation_order = order;
            if !authority.state.revocation_guard.insert(key) {
                return Err(integration_codec(
                    "revocation guard",
                    "the fixed guard could not address its own storage",
                ));
            }
        }
        {
            let encoded = encode_revocation(revoked);
            let mut tombstones = write
                .open_table(INTEGRATION_TOMBSTONES)
                .map_err(|error| engine("opening integration revocation tombstones", error))?;
            tombstones
                .insert(key, encoded.as_slice())
                .map_err(|error| engine("writing integration revocation tombstone", error))?;
            authority.revoked.push((key, revoked));
            authority
                .revoked
                .sort_by_key(|(key, row)| (row.order, *key));
            while authority.revoked.len() > INTEGRATION_REVOKED_MAX_ROWS {
                let (expired, _) = authority.revoked.remove(0);
                let removed = tombstones
                    .remove(expired)
                    .map_err(|error| engine("pruning integration revocation tombstone", error))?;
                if removed.is_none() {
                    return Err(integration_codec(
                        "integration tombstone retention",
                        "the selected tombstone disappeared during one write transaction",
                    ));
                }
            }
        }
        write_authority_state(&write, &authority.state)?;
        write
            .commit()
            .map_err(|error| engine("committing integration revocation", error))?;
        Ok(Some(revoked))
    }

    /// Atomically replace one active integration's exact scopes and roots.
    ///
    /// # Errors
    ///
    /// Codec, generation exhaustion, or engine failure.
    pub fn change_integration_grant(
        &self,
        key: IntegrationKey,
        expected_generation: u64,
        scopes: Vec<Box<str>>,
        roots: Vec<IntegrationRootRow>,
    ) -> Result<IntegrationGrantChange, StoreError> {
        let write = self.begin_durable_write("changing a Runtime integration grant")?;
        let mut authority = normalize_authority(&write)?;
        let outcome;
        {
            let mut table = write
                .open_table(INTEGRATIONS)
                .map_err(|error| engine("opening the integration grant table", error))?;
            let Some(stored) = table
                .get(key)
                .map_err(|error| engine("reading an integration for grant change", error))?
            else {
                return Ok(IntegrationGrantChange::Missing);
            };
            let mut row = decode_integration(stored.value())?;
            if row.revoked_at.is_some() {
                return Ok(IntegrationGrantChange::Revoked);
            }
            if row.grant_generation != expected_generation {
                return Ok(IntegrationGrantChange::Conflict);
            }
            if row.scopes == scopes && row.roots == roots {
                return Ok(IntegrationGrantChange::Unchanged(row));
            }
            let previous_bytes = stored.value().len();
            row.grant_generation = row
                .grant_generation
                .checked_add(1)
                .ok_or_else(|| integration_codec("grant generation", "it is exhausted"))?;
            row.scopes = scopes;
            row.roots = roots;
            let encoded = encode_integration(&row)?;
            let next_bytes = authority
                .state
                .active_bytes
                .checked_sub(previous_bytes)
                .and_then(|bytes| bytes.checked_add(encoded.len()))
                .ok_or_else(|| integration_codec("authority usage", "active bytes overflowed"))?;
            ensure_authority_capacity(authority.state.active_rows, next_bytes)?;
            drop(stored);
            table
                .insert(key, encoded.as_slice())
                .map_err(|error| engine("writing integration grant change", error))?;
            authority.state.active_bytes = next_bytes;
            outcome = IntegrationGrantChange::Changed(row);
        }
        write_authority_state(&write, &authority.state)?;
        write
            .commit()
            .map_err(|error| engine("committing integration grant change", error))?;
        Ok(outcome)
    }

    /// Atomically replace one exact current public key and increment its key generation.
    ///
    /// A row already holding the proposed key at exactly the next generation is a successful replay.
    ///
    /// # Errors
    ///
    /// Codec, generation exhaustion, or engine failure.
    pub fn rotate_integration_key(
        &self,
        key: IntegrationKey,
        expected_generation: u64,
        new_public_key: [u8; KEY_BYTES],
    ) -> Result<IntegrationKeyRotation, StoreError> {
        let write = self.begin_durable_write("rotating a Runtime integration key")?;
        let mut authority = normalize_authority(&write)?;
        let outcome;
        {
            let mut table = write
                .open_table(INTEGRATIONS)
                .map_err(|error| engine("opening the integration grant table", error))?;
            let Some(stored) = table
                .get(key)
                .map_err(|error| engine("reading an integration for key rotation", error))?
            else {
                return Ok(IntegrationKeyRotation::Missing);
            };
            let mut row = decode_integration(stored.value())?;
            if row.revoked_at.is_some() {
                return Ok(IntegrationKeyRotation::Revoked);
            }
            if row.key_generation == expected_generation.saturating_add(1)
                && row.public_key == new_public_key
            {
                return Ok(IntegrationKeyRotation::Replayed(row));
            }
            if row.key_generation != expected_generation || row.public_key == new_public_key {
                return Ok(IntegrationKeyRotation::Conflict);
            }
            let previous_bytes = stored.value().len();
            row.key_generation = row
                .key_generation
                .checked_add(1)
                .ok_or_else(|| integration_codec("key generation", "it is exhausted"))?;
            row.public_key = new_public_key;
            let encoded = encode_integration(&row)?;
            let next_bytes = authority
                .state
                .active_bytes
                .checked_sub(previous_bytes)
                .and_then(|bytes| bytes.checked_add(encoded.len()))
                .ok_or_else(|| integration_codec("authority usage", "active bytes overflowed"))?;
            ensure_authority_capacity(authority.state.active_rows, next_bytes)?;
            drop(stored);
            table
                .insert(key, encoded.as_slice())
                .map_err(|error| engine("writing integration key rotation", error))?;
            authority.state.active_bytes = next_bytes;
            outcome = IntegrationKeyRotation::Rotated(row);
        }
        write_authority_state(&write, &authority.state)?;
        write
            .commit()
            .map_err(|error| engine("committing integration key rotation", error))?;
        Ok(outcome)
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

/// Canonical encoded bytes one active integration contributes to the authority budget.
///
/// # Errors
///
/// [`StoreError::IntegrationCodec`] when the row is outside the closed field bounds.
pub fn integration_authority_bytes(row: &IntegrationRow) -> Result<usize, StoreError> {
    encode_integration(row).map(|encoded| encoded.len())
}

fn normalize_authority(write: &redb::WriteTransaction) -> Result<AuthorityData, StoreError> {
    let ScannedIntegrations {
        active,
        legacy_revoked,
        active_bytes,
    } = scan_integrations(write)?;
    let mut revoked = scan_tombstones(write, legacy_revoked.len(), &active)?;
    let mut state = restore_authority_state(write, active.len(), active_bytes, &revoked)?;
    migrate_legacy_revocations(write, legacy_revoked, &mut revoked, &mut state)?;
    Ok(AuthorityData {
        state,
        active,
        revoked,
    })
}

fn scan_integrations(write: &redb::WriteTransaction) -> Result<ScannedIntegrations, StoreError> {
    let mut active = Vec::new();
    let mut legacy_revoked = Vec::new();
    let mut active_bytes = 0usize;
    let table = write
        .open_table(INTEGRATIONS)
        .map_err(|error| engine("opening the integration grant table", error))?;
    let entries = table
        .range(IntegrationKey::FIRST..=IntegrationKey::LAST)
        .map_err(|error| engine("scanning integration grants", error))?;
    for entry in entries {
        let (key, value) =
            entry.map_err(|error| engine("reading an integration grant row", error))?;
        let row = decode_integration(value.value())?;
        if row.revoked_at.is_some() {
            if legacy_revoked.len() >= INTEGRATION_REVOKED_MAX_ROWS {
                return Err(integration_codec(
                    "integration tombstone retention",
                    "legacy revoked rows exceed the fixed migration limit",
                ));
            }
            legacy_revoked.push((
                key.value(),
                IntegrationRevocation {
                    key_generation: row.key_generation,
                    grant_generation: row.grant_generation,
                    revoked_at: row.revoked_at.ok_or_else(|| {
                        integration_codec("revoked at", "a legacy revoked row lost its timestamp")
                    })?,
                    order: 0,
                },
            ));
        } else {
            active_bytes = active_bytes
                .checked_add(value.value().len())
                .ok_or_else(|| integration_codec("authority usage", "active bytes overflowed"))?;
            ensure_authority_capacity(active.len().saturating_add(1), active_bytes)?;
            active.push((key.value(), row));
        }
    }
    Ok(ScannedIntegrations {
        active,
        legacy_revoked,
        active_bytes,
    })
}

fn scan_tombstones(
    write: &redb::WriteTransaction,
    legacy_count: usize,
    active: &[(IntegrationKey, IntegrationRow)],
) -> Result<Vec<(IntegrationKey, IntegrationRevocation)>, StoreError> {
    let mut revoked = Vec::new();
    let tombstones = write
        .open_table(INTEGRATION_TOMBSTONES)
        .map_err(|error| engine("opening integration revocation tombstones", error))?;
    let entries = tombstones
        .range(IntegrationKey::FIRST..=IntegrationKey::LAST)
        .map_err(|error| engine("scanning integration revocation tombstones", error))?;
    for entry in entries {
        let (key, value) =
            entry.map_err(|error| engine("reading an integration revocation tombstone", error))?;
        if revoked.len() >= INTEGRATION_REVOKED_MAX_ROWS.saturating_sub(legacy_count) {
            return Err(integration_codec(
                "integration tombstone retention",
                "canonical and legacy tombstones exceed the fixed migration limit",
            ));
        }
        if active
            .iter()
            .any(|(active_key, _)| *active_key == key.value())
        {
            return Err(integration_codec(
                "integration authority",
                "one identity is both active and revoked",
            ));
        }
        revoked.push((key.value(), decode_revocation(value.value())?));
    }
    revoked.sort_by_key(|(key, row)| (row.order, *key));
    Ok(revoked)
}

fn restore_authority_state(
    write: &redb::WriteTransaction,
    active_rows: usize,
    active_bytes: usize,
    revoked: &[(IntegrationKey, IntegrationRevocation)],
) -> Result<AuthorityState, StoreError> {
    let stored_state = {
        let state = write
            .open_table(INTEGRATION_AUTHORITY_STATE)
            .map_err(|error| engine("opening integration authority usage", error))?;
        state
            .get(AUTHORITY_STATE_KEY)
            .map_err(|error| engine("reading integration authority usage", error))?
            .map(|value| decode_authority_state(value.value()))
            .transpose()?
    };
    let newest_order = revoked.last().map_or(0, |(_, row)| row.order);
    if let Some(state) = stored_state {
        if state.active_rows != active_rows || state.active_bytes != active_bytes {
            return Err(integration_codec(
                "authority usage",
                "the singleton does not match canonical active rows",
            ));
        }
        if state.next_revocation_order < newest_order
            || revoked
                .iter()
                .any(|(key, _)| !state.revocation_guard.contains(*key))
        {
            return Err(integration_codec(
                "revocation guard",
                "the singleton does not cover canonical tombstones",
            ));
        }
        return Ok(state);
    }
    let mut revocation_guard = IntegrationRevocationGuard::empty();
    for (key, _) in revoked {
        if !revocation_guard.insert(*key) {
            return Err(integration_codec(
                "revocation guard",
                "the fixed guard could not address its own storage",
            ));
        }
    }
    Ok(AuthorityState {
        active_rows,
        active_bytes,
        next_revocation_order: newest_order,
        revocation_guard,
    })
}

fn migrate_legacy_revocations(
    write: &redb::WriteTransaction,
    mut legacy_revoked: Vec<(IntegrationKey, IntegrationRevocation)>,
    revoked: &mut Vec<(IntegrationKey, IntegrationRevocation)>,
    state: &mut AuthorityState,
) -> Result<(), StoreError> {
    if legacy_revoked.is_empty() {
        return Ok(());
    }
    legacy_revoked.sort_by_key(|(key, row)| (row.revoked_at.as_millis(), *key));
    let mut migrated = Vec::with_capacity(legacy_revoked.len());
    for (key, row) in &legacy_revoked {
        let order = state
            .next_revocation_order
            .checked_add(1)
            .ok_or_else(|| integration_codec("revocation order", "it is exhausted"))?;
        state.next_revocation_order = order;
        if !state.revocation_guard.insert(*key) {
            return Err(integration_codec(
                "revocation guard",
                "the fixed guard could not address its own storage",
            ));
        }
        migrated.push((
            *key,
            IntegrationRevocation {
                key_generation: row.key_generation,
                grant_generation: row.grant_generation,
                revoked_at: row.revoked_at,
                order,
            },
        ));
    }
    {
        let mut integrations = write
            .open_table(INTEGRATIONS)
            .map_err(|error| engine("opening the integration grant table", error))?;
        for (key, _) in &legacy_revoked {
            let removed = integrations
                .remove(*key)
                .map_err(|error| engine("removing a legacy revoked integration row", error))?;
            if removed.is_none() {
                return Err(integration_codec(
                    "integration revocation migration",
                    "a selected legacy row disappeared during one write transaction",
                ));
            }
        }
    }
    {
        let mut tombstones = write
            .open_table(INTEGRATION_TOMBSTONES)
            .map_err(|error| engine("opening integration revocation tombstones", error))?;
        for (key, row) in &migrated {
            let encoded = encode_revocation(*row);
            tombstones
                .insert(*key, encoded.as_slice())
                .map_err(|error| engine("writing a migrated revocation tombstone", error))?;
        }
    }
    revoked.extend(migrated);
    revoked.sort_by_key(|(key, row)| (row.order, *key));
    Ok(())
}

fn write_authority_state(
    write: &redb::WriteTransaction,
    state: &AuthorityState,
) -> Result<(), StoreError> {
    let encoded = encode_authority_state(state)?;
    let mut table = write
        .open_table(INTEGRATION_AUTHORITY_STATE)
        .map_err(|error| engine("opening integration authority usage", error))?;
    table
        .insert(AUTHORITY_STATE_KEY, encoded.as_slice())
        .map_err(|error| engine("writing integration authority usage", error))?;
    Ok(())
}

fn ensure_authority_capacity(active_rows: usize, active_bytes: usize) -> Result<(), StoreError> {
    if active_rows > INTEGRATION_ACTIVE_MAX_ROWS || active_bytes > INTEGRATION_AUTHORITY_MAX_BYTES {
        return Err(StoreError::IntegrationAuthorityCapacity {
            active_rows,
            active_bytes,
            max_rows: INTEGRATION_ACTIVE_MAX_ROWS,
            max_bytes: INTEGRATION_AUTHORITY_MAX_BYTES,
        });
    }
    Ok(())
}

fn encode_authority_state(state: &AuthorityState) -> Result<Vec<u8>, StoreError> {
    let mut out = Vec::with_capacity(1 + 24 + REVOCATION_GUARD_BYTES);
    out.push(SCHEMA_VERSION);
    out.extend_from_slice(
        &u64::try_from(state.active_rows)
            .map_err(|_| integration_codec("authority usage", "active rows do not fit u64"))?
            .to_le_bytes(),
    );
    out.extend_from_slice(
        &u64::try_from(state.active_bytes)
            .map_err(|_| integration_codec("authority usage", "active bytes do not fit u64"))?
            .to_le_bytes(),
    );
    out.extend_from_slice(&state.next_revocation_order.to_le_bytes());
    if state.revocation_guard.bits.len() != REVOCATION_GUARD_BYTES {
        return Err(integration_codec(
            "revocation guard",
            "the fixed guard has the wrong byte length",
        ));
    }
    out.extend_from_slice(&state.revocation_guard.bits);
    Ok(out)
}

fn decode_authority_state(bytes: &[u8]) -> Result<AuthorityState, StoreError> {
    let mut cursor = Cursor::new(bytes);
    cursor.version()?;
    let active_rows = usize::try_from(cursor.u64("active integration rows")?)
        .map_err(|_| integration_codec("authority usage", "active rows do not fit usize"))?;
    let active_bytes = usize::try_from(cursor.u64("active authority bytes")?)
        .map_err(|_| integration_codec("authority usage", "active bytes do not fit usize"))?;
    ensure_authority_capacity(active_rows, active_bytes)?;
    let next_revocation_order = cursor.u64("next revocation order")?;
    let bits = cursor
        .take("revocation guard", REVOCATION_GUARD_BYTES)?
        .to_vec()
        .into_boxed_slice();
    cursor.finish()?;
    Ok(AuthorityState {
        active_rows,
        active_bytes,
        next_revocation_order,
        revocation_guard: IntegrationRevocationGuard { bits },
    })
}

fn encode_revocation(row: IntegrationRevocation) -> Vec<u8> {
    let mut out = Vec::with_capacity(33);
    out.push(SCHEMA_VERSION);
    out.extend_from_slice(&row.key_generation.to_le_bytes());
    out.extend_from_slice(&row.grant_generation.to_le_bytes());
    out.extend_from_slice(&row.revoked_at.as_millis().to_le_bytes());
    out.extend_from_slice(&row.order.to_le_bytes());
    out
}

fn decode_revocation(bytes: &[u8]) -> Result<IntegrationRevocation, StoreError> {
    let mut cursor = Cursor::new(bytes);
    cursor.version()?;
    let row = IntegrationRevocation {
        key_generation: cursor.u64("revoked key generation")?,
        grant_generation: cursor.u64("revoked grant generation")?,
        revoked_at: WallMs::from_millis(cursor.u64("revoked at")?),
        order: cursor.u64("revocation order")?,
    };
    cursor.finish()?;
    Ok(row)
}

// This hash layout is durable schema. Changing its lanes, seeds, or width requires migrating every stored guard;
// otherwise a previously retired identity could become a false negative after restart.
fn guard_bits(key: IntegrationKey) -> impl Iterator<Item = usize> {
    let bytes = key.to_bytes();
    (0..REVOCATION_GUARD_HASHES).map(move |lane| {
        let mut hash = 0xcbf2_9ce4_8422_2325_u64 ^ lane.wrapping_mul(0x9e37_79b9_7f4a_7c15);
        for byte in bytes {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        let bit_count = u64::try_from(REVOCATION_GUARD_BYTES * 8).unwrap_or(u64::MAX);
        usize::try_from(hash % bit_count).unwrap_or_default()
    })
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

    fn integration_key(value: u128) -> IntegrationKey {
        IntegrationKey::from_bytes(value.to_le_bytes())
    }

    fn enrollment_key(value: u128) -> EnrollmentKey {
        EnrollmentKey::from_bytes(value.to_le_bytes())
    }

    fn grant() -> IntegrationRow {
        IntegrationRow {
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
        }
    }

    fn seed_integrations(scratch: &Scratch, rows: &[(IntegrationKey, IntegrationRow)]) {
        let write = scratch
            .store
            .begin_durable_write("seeding integration authority test rows")
            .expect("begin test write");
        {
            let mut table = write.open_table(INTEGRATIONS).expect("open integrations");
            for (key, row) in rows {
                let encoded = encode_integration(row).expect("encode seeded integration");
                table
                    .insert(*key, encoded.as_slice())
                    .expect("insert seeded integration");
            }
        }
        write.commit().expect("commit seeded integrations");
    }

    fn seed_tombstones(scratch: &Scratch, rows: &[(IntegrationKey, IntegrationRevocation)]) {
        let write = scratch
            .store
            .begin_durable_write("seeding integration tombstone test rows")
            .expect("begin test write");
        {
            let mut table = write
                .open_table(INTEGRATION_TOMBSTONES)
                .expect("open tombstones");
            for (key, row) in rows {
                let encoded = encode_revocation(*row);
                table
                    .insert(*key, encoded.as_slice())
                    .expect("insert seeded tombstone");
            }
        }
        write.commit().expect("commit seeded tombstones");
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
        let grant = grant();
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
        let revoked = scratch
            .store
            .revoke_integration(integration, WallMs::from_millis(5))
            .expect("revoke")
            .expect("integration exists");
        assert_eq!(revoked.grant_generation, 2);
        assert_eq!(revoked.revoked_at, WallMs::from_millis(5));
        assert_eq!(
            scratch
                .store
                .get_integration(integration)
                .expect("read active"),
            None
        );
        let IntegrationAuthorityParts {
            revoked: tombstones,
            revocation_guard: guard,
            ..
        } = scratch
            .store
            .load_integration_authority()
            .expect("restore compact authority")
            .into_parts();
        assert_eq!(tombstones, vec![(integration, revoked)]);
        assert!(guard.contains(integration));
    }

    #[test]
    fn integration_grant_change_is_atomic_and_generation_bound() {
        let scratch = Scratch::make("grant-change");
        let pending = EnrollmentKey::from_bytes([8; 16]);
        let integration = IntegrationKey::from_bytes([9; 16]);
        scratch
            .store
            .create_enrollment(pending, &enrollment())
            .expect("create enrollment");
        let grant = IntegrationRow {
            public_key: [1; 32],
            client_instance_id: "fixture-instance".into(),
            label: "Fixture".into(),
            manifest_digest: [2; 32],
            scopes: vec!["provider.read".into()],
            roots: Vec::new(),
            key_generation: 1,
            grant_generation: 1,
            approved_at: WallMs::from_millis(3),
            revoked_at: None,
        };
        scratch
            .store
            .approve_enrollment(pending, integration, &grant)
            .expect("approve integration");
        let changed_scopes = vec!["provider.read".into(), "session.list".into()];
        let changed_roots = vec![IntegrationRootRow {
            path: "C:/other".into(),
            identity: [8; ROOT_IDENTITY_BYTES],
        }];
        assert!(matches!(
            scratch
                .store
                .change_integration_grant(
                    integration,
                    1,
                    changed_scopes.clone(),
                    changed_roots.clone(),
                )
                .expect("change grant"),
            IntegrationGrantChange::Changed(IntegrationRow {
                grant_generation: 2,
                ..
            })
        ));
        assert_eq!(
            scratch
                .store
                .change_integration_grant(
                    integration,
                    1,
                    changed_scopes.clone(),
                    changed_roots.clone(),
                )
                .expect("reject stale change"),
            IntegrationGrantChange::Conflict
        );
        assert!(matches!(
            scratch
                .store
                .change_integration_grant(integration, 2, changed_scopes, changed_roots)
                .expect("accept unchanged grant"),
            IntegrationGrantChange::Unchanged(IntegrationRow {
                grant_generation: 2,
                ..
            })
        ));
    }

    #[test]
    fn key_rotation_is_atomic_and_exactly_replayable() {
        let scratch = Scratch::make("key-rotation");
        let pending = EnrollmentKey::from_bytes([10; 16]);
        let integration = IntegrationKey::from_bytes([11; 16]);
        scratch
            .store
            .create_enrollment(pending, &enrollment())
            .expect("create enrollment");
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
            .expect("approve integration");
        let rotated = scratch
            .store
            .rotate_integration_key(integration, 1, [9; 32])
            .expect("rotate key");
        assert!(matches!(
            rotated,
            IntegrationKeyRotation::Rotated(row)
                if row.public_key == [9; 32]
                    && row.key_generation == 2
                    && row.grant_generation == 1
        ));
        assert!(matches!(
            scratch
                .store
                .rotate_integration_key(integration, 1, [9; 32])
                .expect("replay key rotation"),
            IntegrationKeyRotation::Replayed(_)
        ));
        assert_eq!(
            scratch
                .store
                .rotate_integration_key(integration, 1, [8; 32])
                .expect("reject changed replay"),
            IntegrationKeyRotation::Conflict
        );
    }

    #[test]
    fn approval_atomically_refuses_the_active_row_ceiling() {
        let scratch = Scratch::make("active-row-cap");
        let existing = (0..INTEGRATION_ACTIVE_MAX_ROWS)
            .map(|index| (integration_key(index as u128 + 1), grant()))
            .collect::<Vec<_>>();
        seed_integrations(&scratch, &existing);
        assert_eq!(
            scratch
                .store
                .load_integration_authority()
                .expect("initialize bounded authority")
                .into_parts()
                .active
                .len(),
            INTEGRATION_ACTIVE_MAX_ROWS
        );
        let pending = enrollment_key(1_000);
        let candidate = integration_key(1_000);
        scratch
            .store
            .create_enrollment(pending, &enrollment())
            .expect("create pending enrollment");

        let refused = scratch
            .store
            .approve_enrollment(pending, candidate, &grant());

        assert!(matches!(
            refused,
            Err(StoreError::IntegrationAuthorityCapacity {
                active_rows,
                max_rows: INTEGRATION_ACTIVE_MAX_ROWS,
                ..
            }) if active_rows == INTEGRATION_ACTIVE_MAX_ROWS + 1
        ));
        assert!(
            scratch
                .store
                .get_integration(candidate)
                .expect("read refused candidate")
                .is_none()
        );
        assert_eq!(
            scratch
                .store
                .get_enrollment(pending)
                .expect("read pending enrollment")
                .expect("pending enrollment remains")
                .state,
            EnrollmentState::Pending
        );
    }

    #[test]
    fn approval_atomically_refuses_the_encoded_byte_ceiling() {
        let scratch = Scratch::make("active-byte-cap");
        let mut large = grant();
        let path: Box<str> = "x".repeat(MAX_PATH_BYTES).into();
        large.roots = (0..MAX_ROOTS)
            .map(|index| IntegrationRootRow {
                path: path.clone(),
                identity: [u8::try_from(index).unwrap_or_default(); ROOT_IDENTITY_BYTES],
            })
            .collect();
        let one_row_bytes = integration_authority_bytes(&large).expect("measure large authority");
        assert!(one_row_bytes * 3 < INTEGRATION_AUTHORITY_MAX_BYTES);
        assert!(one_row_bytes * 4 > INTEGRATION_AUTHORITY_MAX_BYTES);
        let existing = (1..=3)
            .map(|index| (integration_key(index), large.clone()))
            .collect::<Vec<_>>();
        seed_integrations(&scratch, &existing);
        scratch
            .store
            .load_integration_authority()
            .expect("initialize byte-bounded authority");
        let pending = enrollment_key(2_000);
        let candidate = integration_key(2_000);
        scratch
            .store
            .create_enrollment(pending, &enrollment())
            .expect("create pending enrollment");

        let refused = scratch.store.approve_enrollment(pending, candidate, &large);

        assert!(matches!(
            refused,
            Err(StoreError::IntegrationAuthorityCapacity {
                active_bytes,
                max_bytes: INTEGRATION_AUTHORITY_MAX_BYTES,
                ..
            }) if active_bytes > INTEGRATION_AUTHORITY_MAX_BYTES
        ));
        assert!(
            scratch
                .store
                .get_integration(candidate)
                .expect("read refused candidate")
                .is_none()
        );
        assert_eq!(
            scratch
                .store
                .get_enrollment(pending)
                .expect("read pending enrollment")
                .expect("pending enrollment remains")
                .state,
            EnrollmentState::Pending
        );
    }

    #[test]
    fn oversized_legacy_active_rows_fail_closed_on_restore() {
        let scratch = Scratch::make("legacy-row-cap");
        let rows = (0..=INTEGRATION_ACTIVE_MAX_ROWS)
            .map(|index| (integration_key(index as u128 + 1), grant()))
            .collect::<Vec<_>>();
        seed_integrations(&scratch, &rows);

        assert!(matches!(
            scratch.store.load_integration_authority(),
            Err(StoreError::IntegrationAuthorityCapacity {
                active_rows,
                max_rows: INTEGRATION_ACTIVE_MAX_ROWS,
                ..
            }) if active_rows == INTEGRATION_ACTIVE_MAX_ROWS + 1
        ));
    }

    #[test]
    fn oversized_legacy_authority_bytes_fail_closed_on_restore() {
        let scratch = Scratch::make("legacy-byte-cap");
        let mut large = grant();
        let path: Box<str> = "y".repeat(MAX_PATH_BYTES).into();
        large.roots = (0..MAX_ROOTS)
            .map(|index| IntegrationRootRow {
                path: path.clone(),
                identity: [u8::try_from(index).unwrap_or_default(); ROOT_IDENTITY_BYTES],
            })
            .collect();
        let rows = (1..=4)
            .map(|index| (integration_key(index), large.clone()))
            .collect::<Vec<_>>();
        seed_integrations(&scratch, &rows);

        assert!(matches!(
            scratch.store.load_integration_authority(),
            Err(StoreError::IntegrationAuthorityCapacity {
                active_bytes,
                max_bytes: INTEGRATION_AUTHORITY_MAX_BYTES,
                ..
            }) if active_bytes > INTEGRATION_AUTHORITY_MAX_BYTES
        ));
    }

    #[test]
    fn legacy_revocations_compact_without_allowing_stale_identity_reuse() {
        let scratch = Scratch::make("legacy-revocation-retention");
        let retired = (1..=INTEGRATION_REVOKED_MAX_ROWS)
            .map(|index| {
                let mut row = grant();
                row.grant_generation = 2;
                row.revoked_at = Some(WallMs::from_millis(10));
                (integration_key(index as u128), row)
            })
            .collect::<Vec<_>>();
        let oldest = retired
            .iter()
            .map(|(key, _)| *key)
            .min()
            .expect("one retired row");
        seed_integrations(&scratch, &retired);

        let IntegrationAuthorityParts {
            active,
            revoked: tombstones,
            revocation_guard: guard,
            ..
        } = scratch
            .store
            .load_integration_authority()
            .expect("compact legacy revocations")
            .into_parts();

        assert!(active.is_empty());
        assert_eq!(tombstones.len(), INTEGRATION_REVOKED_MAX_ROWS);
        assert!(guard.contains(oldest));

        let pending = enrollment_key(3_000);
        let replacement = integration_key(3_000);
        scratch
            .store
            .create_enrollment(pending, &enrollment())
            .expect("create pending enrollment");
        scratch
            .store
            .approve_enrollment(pending, replacement, &grant())
            .expect("approve a fresh identity");
        scratch
            .store
            .revoke_integration(replacement, WallMs::from_millis(20))
            .expect("revoke fresh identity")
            .expect("fresh identity was active");
        let IntegrationAuthorityParts {
            revoked: tombstones,
            revocation_guard: guard,
            ..
        } = scratch
            .store
            .load_integration_authority()
            .expect("restore pruned tombstones")
            .into_parts();
        assert_eq!(tombstones.len(), INTEGRATION_REVOKED_MAX_ROWS);
        assert!(guard.contains(oldest));
        assert!(tombstones.iter().all(|(key, _)| *key != oldest));

        let replay = enrollment_key(3_001);
        scratch
            .store
            .create_enrollment(replay, &enrollment())
            .expect("create replay enrollment");
        assert!(matches!(
            scratch.store.approve_enrollment(replay, oldest, &grant()),
            Err(StoreError::IntegrationIdentityUnavailable)
        ));
        assert_eq!(
            scratch
                .store
                .get_enrollment(replay)
                .expect("read replay enrollment")
                .expect("replay enrollment remains")
                .state,
            EnrollmentState::Pending
        );
    }

    #[test]
    fn oversized_legacy_revocations_fail_closed_without_partial_migration() {
        let scratch = Scratch::make("legacy-revocation-overload");
        let retired = (1..=INTEGRATION_REVOKED_MAX_ROWS + 1)
            .map(|index| {
                let mut row = grant();
                row.grant_generation = 2;
                row.revoked_at = Some(WallMs::from_millis(10));
                (integration_key(index as u128), row)
            })
            .collect::<Vec<_>>();
        let retained = retired.first().expect("one retired row").0;
        seed_integrations(&scratch, &retired);

        assert!(matches!(
            scratch.store.load_integration_authority(),
            Err(StoreError::IntegrationCodec {
                field: "integration tombstone retention",
                ..
            })
        ));
        assert!(
            scratch
                .store
                .get_integration(retained)
                .expect("read unchanged legacy row")
                .is_some()
        );
        assert!(matches!(
            scratch.store.load_integration_authority(),
            Err(StoreError::IntegrationCodec {
                field: "integration tombstone retention",
                ..
            })
        ));
    }

    #[test]
    fn oversized_canonical_tombstones_fail_closed_without_rewrite() {
        let scratch = Scratch::make("canonical-tombstone-overload");
        let tombstones = (1..=INTEGRATION_REVOKED_MAX_ROWS + 1)
            .map(|index| {
                (
                    integration_key(index as u128),
                    IntegrationRevocation {
                        key_generation: 1,
                        grant_generation: 2,
                        revoked_at: WallMs::from_millis(10),
                        order: u64::try_from(index).unwrap_or_default(),
                    },
                )
            })
            .collect::<Vec<_>>();
        seed_tombstones(&scratch, &tombstones);

        assert!(matches!(
            scratch.store.load_integration_authority(),
            Err(StoreError::IntegrationCodec {
                field: "integration tombstone retention",
                ..
            })
        ));
        let database = scratch.store.db().expect("open database");
        let read = database.begin_read().expect("begin read");
        let table = read
            .open_table(INTEGRATION_TOMBSTONES)
            .expect("open tombstones");
        let count = table
            .range(IntegrationKey::FIRST..=IntegrationKey::LAST)
            .expect("scan tombstones")
            .count();
        assert_eq!(count, INTEGRATION_REVOKED_MAX_ROWS + 1);
    }

    #[test]
    fn restore_rejects_a_usage_singleton_that_disagrees_with_active_rows() {
        let scratch = Scratch::make("usage-crosscheck");
        seed_integrations(&scratch, &[(integration_key(1), grant())]);
        scratch
            .store
            .load_integration_authority()
            .expect("initialize canonical usage");
        let write = scratch
            .store
            .begin_durable_write("damaging integration usage for a fail-closed test")
            .expect("begin test write");
        {
            let mut table = write
                .open_table(INTEGRATION_AUTHORITY_STATE)
                .expect("open authority state");
            let stored = table
                .get(AUTHORITY_STATE_KEY)
                .expect("read authority state")
                .expect("authority state exists");
            let mut state = decode_authority_state(stored.value()).expect("decode authority state");
            state.active_rows = 0;
            let encoded = encode_authority_state(&state).expect("encode damaged authority state");
            drop(stored);
            table
                .insert(AUTHORITY_STATE_KEY, encoded.as_slice())
                .expect("write damaged authority state");
        }
        write.commit().expect("commit damaged authority state");

        assert!(matches!(
            scratch.store.load_integration_authority(),
            Err(StoreError::IntegrationCodec {
                field: "authority usage",
                ..
            })
        ));
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
