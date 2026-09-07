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
//! It does not say what changed or read any file. It says "look again". The asynchronous watcher observes
//! one directory; [`DirectoryChanges`] observes a subtree without starting a watching thread.

use std::path::Path;

/// How long one wait lasts before it is renewed.
///
/// The renewal exists so a watcher whose reader has gone away ends within a bounded time, and as a floor under
/// a notification the operating system never delivers. Long enough that an idle machine wakes twice a minute,
/// which is far inside the one percent of one CPU an idle Runtime is held to.
#[cfg(windows)]
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

/// A caller-polled subtree notification, bound to the directory's exact kernel identity.
///
/// No thread or timer is allocated. A failed check invalidates any cached absence just like a change does.
#[derive(Debug)]
pub struct DirectoryChanges(platform::Changes);

impl DirectoryChanges {
    /// Begin observing file and directory names before taking a filesystem snapshot.
    ///
    /// # Errors
    /// Returns an error when the directory cannot be observed or the platform has no notification surface.
    pub fn new(directory: &Path) -> std::io::Result<Self> {
        platform::Changes::new(directory).map(Self)
    }

    /// Whether a name changed since the preceding successful check, without waiting.
    ///
    /// # Errors
    /// Returns an error if the original directory was replaced or its notification can no longer be trusted.
    pub fn changed(&mut self) -> std::io::Result<bool> {
        self.0.changed()
    }
}

#[cfg(windows)]
mod platform {
    use std::path::Path;

