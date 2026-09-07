//! One event-driven native observer per installed provider, independent of viewer count.

use std::sync::Arc;
use std::time::Duration;

use runtrol_provider::{NativeActivityWatch, ProviderId};
use runtrol_runtime_protocol::{InstallationState, ProviderList};
use tokio::sync::watch;
use tokio::task::JoinSet;

use crate::Composed;
use crate::provider_prepare::PreparedDriver;
use crate::serve::{DiscoveryGates, MODEL_PREPARATION_BUDGET_MS};

const RECOVER_AFTER: Duration = Duration::from_secs(1);
const COMPATIBILITY_INTERVAL: Duration = Duration::from_millis(250);

pub(crate) async fn run(
    composed: Arc<Composed>,
    discovering: Arc<DiscoveryGates>,
    mut inventory: watch::Receiver<Arc<ProviderList>>,
) {
    let mut workers = JoinSet::new();
    let mut started = std::collections::BTreeMap::new();
    let mut windows = composed.windows.changes();
    loop {
        for provider in composed.registry.usable() {
            let id = provider.id();
            let missing = known_missing(&inventory.borrow(), id);
            if !started.values().any(|known| *known == id)
                && !missing
                && matches!(
                    tokio::time::timeout(
                        Duration::from_millis(MODEL_PREPARATION_BUDGET_MS),
                        runtrol_core::probe::inspect_manifest(&provider.manifest),
                    )
                    .await,
                    Ok(Ok(_))
                )
            {
                let worker = workers.spawn(watch_provider(
                    Arc::clone(&composed),
                    Arc::clone(&discovering),
                    id,
                    inventory.clone(),
                ));
                started.insert(worker.id(), id);
            }
        }
        tokio::select! {
            changed = inventory.changed() => if changed.is_err() { return; },
            changed = windows.changed() => {
                if changed.is_err() { return; }
                // Window ownership is an independent structural source. Refresh its projection without
                // rereading any provider catalogue or asking a CLI to repeat its activity observation.
                for provider in started.values().copied() {
                    let refreshed = tokio::time::timeout(Duration::from_millis(MODEL_PREPARATION_BUDGET_MS), async {
                        let _lane = discovering.lane(provider).await.lock_owned().await;
                        let snapshot = discovering.native_observations.subscribe().borrow().clone();
                        if let Some(activity) = snapshot.get(&provider).and_then(|entry| entry.observation.as_ref()) {
                            composed.focus_proofs.lock().await.remove(&provider);
                            crate::runtime_serve::reconcile_native_activity(&composed, provider, &activity.activity).await;
                        }
                    }).await;
                    if refreshed.is_err() {
                        discovering.native_observations.unavailable(provider);
                    }
                }
                discovering.native_observations.projection_changed();
            },
            result = workers.join_next_with_id(), if !workers.is_empty() => {
                let task = match result {
                    Some(Ok((task, ()))) => task,
                    Some(Err(error)) => error.id(),
                    None => return,
                };
                if let Some(provider) = started.remove(&task) {
                    discovering.native_observations.unavailable(provider);
                }
                tokio::time::sleep(RECOVER_AFTER).await;
            }
        }
    }
}

async fn watch_provider(
    composed: Arc<Composed>,
    discovering: Arc<DiscoveryGates>,
    provider: ProviderId,
    mut inventory: watch::Receiver<Arc<ProviderList>>,
) {
    loop {
        if known_missing(&inventory.borrow(), provider) {
            discovering.native_observations.unavailable(provider);
        }
        if !wait_for_installation(&mut inventory, provider).await {
            return;
        }
        let prepared =
            tokio::time::timeout(Duration::from_millis(MODEL_PREPARATION_BUDGET_MS), async {
                let _lane = discovering.lane(provider).await.lock_owned().await;
                discovering.native_driver(&composed, provider).await
            })
            .await;
        let Ok(Ok(prepared)) = prepared else {
            discovering.native_observations.unavailable(provider);
            tokio::select! {
                () = tokio::time::sleep(RECOVER_AFTER) => {},
                changed = inventory.changed() => if changed.is_err() { return; },
            }
            continue;
        };
        let installed = installation(&inventory.borrow(), provider);
        let watched = tokio::time::timeout(
            Duration::from_millis(MODEL_PREPARATION_BUDGET_MS),
            prepared.driver.watch_native_activity(),
        )
        .await;
        let Ok(Ok(mut source)) = watched else {
            discovering.native_observations.unavailable(provider);
            tokio::select! {
                () = tokio::time::sleep(RECOVER_AFTER) => {},
                changed = inventory.changed() => if changed.is_err() { return; },
            }
            continue;
        };
        // Source installation precedes the initial observation. A change during that read remains pending.
        loop {
            if !observe(&composed, &discovering, provider, &prepared).await {
                discovering.native_observations.unavailable(provider);
                // Files may be midway through replacement. Recovery is only armed by failed proof;
                // healthy idle sources have no timer and perform no filesystem work.
                tokio::select! {
                    () = tokio::time::sleep(RECOVER_AFTER) => {},
                    changed = inventory.changed() => if changed.is_err() { return; },
                }
                break;
            }
            let mut source_failed = false;
            loop {
                tokio::select! {
                    biased;
                    changed = inventory.changed() => {
                        if changed.is_err() { return; }
                        if installation(&inventory.borrow_and_update(), provider) != installed { break; }
                    },
                    changed = wait_source(&mut source) => {
                        if changed.is_err() {
                            discovering.native_observations.unavailable(provider);
                            source_failed = true;
                        }
                        break;
                    }
                }
            }
            if installation(&inventory.borrow(), provider) != installed || source_failed {
                break;
            }
        }
    }
}

