//! Absent logs are valid only while a complete search's directory notification remains unchanged.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use runtrol_childproc::watch::DirectoryChanges;
use runtrol_provider::NativeSessionId;

use super::{Followed, MAX_LOCK_ENTRIES, search_log};

#[derive(Debug, Default)]
pub(super) struct MissingLogs {
    watching: Option<(PathBuf, DirectoryChanges)>,
    absent: HashSet<Box<str>>,
    #[cfg(test)]
    searches: usize,
}

impl MissingLogs {
    pub(super) fn begin(
        &mut self,
        home: &Path,
        owned: &[NativeSessionId],
        followed: &HashMap<Box<str>, Followed>,
    ) {
        self.absent
            .retain(|thread| owned.iter().any(|owned| owned.as_str() == thread.as_ref()));
        if owned
            .iter()
            .all(|thread| followed.contains_key(thread.as_str()))
        {
            self.absent.clear();
            self.watching = None;
            return;
        }
        if let Some((root, watching)) = &mut self.watching {
            if root == home {
                match watching.changed() {
                    Ok(false) => return,
                    Ok(true) => {
                        self.absent.clear();
                        return;
                    }
                    Err(_) => self.absent.clear(),
                }
            }
            self.watching = None;
        }
        self.absent.clear();
        // Observe the home so creating the first sessions/day directory is also an invalidation.
        // Refused or unsupported notifications retain no absence and keep the existing bounded search.
        self.watching = match DirectoryChanges::new(home) {
            Ok(watching) => Some((home.to_owned(), watching)),
            Err(_) => None,
        };
    }

    pub(super) fn locate(&mut self, home: &Path, thread: &str) -> Option<PathBuf> {
        if self.absent.contains(thread) {
            return None;
        }
        #[cfg(test)]
        {
            self.searches += 1;
        }
        let (found, complete) = search_log(home, thread);
        if found.is_none()
            && complete
            && self.watching.is_some()
            && self.absent.len() < MAX_LOCK_ENTRIES
        {
            self.absent.insert(thread.into());
        }
        found
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::super::answering;
    use super::super::tests::{OPEN_THREAD, fixture_identity, home, opened};
    use super::*;

    #[test]
    fn an_incomplete_tree_never_supplies_cached_absence() {
        let (_scratch, root) = home(&[]);
        for index in 0..super::super::MAX_DAY_DIRECTORIES {
            std::fs::create_dir_all(root.join("sessions/2026/08").join(format!("extra-{index}")))
                .expect("create a tree beyond the existing day bound");
        }
        let owned = [NativeSessionId::new(OPEN_THREAD).expect("fixture identity")];
        let mut missing = MissingLogs::default();
        missing.begin(&root, &owned, &HashMap::new());
        assert!(missing.locate(&root, OPEN_THREAD).is_none());
        assert!(
            missing.absent.is_empty(),
            "an incomplete search is not evidence of absence"
        );
    }

    #[test]
    fn an_unchanged_missing_log_is_not_walked_again_and_creation_invalidates_it() {
        let (_scratch, root) = home(&[]);
        let owned = [NativeSessionId::new(OPEN_THREAD).expect("fixture identity")];
        let mut followed = HashMap::new();
        let mut missing = MissingLogs::default();
        missing.begin(&root, &owned, &followed);
        assert!(missing.locate(&root, OPEN_THREAD).is_none());
        let searches = missing.searches;
        for _ in 0..20 {
            missing.begin(&root, &owned, &followed);
            assert!(missing.locate(&root, OPEN_THREAD).is_none());
        }
        assert_eq!(
            missing.searches, searches,
            "no time window or repeated tree walk"
        );
        let day = root.join("sessions/2026/09/07");
        std::fs::create_dir_all(&day).expect("create a new day while the prompt is open");
        let log = day.join(format!("rollout-first-{OPEN_THREAD}.jsonl"));
        std::fs::write(&log, opened("first")).expect("the first turn creates its log");
        missing.begin(&root, &owned, &followed);
        assert_eq!(missing.locate(&root, OPEN_THREAD), Some(log));
        assert!(
            answering(
                &root,
                &mut followed,
                OPEN_THREAD,
                fixture_identity(1).expect("fixture owner"),
                &mut missing,
            ),
            "the first created log immediately publishes its open turn"
        );
        missing.begin(&root, &[], &followed);
        assert!(missing.absent.is_empty());
        assert!(
            missing.watching.is_none(),
            "the final owner releases the notification handle"
        );
    }
}
