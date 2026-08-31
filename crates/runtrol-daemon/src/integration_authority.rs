//! Commit-coupled in-memory authority for public Runtime integrations.
//!
//! The durable store remains the source of truth. This bounded projection is restored before any listener starts,
//! and production mutations publish only their exact committed result. Runtime request paths never repopulate it
//! from a cold database read.

use std::collections::BTreeMap;
use std::sync::{Arc, PoisonError, RwLock};

use runtrol_ipc::{GenerationAuthorityLine, GenerationAuthorityRoot};
use runtrol_store::{
    INTEGRATION_ACTIVE_MAX_ROWS, INTEGRATION_AUTHORITY_MAX_BYTES, INTEGRATION_REVOKED_MAX_ROWS,
    IntegrationAuthorityParts, IntegrationKey, IntegrationRevocation, IntegrationRevocationGuard,
    IntegrationRow, Store, StoreError, integration_authority_bytes,
};
use tokio::sync::watch;

/// Why a committed authority projection refused a row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AuthorityPublishError {
    /// An active publication carried revoked or malformed authority.
    InvalidActiveRow,
    /// Either version component moved backward or crossed the current partial order.
    StaleOrDivergent,
    /// One exact version named two different rows.
    SameVersionDivergence,
    /// A permanently retired identity cannot become active again.
    RetiredIdentity,
    /// The durable commit and bounded in-memory projection disagree about admission.
    Capacity,
}

struct AuthorityProjection {
    rows: BTreeMap<IntegrationKey, Arc<IntegrationRow>>,
    tombstones: BTreeMap<IntegrationKey, IntegrationRevocation>,
    revocation_guard: IntegrationRevocationGuard,
    active_bytes: usize,
}

/// A read-optimized projection of exact committed active rows and compact revocation guards.
pub(crate) struct IntegrationAuthority {
    projection: RwLock<AuthorityProjection>,
    revision: watch::Sender<u64>,
}

impl IntegrationAuthority {
    /// Restore bounded authority before public listeners can accept a connection.
    pub(crate) fn restore(store: &Store) -> Result<Self, StoreError> {
        let IntegrationAuthorityParts {
            active,
            revoked,
            revocation_guard,
            active_bytes,
        } = store.load_integration_authority()?.into_parts();
        let rows = active
            .into_iter()
            .map(|(key, row)| (key, Arc::new(row)))
            .collect::<BTreeMap<_, _>>();
        let tombstones = revoked.into_iter().collect::<BTreeMap<_, _>>();
        if rows.len() > INTEGRATION_ACTIVE_MAX_ROWS
            || active_bytes > INTEGRATION_AUTHORITY_MAX_BYTES
            || tombstones.len() > INTEGRATION_REVOKED_MAX_ROWS
        {
            return Err(StoreError::IntegrationAuthorityCapacity {
                active_rows: rows.len(),
                active_bytes,
                max_rows: INTEGRATION_ACTIVE_MAX_ROWS,
                max_bytes: INTEGRATION_AUTHORITY_MAX_BYTES,
            });
        }
        let (revision, _) = watch::channel(0);
        Ok(Self {
            projection: RwLock::new(AuthorityProjection {
                rows,
                tombstones,
                revocation_guard,
                active_bytes,
            }),
            revision,
        })
    }

    /// Read one exact current active row without touching durable storage.
    pub(crate) fn row(&self, key: IntegrationKey) -> Option<Arc<IntegrationRow>> {
        self.projection
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .rows
            .get(&key)
            .cloned()
    }

    /// Whether an integration identity was permanently retired.
    pub(crate) fn was_revoked(&self, key: IntegrationKey) -> bool {
        self.projection
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .revocation_guard
            .contains(key)
    }

