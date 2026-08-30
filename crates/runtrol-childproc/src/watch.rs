//! Notice when a directory gains or loses a file, at no cost while nothing does.
//!
//! A coding CLI that keeps one file per open conversation says, by creating and removing those files, exactly
//! when its set of open conversations changed. Asking on a clock costs the machine something forever and still
//! answers late; waiting on the change costs nothing until it happens and answers at once.
//!
//! Measured 2026-08-30 on this filesystem: a directory's modification time moves when a file is created (yes)
//! and removed (yes) and not when one is written to (no), which is the same shape the notification below has.
//! Both are the right signal for "a session started or ended" and the wrong one for "a session said something",
//! which the provider's own turn boundaries answer instead.
//!
//! # What this is not
//!
//! It does not say what changed, read any file, or watch subdirectories. It says "look again".

use std::path::Path;

/// How long one wait lasts before it is renewed.
///
/// The renewal exists so a watcher whose reader has gone away ends within a bounded time, and as a floor under
/// a notification the operating system never delivers. Long enough that an idle machine wakes twice a minute,
/// which is far inside the one percent of one CPU an idle Runtime is held to.
const RENEW_AFTER_SECONDS: u32 = 30;

/// Watch one directory's file set. `None` when the path cannot be watched, which a caller reads as "this
/// surface cannot be waited on", never as "nothing will change".
///
/// The receiver yields once per observed change, coalesced: a burst of creations may arrive as one message,
/// because the answer to every one of them is the same single act of looking again. It closes when the
/// watching thread ends, which happens within [`RENEW_AFTER_SECONDS`] of the receiver being dropped.
#[must_use]
pub fn watch_directory(directory: &Path) -> Option<tokio::sync::mpsc::Receiver<()>> {
    platform::watch_directory(directory)
}

#[cfg(windows)]
mod platform {
    use std::path::Path;

    use windows_sys::Win32::Foundation::{HANDLE, WAIT_FAILED, WAIT_OBJECT_0};
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_NOTIFY_CHANGE_DIR_NAME, FILE_NOTIFY_CHANGE_FILE_NAME, FindCloseChangeNotification,
        FindFirstChangeNotificationW, FindNextChangeNotification,
    };
    use windows_sys::Win32::System::Threading::WaitForSingleObject;

    use super::RENEW_AFTER_SECONDS;

    /// One slot: every pending change asks for the same one act of looking again, so a full channel already
    /// carries the message a second one would.
    const ONE_PENDING_LOOK: usize = 1;

    #[expect(
        unsafe_code,
        reason = "the change notification family is a C API with no safe wrapper; each call is documented at its site"
    )]
    pub(super) fn watch_directory(directory: &Path) -> Option<tokio::sync::mpsc::Receiver<()>> {
        use std::os::windows::ffi::OsStrExt as _;

        if !directory.is_dir() {
            return None;
        }
        let wide: Vec<u16> = directory
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        // SAFETY: a null-terminated path, no subtree, and the two filters this watch is about.
        let handle = unsafe {
            FindFirstChangeNotificationW(
                wide.as_ptr(),
                0,
                FILE_NOTIFY_CHANGE_FILE_NAME | FILE_NOTIFY_CHANGE_DIR_NAME,
            )
        };
        if handle.is_null() || handle == windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
            return None;
        }
        let (sender, receiver) = tokio::sync::mpsc::channel(ONE_PENDING_LOOK);
        // Owned before the move, so the thread receives one value that owns the handle rather than a raw
        // pointer it would have to be trusted with.
        let owned = Owned(handle);
        // A thread of its own because the wait blocks with no cost, which is the whole point: an executor
        // thread may not be held that way, and a task that polls would spend the machine it is saving.
        let watching = std::thread::Builder::new()
            .name("runtrol-directory-watch".to_owned())
            .spawn(move || {
                // Named whole so the closure captures the owner rather than the handle field: precise field
                // capture would move a raw pointer across threads, which is the thing this type exists to stop.
                let owned = owned;
                loop {
                    // SAFETY: the handle is owned here for the length of this thread.
                    let waited = unsafe {
                        WaitForSingleObject(owned.0, RENEW_AFTER_SECONDS.saturating_mul(1000))
                    };
                    if waited == WAIT_FAILED {
                        return;
                    }
                    if waited == WAIT_OBJECT_0 {
                        match sender.try_send(()) {
                            // A full channel already carries the same message.
                            Ok(()) | Err(tokio::sync::mpsc::error::TrySendError::Full(())) => {}
                            Err(tokio::sync::mpsc::error::TrySendError::Closed(())) => return,
                        }
                        // SAFETY: renewing the same owned handle, which the next wait uses.
                        if unsafe { FindNextChangeNotification(owned.0) } == 0 {
                            return;
                        }
                    } else if sender.is_closed() {
                        return;
                    }
                }
            });
        match watching {
            Ok(_) => Some(receiver),
            // The thread never started, so the handle is closed by dropping the value that owns it.
            Err(_) => None,
        }
    }

    /// The notification handle, closed exactly once when the watching thread ends.
    struct Owned(HANDLE);

    #[expect(
        unsafe_code,
        reason = "a notification handle is a kernel object with no thread affinity; this type is the proof that exactly one thread owns it"
    )]
    // SAFETY: the handle is created before the thread starts and touched only by that thread afterwards, and
    // this type is the only path to it.
    unsafe impl Send for Owned {}

    impl Drop for Owned {
        #[expect(
            unsafe_code,
            reason = "closing a change notification handle is a kernel call with no safe wrapper"
        )]
        fn drop(&mut self) {
            // SAFETY: this type owns the handle and this runs once.
            unsafe { FindCloseChangeNotification(self.0) };
        }
    }
}