    use windows_sys::Win32::Foundation::{HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT};
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_NOTIFY_CHANGE_DIR_NAME, FILE_NOTIFY_CHANGE_FILE_NAME, FindCloseChangeNotification,
        FindFirstChangeNotificationW, FindNextChangeNotification,
    };
    use windows_sys::Win32::System::Threading::WaitForSingleObject;

    use super::RENEW_AFTER_SECONDS;

    /// One slot: every pending change asks for the same one act of looking again, so a full channel already
    /// carries the message a second one would.
    const ONE_PENDING_LOOK: usize = 1;

    pub(super) fn watch_directory(directory: &Path) -> Option<tokio::sync::mpsc::Receiver<()>> {
        let Ok(owned) = Owned::open(directory, false) else {
            return None;
        };
        let (sender, receiver) = tokio::sync::mpsc::channel(ONE_PENDING_LOOK);
        let watching = std::thread::Builder::new()
            .name("runtrol-directory-watch".to_owned())
            .spawn(move || {
                let mut owned = owned;
                loop {
                    match owned.changed(RENEW_AFTER_SECONDS.saturating_mul(1000)) {
                        Ok(true) => match sender.try_send(()) {
                            Ok(()) | Err(tokio::sync::mpsc::error::TrySendError::Full(())) => {}
                            Err(tokio::sync::mpsc::error::TrySendError::Closed(())) => return,
                        },
                        Ok(false) if !sender.is_closed() => {}
                        Ok(false) | Err(_) => return,
                    }
                }
            });
        match watching {
            Ok(_) => Some(receiver),
            // The thread never started, so the handle is closed by dropping the value that owns it.
            Err(_) => None,
        }
    }

    impl Owned {
        #[expect(
            unsafe_code,
            reason = "the notification API borrows the terminated path and returns one owned handle"
        )]
        fn open(directory: &Path, subtree: bool) -> std::io::Result<Self> {
            use std::os::windows::ffi::OsStrExt as _;

            if !directory.is_absolute() || !directory.is_dir() {
                return Err(std::io::Error::other(
                    "the notification root is not an absolute directory",
                ));
            }
            let wide: Vec<u16> = directory
                .as_os_str()
                .encode_wide()
                .chain(std::iter::once(0))
                .collect();
            // SAFETY: the terminated path remains live and the flags request only name changes.
            let handle = unsafe {
                FindFirstChangeNotificationW(
                    wide.as_ptr(),
                    i32::from(subtree),
                    FILE_NOTIFY_CHANGE_FILE_NAME | FILE_NOTIFY_CHANGE_DIR_NAME,
                )
            };
            if handle.is_null() || handle == windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
                return Err(std::io::Error::last_os_error());
            }
            Ok(Self(handle))
        }

        #[expect(
            unsafe_code,
            reason = "one owned notification is waited on and rearmed only after it signals"
        )]
        fn changed(&mut self, timeout_ms: u32) -> std::io::Result<bool> {
            // SAFETY: the notification remains owned for the complete wait.
            match unsafe { WaitForSingleObject(self.0, timeout_ms) } {
                WAIT_TIMEOUT => Ok(false),
                WAIT_OBJECT_0 => {
                    // SAFETY: rearm follows a signalled wait on this same live handle.
                    if unsafe { FindNextChangeNotification(self.0) } == 0 {
                        Err(std::io::Error::last_os_error())
                    } else {
                        Ok(true)
                    }
                }
                _ => Err(std::io::Error::last_os_error()),
            }
        }
    }

    /// The notification handle, closed exactly once when the watching thread ends.
    #[derive(Debug)]
    struct Owned(HANDLE);

    #[derive(Debug)]
    pub(super) struct Changes {
        directory: std::path::PathBuf,
        identity: (u64, [u8; 16]),
        notification: Owned,
    }

    impl Changes {
        pub(super) fn new(directory: &Path) -> std::io::Result<Self> {
            require_local_directory(directory)?;
            let identity = directory_identity(directory)?;
            let notification = Owned::open(directory, true)?;
            if directory_identity(directory)? != identity {
                return Err(std::io::Error::other(
                    "the notification root changed while opening it",
                ));
            }
            Ok(Self {
                directory: directory.to_owned(),
                identity,
                notification,
            })
        }

        pub(super) fn changed(&mut self) -> std::io::Result<bool> {
            if directory_identity(&self.directory)? != self.identity {
                return Err(std::io::Error::other("the notification root was replaced"));
            }
            self.notification.changed(0)
        }
    }

    #[expect(
        unsafe_code,
        reason = "GetDriveTypeW only reads the terminated drive root"
    )]
    fn require_local_directory(directory: &Path) -> std::io::Result<()> {
        use std::path::{Component, Prefix};
        use windows_sys::Win32::Storage::FileSystem::GetDriveTypeW;
        use windows_sys::Win32::System::WindowsProgramming::DRIVE_FIXED;
        let canonical = std::fs::canonicalize(directory)?;
        let Some(Component::Prefix(prefix)) = canonical.components().next() else {
            return Err(std::io::ErrorKind::Unsupported.into());
        };
        let (Prefix::Disk(drive) | Prefix::VerbatimDisk(drive)) = prefix.kind() else {
            return Err(std::io::ErrorKind::Unsupported.into());
        };
        let root = [u16::from(drive), u16::from(b':'), u16::from(b'\\'), 0];
        // SAFETY: root is a complete, terminated drive-root string for this call.
        if unsafe { GetDriveTypeW(root.as_ptr()) } != DRIVE_FIXED {
            return Err(std::io::ErrorKind::Unsupported.into());
        }
        Ok(())
    }

    #[expect(
        unsafe_code,
        reason = "the live directory handle and exact FILE_ID_INFO output are borrowed by one identity query"
    )]
    fn directory_identity(directory: &Path) -> std::io::Result<(u64, [u8; 16])> {
        use std::os::windows::{fs::OpenOptionsExt as _, io::AsRawHandle as _};
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_BACKUP_SEMANTICS, FILE_ID_INFO, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE,
            FILE_SHARE_READ, FILE_SHARE_WRITE, FileIdInfo, GetFileInformationByHandleEx,
        };
        let file = std::fs::OpenOptions::new()
            .access_mode(FILE_READ_ATTRIBUTES)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(directory)?;
        let mut info = FILE_ID_INFO::default();
        let size = u32::try_from(size_of::<FILE_ID_INFO>()).map_err(std::io::Error::other)?;
        // SAFETY: file owns a live handle and info is writable for exactly the supplied structure size.
        if unsafe {
            GetFileInformationByHandleEx(
                file.as_raw_handle(),
                FileIdInfo,
                (&raw mut info).cast(),
                size,
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error());
        }
        Ok((info.VolumeSerialNumber, info.FileId.Identifier))
    }

    #[expect(
        unsafe_code,
        reason = "a notification handle is a kernel object with no thread affinity; this type is the proof that exactly one thread owns it"
    )]
    // SAFETY: the handle has no thread affinity. This owner moves between threads, and all operations
    // require exclusive access to it; no raw handle escapes this module.
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

    #[derive(Debug)]
    pub(super) struct Changes;

    impl Changes {
        pub(super) fn new(_directory: &Path) -> std::io::Result<Self> {
            Err(std::io::ErrorKind::Unsupported.into())
        }
        #[expect(
            clippy::unused_self,
            reason = "the unsupported platform keeps the same instance API without claiming change evidence"
        )]
        pub(super) fn changed(&mut self) -> std::io::Result<bool> {
            Err(std::io::ErrorKind::Unsupported.into())
        }
    }

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
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

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
        let root = scratch();
        assert!(watch_directory(&root.join("absent")).is_none());
        drop(fs::remove_dir_all(&root));
    }

    #[cfg(windows)]
    #[test]
    fn a_subtree_guard_notices_creation_and_refuses_a_replaced_root() {
        let root = scratch();
        let directory = root.join("watched");
        fs::create_dir(&directory).expect("create watched directory");
        let mut changes = DirectoryChanges::new(&directory).expect("observe subtree");
        assert!(!changes.changed().expect("unchanged"));
        fs::create_dir_all(directory.join("new/day")).expect("create descendant");
        assert!(changes.changed().expect("descendant changed"));
        fs::rename(&directory, root.join("moved")).expect("move watched root");
        fs::create_dir(&directory).expect("replace original path");
        assert!(
            changes.changed().is_err(),
            "the original watch cannot approve the replacement tree"
        );
        drop(changes);
        fs::remove_dir_all(root).expect("clean subtree fixture");
    }

    /// A file appearing is what "a session started" looks like, and it must arrive without anybody asking.
    #[cfg(windows)]
    #[test]
    fn a_file_appearing_wakes_the_watcher() {
        use std::thread;
        use std::time::Duration;

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
