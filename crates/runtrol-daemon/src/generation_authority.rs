//! Successor-owned authorization relay for draining Runtime generations.
//!
//! A draining generation freezes the grants it knew before releasing the store. It can then accept only
//! revocation, key invalidation, or an authority intersection beneath that ceiling. It can never admit a new
//! integration or authority widened after handoff.

use std::collections::BTreeMap;
use std::sync::RwLock;

use runtrol_ipc::GenerationAuthorityLine;
use runtrol_provider::WallMs;
use runtrol_store::{IntegrationKey, IntegrationRow};
use tokio::sync::watch;

const RELAY_STALE_AFTER_MS: u64 = 5_000;

/// Private grant state for this generation.
pub(crate) struct GenerationAuthorityRelay {
    state: RwLock<RelayState>,
    revision: watch::Sender<u64>,
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
        let (revision, _) = watch::channel(0);
        Self {
            state: RwLock::new(RelayState::Primary),
            revision,
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
    pub(crate) fn freeze(&self, authority: &crate::integration_authority::IntegrationAuthority) {
        let ceiling = authority.rows();
        let current = ceiling.clone();
        let Ok(mut state) = self.state.write() else {
            return;
        };
        *state = RelayState::Draining {
            ceiling,
            current,
            successor_digest: None,
            // The frozen ceiling is safe authority while the successor opens the released store and sends its
            // first intersection. Starting unavailable here closed every live terminal during that handoff race.
            last_update_ms: Some(WallMs::now().as_millis()),
        };
        drop(state);
        self.revision
            .send_modify(|revision| *revision = revision.wrapping_add(1));
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
        let incoming: BTreeMap<IntegrationKey, &GenerationAuthorityLine> = authorities
            .iter()
            .map(|line| (IntegrationKey::from_bytes(line.integration_key), line))
            .collect();
        if incoming.len() != authorities.len() {
            return Err(RelayFailure::Unavailable);
        }
        let mut next = BTreeMap::new();
        for (key, frozen) in ceiling.iter() {
            let previous = current.get(key).unwrap_or(frozen);
            if previous.revoked_at.is_some() {
                next.insert(*key, previous.clone());
                continue;
            }
            let Some(update) = incoming.get(key) else {
                let mut revoked = previous.clone();
                revoked.revoked_at = Some(WallMs::now());
                next.insert(*key, revoked);
                continue;
            };
            let expected_id = crate::runtime_auth::integration_id(*key);
            if update.integration_id.as_ref() != expected_id.as_str()
                || update.public_key != frozen.public_key
                || update.key_generation != frozen.key_generation
                || update.revoked
            {
                let mut revoked = previous.clone();
                revoked.revoked_at = Some(WallMs::now());
                next.insert(*key, revoked);
                continue;
            }
            if update.grant_generation < previous.grant_generation {
                // A late snapshot from an older successor must not refresh the relay clock or replace newer
                // authority after a second upgrade. The current successor will provide a monotonic snapshot.
                return Err(RelayFailure::Unavailable);
            }
            let mut intersected = frozen.clone();
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
            if update.grant_generation == previous.grant_generation && intersected != *previous {
                // One durable generation has one exact value. A peer presenting different authority under the
                // same generation is stale or malformed and cannot become the relay successor.
                return Err(RelayFailure::Unavailable);
            }
            next.insert(*key, intersected);
        }
        let changed = *current != next;
        *current = next;
        // A long-lived terminal may outlive more than one daemon upgrade. The newest monotonic owner replaces
        // the previous relay binding; late snapshots cannot roll the rows back because of the checks above.
        *successor_digest = Some(successor.into());
        *last_update_ms = Some(WallMs::now().as_millis());
        drop(state);
        if changed {
            self.revision
                .send_modify(|revision| *revision = revision.wrapping_add(1));
        }
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

    /// Subscribe to authority changes relayed from the successor.
    pub(crate) fn subscribe(&self) -> watch::Receiver<u64> {
        self.revision.subscribe()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtrol_ipc::GenerationAuthorityRoot;
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

    fn relay(key: IntegrationKey, ceiling: IntegrationRow) -> GenerationAuthorityRelay {
        GenerationAuthorityRelay {
            state: RwLock::new(RelayState::Draining {
                ceiling: BTreeMap::from([(key, ceiling.clone())]),
                current: BTreeMap::from([(key, ceiling)]),
                successor_digest: None,
                last_update_ms: Some(WallMs::now().as_millis()),
            }),
            revision: watch::channel(0).0,
        }
    }

    fn line(
        key: IntegrationKey,
        scopes: &[&str],
        generation: u64,
        revoked: bool,
    ) -> GenerationAuthorityLine {
        GenerationAuthorityLine {
            integration_key: key.to_bytes(),
            integration_id: crate::runtime_auth::integration_id(key).as_str().into(),
            public_key: [7; 32],
            scopes: scopes
                .iter()
                .map(|scope| Box::<str>::from(*scope))
                .collect(),
            roots: vec![GenerationAuthorityRoot {
                path: "C:\\work".into(),
                identity: [9; 24],
            }],
            key_generation: 1,
            grant_generation: generation,
            revoked,
        }
    }

    fn bound_successor(relay: &GenerationAuthorityRelay) -> Option<Box<str>> {
        let state = relay.state.read().expect("read relay state");
        let RelayState::Draining {
            successor_digest, ..
        } = &*state
        else {
            panic!("test relay must be draining");
        };
        successor_digest.clone()
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
            revision: watch::channel(0).0,
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
            revision: watch::channel(0).0,
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
            revision: watch::channel(0).0,
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
    fn a_long_lived_generation_rebinds_from_b_to_equivalent_c() {
        let key = IntegrationKey::from_bytes([5; 16]);
        let relay = relay(key, row(&["session.list"], 1));

        relay
            .apply("generation-b", &[line(key, &["session.list"], 1, false)])
            .expect("B binds to draining A");
        assert_eq!(bound_successor(&relay).as_deref(), Some("generation-b"));

        relay
            .apply("generation-c", &[line(key, &["session.list"], 1, false)])
            .expect("equivalent C replaces B after the next upgrade");
        assert_eq!(bound_successor(&relay).as_deref(), Some("generation-c"));
        assert_eq!(relay.row(key).expect("C authority").grant_generation, 1);
    }

    #[test]
    fn a_late_b_snapshot_cannot_roll_c_back_or_rebind_the_relay() {
        let key = IntegrationKey::from_bytes([6; 16]);
        let relay = relay(key, row(&["session.list"], 1));
        relay
            .apply("generation-b", &[line(key, &["session.list"], 1, false)])
            .expect("B binds first");
        relay
            .apply("generation-c", &[line(key, &["session.list"], 2, false)])
            .expect("C advances authority");

        assert_eq!(
            relay.apply("generation-b", &[line(key, &["session.list"], 1, false)]),
            Err(RelayFailure::Unavailable)
        );
        assert_eq!(bound_successor(&relay).as_deref(), Some("generation-c"));
        assert_eq!(
            relay.row(key).expect("C remains current").grant_generation,
            2
        );
    }

    #[test]
    fn one_generation_cannot_name_two_different_authorities() {
        let key = IntegrationKey::from_bytes([7; 16]);
        let relay = relay(key, row(&["session.list", "session.input.write"], 1));
        relay
            .apply("generation-b", &[line(key, &["session.list"], 2, false)])
            .expect("B narrows the ceiling");

        assert_eq!(
            relay.apply(
                "generation-c",
                &[line(
                    key,
                    &["session.list", "session.input.write"],
                    2,
                    false,
                )],
            ),
            Err(RelayFailure::Unavailable)
        );
        assert_eq!(bound_successor(&relay).as_deref(), Some("generation-b"));
        assert_eq!(
            relay.row(key).expect("B narrowing remains").scopes,
            vec![Box::<str>::from("session.list")]
        );
    }

    #[test]
    fn relayed_revocation_cannot_be_resurrected_by_a_newer_successor() {
        let key = IntegrationKey::from_bytes([8; 16]);
        let relay = relay(key, row(&["session.list"], 1));
        relay
            .apply("generation-b", &[line(key, &["session.list"], 2, true)])
            .expect("B revokes the frozen row");
        assert!(
            relay
                .row(key)
                .expect("revoked row remains structural")
                .revoked_at
                .is_some()
        );

        relay
            .apply("generation-c", &[line(key, &["session.list"], 3, false)])
            .expect("C may heartbeat but cannot revive authority");
        let current = relay.row(key).expect("revocation remains structural");
        assert!(current.revoked_at.is_some());
        assert_eq!(current.grant_generation, 1);
        assert_eq!(bound_successor(&relay).as_deref(), Some("generation-c"));
    }
}
