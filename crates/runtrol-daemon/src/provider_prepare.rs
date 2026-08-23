//! Shared provider preparation below both daemon request boundaries.

use std::collections::BTreeSet;
use std::sync::Arc;

use runtrol_core::registry::KindStatus;
use runtrol_drivers::DriverContext;
use runtrol_provider::{Provider, ProviderId};
use sha2::{Digest as _, Sha256};

use crate::Composed;

/// A provider could not be prepared from registered runtime observations.
pub(crate) struct ProviderPreparationError {
    message: String,
}

impl ProviderPreparationError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    /// Safe local control-surface explanation.
    pub(crate) fn message(&self) -> &str {
        &self.message
    }
}

/// Build one declared and available driver, with runtime resolution owned by the probe.
pub(crate) async fn driver(
    composed: &Composed,
    id: ProviderId,
) -> Result<Box<dyn Provider>, ProviderPreparationError> {
    Ok(prepared_driver(composed, id).await?.driver)
}

/// One exact prepared driver plus a non-secret digest of the binary facts used for the probe.
pub(crate) struct PreparedDriver {
    pub(crate) driver: Box<dyn Provider>,
    pub(crate) binary_identity: [u8; 32],
}

/// Build a driver and retain the exact binary identity needed to scope a provider cursor.
pub(crate) async fn prepared_driver(
    composed: &Composed,
    id: ProviderId,
) -> Result<PreparedDriver, ProviderPreparationError> {
    let provider = id.as_str();
    let Some(declared) = composed.registry.get(id) else {
        return Err(ProviderPreparationError::new(format!(
            "no provider called {provider}"
        )));
    };
    match declared.kind {
        KindStatus::Available => {}
        KindStatus::Unavailable { why } => return Err(ProviderPreparationError::new(why)),
        KindStatus::Unknown => {
            return Err(ProviderPreparationError::new(format!(
                "{provider} names a kind nothing in this build declares"
            )));
        }
    }

    let Some(entry) = composed.driver_for(declared.manifest.kind.as_str()) else {
        return Err(ProviderPreparationError::new(
            "this build has no driver for that kind",
        ));
    };
    let Some(make) = entry.make else {
        return Err(ProviderPreparationError::new(
            entry
                .unavailable
                .unwrap_or("this build cannot serve that kind"),
        ));
    };

    let mut cache = runtrol_core::ProbeCache::open(composed.home.paths().probe_cache());
    let bound_flags = entry.flags.iter().map(|flag| flag.flag).collect::<Vec<_>>();
    // Resolution belongs to the probe, and the returned value is the exact program handed to the driver. Resolving
    // again here would let a PATH change select a different executable from the one whose version and flags were read.
    let (program, probed) = runtrol_core::probe_program(
        &declared.manifest,
        &bound_flags,
        &mut cache,
        &composed.containment,
    )
    .await
    .map_err(|error| ProviderPreparationError::new(error.to_string()))?;
    {
        // The save is a re-read-merge-replace of one shared file; two finishing at once must not interleave.
        let _writing = composed.probe_cache_writing.lock().await;
        cache
            .save()
            .map_err(|error| ProviderPreparationError::new(error.to_string()))?;
    }
    crate::runtime_inventory::invalidate_provider_inventory(composed).await;

    let checked = checked_flags(provider, entry, probed.flags)?;

    let encoded_facts = serde_json::to_vec(&probed.bin)
        .map_err(|error| ProviderPreparationError::new(error.to_string()))?;
    let binary_identity: [u8; 32] = Sha256::digest(encoded_facts).into();
    Ok(PreparedDriver {
        driver: make(&DriverContext {
            provider: id,
            models: declared.manifest.models.clone(),
            sessions: declared.manifest.sessions.clone(),
            program,
            transport_argv: declared.manifest.transport.argv.clone(),
            available_flags: checked.available,
            unavailable_flags: checked.unavailable,
            contained_by: Arc::clone(&composed.containment),
        }),
        binary_identity,
    })
}

/// How long one model catalogue answer may stand before the provider is asked again.
///
/// Short on purpose: the catalogue depends on login state and provider-owned settings files, not
/// only on the binary, so this is a memoization of a moment rather than a store. Five minutes turns
/// "every picker click spawns a process" into "one spawn per provider per window" without letting a
/// login change stay invisible for long. A stale answer also cannot mislead a start: the CLI itself
/// still refuses a model it no longer has, loudly.
pub(crate) const MODEL_CATALOGUE_TTL: std::time::Duration = std::time::Duration::from_mins(5);

/// One memoized catalogue, valid only for the exact binary it was read from.
pub(crate) struct CachedModelCatalogue {
    pub(crate) identity: [u8; 32],
    pub(crate) read_at: std::time::Instant,
    pub(crate) catalogue: runtrol_provider::ModelCatalog,
}

/// The provider's model catalogue, memoized for a bounded moment per exact binary.
///
/// Measured cost this removes: opencode spawns its whole CLI per listing and claude re-runs
/// `--help`, and the session-start validation repeated both on every start. `Unknown` answers are
/// never memoized (an answer that says "ask again" must be asked again); errors propagate and leave
/// nothing behind.
pub(crate) async fn cached_models(
    composed: &Composed,
    id: ProviderId,
    prepared: &PreparedDriver,
) -> Result<runtrol_provider::ModelCatalog, runtrol_provider::ProviderError> {
    {
        let cache = composed.model_catalogues.lock().await;
        if let Some(entry) = cache.get(&id)
            && entry.identity == prepared.binary_identity
            && entry.read_at.elapsed() < MODEL_CATALOGUE_TTL
        {
            return Ok(entry.catalogue.clone());
        }
    }
    let catalogue = prepared.driver.models().await?;
    if !matches!(catalogue, runtrol_provider::ModelCatalog::Unknown { .. }) {
        composed.model_catalogues.lock().await.insert(
            id,
            CachedModelCatalogue {
                identity: prepared.binary_identity,
                read_at: std::time::Instant::now(),
                catalogue: catalogue.clone(),
            },
        );
    }
    Ok(catalogue)
}

