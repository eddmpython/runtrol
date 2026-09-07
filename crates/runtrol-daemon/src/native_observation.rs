//! Reusable read-only activity drivers, validated against their exact program and launcher files.

use std::collections::BTreeMap;
use std::future::Future;
use std::sync::Arc;

use runtrol_core::probe::{ProgramFacts, inspect_program};
use runtrol_provider::ProviderId;
use tokio::sync::Mutex;

use crate::provider_prepare::PreparedDriver;

enum NativeDriver {
    Prepared(Arc<ProgramFacts>),
    Observed(Arc<PreparedDriver>),
}

impl NativeDriver {
    fn facts(&self) -> Option<&ProgramFacts> {
        match self {
            Self::Prepared(facts) => Some(facts),
            Self::Observed(driver) => driver.program_facts.as_deref(),
        }
    }
}

/// Only registered providers enter this map. Preparation publishes identities; observation adds validated drivers.
#[derive(Default)]
pub(crate) struct NativeDrivers {
    drivers: Mutex<BTreeMap<ProviderId, NativeDriver>>,
}

impl NativeDrivers {
    /// A fresh PATH discovery supersedes an observer even when its old executable remains installed.
    pub(crate) async fn record_preparation(&self, provider: ProviderId, facts: &Arc<ProgramFacts>) {
        let mut drivers = self.drivers.lock().await;
        if drivers
            .get(&provider)
            .is_none_or(|cached| cached.facts() != Some(facts))
        {
            drivers.insert(provider, NativeDriver::Prepared(Arc::clone(facts)));
        }
    }

    /// The caller holds its provider lane. Other providers never wait for this provider's filesystem work.
    pub(crate) async fn get_or_prepare<E, F>(
        &self,
        provider: ProviderId,
        prepare: impl FnOnce() -> F,
    ) -> Result<Arc<PreparedDriver>, ()>
    where
        F: Future<Output = Result<PreparedDriver, E>>,
    {
        let cached = match self.drivers.lock().await.get(&provider) {
            Some(NativeDriver::Observed(driver)) => Some(Arc::clone(driver)),
            _ => None,
        };
        if let Some(cached) = cached {
            let program = cached.terminal_program.as_ref().ok_or(())?;
            if inspect_program(program)
                .await
                .is_ok_and(|facts| Some(&facts) == cached.program_facts.as_deref())
                && matches!(self.drivers.lock().await.get(&provider), Some(NativeDriver::Observed(current)) if Arc::ptr_eq(current, &cached))
            {
                return Ok(cached);
            }
            // A missing or replaced file closes reuse. Only full discovery may choose its replacement.
            let mut drivers = self.drivers.lock().await;
            if matches!(drivers.get(&provider), Some(NativeDriver::Observed(current)) if Arc::ptr_eq(current, &cached))
            {
                drivers.remove(&provider);
            }
        }

        // Full preparation owns a much larger future. Create it only on a miss, away from the small hot observer.
        let prepared = Arc::new(Box::pin(prepare()).await.map_err(|_| ())?);
        let program = prepared.terminal_program.as_ref().ok_or(())?;
        let facts = prepared.program_facts.as_deref().ok_or(())?;
        if inspect_program(program).await.map_err(|_| ())? != *facts {
            // Preserve the initial launcher baseline as well as the binary. A changed launcher may leave
            // the old executable installed, so the binary identity alone cannot validate first publication.
            return Err(());
        }
        let mut drivers = self.drivers.lock().await;
        if drivers
            .get(&provider)
            .is_some_and(|latest| latest.facts() != Some(facts))
        {
            // A concurrent full preparation selected another installed program while these files were checked.
            return Err(());
        }
        drivers.insert(provider, NativeDriver::Observed(Arc::clone(&prepared)));
        Ok(prepared)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider_prepare::binary_identity;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Observer(ProviderId);

    #[async_trait::async_trait]
    impl runtrol_provider::Provider for Observer {
        fn id(&self) -> ProviderId {
            self.0
        }

        async fn open(
            &self,
            _: runtrol_provider::OpenIntent,
        ) -> Result<Box<dyn runtrol_provider::Agent>, runtrol_provider::ProviderError> {
            panic!("a read-only observation must never open a provider session")
        }
    }

    struct Fixture {
        directory: PathBuf,
        program: runtrol_childproc::Program,
        prepared: AtomicUsize,
    }

    impl Fixture {
        fn new() -> Self {
            let directory = std::env::temp_dir()
                .join(format!("runtrol-native-observer-{}", uuid::Uuid::now_v7()));
            std::fs::create_dir(&directory).expect("create owned fixture");
            let path = directory.join("observer.exe");
            std::fs::write(&path, "unexecuted provider fixture").expect("write fixture");
            Self {
                program: runtrol_childproc::resolve(path.to_str().expect("UTF-8 fixture"))
                    .expect("resolve fixture"),
                directory,
                prepared: AtomicUsize::new(0),
            }
        }

        async fn prepare(&self, provider: ProviderId) -> Result<PreparedDriver, ()> {
            self.prepared.fetch_add(1, Ordering::SeqCst);
            let facts = inspect_program(&self.program).await.map_err(|_| ())?;
            Ok(PreparedDriver {
                driver: Box::new(Observer(provider)),
                binary_identity: binary_identity(&facts.binary).map_err(|_| ())?,
                terminal_program: Some(self.program.clone()),
                program_facts: Some(Arc::new(facts)),
            })
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.directory).expect("remove owned fixture");
        }
    }

