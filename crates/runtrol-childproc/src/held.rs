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

/// The subcommand this executable answers when it is asked, as a helper, who holds a file.
pub const HOLDER_SUBCOMMAND: &str = "who-holds";

/// Print the process holding one path, for a caller that spawned this executable to ask.
///
/// Prints the process identifier alone, or nothing when nobody holds it. The exit code is the answer's
/// presence so a caller that cannot read output still learns something.
#[must_use]
pub fn run_who_holds(path: &Path) -> bool {
    match platform::holder_of(path) {
        Some(pid) => {
            // A helper whose one line cannot be written has failed to answer, which the exit code then says.
            std::io::Write::write_fmt(
                &mut std::io::stdout().lock(),
                format_args!(
                    "{pid}
"
                ),
            )
            .is_ok()
        }
        None => false,
    }
}

/// The same answer as [`holder_of`], asked here instead of in a helper.
///
/// Costs this process the machinery named there, for the life of the process. It exists for the helper itself
/// and for tests, which are measuring the answer rather than paying for a Runtime.
#[must_use]
pub fn holder_of_here(path: &Path) -> Option<u32> {
    platform::holder_of(path)
}

/// Which live process holds this path, when the operating system will say.
///
/// [`write_locked`] answers "is somebody holding it"; this answers "who". A CLI that keeps one lock file per
/// conversation therefore names, for free, the exact process that owns each conversation, which is what binds
/// a conversation identity to a terminal nobody here started.
///
/// `None` when nothing holds it, when the platform cannot say, or when the answer did not arrive. A caller
/// treats `None` as "no binding known", never as "no process".
///
/// Measured 2026-08-30 on the operator's machine: asking this of one live `thread-writer-locks/<id>.lock`
/// returned exactly one holder, `codex.exe` pid 20404, which was the process running that conversation.
#[expect(
    clippy::result_map_or_into_option,
    reason = "this workspace refuses Result::ok because it discards a cause; a helper that printed something other than a process identifier has no cause to keep"
)]
#[must_use]
pub fn holder_of(path: &Path) -> Option<u32> {
    // Asked in a helper of its own, not here. Windows answers this through its Restart Manager, and loading
    // that machinery costs the asking process 5.3 MiB of resident memory for the life of the process
    // (measured 2026-08-30: 10.3 MiB before the first ask, 15.6 after it, 15.8 after twenty more). A Runtime
    // held to twenty megabytes while idle cannot spend a quarter of that on one question, and a process that
    // exits after answering spends nothing that lasts.
    let Ok(program) = std::env::current_exe() else {
        return None;
    };
    let asked = std::process::Command::new(program)
        .arg(HOLDER_SUBCOMMAND)
        .arg(path)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output();
    let Ok(answer) = asked else {
        return None;
    };
    let printed = String::from_utf8_lossy(&answer.stdout);
    printed.trim().parse().map_or(None, Some)
}

#[cfg(windows)]
mod platform {
    use std::fs::OpenOptions;
    use std::os::windows::fs::OpenOptionsExt as _;
    use std::path::Path;

    /// Deny every other handle for the length of this open. A writer that already has the file open makes
    /// this fail with a sharing violation, which is the answer.
    const SHARE_NONE: u32 = 0;

    /// Read access only: the question is whether somebody else holds the file, and asking it with write
    /// access would be asking for more than the answer needs. Measured 2026-08-29 on the operator's own
    /// machine: a read-only open with no sharing is refused on exactly the locks a live process holds, the
    /// same set a read-write open is refused on.
    pub(super) fn write_locked(path: &Path) -> bool {
        if !path.exists() {
            return false;
        }
        OpenOptions::new()
            .read(true)
            .share_mode(SHARE_NONE)
            .open(path)
            .is_err()
    }

