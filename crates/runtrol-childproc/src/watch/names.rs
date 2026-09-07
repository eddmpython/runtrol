//! Bounded filename invalidation without a thread or a polling clock.

use std::ffi::OsString;
use std::path::Path;

/// Filename evidence since the preceding completed read.
#[derive(Debug, PartialEq, Eq)]
pub enum ChangedNames {
    /// Names supplied by the filesystem, including both sides of a rename.
    /// A consumer unable to map a short-name alias must rescan its entire scope.
    Names(Vec<OsString>),
    /// The bounded notification overflowed or its record framing was incomplete.
    Rescan,
}

/// One directory read, with a fixed buffer owned until kernel cancellation completes.
pub struct DirectoryNames(platform::Names);

impl std::fmt::Debug for DirectoryNames {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DirectoryNames")
            .finish_non_exhaustive()
    }
}

impl DirectoryNames {
    /// Arm names and writes before taking the initial directory snapshot.
    ///
    /// # Errors
    /// Refuses unavailable, replaced, remote, or unsupported directory roots.
    pub fn new(directory: &Path) -> std::io::Result<Self> {
        Self::with_subtree(directory, false)
    }

    /// Arm one bounded names buffer for this root and, optionally, its subtree.
    ///
    /// # Errors
    /// Refuses unavailable, replaced, remote, or unsupported directory roots.
    pub fn with_subtree(directory: &Path, subtree: bool) -> std::io::Result<Self> {
        platform::Names::new(directory, subtree).map(Self)
    }

    /// Consume at most one bounded batch and rearm before returning it.
    ///
    /// # Errors
    /// A failed read, root identity, or rearm cannot preserve cached evidence.
    pub fn changed(&mut self) -> std::io::Result<Option<ChangedNames>> {
        self.0.changed()
    }

    /// Wait for readiness without consuming names or borrowing the owner.
    ///
    /// Cancelling this future unregisters only its pool callback. The owner's pending read remains
    /// armed; dropping the owner cancels and joins that read before freeing its buffer.
    ///
    /// # Errors
    /// Reports a closed owner or unavailable completion wait.
    pub fn wait(&self) -> impl Future<Output = std::io::Result<()>> + Send + use<> {
        self.0.wait()
    }
}

#[cfg(windows)]
#[expect(
    unsafe_code,
    reason = "this module owns one overlapped directory read and its stable kernel buffers"
)]
mod platform {
    use std::fs::{File, OpenOptions};
    use std::os::windows::ffi::OsStringExt as _;
    use std::os::windows::fs::OpenOptionsExt as _;
    use std::os::windows::io::{AsHandle as _, AsRawHandle as _, FromRawHandle as _, OwnedHandle};
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use windows_sys::Win32::Foundation::{
        ERROR_IO_INCOMPLETE, ERROR_IO_PENDING, ERROR_NOT_FOUND, ERROR_NOTIFY_ENUM_DIR,
        ERROR_OPERATION_ABORTED,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OVERLAPPED, FILE_LIST_DIRECTORY,
        FILE_NOTIFY_CHANGE_DIR_NAME, FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE,
        FILE_NOTIFY_CHANGE_SIZE, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ,
        FILE_SHARE_WRITE, ReadDirectoryChangesW,
    };
    use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
    use windows_sys::Win32::System::Threading::{
        CancelThreadpoolIo, CloseThreadpoolIo, CreateEventW, CreateThreadpoolIo,
        PTP_CALLBACK_INSTANCE, PTP_IO, ResetEvent, SetEvent, SetEventWhenCallbackReturns,
        StartThreadpoolIo, WaitForThreadpoolIoCallbacks,
    };

    use super::{ChangedNames, OsString, Path};
    use crate::watch::platform::{directory_identity, file_identity, require_local_directory};

    const BUFFER_BYTES: u32 = 16 * 1024;
    const BUFFER_WORDS: usize = BUFFER_BYTES as usize / std::mem::size_of::<u32>();

    struct Pending {
        overlapped: OVERLAPPED,
        buffer: [u32; BUFFER_WORDS],
    }

    struct Signal {
        event: OwnedHandle,
        alive: AtomicBool,
    }

    pub(super) struct Names {
        directory: PathBuf,
        identity: (u64, [u8; 16]),
        file: File,
        io: PTP_IO,
        signal: Arc<Signal>,
        pending: Box<Pending>,
        armed: bool,
        subtree: bool,
    }