/// Exact confirmed and unavailable optional flags for one prepared driver.
#[derive(Debug)]
pub(crate) struct CheckedFlags {
    pub(crate) available: BTreeSet<Box<str>>,
    pub(crate) unavailable: std::collections::BTreeMap<Box<str>, &'static str>,
}

/// Validate a driver's required and optional flags against the exact probed program.
pub(crate) fn checked_flags(
    provider: &str,
    driver: &runtrol_drivers::DriverKind,
    observed: runtrol_core::Flags,
) -> Result<CheckedFlags, ProviderPreparationError> {
    let available: BTreeSet<Box<str>> = match observed {
        runtrol_core::Flags::Observed(flags) => flags.into_iter().map(Into::into).collect(),
        runtrol_core::Flags::Unknown { why } if driver.flags.iter().any(|flag| flag.required) => {
            return Err(ProviderPreparationError::new(format!(
                "{provider} could not confirm the flags its driver requires: {why}"
            )));
        }
        runtrol_core::Flags::Unknown { .. } => BTreeSet::default(),
    };
    for required in driver.flags.iter().filter(|flag| flag.required) {
        if !available.contains(required.flag) {
            return Err(ProviderPreparationError::new(format!(
                "{provider} does not accept required flag {}: {}",
                required.flag, required.without_it
            )));
        }
    }
    let unavailable = driver
        .flags
        .iter()
        .filter(|flag| !flag.required && !available.contains(flag.flag))
        .map(|flag| (Box::<str>::from(flag.flag), flag.without_it))
        .collect();
    Ok(CheckedFlags {
        available,
        unavailable,
    })
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    struct CountingProvider {
        id: ProviderId,
        asked: Arc<AtomicUsize>,
        answer: runtrol_provider::ModelCatalog,
    }

    #[async_trait::async_trait]
    impl Provider for CountingProvider {
        fn id(&self) -> ProviderId {
            self.id
        }

        async fn models(
            &self,
        ) -> Result<runtrol_provider::ModelCatalog, runtrol_provider::ProviderError> {
            self.asked.fetch_add(1, Ordering::SeqCst);
            Ok(self.answer.clone())
        }

        async fn open(
            &self,
            _intent: runtrol_provider::OpenIntent,
        ) -> Result<Box<dyn runtrol_provider::Agent>, runtrol_provider::ProviderError> {
            Err(runtrol_provider::ProviderError::Unsupported {
                provider: self.id,
                what: "opening".to_owned(),
                why: "this fake only lists models",
            })
        }
    }

    fn prepared(
        asked: &Arc<AtomicUsize>,
        identity: u8,
        answer: runtrol_provider::ModelCatalog,
    ) -> PreparedDriver {
        PreparedDriver {
            driver: Box::new(CountingProvider {
                id: ProviderId::parse("claude").expect("a builtin id"),
                asked: Arc::clone(asked),
                answer,
            }),
            binary_identity: [identity; 32],
        }
    }

    fn aliases() -> runtrol_provider::ModelCatalog {
        runtrol_provider::ModelCatalog::Aliases {
            aliases: vec!["sonnet".into()],
            reasoning_efforts: Vec::new(),
            why: "a test catalogue".into(),
        }
    }

    #[tokio::test]
    async fn a_second_listing_within_the_ttl_is_answered_from_the_memo() {
        let scratch =
            std::env::temp_dir().join(format!("runtrol-model-memo-{}", std::process::id()));
        if scratch.exists() {
            std::fs::remove_dir_all(&scratch).expect("clear the previous run");
        }
        std::fs::create_dir(&scratch).expect("create the scratch home");
        let composed = Composed::for_tests(
            scratch.to_str().expect("UTF-8 scratch path"),
            runtrol_drivers::builtin(),
        )
        .expect("a fresh home composes");
        let id = ProviderId::parse("claude").expect("a builtin id");
        let asked = Arc::new(AtomicUsize::new(0));

        let first = prepared(&asked, 7, aliases());
        cached_models(&composed, id, &first)
            .await
            .expect("first listing");
        cached_models(&composed, id, &first)
            .await
            .expect("second listing");
        assert_eq!(
            asked.load(Ordering::SeqCst),
            1,
            "two listings, one live ask"
        );

        // A different binary identity is a different program: the memo must not answer for it.
        let replaced = prepared(&asked, 8, aliases());
        cached_models(&composed, id, &replaced)
            .await
            .expect("listing after replacement");
        assert_eq!(
            asked.load(Ordering::SeqCst),
            2,
            "a replaced binary is asked again"
        );

        // An Unknown answer says "ask again", so it is never memoized.
        let unknowing = prepared(
            &asked,
            9,
            runtrol_provider::ModelCatalog::unknown("negotiation has not happened"),
        );
        cached_models(&composed, id, &unknowing)
            .await
            .expect("unknown listing");
        cached_models(&composed, id, &unknowing)
            .await
            .expect("unknown listing again");
        assert_eq!(
            asked.load(Ordering::SeqCst),
            4,
            "unknown answers are asked every time"
        );

        drop(composed);
        std::fs::remove_dir_all(&scratch).expect("remove the scratch home");
    }
}