    /// Publish one exact active row only after its durable mutation committed.
    ///
    /// Versions use a component-wise partial order. Both components must be nondecreasing, and at least one must
    /// increase. An equal pair is accepted only for the exact same row.
    pub(crate) fn publish_committed(
        &self,
        key: IntegrationKey,
        row: IntegrationRow,
    ) -> Result<(), AuthorityPublishError> {
        if row.revoked_at.is_some() {
            return Err(AuthorityPublishError::InvalidActiveRow);
        }
        let row_bytes = integration_authority_bytes(&row)
            .map_err(|_| AuthorityPublishError::InvalidActiveRow)?;
        {
            let mut projection = self
                .projection
                .write()
                .unwrap_or_else(PoisonError::into_inner);
            if projection.revocation_guard.contains(key) {
                return Err(AuthorityPublishError::RetiredIdentity);
            }
            if let Some(current) = projection.rows.get(&key) {
                match compare_versions(current, &row) {
                    VersionAdvance::Same if current.as_ref() == &row => return Ok(()),
                    VersionAdvance::Same => {
                        return Err(AuthorityPublishError::SameVersionDivergence);
                    }
                    VersionAdvance::StaleOrDivergent => {
                        return Err(AuthorityPublishError::StaleOrDivergent);
                    }
                    VersionAdvance::Newer => {}
                }
                let current_bytes = integration_authority_bytes(current)
                    .map_err(|_| AuthorityPublishError::InvalidActiveRow)?;
                let next_bytes = projection
                    .active_bytes
                    .checked_sub(current_bytes)
                    .and_then(|bytes| bytes.checked_add(row_bytes))
                    .ok_or(AuthorityPublishError::Capacity)?;
                if next_bytes > INTEGRATION_AUTHORITY_MAX_BYTES {
                    return Err(AuthorityPublishError::Capacity);
                }
                projection.active_bytes = next_bytes;
                projection.rows.insert(key, Arc::new(row));
            } else {
                let next_rows = projection.rows.len().saturating_add(1);
                let next_bytes = projection
                    .active_bytes
                    .checked_add(row_bytes)
                    .ok_or(AuthorityPublishError::Capacity)?;
                if next_rows > INTEGRATION_ACTIVE_MAX_ROWS
                    || next_bytes > INTEGRATION_AUTHORITY_MAX_BYTES
                {
                    return Err(AuthorityPublishError::Capacity);
                }
                projection.active_bytes = next_bytes;
                projection.rows.insert(key, Arc::new(row));
            }
        }
        self.wake();
        Ok(())
    }

    /// Publish one exact compact tombstone only after durable revocation committed.
    pub(crate) fn publish_revocation(
        &self,
        key: IntegrationKey,
        revoked: IntegrationRevocation,
    ) -> Result<(), AuthorityPublishError> {
        {
            let mut projection = self
                .projection
                .write()
                .unwrap_or_else(PoisonError::into_inner);
            if let Some(current) = projection.tombstones.get(&key) {
                match compare_tombstone_versions(*current, revoked) {
                    VersionAdvance::Same if *current == revoked => return Ok(()),
                    VersionAdvance::Same => {
                        return Err(AuthorityPublishError::SameVersionDivergence);
                    }
                    VersionAdvance::StaleOrDivergent => {
                        return Err(AuthorityPublishError::StaleOrDivergent);
                    }
                    VersionAdvance::Newer => {}
                }
            } else if let Some(current) = projection.rows.get(&key) {
                match compare_row_to_tombstone(current, revoked) {
                    VersionAdvance::Newer => {}
                    VersionAdvance::Same => {
                        return Err(AuthorityPublishError::SameVersionDivergence);
                    }
                    VersionAdvance::StaleOrDivergent => {
                        return Err(AuthorityPublishError::StaleOrDivergent);
                    }
                }
                let current_bytes = integration_authority_bytes(current)
                    .map_err(|_| AuthorityPublishError::InvalidActiveRow)?;
                projection.active_bytes = projection
                    .active_bytes
                    .checked_sub(current_bytes)
                    .ok_or(AuthorityPublishError::Capacity)?;
                projection.rows.remove(&key);
            } else if projection.revocation_guard.contains(key) {
                return Err(AuthorityPublishError::RetiredIdentity);
            }
            if !projection.revocation_guard.insert(key) {
                return Err(AuthorityPublishError::Capacity);
            }
            projection.tombstones.insert(key, revoked);
            while projection.tombstones.len() > INTEGRATION_REVOKED_MAX_ROWS {
                let Some(expired) = projection
                    .tombstones
                    .iter()
                    .min_by_key(|(candidate, row)| (row.order, **candidate))
                    .map(|(candidate, _)| *candidate)
                else {
                    return Err(AuthorityPublishError::Capacity);
                };
                projection.tombstones.remove(&expired);
            }
        }
        self.wake();
        Ok(())
    }

    /// Subscribe to committed authority changes. The value is only an epoch; readers always fetch the row.
    pub(crate) fn subscribe(&self) -> watch::Receiver<u64> {
        self.revision.subscribe()
    }

    /// Clone exact active rows for a generation handoff ceiling.
    pub(crate) fn rows(&self) -> BTreeMap<IntegrationKey, IntegrationRow> {
        self.projection
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .rows
            .iter()
            .map(|(key, row)| (*key, row.as_ref().clone()))
            .collect()
    }