    // SAFETY: one owner accesses the operation. Its boxed storage never moves while the kernel uses it;
    // moving the owner between blocking workers transfers no concurrent access to those buffers.
    unsafe impl Send for Names {}

    impl Names {
        pub(super) fn new(directory: &Path, subtree: bool) -> std::io::Result<Self> {
            require_local_directory(directory)?;
            let identity = directory_identity(directory)?;
            let file = OpenOptions::new()
                .access_mode(FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES)
                .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
                .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OVERLAPPED)
                .open(directory)?;
            Self::opened(directory, identity, file, subtree)
        }

        fn opened(
            directory: &Path,
            identity: (u64, [u8; 16]),
            file: File,
            subtree: bool,
        ) -> std::io::Result<Self> {
            // A path may change twice around open. Only the actual watched handle can prove which
            // directory the queued I/O will observe; matching path checks alone cannot reject ABA.
            if file_identity(&file)? != identity {
                return Err(std::io::Error::other(
                    "the opened notification root does not match the accepted directory",
                ));
            }
            // SAFETY: a private, unnamed manual-reset event has no borrowed pointers.
            let event = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
            if event.is_null() {
                return Err(std::io::Error::last_os_error());
            }
            // SAFETY: the successful event open transfers its ownership exactly once.
            let event = unsafe { OwnedHandle::from_raw_handle(event) };
            let pending = Box::new(Pending {
                overlapped: OVERLAPPED::default(),
                buffer: [0; BUFFER_WORDS],
            });
            let signal = Arc::new(Signal {
                event,
                alive: AtomicBool::new(true),
            });
            // A non-null OVERLAPPED event still ties directory I/O to the issuing worker's life.
            // The OS completion pool owns a null-event request instead; its callback signals our
            // separate event. One outstanding read bounds both callback work and retained storage.
            // SAFETY: the owned file and Arc allocation outlive all callbacks, joined in Drop.
            let io = unsafe {
                CreateThreadpoolIo(
                    file.as_raw_handle(),
                    Some(completed),
                    Arc::as_ptr(&signal).cast_mut().cast(),
                    std::ptr::null(),
                )
            };
            if io == 0 {
                return Err(std::io::Error::last_os_error());
            }
            let mut observer = Self {
                directory: directory.to_owned(),
                identity,
                file,
                io,
                signal,
                pending,
                armed: false,
                subtree,
            };
            observer.arm()?;
            if directory_identity(directory)? != identity {
                return Err(std::io::Error::other(
                    "the notification root changed while opening it",
                ));
            }
            Ok(observer)
        }

        fn arm(&mut self) -> std::io::Result<()> {
            // SAFETY: the previous operation has completed; only this owner rearms its private event.
            if unsafe { ResetEvent(self.signal.event.as_raw_handle()) } == 0 {
                return Err(std::io::Error::last_os_error());
            }
            self.pending.overlapped = OVERLAPPED::default();
            // SAFETY: the DWORD-aligned buffer and OVERLAPPED have stable boxed addresses. Drop joins
            // completion before storage or handles are released. Every issued I/O has one pool start.
            if unsafe {
                StartThreadpoolIo(self.io);
                ReadDirectoryChangesW(
                    self.file.as_raw_handle(),
                    self.pending.buffer.as_mut_ptr().cast(),
                    BUFFER_BYTES,
                    i32::from(self.subtree),
                    FILE_NOTIFY_CHANGE_FILE_NAME
                        | FILE_NOTIFY_CHANGE_DIR_NAME
                        | FILE_NOTIFY_CHANGE_LAST_WRITE
                        | FILE_NOTIFY_CHANGE_SIZE,
                    std::ptr::null_mut(),
                    &raw mut self.pending.overlapped,
                    None,
                )
            } == 0
            {
                let error = std::io::Error::last_os_error();
                if error.raw_os_error() != Some(ERROR_IO_PENDING.cast_signed()) {
                    // SAFETY: immediate failure issued no operation, so cancel its expected callback.
                    unsafe { CancelThreadpoolIo(self.io) };
                    return Err(error);
                }
            }
            self.armed = true;
            Ok(())
        }

