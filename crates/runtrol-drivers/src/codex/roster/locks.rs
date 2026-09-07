//! Writer binding invalidation from one bounded directory notification source.

use std::sync::Arc;

use async_trait::async_trait;
use runtrol_childproc::watch::{ChangedNames, DirectoryNames};
use runtrol_provider::{NativeActivityWatch, NativeSessionId, ProviderError, ProviderId};
use tokio::sync::Mutex;

use super::{CodexRoster, LOCK_EXTENSION, LOCKS_DIRECTORY};

const COORDINATION_LOCK: &str = ".coordination.lock";

pub(super) struct LockWatch {
    provider: ProviderId,
    source: Box<dyn NativeActivityWatch>,
    names: Arc<Mutex<Option<DirectoryNames>>>,
}

impl LockWatch {
    pub(super) fn new(
        provider: ProviderId,
        source: Box<dyn NativeActivityWatch>,
        names: Arc<Mutex<Option<DirectoryNames>>>,
    ) -> Self {
        Self {
            provider,
            source,
            names,
        }
    }
}

#[async_trait]
impl NativeActivityWatch for LockWatch {
    async fn changed(&mut self) -> Result<(), ProviderError> {
        let ready = self.names.lock().await.as_ref().map(DirectoryNames::wait);
        let Some(ready) = ready else {
            return self.source.changed().await;
        };
        // The parent notification and this read can complete in either order. Retain a separate
        // readiness wake so an early parent scan cannot strand a later completed names batch.
        tokio::select! {
            result = self.source.changed() => result,
            result = ready => result.map_err(|error| ProviderError::Protocol {
                provider: self.provider,
                doing: "watching writer lock ownership changes",
                detail: error.to_string(),
            }),
        }
    }
}

impl CodexRoster {
    pub(super) fn refresh_ownership(&self) {
        let Ok(home) = &self.home else {
            return;
        };
        let mut notifications = self.lock_changes.blocking_lock();
        let changed = match notifications.as_mut() {
            Some(watch) => {
                if let Ok(changed) = watch.changed() {
                    changed
                } else {
                    // Lost notification proof invalidates every retained binding before reinstalling.
                    *notifications = None;
                    Some(ChangedNames::Rescan)
                }
            }
            None => Some(ChangedNames::Rescan),
        };
        if notifications.is_none()
            && let Ok(watch) = DirectoryNames::new(&home.join(LOCKS_DIRECTORY))
        {
            // Arm before the snapshot. An unavailable root never preserves cached ownership.
            *notifications = Some(watch);
        }
        let mut bound = self.bound.blocking_lock();
        let dead = bound
            .values()
            .any(|binding| (self.identify)(binding.identity.pid()) != Some(binding.identity));
        if changed.is_some() || dead {
            *self.owned.blocking_lock() = None;
        }
        match changed {
            Some(ChangedNames::Names(names)) => {
                for name in names {
                    if name
                        .to_str()
                        .is_some_and(|name| name.eq_ignore_ascii_case(COORDINATION_LOCK))
                    {
                        continue;
                    }
                    let Some(thread) = lock_name(&name) else {
                        bound.clear();
                        break;
                    };
                    // Windows may change casing. Every notification, including a write or both sides
                    // of a rename, invalidates this binding even if its previous process is alive.
                    bound.retain(|known, _| !known.eq_ignore_ascii_case(thread.as_str()));
                }
            }
            Some(ChangedNames::Rescan) => bound.clear(),
            None => {}
        }
    }
}

#[expect(
    clippy::manual_ok_err,
    reason = "a rejected name explicitly forces a complete cache invalidation; Result::ok is forbidden by the project"
)]
pub(super) fn lock_name(name: &std::ffi::OsStr) -> Option<NativeSessionId> {
    let name = name.to_str()?;
    let (stem, extension) = name.rsplit_once('.')?;
    // A four-character extension cannot be an 8.3 alias. Unmappable notifications, including
    // short aliases, must invalidate the whole cache rather than silently preserving a binding.
    if !extension.eq_ignore_ascii_case(LOCK_EXTENSION) || stem.contains(['/', '\\', ':', '\0']) {
        return None;
    }
    let Ok(identity) = uuid::Uuid::parse_str(stem) else {
        return None;
    };
    match NativeSessionId::new(&identity.to_string()) {
        Ok(native) => Some(native),
        Err(_) => None,
    }
}

#[cfg(all(test, windows))]
#[expect(
    clippy::expect_used,
    reason = "fixture setup and bounded waits must fail the regression on error"
)]
mod tests {
    use super::super::{
        Bound, holder,
        tests::{OPEN_THREAD, fixture_identity, home},
    };
    use super::*;
    use std::fs;
    use std::time::Duration;

