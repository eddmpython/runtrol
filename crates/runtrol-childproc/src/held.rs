//! Whether another live process is holding a file it writes.
//!
//! A coding CLI that keeps one lock file per conversation it has open answers, for free and without being
//! asked, the question every surface here needs: which conversations does a live process own right now. The
//! lock is released when that process ends, however it ends, so a stale record cannot claim a dead process.
//!
//! Measured 2026-08-29 on Windows against the operator's own machine: of the seven lock files one CLI keeps,
//! the four whose conversation a live process owned refused an exclusive open and the three left by finished
//! processes did not. The conversation that was answering at that moment was among the four.
//!
//! # What this is not
//!
//! It does not read the file, take a lock anybody else waits on, or block. It asks once and answers now.

use std::path::Path;

/// Whether a live process holds this path in a way that excludes another writer.
///
/// `false` for a path that does not exist, cannot be opened, or is held by nobody. A caller that needs to
/// tell "nobody holds it" from "it is not there" asks the filesystem separately: for the question this
/// exists to answer, both mean the same thing.
#[must_use]
pub fn write_locked(path: &Path) -> bool {
    platform::write_locked(path)
}

#[cfg(windows)]
mod platform {
    use std::fs::OpenOptions;
    use std::os::windows::fs::OpenOptionsExt as _;
    use std::path::Path;

    /// Deny every other handle for the length of this open. A writer that already has the file open makes
    /// this fail with a sharing violation, which is the answer.
    const SHARE_NONE: u32 = 0;

    pub(super) fn write_locked(path: &Path) -> bool {
        if !path.exists() {
            return false;
        }
        OpenOptions::new()
            .read(true)
            .write(true)
            .share_mode(SHARE_NONE)
            .open(path)
            .is_err()
    }
}

#[cfg(unix)]
mod platform {
    use std::fs::OpenOptions;
    use std::os::fd::AsRawFd as _;
    use std::path::Path;

    pub(super) fn write_locked(path: &Path) -> bool {
        let Ok(file) = OpenOptions::new().read(true).open(path) else {
            return false;
        };
        // A shared, non-blocking test. It conflicts only with an exclusive holder, and it is taken for the
        // few microseconds before the file is dropped, so a writer asking for its own lock in that window is
        // the only cost. Asking for an exclusive test lock instead would conflict with other readers too.
        //
        // SAFETY: `fd` is owned by `file`, which outlives both calls, and `flock` touches nothing else.
        let held = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_SH | libc::LOCK_NB) } != 0;
        if !held {
            // SAFETY: the same live descriptor, releasing what the line above took.
            unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_UN) };
        }
        held
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    static NEXT: AtomicUsize = AtomicUsize::new(0);

    fn scratch(name: &str) -> std::path::PathBuf {
        let serial = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "runtrol-held-{}-{serial}-{name}",
            std::process::id()
        ));
        drop(fs::remove_file(&path));
        path
    }

    #[test]
    fn a_path_nobody_holds_is_not_locked() {
        let path = scratch("free");
        fs::write(&path, b"").expect("the scratch file is written");
        assert!(!write_locked(&path));
        drop(fs::remove_file(&path));
    }

    #[test]
    fn a_missing_path_is_not_locked() {
        assert!(!write_locked(&scratch("absent")));
    }

    /// The lock this reports is one another process holds. This process holding it is the closest a unit
    /// test can get on Windows, where an open handle is the lock; on Unix an advisory lock taken by this
    /// same process is re-entrant by design, so only the Windows half of the claim is asserted here and the
    /// cross-process half is measured against the real CLI (`codex/roster.rs`).
    #[cfg(windows)]
    #[test]
    fn a_path_this_process_holds_exclusively_is_locked() {
        use std::fs::OpenOptions;
        use std::os::windows::fs::OpenOptionsExt as _;

        let path = scratch("held");
        fs::write(&path, b"").expect("the scratch file is written");
        let holder = OpenOptions::new()
            .read(true)
            .write(true)
            .share_mode(0)
            .open(&path)
            .expect("this process takes the file");
        assert!(write_locked(&path));
        drop(holder);
        assert!(!write_locked(&path));
        drop(fs::remove_file(&path));
    }
}