    #[tokio::test]
    async fn unchanged_observations_reuse_discovery_but_replacement_prepares_again() {
        let fixture = Fixture::new();
        let provider = ProviderId::parse("observer").expect("fixture provider");
        let drivers = NativeDrivers::default();
        let first = drivers
            .get_or_prepare(provider, || async { fixture.prepare(provider).await })
            .await
            .expect("initial observer");
        let second = drivers
            .get_or_prepare(provider, || async { fixture.prepare(provider).await })
            .await
            .expect("same observer");
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(fixture.prepared.load(Ordering::SeqCst), 1);

        std::fs::write(
            fixture.program.path().as_std_path(),
            "a different, larger provider fixture",
        )
        .expect("replace fixture");
        let replaced = drivers
            .get_or_prepare(provider, || async { fixture.prepare(provider).await })
            .await
            .expect("replacement observer");
        assert!(!Arc::ptr_eq(&first, &replaced));
        assert_ne!(first.binary_identity, replaced.binary_identity);
        assert_eq!(fixture.prepared.load(Ordering::SeqCst), 2);

        std::fs::remove_file(fixture.program.path().as_std_path()).expect("remove executable");
        let failed = drivers
            .get_or_prepare(provider, || async { Err::<PreparedDriver, ()>(()) })
            .await;
        assert!(
            failed.is_err(),
            "missing files must not reuse a former observer"
        );
        assert!(
            drivers.drivers.lock().await.is_empty(),
            "failed rediscovery retains no stale driver"
        );
    }

    #[tokio::test]
    async fn new_discovery_replaces_an_observer_even_when_its_previous_program_is_unchanged() {
        let first_fixture = Fixture::new();
        let second_fixture = Fixture::new();
        let provider = ProviderId::parse("observer").expect("fixture provider");
        let drivers = NativeDrivers::default();
        let first = drivers
            .get_or_prepare(provider, || async { first_fixture.prepare(provider).await })
            .await
            .expect("initial observer");
        drivers
            .record_preparation(
                provider,
                first.program_facts.as_ref().expect("initial facts"),
            )
            .await;
        let same = drivers
            .get_or_prepare(provider, || async { first_fixture.prepare(provider).await })
            .await
            .expect("unchanged discovery");
        assert!(Arc::ptr_eq(&first, &same));

        let replacement = second_fixture
            .prepare(provider)
            .await
            .expect("prepare second program");
        drivers
            .record_preparation(
                provider,
                replacement
                    .program_facts
                    .as_ref()
                    .expect("replacement facts"),
            )
            .await;
        let next = drivers
            .get_or_prepare(provider, || async { Ok::<_, ()>(replacement) })
            .await
            .expect("observer after new PATH discovery");
        assert_ne!(first.binary_identity, next.binary_identity);
        assert!(first_fixture.program.path().as_std_path().exists());
    }