        pub(super) fn changed(&mut self) -> std::io::Result<Option<ChangedNames>> {
            if directory_identity(&self.directory)? != self.identity {
                return Err(std::io::Error::other("the notification root was replaced"));
            }
            let mut transferred = 0;
            // SAFETY: the operation remains armed and its buffers are owned by self for this query.
            let completed = unsafe {
                GetOverlappedResult(
                    self.file.as_raw_handle(),
                    &raw const self.pending.overlapped,
                    &raw mut transferred,
                    0,
                )
            };
            let error = if completed == 0 {
                Some(std::io::Error::last_os_error())
            } else {
                None
            };
            if error.as_ref().is_some_and(|error| {
                error.raw_os_error() == Some(ERROR_IO_INCOMPLETE.cast_signed())
            }) {
                return Ok(None);
            }
            // SAFETY: completion is known. Join its queued/running callback before reusing any
            // operation state or resetting the event it signals on return.
            unsafe { WaitForThreadpoolIoCallbacks(self.io, 0) };
            self.armed = false;
            let names = match error {
                Some(error)
                    if error.raw_os_error() == Some(ERROR_NOTIFY_ENUM_DIR.cast_signed()) =>
                {
                    ChangedNames::Rescan
                }
                Some(error) => return Err(error),
                None => decode(&self.pending.buffer, transferred),
            };
            self.arm()?;
            Ok(Some(names))
        }

        pub(super) fn wait(&self) -> impl Future<Output = std::io::Result<()>> + Send + use<> {
            let signal = Arc::clone(&self.signal);
            async move {
                if !signal.alive.load(Ordering::Acquire) {
                    return Err(std::io::Error::other("the filename observer was closed"));
                }
                crate::contain::job::wait::signaled(signal.event.as_handle())
                    .await
                    .map_err(std::io::Error::other)?;
                if !signal.alive.load(Ordering::Acquire) {
                    return Err(std::io::Error::other("the filename observer was closed"));
                }
                Ok(())
            }
        }
    }

    unsafe extern "system" fn completed(
        instance: PTP_CALLBACK_INSTANCE,
        context: *mut std::ffi::c_void,
        _overlapped: *mut std::ffi::c_void,
        _result: u32,
        _bytes: usize,
        _io: PTP_IO,
    ) {
        // SAFETY: Names keeps this Arc allocation and event alive until all callbacks have joined.
        let signal = unsafe { &*context.cast::<Signal>() };
        // SAFETY: defer the notification until this callback returns; the handle remains owned.
        unsafe { SetEventWhenCallbackReturns(instance, signal.event.as_raw_handle()) };
    }

    impl Drop for Names {
        fn drop(&mut self) {
            self.signal.alive.store(false, Ordering::Release);
            if self.armed {
                // SAFETY: this exact operation is owned and armed. Cancellation only requests completion.
                if unsafe {
                    CancelIoEx(
                        self.file.as_raw_handle(),
                        &raw const self.pending.overlapped,
                    )
                } == 0
                {
                    let error = std::io::Error::last_os_error();
                    if error.raw_os_error() != Some(ERROR_NOT_FOUND.cast_signed()) {
                        eprintln!("runtrol: directory notification cancellation failed: {error}");
                    }
                }
                let mut transferred = 0;
                // SAFETY: wait for the exact operation even when cancellation lost a completion race.
                // The kernel no longer owns the boxed pointers when this blocking completion returns.
                if unsafe {
                    GetOverlappedResult(
                        self.file.as_raw_handle(),
                        &raw const self.pending.overlapped,
                        &raw mut transferred,
                        1,
                    )
                } == 0
                {
                    let error = std::io::Error::last_os_error();
                    if error.raw_os_error() != Some(ERROR_OPERATION_ABORTED.cast_signed()) {
                        eprintln!("runtrol: directory notification ended with an error: {error}");
                    }
                }
            }
            // SAFETY: cancellation/completion has joined the I/O. Retain context and storage through
            // every queued/running callback, then close the pool object before either can be freed.
            unsafe {
                WaitForThreadpoolIoCallbacks(self.io, 0);
                CloseThreadpoolIo(self.io);
            }
            // SAFETY: completion has joined before waking any waiter retained after the owner closes.
            if unsafe { SetEvent(self.signal.event.as_raw_handle()) } == 0 {
                eprintln!(
                    "runtrol: closing directory notification failed to wake its waiter: {}",
                    std::io::Error::last_os_error()
                );
            }
        }
    }

    fn decode(words: &[u32], transferred: u32) -> ChangedNames {
        if transferred == 0 || transferred > BUFFER_BYTES {
            return ChangedNames::Rescan;
        }
        let Ok(length) = usize::try_from(transferred) else {
            return ChangedNames::Rescan;
        };
        if length > std::mem::size_of_val(words) {
            return ChangedNames::Rescan;
        }
        // SAFETY: the completed DWORD buffer owns length initialized bytes, bounded by its allocation.
        let bytes = unsafe { std::slice::from_raw_parts(words.as_ptr().cast::<u8>(), length) };
        let mut names = Vec::new();
        let mut offset = 0;
        loop {
            let Some(record) = bytes.get(offset..) else {
                return ChangedNames::Rescan;
            };
            let field = |start| {
                let slice = record.get(start..start + 4)?;
                let Ok(bytes) = <[u8; 4]>::try_from(slice) else {
                    return None;
                };
                Some(u32::from_le_bytes(bytes))
            };
            let (Some(next), Some(action), Some(size)) = (field(0), field(4), field(8)) else {
                return ChangedNames::Rescan;
            };
            if !(1..=5).contains(&action) || size == 0 || size % 2 != 0 {
                return ChangedNames::Rescan;
            }
            let Ok(size) = usize::try_from(size) else {
                return ChangedNames::Rescan;
            };
            let Some(name) = record.get(12..12 + size) else {
                return ChangedNames::Rescan;
            };
            let (pairs, _) = name.as_chunks::<2>();
            let units: Vec<_> = pairs.iter().map(|unit| u16::from_le_bytes(*unit)).collect();
            names.push(OsString::from_wide(&units));
            if next == 0 {
                return ChangedNames::Names(names);
            }
            let Ok(next) = usize::try_from(next) else {
                return ChangedNames::Rescan;
            };
            if next < 12 + size || next % 4 != 0 {
                return ChangedNames::Rescan;
            }
            offset += next;
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn an_opened_intermediate_root_is_refused_even_when_the_path_matches_again() {
            let root = crate::watch::names::tests::Scratch::new();
            let intermediate = root.0.join("intermediate");
            std::fs::create_dir(&intermediate).expect("different directory object");
            let accepted = directory_identity(&root.0).expect("accepted root");
            let file = OpenOptions::new()
                .access_mode(FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES)
                .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
                .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OVERLAPPED)
                .open(&intermediate)
                .expect("file opened during path replacement");
            assert_eq!(
                directory_identity(&root.0).expect("path matches again"),
                accepted
            );
            assert!(Names::opened(&root.0, accepted, file, false).is_err());
        }

        #[test]
        fn incomplete_or_overflowed_batches_require_a_rescan() {
            assert_eq!(decode(&[], 0), ChangedNames::Rescan);
            assert_eq!(decode(&[], BUFFER_BYTES + 1), ChangedNames::Rescan);
            assert_eq!(
                decode(&[0, 1, 4, 0x0062_0061], 16),
                ChangedNames::Names(vec!["ab".into()])
            );
            assert_eq!(decode(&[0, 1, 4], 12), ChangedNames::Rescan);
            assert_eq!(decode(&[0, 1, 3, 0], 16), ChangedNames::Rescan);
            assert_eq!(decode(&[4, 1, 4, 0x0062_0061], 16), ChangedNames::Rescan);
            assert_eq!(decode(&[16, 1, 4, 0x0062_0061], 16), ChangedNames::Rescan);
            assert_eq!(decode(&[0, 9, 4, 0x0062_0061], 16), ChangedNames::Rescan);
        }
    }
}

