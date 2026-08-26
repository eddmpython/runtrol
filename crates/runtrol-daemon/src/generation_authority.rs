//! Successor-owned authorization relay for draining Runtime generations.
//!
//! A draining generation freezes the grants it knew before releasing the store. It can then accept only
//! revocation, key invalidation, or an authority intersection beneath that ceiling. It can never admit a new
//! integration or authority widened after handoff.

use std::collections::BTreeMap;
use std::sync::RwLock;

use runtrol_ipc::{GenerationAuthorityLine, GenerationAuthorityRoot};
use runtrol_provider::WallMs;
use runtrol_store::{IntegrationKey, IntegrationRow, Store};

const RELAY_STALE_AFTER_MS: u64 = 5_000;

/// Private grant state for this generation.
pub(crate) struct GenerationAuthorityRelay {
    state: RwLock<RelayState>,
}

enum RelayState {
    Primary,
    Draining {
        ceiling: BTreeMap<IntegrationKey, IntegrationRow>,
        current: BTreeMap<IntegrationKey, IntegrationRow>,
        successor_digest: Option<Box<str>>,
        last_update_ms: Option<u64>,
    },
}

impl Default for GenerationAuthorityRelay {
    fn default() -> Self {
        Self {
            state: RwLock::new(RelayState::Primary),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RelayFailure {
    State,
    Unavailable,
    Missing,
}

impl GenerationAuthorityRelay {
    /// Freeze the exact pre-handoff authority ceiling before the store is released.
    pub(crate) fn freeze(&self, store: &Store) {
        let rows = store.list_integrations().unwrap_or_default();
        let ceiling: BTreeMap<_, _> = rows.into_iter().collect();
        let current = ceiling.clone();
        let Ok(mut state) = self.state.write() else {
            return;
        };
        *state = RelayState::Draining {
            ceiling,
            current,
            successor_digest: None,
            last_update_ms: None,
        };
    }

    /// Intersect one complete successor snapshot with the frozen handoff ceiling.
    pub(crate) fn apply(
        &self,
        successor: &str,
        authorities: &[GenerationAuthorityLine],
    ) -> Result<(), RelayFailure> {
        let mut state = self.state.write().map_err(|_| RelayFailure::State)?;
        let RelayState::Draining {
            ceiling,
            current,
            successor_digest,
            last_update_ms,
        } = &mut *state
        else {
            return Err(RelayFailure::Unavailable);
        };
        match successor_digest {
            Some(bound) if bound.as_ref() != successor => return Err(RelayFailure::Unavailable),
            Some(_) => {}
            None => *successor_digest = Some(successor.into()),
        }
        let incoming: BTreeMap<IntegrationKey, &GenerationAuthorityLine> = authorities
            .iter()
            .map(|line| (IntegrationKey::from_bytes(line.integration_key), line))
            .collect();
        current.clear();
        for (key, frozen) in ceiling.iter() {
            let mut intersected = frozen.clone();
            let Some(update) = incoming.get(key) else {
                intersected.revoked_at = Some(WallMs::now());
                current.insert(*key, intersected);
                continue;
            };
            let expected_id = crate::runtime_auth::integration_id(*key);
            if update.integration_id.as_ref() != expected_id.as_str()
                || update.public_key != frozen.public_key
                || update.key_generation != frozen.key_generation
                || update.grant_generation < frozen.grant_generation
                || update.revoked
            {
                intersected.revoked_at = Some(WallMs::now());
                current.insert(*key, intersected);
                continue;
            }
            intersected
                .scopes
                .retain(|scope| update.scopes.contains(scope));
            intersected.roots.retain(|root| {
                update.roots.iter().any(|candidate| {
                    candidate.path == root.path && candidate.identity == root.identity
                })
            });
            intersected.grant_generation = update.grant_generation;
            intersected.revoked_at = None;
            current.insert(*key, intersected);
        }
        *last_update_ms = Some(WallMs::now().as_millis());
        Ok(())
    }

    /// Current relayed row for authentication or request revalidation in a draining generation.
    pub(crate) fn row(&self, key: IntegrationKey) -> Result<IntegrationRow, RelayFailure> {
        let state = self.state.read().map_err(|_| RelayFailure::State)?;
        let RelayState::Draining {
            current,
            last_update_ms,
            ..
        } = &*state
        else {
            return Err(RelayFailure::Unavailable);
        };
        let Some(last_update_ms) = *last_update_ms else {
            return Err(RelayFailure::Unavailable);
        };
        if WallMs::now().as_millis().saturating_sub(last_update_ms) > RELAY_STALE_AFTER_MS {
            return Err(RelayFailure::Unavailable);
        }
        current.get(&key).cloned().ok_or(RelayFailure::Missing)
    }

    /// Complete current primary-generation authority snapshot for each draining peer.
    pub(crate) fn snapshot(store: &Store) -> Result<Vec<GenerationAuthorityLine>, RelayFailure> {
        let rows = store.list_integrations().map_err(|_| RelayFailure::State)?;
        Ok(rows
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
                revoked: row.revoked_at.is_some(),
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtrol_store::IntegrationRootRow;

    fn row(scopes: &[&str], generation: u64) -> IntegrationRow {
        IntegrationRow {
            public_key: [7; 32],
            client_instance_id: "client".into(),
            label: "client".into(),
            manifest_digest: [8; 32],
            scopes: scopes
                .iter()
                .map(|scope| Box::<str>::from(*scope))
                .collect(),
            roots: vec![IntegrationRootRow {
                path: "C:\\work".into(),
                identity: [9; 24],
            }],
            key_generation: 1,
            grant_generation: generation,
            approved_at: WallMs::from_millis(1),
            revoked_at: None,
        }
    }

    #[test]
    fn an_update_can_shrink_but_cannot_widen_the_frozen_ceiling() {
        let key = IntegrationKey::from_bytes([1; 16]);
        let ceiling = row(&["session.list"], 1);
        let relay = GenerationAuthorityRelay {
            state: RwLock::new(RelayState::Draining {
                ceiling: BTreeMap::from([(key, ceiling.clone())]),
                current: BTreeMap::from([(key, ceiling)]),
                successor_digest: None,
                last_update_ms: None,
            }),
        };
        relay
            .apply(
                &"a".repeat(64),
                &[GenerationAuthorityLine {
                    integration_key: key.to_bytes(),
                    integration_id: crate::runtime_auth::integration_id(key).as_str().into(),
                    public_key: [7; 32],
                    scopes: vec!["session.list".into(), "session.input.write".into()],
                    roots: vec![GenerationAuthorityRoot {
                        path: "C:\\work".into(),
                        identity: [9; 24],
                    }],
                    key_generation: 1,
                    grant_generation: 2,
                    revoked: false,
                }],
            )
            .expect("apply relay update");
        let current = relay.row(key).expect("relayed row");
        assert_eq!(current.scopes, vec![Box::<str>::from("session.list")]);
        assert_eq!(current.grant_generation, 2);
    }

    #[test]
    fn key_rotation_invalidates_the_old_generation_instead_of_admitting_the_new_key() {
        let key = IntegrationKey::from_bytes([2; 16]);
        let ceiling = row(&["session.list"], 1);
        let relay = GenerationAuthorityRelay {
            state: RwLock::new(RelayState::Draining {
                ceiling: BTreeMap::from([(key, ceiling.clone())]),
                current: BTreeMap::from([(key, ceiling)]),
                successor_digest: None,
                last_update_ms: None,
            }),
        };
        relay
            .apply(
                &"b".repeat(64),
                &[GenerationAuthorityLine {
                    integration_key: key.to_bytes(),
                    integration_id: crate::runtime_auth::integration_id(key).as_str().into(),
                    public_key: [3; 32],
                    scopes: vec!["session.list".into()],
                    roots: vec![GenerationAuthorityRoot {
                        path: "C:\\work".into(),
                        identity: [9; 24],
                    }],
                    key_generation: 2,
                    grant_generation: 2,
                    revoked: false,
                }],
            )
            .expect("apply key rotation");
        assert!(
            relay
                .row(key)
                .expect("old row remains only as a revocation")
                .revoked_at
                .is_some()
        );
    }

    #[test]
    fn a_missing_row_is_revoked_and_a_new_integration_is_ignored() {
        let old = IntegrationKey::from_bytes([3; 16]);
        let new = IntegrationKey::from_bytes([4; 16]);
        let ceiling = row(&["session.list"], 1);
        let relay = GenerationAuthorityRelay {
            state: RwLock::new(RelayState::Draining {
                ceiling: BTreeMap::from([(old, ceiling.clone())]),
                current: BTreeMap::from([(old, ceiling)]),
                successor_digest: None,
                last_update_ms: None,
            }),
        };
        relay
            .apply(
                &"c".repeat(64),
                &[GenerationAuthorityLine {
                    integration_key: new.to_bytes(),
                    integration_id: crate::runtime_auth::integration_id(new).as_str().into(),
                    public_key: [7; 32],
                    scopes: vec!["session.list".into()],
                    roots: Vec::new(),
                    key_generation: 1,
                    grant_generation: 1,
                    revoked: false,
                }],
            )
            .expect("apply complete successor snapshot");
        assert!(
            relay
                .row(old)
                .expect("old row is retained")
                .revoked_at
                .is_some()
        );
        assert_eq!(relay.row(new), Err(RelayFailure::Missing));
    }

    #[test]
    fn the_first_successor_digest_is_bound_and_a_stale_relay_is_unavailable() {
        let key = IntegrationKey::from_bytes([5; 16]);
        let ceiling = row(&["session.list"], 1);
        let relay = GenerationAuthorityRelay {
            state: RwLock::new(RelayState::Draining {
                ceiling: BTreeMap::from([(key, ceiling.clone())]),
                current: BTreeMap::from([(key, ceiling)]),
                successor_digest: Some("bound".into()),
                last_update_ms: Some(1),
            }),
        };
        assert_eq!(relay.apply("other", &[]), Err(RelayFailure::Unavailable));
        assert_eq!(relay.row(key), Err(RelayFailure::Unavailable));
    }
}