    #[tokio::test]
    async fn concurrent_new_discovery_cannot_be_overwritten_by_an_older_preparation() {
        let fixture = Fixture::new();
        let provider = ProviderId::parse("observer").expect("fixture provider");
        let drivers = NativeDrivers::default();
        let mut newer = inspect_program(&fixture.program)
            .await
            .expect("initial facts");
        newer.binary.size += 1;
        let result = drivers
            .get_or_prepare(provider, || async {
                let prepared = fixture.prepare(provider).await?;
                drivers
                    .record_preparation(provider, &Arc::new(newer.clone()))
                    .await;
                Ok::<_, ()>(prepared)
            })
            .await;
        assert!(result.is_err());
        assert!(
            matches!(drivers.drivers.lock().await.get(&provider), Some(NativeDriver::Prepared(facts)) if **facts == newer)
        );
    }

    #[tokio::test]
    async fn replacement_between_preparation_and_first_observation_is_refused() {
        let fixture = Fixture::new();
        let provider = ProviderId::parse("observer").expect("fixture provider");
        let drivers = NativeDrivers::default();
        let result = drivers
            .get_or_prepare(provider, || async {
                let prepared = fixture.prepare(provider).await?;
                std::fs::write(
                    fixture.program.path().as_std_path(),
                    "changed after the binary was probed",
                )
                .expect("replace during preparation");
                Ok::<_, ()>(prepared)
            })
            .await;
        assert!(result.is_err());
        assert!(drivers.drivers.lock().await.is_empty());
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn launcher_replacement_before_first_observation_cannot_seed_a_stale_observer() {
        let mut fixture = Fixture::new();
        let launcher = fixture.directory.join("observer.cmd");
        std::fs::write(
            &launcher,
            "@ECHO off\r\nSET dp0=%~dp0\r\n\"%dp0%\\observer.exe\" %*\r\n",
        )
        .expect("write launcher");
        fixture.program = runtrol_childproc::resolve(launcher.to_str().expect("UTF-8 launcher"))
            .expect("resolve original launcher");
        let provider = ProviderId::parse("observer").expect("fixture provider");
        let drivers = NativeDrivers::default();
        let result = drivers
            .get_or_prepare(provider, || async {
                let prepared = fixture.prepare(provider).await?;
                std::fs::write(
                    fixture.directory.join("replacement.exe"),
                    "replacement binary",
                )
                .expect("write replacement");
                std::fs::write(
                    &launcher,
                    "@ECHO off\r\nSET dp0=%~dp0\r\n\"%dp0%\\replacement.exe\" %*\r\n",
                )
                .expect("redirect launcher after preparation");
                assert_eq!(
                    binary_identity(
                        &inspect_program(&fixture.program)
                            .await
                            .expect("old binary remains")
                            .binary
                    )
                    .unwrap_or_else(|error| panic!("binary identity: {}", error.message())),
                    prepared.binary_identity,
                    "checking the binary alone would incorrectly accept this observer"
                );
                Ok::<_, ()>(prepared)
            })
            .await;
        assert!(result.is_err());
        assert!(drivers.drivers.lock().await.is_empty());
        fixture.program = runtrol_childproc::resolve(launcher.to_str().expect("UTF-8 launcher"))
            .expect("resolve changed launcher");
        let next = drivers
            .get_or_prepare(provider, || async { fixture.prepare(provider).await })
            .await
            .expect("fresh discovery can publish the replacement");
        assert_eq!(
            next.terminal_program
                .as_ref()
                .expect("resolved program")
                .path(),
            fixture.program.path()
        );
    }
}