    /// Project active rows onto the content-free generation handoff wire.
    pub(crate) fn generation_snapshot(&self) -> Vec<GenerationAuthorityLine> {
        self.rows()
            .into_iter()
            .map(|(key, row)| GenerationAuthorityLine {
                integration_key: key.to_bytes(),
                integration_id: crate::runtime_auth::integration_id(key).as_str().into(),
                public_key: row.public_key,
                scopes: row.scopes,
                roots: row
                    .roots
                    .into_iter()
                    .map(|root| GenerationAuthorityRoot {
                        path: root.path,
                        identity: root.identity,
                    })
                    .collect(),
                key_generation: row.key_generation,
                grant_generation: row.grant_generation,
                revoked: false,
            })
            .collect()
    }

    fn wake(&self) {
        self.revision
            .send_modify(|revision| *revision = revision.wrapping_add(1));
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum VersionAdvance {
    Same,
    Newer,
    StaleOrDivergent,
}

fn compare_versions(current: &IntegrationRow, candidate: &IntegrationRow) -> VersionAdvance {
    compare_version_pair(
        current.key_generation,
        current.grant_generation,
        candidate.key_generation,
        candidate.grant_generation,
    )
}

fn compare_row_to_tombstone(
    current: &IntegrationRow,
    candidate: IntegrationRevocation,
) -> VersionAdvance {
    compare_version_pair(
        current.key_generation,
        current.grant_generation,
        candidate.key_generation,
        candidate.grant_generation,
    )
}

fn compare_tombstone_versions(
    current: IntegrationRevocation,
    candidate: IntegrationRevocation,
) -> VersionAdvance {
    compare_version_pair(
        current.key_generation,
        current.grant_generation,
        candidate.key_generation,
        candidate.grant_generation,
    )
}

const fn compare_version_pair(
    current_key: u64,
    current_grant: u64,
    candidate_key: u64,
    candidate_grant: u64,
) -> VersionAdvance {
    if candidate_key == current_key && candidate_grant == current_grant {
        VersionAdvance::Same
    } else if candidate_key >= current_key && candidate_grant >= current_grant {
        VersionAdvance::Newer
    } else {
        VersionAdvance::StaleOrDivergent
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtrol_provider::WallMs;
    use runtrol_store::IntegrationRootRow;

    fn row(key_generation: u64, grant_generation: u64) -> IntegrationRow {
        IntegrationRow {
            public_key: [7; 32],
            client_instance_id: "client".into(),
            label: "Client".into(),
            manifest_digest: [8; 32],
            scopes: vec!["session.list".into()],
            roots: vec![IntegrationRootRow {
                path: "C:\\work".into(),
                identity: [9; 24],
            }],
            key_generation,
            grant_generation,
            approved_at: WallMs::from_millis(1),
            revoked_at: None,
        }
    }

    fn authority() -> IntegrationAuthority {
        let (revision, _) = watch::channel(0);
        IntegrationAuthority {
            projection: RwLock::new(AuthorityProjection {
                rows: BTreeMap::new(),
                tombstones: BTreeMap::new(),
                revocation_guard: IntegrationRevocationGuard::empty(),
                active_bytes: 0,
            }),
            revision,
        }
    }

    fn revoked(key_generation: u64, grant_generation: u64) -> IntegrationRevocation {
        IntegrationRevocation {
            key_generation,
            grant_generation,
            revoked_at: WallMs::from_millis(3),
            order: 1,
        }
    }

    #[test]
    fn a_changed_committed_row_wakes_subscribers_once() {
        let key = IntegrationKey::from_bytes([1; 16]);
        let authority = authority();
        let updates = authority.subscribe();

        authority
            .publish_committed(key, row(1, 1))
            .expect("publish initial authority");
        assert!(updates.has_changed().expect("authority sender stays live"));
        let observed = authority.subscribe();
        authority
            .publish_committed(key, row(1, 1))
            .expect("replay exact authority");
        assert!(!observed.has_changed().expect("authority sender stays live"));
        authority
            .publish_committed(key, row(1, 2))
            .expect("publish newer authority");
        assert!(observed.has_changed().expect("changed row wakes the view"));
        assert_eq!(authority.row(key).expect("current row").grant_generation, 2);
    }

    #[test]
    fn versions_follow_a_component_wise_partial_order() {
        let key = IntegrationKey::from_bytes([2; 16]);
        let authority = authority();
        authority
            .publish_committed(key, row(2, 2))
            .expect("publish current row");

        assert_eq!(
            authority.publish_committed(key, row(3, 1)),
            Err(AuthorityPublishError::StaleOrDivergent)
        );
        assert_eq!(
            authority.publish_committed(key, row(1, 3)),
            Err(AuthorityPublishError::StaleOrDivergent)
        );
        authority
            .publish_committed(key, row(3, 2))
            .expect("one increasing and one equal component is newer");
    }

    #[test]
    fn one_version_cannot_name_two_rows() {
        let key = IntegrationKey::from_bytes([3; 16]);
        let authority = authority();
        authority
            .publish_committed(key, row(1, 1))
            .expect("publish current row");
        let mut divergent = row(1, 1);
        divergent.scopes.push("session.get".into());

        assert_eq!(
            authority.publish_committed(key, divergent),
            Err(AuthorityPublishError::SameVersionDivergence)
        );
        assert_eq!(authority.row(key).expect("original row").scopes.len(), 1);
    }

    #[test]
    fn stale_cold_read_cannot_resurrect_a_revoked_row() {
        let key = IntegrationKey::from_bytes([4; 16]);
        let authority = authority();
        authority
            .publish_committed(key, row(1, 1))
            .expect("publish active row");
        let stale_auth_read = authority.row(key).expect("cold read before revoke");
        authority
            .publish_revocation(key, revoked(1, 2))
            .expect("publish committed revocation");

        assert_eq!(
            authority.publish_committed(key, stale_auth_read.as_ref().clone()),
            Err(AuthorityPublishError::RetiredIdentity)
        );
        assert!(authority.row(key).is_none());
        assert!(authority.was_revoked(key));
    }

    #[test]
    fn revocation_published_before_delayed_approval_projection_blocks_resurrection() {
        let key = IntegrationKey::from_bytes([7; 16]);
        let authority = authority();
        authority
            .publish_revocation(key, revoked(1, 2))
            .expect("publish the later durable revocation first");

        assert_eq!(
            authority.publish_committed(key, row(1, 1)),
            Err(AuthorityPublishError::RetiredIdentity)
        );
        assert!(authority.row(key).is_none());
        assert!(authority.was_revoked(key));
    }

    #[test]
    fn generation_snapshot_contains_only_active_structural_authority() {
        let active = IntegrationKey::from_bytes([5; 16]);
        let retired = IntegrationKey::from_bytes([6; 16]);
        let authority = authority();
        authority
            .publish_committed(active, row(1, 3))
            .expect("publish active row");
        authority
            .publish_committed(retired, row(1, 1))
            .expect("publish row to revoke");
        authority
            .publish_revocation(retired, revoked(1, 2))
            .expect("publish revocation");

        let snapshot = authority.generation_snapshot();
        assert_eq!(snapshot.len(), 1);
        let line = snapshot.first().expect("one authority line");
        assert_eq!(line.integration_key, active.to_bytes());
        assert_eq!(line.grant_generation, 3);
        assert_eq!(line.roots.len(), 1);
    }

    #[test]
    fn projection_refuses_an_active_row_over_the_fixed_count_ceiling() {
        let authority = authority();
        for byte in 0..INTEGRATION_ACTIVE_MAX_ROWS {
            authority
                .publish_committed(
                    IntegrationKey::from_bytes(
                        [u8::try_from(byte).expect("row ceiling fits test identities"); 16],
                    ),
                    row(1, 1),
                )
                .expect("publish authority inside the row ceiling");
        }

        assert_eq!(
            authority.publish_committed(
                IntegrationKey::from_bytes(
                    [u8::try_from(INTEGRATION_ACTIVE_MAX_ROWS)
                        .expect("row ceiling fits test identities"); 16]
                ),
                row(1, 1),
            ),
            Err(AuthorityPublishError::Capacity)
        );
        assert_eq!(authority.rows().len(), INTEGRATION_ACTIVE_MAX_ROWS);
    }

    #[test]
    fn projection_refuses_active_bytes_over_the_fixed_byte_ceiling() {
        let mut large = row(1, 1);
        let path: Box<str> = "x".repeat(32 * 1024).into();
        large.roots = (0..32)
            .map(|index| IntegrationRootRow {
                path: path.clone(),
                identity: [index; 24],
            })
            .collect();
        let row_bytes = integration_authority_bytes(&large).expect("encode one bounded large row");
        assert!(row_bytes * 3 < INTEGRATION_AUTHORITY_MAX_BYTES);
        assert!(row_bytes * 4 > INTEGRATION_AUTHORITY_MAX_BYTES);

        let authority = authority();
        for byte in 0..3 {
            authority
                .publish_committed(IntegrationKey::from_bytes([byte; 16]), large.clone())
                .expect("publish authority inside the byte ceiling");
        }
        assert_eq!(
            authority.publish_committed(IntegrationKey::from_bytes([3; 16]), large),
            Err(AuthorityPublishError::Capacity)
        );
        assert_eq!(authority.rows().len(), 3);
    }
}