#[cfg(not(windows))]
mod platform {
    use super::{ChangedNames, Path};
    pub(super) struct Names;
    impl Names {
        pub(super) fn new(_directory: &Path, _subtree: bool) -> std::io::Result<Self> {
            Err(std::io::ErrorKind::Unsupported.into())
        }
        #[expect(
            clippy::unused_self,
            reason = "unsupported platforms preserve the observer API"
        )]
        pub(super) fn changed(&mut self) -> std::io::Result<Option<ChangedNames>> {
            Err(std::io::ErrorKind::Unsupported.into())
        }
        #[expect(
            clippy::unused_self,
            reason = "unsupported platforms preserve the observer API"
        )]
        pub(super) fn wait(&self) -> impl Future<Output = std::io::Result<()>> + Send + use<> {
            std::future::ready(Err(std::io::ErrorKind::Unsupported.into()))
        }
    }
}

#[cfg(all(test, windows))]
#[expect(
    clippy::expect_used,
    clippy::panic,
    reason = "exact fixture setup, teardown and bounded waits fail the regression on error"
)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;

    static NEXT: AtomicU64 = AtomicU64::new(0);

    pub(super) struct Scratch(pub(super) std::path::PathBuf);
    impl Scratch {
        pub(super) fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "runtrol-names-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&root).expect("create exact fixture");
            Self(root)
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).expect("remove exact fixture");
        }
    }

    async fn batch(watch: &mut DirectoryNames) -> Vec<OsString> {
        tokio::time::timeout(Duration::from_secs(2), watch.wait())
            .await
            .expect("bounded notification")
            .expect("readiness");
        let Some(ChangedNames::Names(names)) = watch.changed().expect("completed read") else {
            panic!("small complete fixture must retain names");
        };
        names
    }

    #[tokio::test]
    async fn initial_and_rearming_workers_can_exit_without_cancelling_observation() {
        let root = Scratch::new();
        let directory = root.0.clone();
        let mut watch = std::thread::spawn(move || {
            DirectoryNames::new(&directory).expect("arm on a temporary worker")
        })
        .join()
        .expect("the initial worker exits");
        assert_eq!(
            watch.changed().expect("initial I/O survives worker exit"),
            None
        );
        for name in ["first.lock", "second.lock"] {
            fs::write(root.0.join(name), b"").expect("creation after issuing worker exits");
            let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
            loop {
                tokio::time::timeout_at(deadline, watch.wait())
                    .await
                    .expect("creation deadline")
                    .expect("event remains readable");
                let (returned, names) = std::thread::spawn(move || {
                    let names = watch
                        .changed()
                        .expect("consume and rearm on another worker");
                    (watch, names)
                })
                .join()
                .expect("the rearming worker exits");
                watch = returned;
                if matches!(names, Some(ChangedNames::Names(names)) if names.contains(&name.into()))
                {
                    break;
                }
            }
            // Any additional event is valid; cancellation of the newly armed read is not.
            watch.changed().expect("rearmed I/O survives worker exit");
        }
    }

    #[tokio::test]
    async fn names_preserve_creation_rename_and_same_name_replacement() {
        let root = Scratch::new();
        let first = root.0.join("first.lock");
        let second = root.0.join("second.lock");
        let mut watch = DirectoryNames::new(&root.0).expect("armed before creation");
        fs::write(&first, b"").expect("create lock");
        assert!(
            batch(&mut watch)
                .await
                .contains(&OsString::from("first.lock"))
        );
        // A fresh observer separates OS duplicate write notifications from the next operation.
        drop(watch);
        let mut watch = DirectoryNames::new(&root.0).expect("armed before rename");
        fs::rename(&first, &second).expect("rename lock");
        let names = batch(&mut watch).await;
        assert!(names.contains(&OsString::from("first.lock")));
        assert!(names.contains(&OsString::from("second.lock")));
        drop(watch);
        let mut watch = DirectoryNames::new(&root.0).expect("armed before replacement");
        fs::remove_file(&second).expect("remove lock");
        fs::write(&second, b"").expect("replace same name");
        assert!(
            batch(&mut watch)
                .await
                .contains(&OsString::from("second.lock"))
        );
    }

    #[tokio::test]
    async fn cancelled_wait_keeps_writes_and_owner_drop_joins_pending_io() {
        let root = Scratch::new();
        let file = root.0.join("writer.lock");
        fs::write(&file, b"").expect("initial lock");
        let mut watch = DirectoryNames::new(&root.0).expect("watch lock");
        assert!(
            tokio::time::timeout(Duration::from_millis(10), watch.wait())
                .await
                .is_err()
        );
        fs::write(&file, b"changed").expect("same name write");
        let ready = watch.wait();
        tokio::time::timeout(Duration::from_secs(2), ready)
            .await
            .expect("write deadline")
            .expect("ready");
        assert!(
            batch(&mut watch)
                .await
                .contains(&OsString::from("writer.lock")),
            "wait does not consume names"
        );
        let retained = watch.wait();
        drop(watch);
        assert!(
            tokio::time::timeout(Duration::from_secs(2), retained)
                .await
                .expect("closed wake")
                .is_err()
        );
        fs::remove_file(&file).expect("no pending read owns removed file");
    }

    #[test]
    fn replaced_directory_refuses_old_names_proof() {
        let root = Scratch::new();
        let watched = root.0.join("watched");
        fs::create_dir(&watched).expect("directory");
        let mut watch = DirectoryNames::new(&watched).expect("watch");
        fs::rename(&watched, root.0.join("moved")).expect("move root");
        fs::create_dir(&watched).expect("replacement");
        assert!(watch.changed().is_err());
    }
}
