//! Provider-owned structural revisions and waits on their original filesystem/process sources.

#![expect(
    clippy::disallowed_types,
    reason = "this synchronous metadata mutex is never held across an await or an I/O operation"
)]

use std::future::{Future, poll_fn};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::Poll;

use async_trait::async_trait;
use runtrol_childproc::{
    ProcessExit,
    watch::{ChangedNames, DirectoryChanges, DirectoryNames},
};
use runtrol_provider::{NativeActivityWatch, ProcessIdentity, ProviderError, ProviderId};
use sha2::{Digest as _, Sha256};

use crate::roster_scan::RosterScan;

/// A prepared driver whose native observation has no changing source.
pub(crate) struct Unchanging;

#[async_trait]
impl NativeActivityWatch for Unchanging {
    async fn changed(&mut self) -> Result<(), ProviderError> {
        std::future::pending().await
    }
}

/// Optional filesystem invalidation keys. An unsupported timestamp supplies no model-state evidence.
#[expect(
    clippy::manual_ok_err,
    reason = "unsupported filesystem timestamps explicitly disable that invalidation key; the project forbids silently discarding errors with Result::ok"
)]
pub(crate) fn source_times(
    metadata: &std::fs::Metadata,
) -> (Option<std::time::SystemTime>, Option<std::time::SystemTime>) {
    let created = match metadata.created() {
        Ok(value) => Some(value),
        Err(_) => None,
    };
    let modified = match metadata.modified() {
        Ok(value) => Some(value),
        Err(_) => None,
    };
    (created, modified)
}