    const OTHER: &str = "01a0471d-786c-7561-a8d5-db5ddb837c0c";

    fn wait(roster: &CodexRoster) {
        let ready = roster
            .lock_changes
            .blocking_lock()
            .as_ref()
            .expect("armed names")
            .wait();
        tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("fixture runtime")
            .block_on(async {
                tokio::time::timeout(Duration::from_secs(2), ready)
                    .await
                    .expect("notification deadline")
                    .expect("ready");
            });
    }

    fn retain(roster: &CodexRoster, thread: &str) {
        roster.bound.blocking_lock().insert(
            thread.into(),
            Bound {
                identity: fixture_identity(std::process::id()).expect("live fixture"),
            },
        );
    }

    #[test]
    fn unrelated_names_keep_holder_but_same_name_replacement_and_writes_invalidate() {
        let (_kept, root) = home(&[(OPEN_THREAD, String::new())]);
        let roster = CodexRoster::rooted(root.clone());
        roster.refresh_ownership();
        retain(&roster, OPEN_THREAD);
        let locks = root.join(LOCKS_DIRECTORY);
        fs::write(locks.join(format!("{OTHER}.lock")), b"").expect("unrelated lock");
        wait(&roster);
        roster.refresh_ownership();
        assert!(
            holder(
                |_| panic!("an unrelated lock must not respawn the holder helper"),
                fixture_identity,
                &locks,
                &mut roster.bound.blocking_lock(),
                OPEN_THREAD,
            )
            .is_some()
        );

        // Refresh the OS baseline so duplicate creation events cannot mask the replacement event.
        *roster.lock_changes.blocking_lock() =
            Some(DirectoryNames::new(&locks).expect("replacement baseline"));
        let lock = locks.join(format!("{OPEN_THREAD}.lock"));
        fs::remove_file(&lock).expect("same name removal with old process still alive");
        fs::write(&lock, b"").expect("same name replacement");
        wait(&roster);
        roster.refresh_ownership();
        assert!(roster.bound.blocking_lock().is_empty());

        *roster.lock_changes.blocking_lock() =
            Some(DirectoryNames::new(&locks).expect("write baseline"));
        retain(&roster, OPEN_THREAD);
        fs::write(&lock, b"new").expect("same name write");
        wait(&roster);
        roster.refresh_ownership();
        assert!(roster.bound.blocking_lock().is_empty());
    }

    #[test]
    fn coordination_is_not_a_conversation_and_does_not_invalidate_writers() {
        use std::os::windows::fs::OpenOptionsExt as _;

        let (_kept, root) = home(&[(OPEN_THREAD, String::new())]);
        let roster = CodexRoster::rooted(root.clone());
        roster.refresh_ownership();
        retain(&roster, OPEN_THREAD);
        let coordination = root.join(LOCKS_DIRECTORY).join(COORDINATION_LOCK);
        fs::write(&coordination, b"").expect("coordination creation");
        wait(&roster);
        roster.refresh_ownership();
        assert!(roster.bound.blocking_lock().contains_key(OPEN_THREAD));
        let _coordination_owner = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&coordination)
            .expect("live coordination owner");
        assert!(
            super::super::owned_threads(
                ProviderId::parse("codex").expect("fixture provider"),
                &root.join(LOCKS_DIRECTORY)
            )
            .expect("read held files")
            .is_empty(),
            "coordination is never a live conversation"
        );
        assert!(lock_name(std::ffi::OsStr::new(COORDINATION_LOCK)).is_none());
        assert!(lock_name(std::ffi::OsStr::new("not-a-thread.lock")).is_none());
    }

    #[test]
    fn rename_invalidates_both_bindings_and_alias_invalidates_all() {
        let (_kept, root) = home(&[(OPEN_THREAD, String::new())]);
        let roster = CodexRoster::rooted(root.clone());
        roster.refresh_ownership();
        retain(&roster, OPEN_THREAD);
        retain(&roster, OTHER);
        let locks = root.join(LOCKS_DIRECTORY);
        fs::rename(
            locks.join(format!("{OPEN_THREAD}.lock")),
            locks.join(format!("{OTHER}.lock")),
        )
        .expect("rename lock");
        wait(&roster);
        roster.refresh_ownership();
        assert!(roster.bound.blocking_lock().is_empty());
        *roster.lock_changes.blocking_lock() =
            Some(DirectoryNames::new(&locks).expect("alias baseline"));
        retain(&roster, OPEN_THREAD);
        fs::write(locks.join("EXTERN~9.LOC"), b"").expect("unmappable notification");
        wait(&roster);
        roster.refresh_ownership();
        assert!(roster.bound.blocking_lock().is_empty());
    }
}
