//! One bounded native observation projection, shared by guards, the source producer and subscribers.

use std::collections::BTreeMap;
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use runtrol_provider::{NativeProcessActivity, NativeProcessObservation, ProviderId};
use tokio::sync::watch;

use crate::provider_prepare::PreparedDriver;

#[derive(Clone)]
pub(crate) struct NativeObservation {
    measured_at: Instant,
    pub(crate) observation: Option<NativeProcessObservation>,
    source: Weak<PreparedDriver>,
    source_id: String,
}

impl NativeObservation {
    pub(crate) fn catalogue_revision(&self) -> Option<String> {
        Some(format!(
            "{}:{}",
            self.source_id,
            self.observation.as_ref()?.catalogue_revision?
        ))
    }
}

pub(crate) type NativeSnapshot = BTreeMap<ProviderId, NativeObservation>;

pub(crate) struct NativeObservations {
    updates: watch::Sender<Arc<NativeSnapshot>>,
}

impl NativeObservations {
    pub(crate) fn new(providers: &[ProviderId]) -> Self {
        let snapshot = providers
            .iter()
            .copied()
            .map(|provider| {
                (
                    provider,
                    NativeObservation {
                        measured_at: Instant::now(),
                        observation: None,
                        source: Weak::new(),
                        source_id: uuid::Uuid::now_v7().to_string(),
                    },
                )
            })
            .collect();
        Self {
            updates: watch::channel(Arc::new(snapshot)).0,
        }
    }

    pub(crate) fn subscribe(&self) -> watch::Receiver<Arc<NativeSnapshot>> {
        self.updates.subscribe()
    }

    pub(crate) fn projection_changed(&self) {
        self.updates.send_modify(|_| {});
    }

    pub(crate) fn cached(
        &self,
        provider: ProviderId,
        age: Duration,
    ) -> Option<NativeProcessActivity> {
        let snapshot = self.updates.borrow();
        let entry = snapshot.get(&provider)?;
        (entry.measured_at.elapsed() < age).then(|| {
            entry
                .observation
                .as_ref()
                .map(|observation| observation.activity.clone())
        })?
    }

    pub(crate) fn record(
        &self,
        provider: ProviderId,
        observation: NativeProcessObservation,
        source: Option<&Arc<PreparedDriver>>,
    ) -> bool {
        let mut ended = false;
        self.updates.send_if_modified(|snapshot| {
            let Some(previous) = snapshot.get(&provider) else {
                return false;
            };
            let source_changed = source.is_some_and(|source| {
                previous
                    .source
                    .upgrade()
                    .is_none_or(|previous| !Arc::ptr_eq(&previous, source))
            });
            ended = previous.observation.as_ref().is_some_and(|previous| {
                previous.activity.active.iter().any(|native| {
                    !observation.activity.active.contains(native)
                        && !observation.unknown_activity.contains(native)
                })
            });
            let changed = source_changed || previous.observation.as_ref() != Some(&observation);
            let entries = Arc::make_mut(snapshot);
            let Some(entry) = entries.get_mut(&provider) else {
                return false;
            };
            entry.measured_at = Instant::now();
            entry.observation = Some(observation);
            if let Some(source) = source {
                entry.source = Arc::downgrade(source);
            }
            if source_changed {
                entry.source_id = uuid::Uuid::now_v7().to_string();
            }
            changed
        });
        ended
    }

    pub(crate) fn unavailable(&self, provider: ProviderId) {
        self.updates.send_if_modified(|snapshot| {
            if snapshot
                .get(&provider)
                .is_none_or(|entry| entry.observation.is_none())
            {
                return false;
            }
            let Some(entry) = Arc::make_mut(snapshot).get_mut(&provider) else {
                return false;
            };
            entry.observation = None;
            true
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unchanged_observation_refreshes_guard_age_without_waking_subscribers() {
        let provider = ProviderId::parse("fixture").expect("provider");
        let observations = NativeObservations::new(&[provider]);
        let mut updates = observations.subscribe();
        let first = NativeProcessObservation::default();
        observations.record(provider, first.clone(), None);
        assert!(updates.has_changed().expect("publisher is alive"));
        drop(updates.borrow_and_update());
        observations.record(provider, first, None);
        assert!(!updates.has_changed().expect("publisher is alive"));
        assert!(
            observations
                .cached(provider, Duration::from_secs(1))
                .is_some()
        );
        observations.unavailable(provider);
        assert!(updates.has_changed().expect("publisher is alive"));
        assert!(
            observations
                .cached(provider, Duration::from_secs(1))
                .is_none()
        );
    }
}