    /// The Restart Manager answers "which processes have this file open". It exists so an installer can ask
    /// before replacing a file, and the question it answers is exactly the one here. It opens a short session,
    /// registers the one path, reads the list and closes, touching nothing that another process waits on.
    #[expect(
        unsafe_code,
        reason = "the Restart Manager is a C API with no safe wrapper; each call is documented at its site"
    )]
    pub(super) fn holder_of(path: &Path) -> Option<u32> {
        use std::os::windows::ffi::OsStrExt as _;

        use windows_sys::Win32::System::RestartManager::{
            RM_PROCESS_INFO, RmEndSession, RmGetList, RmRegisterResources, RmStartSession,
        };

        /// The session key buffer the API fills, sized by its own documented maximum plus the terminator.
        const SESSION_KEY_LEN: usize = 33;
        /// One lock file has one holder; a handful of slots covers a surprise without a second call.
        const MAX_HOLDERS: u32 = 8;
        const MAX_HOLDER_SLOTS: usize = MAX_HOLDERS as usize;
        const SUCCESS: u32 = 0;

        if !path.exists() {
            return None;
        }
        let wide: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let mut session: u32 = 0;
        let mut key = [0_u16; SESSION_KEY_LEN];
        // SAFETY: both pointers address local buffers of the sizes this API documents.
        let started = unsafe { RmStartSession(&raw mut session, 0, key.as_mut_ptr()) };
        if started != SUCCESS {
            return None;
        }
        let files = [wide.as_ptr()];
        // SAFETY: one path, no applications and no services, matching the counts passed.
        let registered = unsafe {
            RmRegisterResources(
                session,
                1,
                files.as_ptr(),
                0,
                std::ptr::null(),
                0,
                std::ptr::null(),
            )
        };
        let mut holder = None;
        if registered == SUCCESS {
            let mut needed: u32 = 0;
            let mut count: u32 = MAX_HOLDERS;
            let mut infos =
                [const { std::mem::MaybeUninit::<RM_PROCESS_INFO>::zeroed() }; MAX_HOLDER_SLOTS];
            let mut reason: u32 = 0;
            // SAFETY: `count` states the slots available and the API writes no more than that; every
            // pointer addresses a local of the declared size.
            let listed = unsafe {
                RmGetList(
                    session,
                    &raw mut needed,
                    &raw mut count,
                    infos.as_mut_ptr().cast::<RM_PROCESS_INFO>(),
                    &raw mut reason,
                )
            };
            // More holders than slots still fills the slots, and one of them is the answer wanted.
            let filled = (listed == SUCCESS || needed > count) && count > 0;
            if filled && let Some(first) = infos.first() {
                // SAFETY: the call above initialised at least one entry.
                let info = unsafe { first.assume_init_ref() };
                holder = Some(info.Process.dwProcessId);
            }
        }
        // SAFETY: the session started above is closed exactly once.
        unsafe { RmEndSession(session) };
        holder
    }
}

#[cfg(unix)]
mod platform {
    use std::fs::OpenOptions;
    use std::os::fd::AsRawFd as _;
    use std::path::Path;

    #[expect(
        unsafe_code,
        reason = "an advisory lock test has no safe wrapper in std; flock is the call itself"
    )]
    /// Unix has no portable "who holds this advisory lock" call. `fcntl(F_GETLK)` names a holder for record
    /// locks but not for `flock`, and walking `/proc/*/fd` is Linux only and costs a scan of every process.
    /// Answering `None` keeps the caller honest (no binding known) until a unix user needs one measured here.
    pub(super) const fn holder_of(_path: &Path) -> Option<u32> {
        None
    }

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
    /// The holder question answered against a file this process is holding: the answer must be this process.
    /// The cross-process half is measured against the real CLI (2026-08-30: one live conversation lock named
    /// `codex.exe` pid 20404, the process running that conversation).
    #[cfg(windows)]
    #[test]
    fn the_process_holding_a_file_is_the_one_named() {
        use std::fs::OpenOptions;
        use std::os::windows::fs::OpenOptionsExt as _;

        let path = scratch("holder");
        fs::write(&path, b"").expect("the scratch file is written");
        assert_eq!(
            platform::holder_of(&path),
            None,
            "a file nobody has open has no holder"
        );
        let holder = OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&path)
            .expect("this process takes the file");
        assert_eq!(platform::holder_of(&path), Some(std::process::id()));
        drop(holder);
        drop(fs::remove_file(&path));
    }

    /// What asking the operating system who holds a file costs this process, printed rather than asserted.
    ///
    /// `cargo test -p runtrol-childproc --lib held::tests::what_asking -- --ignored --nocapture`
    #[ignore = "prints a measurement of this machine"]
    #[cfg(windows)]
    #[test]
    fn what_asking_who_holds_a_file_costs() {
        use std::fs::OpenOptions;
        use std::os::windows::fs::OpenOptionsExt as _;

        let path = scratch("cost");
        fs::write(&path, b"").expect("the scratch file is written");
        let holder = OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&path)
            .expect("this process takes the file");
        let before = crate::footprint::resident_bytes(std::process::id());
        assert_eq!(platform::holder_of(&path), Some(std::process::id()));
        let once = crate::footprint::resident_bytes(std::process::id());
        for _ in 0..20 {
            let _asked = platform::holder_of(&path);
        }
        let many = crate::footprint::resident_bytes(std::process::id());
        println!(
            "resident bytes: before={before:?} after one ask={once:?} after twenty more={many:?}"
        );
        drop(holder);
        drop(fs::remove_file(&path));
    }

    #[test]
    fn a_missing_path_has_no_holder() {
        assert_eq!(platform::holder_of(&scratch("absent-holder")), None);
    }

    #[cfg(windows)]
    #[test]
    fn a_path_this_process_holds_exclusively_is_locked() {
        use std::fs::OpenOptions;
        use std::os::windows::fs::OpenOptionsExt as _;

        let path = scratch("held");
        fs::write(&path, b"").expect("the scratch file is written");
        let holder = OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&path)
            .expect("this process takes the file");
        assert!(write_locked(&path));
        drop(holder);
        assert!(!write_locked(&path));
        drop(fs::remove_file(&path));
    }
}