#[cfg(not(windows))]
mod platform {
    use std::path::Path;

    /// Unix has no portable wait-for-directory-change in std. `inotify` is Linux only and `kqueue` is BSD
    /// only, so each would be a dependency measured against a real user of it. Answering `None` keeps the
    /// caller honest: this surface cannot be waited on here, and the requests that can see a change still do.
    pub(super) const fn watch_directory(
        _directory: &Path,
    ) -> Option<tokio::sync::mpsc::Receiver<()>> {
        None
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;
    use std::{fs, thread};

    use super::*;

    static NEXT: AtomicU64 = AtomicU64::new(0);

    fn scratch() -> PathBuf {
        let serial = NEXT.fetch_add(1, Ordering::Relaxed);
        let root =
            std::env::temp_dir().join(format!("runtrol-watch-{}-{serial}", std::process::id()));
        fs::create_dir_all(&root).expect("the scratch directory is created");
        root
    }

    #[test]
    fn a_path_that_is_not_a_directory_cannot_be_watched() {
        assert!(watch_directory(&scratch().join("absent")).is_none());
    }

    /// A file appearing is what "a session started" looks like, and it must arrive without anybody asking.
    #[cfg(windows)]
    #[test]
    fn a_file_appearing_wakes_the_watcher() {
        let root = scratch();
        let mut changes = watch_directory(&root).expect("a real directory is watchable");
        // The watcher is already waiting; the write below is the event it is waiting for.
        let writing = root.clone();
        let writer = thread::spawn(move || {
            thread::sleep(Duration::from_millis(120));
            fs::write(writing.join("session.json"), b"{}").expect("the file is created");
        });
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("a test runtime is built");
        let woken = runtime.block_on(async {
            tokio::time::timeout(Duration::from_secs(10), changes.recv())
                .await
                .is_ok()
        });
        writer.join().expect("the writer finishes");
        assert!(woken, "creating a file did not wake the watch");
        drop(fs::remove_dir_all(&root));
    }
}