fn known_missing(inventory: &ProviderList, provider: ProviderId) -> bool {
    installation(inventory, provider)
        .is_some_and(|observation| observation.state == InstallationState::Missing)
}

async fn wait_for_installation(
    inventory: &mut watch::Receiver<Arc<ProviderList>>,
    provider: ProviderId,
) -> bool {
    while known_missing(&inventory.borrow(), provider) {
        // Installation discovery owns when an absent executable becomes available. An existing
        // observer must not turn that cached absence into an independent one-second retry loop.
        if inventory.changed().await.is_err() {
            return false;
        }
    }
    true
}

fn installation(
    inventory: &ProviderList,
    provider: ProviderId,
) -> Option<runtrol_runtime_protocol::InstallationObservation> {
    inventory
        .providers
        .iter()
        .find(|entry| entry.provider_id.as_str() == provider.as_str())
        .map(|entry| entry.installation.clone())
}

async fn wait_source(source: &mut Option<Box<dyn NativeActivityWatch>>) -> Result<(), ()> {
    if let Some(source) = source {
        source.changed().await.map_err(|_| ())
    } else {
        // Unsupported drivers retain their explicit compatibility observer; this is not a source wait.
        tokio::time::sleep(COMPATIBILITY_INTERVAL).await;
        Ok(())
    }
}

async fn observe(
    composed: &Arc<Composed>,
    discovering: &DiscoveryGates,
    provider: ProviderId,
    prepared: &Arc<PreparedDriver>,
) -> bool {
    let answer = tokio::time::timeout(Duration::from_millis(MODEL_PREPARATION_BUDGET_MS), async {
        let _lane = discovering.lane(provider).await.lock_owned().await;
        // A provider update may replace this prepared observer while its previous source sleeps.
        let current = discovering.native_driver(composed, provider).await?;
        if !Arc::ptr_eq(&current, prepared) {
            return Err(());
        }
        let observed = prepared.driver.native_observation().await.map_err(|_| ())?;
        crate::runtime_serve::reconcile_native_activity(composed, provider, &observed.activity)
            .await;
        let ended = discovering
            .native_observations
            .record(provider, observed, Some(prepared));
        if ended {
            composed.account_probe_wake.provider(provider).await;
        }
        Ok::<_, ()>(())
    })
    .await;
    matches!(answer, Ok(Ok(())))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inventory(state: InstallationState) -> Arc<ProviderList> {
        Arc::new(
            serde_json::from_value(serde_json::json!({
                "providers": [{
                    "providerId": "fixture", "displayName": "Fixture",
                    "installation": { "state": state }
                }]
            }))
            .expect("a structural installation fixture"),
        )
    }

    #[tokio::test]
    async fn an_absent_installation_parks_until_its_inventory_changes() {
        let provider = ProviderId::parse("fixture").expect("provider identity");
        let (publish, mut receiving) = watch::channel(inventory(InstallationState::Missing));
        for _ in 0..2 {
            let mut waiting = Box::pin(wait_for_installation(&mut receiving, provider));
            std::future::poll_fn(|context| {
                assert!(std::future::Future::poll(waiting.as_mut(), context).is_pending());
                std::task::Poll::Ready(())
            })
            .await;
            drop(waiting);
            // An unrelated publication and a cancelled caller cannot turn absence into a retry.
            publish.send_replace(inventory(InstallationState::Missing));
        }
        publish.send_replace(inventory(InstallationState::Unavailable));
        assert!(wait_for_installation(&mut receiving, provider).await);
        assert!(!known_missing(&receiving.borrow(), provider));
        publish.send_replace(inventory(InstallationState::Usable));
        assert!(wait_for_installation(&mut receiving, provider).await);
    }

    #[tokio::test]
    async fn an_empty_initial_inventory_does_not_suppress_bounded_discovery() {
        let provider = ProviderId::parse("fixture").expect("provider identity");
        let (_publish, mut receiving) = watch::channel(Arc::new(ProviderList {
            providers: Vec::new(),
        }));
        assert!(wait_for_installation(&mut receiving, provider).await);
        assert!(!known_missing(&receiving.borrow(), provider));
    }

    #[tokio::test]
    async fn an_absent_installation_stops_when_inventory_closes() {
        let provider = ProviderId::parse("fixture").expect("provider identity");
        let (publish, mut receiving) = watch::channel(inventory(InstallationState::Missing));
        drop(publish);
        assert!(!wait_for_installation(&mut receiving, provider).await);
    }
}