/// Revision of a provider-owned catalogue metadata index, without reading any indexed value.
pub(crate) fn catalogue_stamp(path: &Path) -> std::io::Result<[u8; 32]> {
    let mut stamp = Sha256::new();
    match std::fs::metadata(path) {
        Ok(metadata) => {
            stamp.update([1]);
            stamp.update(metadata.len().to_le_bytes());
            let modified = metadata
                .modified()?
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(std::io::Error::other)?;
            stamp.update(modified.as_nanos().to_le_bytes());
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => stamp.update([0]),
        Err(error) => return Err(error),
    }
    Ok(stamp.finalize().into())
}

/// Only a digest of allowed structural metadata is retained here, never its source bytes.
#[derive(Debug, Default)]
struct Observed {
    fingerprint: Option<[u8; 32]>,
    revision: u64,
    processes: Vec<ProcessIdentity>,
}

/// Shared by one prepared driver's observer and its source watcher.
#[derive(Clone, Debug, Default)]
pub(crate) struct NativeSource(Arc<Mutex<Observed>>);

impl NativeSource {
    /// Publish metadata after a complete bounded observation. Byte growth alone is never a fingerprint.
    pub(crate) fn record(
        &self,
        provider: ProviderId,
        fingerprint: [u8; 32],
        processes: Vec<ProcessIdentity>,
    ) -> Result<u64, ProviderError> {
        let mut observed = self.0.lock().map_err(|error| failure(provider, error))?;
        if observed.fingerprint != Some(fingerprint) {
            observed.revision = observed.revision.checked_add(1).ok_or_else(|| {
                failure(provider, "the native metadata revision space is exhausted")
            })?;
            observed.fingerprint = Some(fingerprint);
        }
        observed.processes = processes;
        Ok(observed.revision)
    }

    /// Install notifications before the first observation. Called inside the driver's blocking admission.
    pub(crate) fn watch(
        &self,
        provider: ProviderId,
        root: &Path,
        scan: &'static RosterScan,
        scopes: &[&'static str],
    ) -> Result<Box<dyn NativeActivityWatch>, ProviderError> {
        let directory = match std::fs::metadata(root) {
            Ok(_) => Some(Arc::new(FilteredDirectory {
                root: std::fs::canonicalize(root).map_err(|error| failure(provider, error))?,
                names: Mutex::new(
                    DirectoryNames::with_subtree(root, true)
                        .map_err(|error| failure(provider, error))?,
                ),
                scopes: scopes.to_vec(),
                pending: AtomicBool::new(false),
            })),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(failure(provider, error)),
        };
        let mut parent = root
            .parent()
            .ok_or_else(|| failure(provider, "the provider observation root has no parent"))?;
        while !parent.is_dir() {
            parent = parent.parent().ok_or_else(|| {
                failure(
                    provider,
                    "the provider observation root has no observable ancestor",
                )
            })?;
        }
        // A directory notification does not signal replacement of its own root. Its parent's names
        // provide that wake; the retained directory identity refuses a replacement on the next scan.
        let ancestor = DirectoryChanges::with_names(parent, false)
            .map_err(|error| failure(provider, error))?;
        Ok(Box::new(SourceWatch {
            provider,
            source: self.clone(),
            directory,
            ancestor,
            scan,
            processes: Vec::new(),
        }))
    }
}

struct FilteredDirectory {
    root: PathBuf,
    names: Mutex<DirectoryNames>,
    scopes: Vec<&'static str>,
    pending: AtomicBool,
}

impl FilteredDirectory {
    fn relevant(&self, batch: ChangedNames) -> bool {
        let ChangedNames::Names(names) = batch else {
            return true;
        };
        names.into_iter().any(|name| {
            let path = Path::new(&name);
            let mut components = path.components();
            let Some(std::path::Component::Normal(first)) = components.next() else {
                return true;
            };
            if components.any(|component| !matches!(component, std::path::Component::Normal(_))) {
                return true;
            }
            let matches = |name: &std::ffi::OsStr| {
                name.to_str().is_none_or(|name| {
                    self.scopes
                        .iter()
                        .any(|scope| name.eq_ignore_ascii_case(scope))
                })
            };
            if matches(first) {
                return true;
            }
            // Windows may report an 8.3 alias. Resolve an unrelated first component before excluding
            // it. Deleted, inaccessible or redirected components retain full invalidation proof.
            let Ok(long) = std::fs::canonicalize(self.root.join(first)) else {
                return true;
            };
            if long.parent() != Some(self.root.as_path()) {
                return true;
            }
            long.file_name().is_none_or(matches)
        })
    }
}

struct SourceWatch {
    provider: ProviderId,
    source: NativeSource,
    directory: Option<Arc<FilteredDirectory>>,
    ancestor: DirectoryChanges,
    scan: &'static RosterScan,
    processes: Vec<(ProcessIdentity, Arc<ProcessExit>)>,
}

#[async_trait]
impl NativeActivityWatch for SourceWatch {
    async fn changed(&mut self) -> Result<(), ProviderError> {
        type Wait<'a> = Pin<Box<dyn Future<Output = std::io::Result<()>> + Send + 'a>>;
        let identities = self
            .source
            .0
            .lock()
            .map_err(|error| failure(self.provider, error))?
            .processes
            .clone();
        self.processes
            .retain(|(identity, _)| identities.contains(identity));
        let missing: Vec<_> = identities
            .into_iter()
            .filter(|identity| !self.processes.iter().any(|(known, _)| known == identity))
            .collect();
        if !missing.is_empty() {
            let provider = self.provider;
            let fresh = self
                .scan
                .run(
                    provider,
                    "retaining native process completion events",
                    move || {
                        missing
                            .into_iter()
                            .map(|identity| {
                                ProcessExit::open(identity)
                                    .map(|process| (identity, Arc::new(process)))
                                    .map_err(|error| failure(provider, error))
                            })
                            .collect::<Result<Vec<_>, _>>()
                    },
                )
                .await?;
            self.processes.extend(fresh);
        }
        loop {
            let root_missing = self.directory.is_none();
            let mut waits: Vec<Wait<'_>> = vec![Box::pin(self.ancestor.wait())];
            if let Some(directory) = &self.directory {
                if directory.pending.swap(false, Ordering::AcqRel) {
                    return Ok(());
                }
                let owned = Arc::clone(directory);
                let provider = self.provider;
                let ready = self
                    .scan
                    .run(
                        provider,
                        "filtering native source notifications",
                        move || {
                            let mut names = owned
                                .names
                                .lock()
                                .map_err(|error| failure(provider, error))?;
                            if let Some(batch) =
                                names.changed().map_err(|error| failure(provider, error))?
                                && owned.relevant(batch)
                            {
                                // The worker owns consumed evidence even if its caller is cancelled.
                                owned.pending.store(true, Ordering::Release);
                            }
                            Ok(names.wait())
                        },
                    )
                    .await?;
                if directory.pending.swap(false, Ordering::AcqRel) {
                    return Ok(());
                }
                waits.push(Box::pin(ready));
            }
            waits.extend(
                self.processes
                    .iter()
                    .map(|(_, process)| Box::pin(process.wait()) as Wait<'_>),
            );
            let winner = poll_fn(|context| {
                for (index, wait) in waits.iter_mut().enumerate() {
                    if let Poll::Ready(answer) = wait.as_mut().poll(context) {
                        return Poll::Ready(answer.map(|()| index));
                    }
                }
                Poll::Pending
            })
            .await
            .map_err(|error| failure(self.provider, error))?;
            drop(waits);
            if root_missing {
                return Err(failure(
                    self.provider,
                    "the absent source path changed and must be rearmed",
                ));
            }
            if winner != 1 {
                return Ok(());
            }
            // One completed names batch is consumed on the next iteration. Unrelated writes only
            // return to the kernel wait; they never request a new provider observation.
        }
    }
}

fn failure(provider: ProviderId, error: impl std::fmt::Display) -> ProviderError {
    ProviderError::Protocol {
        provider,
        doing: "watching provider-owned native structural metadata",
        detail: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[tokio::test]
    async fn an_absent_provider_home_waits_for_each_new_ancestor_without_idle_retries() {
        static SCAN: std::sync::LazyLock<RosterScan> =
            std::sync::LazyLock::new(RosterScan::default);
        let parent = std::env::temp_dir().join(format!(
            "native-missing-{}",
            runtrol_provider::TerminalId::now()
        ));
        std::fs::create_dir(&parent).expect("isolated ancestor");
        let middle = parent.join("configuration");
        let root = middle.join("provider");
        let source = NativeSource::default();
        let provider = ProviderId::parse("fixture").expect("provider identity");
        let mut watcher = source
            .watch(provider, &root, &SCAN, &["structural.json"])
            .expect("nearest ancestor watches");
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(30), watcher.changed())
                .await
                .is_err()
        );
        std::fs::create_dir(&middle).expect("first new ancestor");
        tokio::time::timeout(std::time::Duration::from_secs(2), watcher.changed())
            .await
            .expect("ancestor creation wakes")
            .expect_err("a missing path requires rearming at its new nearest ancestor");
        watcher = source
            .watch(provider, &root, &SCAN, &["structural.json"])
            .expect("new nearest ancestor watches");
        std::fs::create_dir(&root).expect("provider home appears");
        tokio::time::timeout(std::time::Duration::from_secs(2), watcher.changed())
            .await
            .expect("root creation wakes")
            .expect_err("the new root requires its own write notification");
        watcher = source
            .watch(provider, &root, &SCAN, &["structural.json"])
            .expect("new provider root watches");
        std::fs::write(root.join("structural.json"), b"{}").expect("provider source writes");
        tokio::time::timeout(std::time::Duration::from_secs(2), watcher.changed())
            .await
            .expect("source write wakes")
            .expect("source is readable");
        drop(watcher);
        std::fs::remove_dir_all(parent).expect("all source handles close before cleanup");
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn idle_source_wait_survives_cancellation_and_observes_an_in_place_write() {
        use std::io::Write as _;
        static SCAN: std::sync::LazyLock<RosterScan> =
            std::sync::LazyLock::new(RosterScan::default);
        let parent = std::env::temp_dir().join(format!(
            "native-source-{}",
            runtrol_provider::TerminalId::now()
        ));
        let root = parent.join("provider");
        std::fs::create_dir_all(&root).expect("isolated source");
        let path = root.join("structural.json");
        std::fs::write(&path, b"{}").expect("source exists before watch installation");
        let source = NativeSource::default();
        let provider = ProviderId::parse("fixture").expect("provider identity");
        let process = runtrol_childproc::process_identity(std::process::id())
            .expect("exact fixture identity");
        source
            .record(provider, [1; 32], vec![process])
            .expect("first structural observation");
        let mut watcher = source
            .watch(provider, &root, &SCAN, &["structural.json"])
            .expect("native source watches");
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(30), watcher.changed())
                .await
                .is_err()
        );
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("same source opens");
        file.write_all(b"\n{}")
            .expect("provider source writes in place");
        drop(file);
        tokio::time::timeout(std::time::Duration::from_secs(2), watcher.changed())
            .await
            .expect("the source signals without a polling timer")
            .expect("the original source remains available");
        assert_eq!(
            source
                .record(provider, [1; 32], vec![process])
                .expect("metadata stayed equal"),
            1
        );
        drop(watcher);
        std::fs::remove_dir_all(&parent).expect("all kernel waits release before cleanup");
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn unrelated_writes_park_but_nested_metadata_and_rename_still_wake() {
        static SCAN: std::sync::LazyLock<RosterScan> =
            std::sync::LazyLock::new(RosterScan::default);
        let parent = std::env::temp_dir().join(format!(
            "native-filter-{}",
            runtrol_provider::TerminalId::now()
        ));
        let root = parent.join("provider");
        std::fs::create_dir_all(root.join("metadata/day")).expect("structural subtree");
        std::fs::create_dir_all(root.join("cache")).expect("unrelated subtree");
        let source = NativeSource::default();
        let provider = ProviderId::parse("fixture").expect("fixture identity");
        let mut watcher = source
            .watch(provider, &root, &SCAN, &["metadata", "index.jsonl"])
            .expect("bounded source notification");
        std::fs::write(root.join("cache/noise.bin"), b"synthetic").expect("unrelated write");
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(60), watcher.changed())
                .await
                .is_err(),
            "an unrelated recursive write must not request a full observation"
        );
        std::fs::write(root.join("metadata/day/turn.jsonl"), b"{}\n")
            .expect("nested structural write");
        tokio::time::timeout(std::time::Duration::from_secs(2), watcher.changed())
            .await
            .expect("nested metadata deadline")
            .expect("cancelled wait retains the next change");
        drop(watcher);
        let mut watcher = source
            .watch(provider, &root, &SCAN, &["metadata", "index.jsonl"])
            .expect("new watch before rename");
        std::fs::rename(root.join("metadata"), root.join("replaced"))
            .expect("move original structure");
        tokio::time::timeout(std::time::Duration::from_secs(2), watcher.changed())
            .await
            .expect("rename deadline")
            .expect("the old structural name invalidates the observation");
        drop(watcher);
        std::fs::remove_dir_all(parent).expect("close pending I/O before exact fixture cleanup");
    }

    #[test]
    fn unchanged_metadata_reuses_its_revision_and_new_metadata_advances_once() {
        let source = NativeSource::default();
        let provider = ProviderId::parse("fixture").expect("fixture identity");
        assert_eq!(
            source.record(provider, [1; 32], Vec::new()).expect("first"),
            1
        );
        assert_eq!(
            source.record(provider, [1; 32], Vec::new()).expect("same"),
            1
        );
        assert_eq!(
            source
                .record(provider, [2; 32], Vec::new())
                .expect("changed"),
            2
        );
        assert_eq!(
            source.record(provider, [2; 32], Vec::new()).expect("same"),
            2
        );
    }
}
