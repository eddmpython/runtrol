//! Bounded operating-system process ancestry for provider process attribution.
//!
//! A package-manager launcher may remain as the pseudo-terminal root while the actual provider executable runs as
//! its child. Provider-owned structural records name that child. Equality alone therefore leaves the terminal
//! identity-pending forever and creates a second provider-native row beside it. This module answers only the
//! structural question: whether one current process is the root or a descendant of another current process.

#[cfg(any(windows, target_os = "macos"))]
use std::mem::size_of;

use runtrol_provider::ProcessIdentity;

const MAX_ANCESTOR_DEPTH: usize = 64;

/// A bounded snapshot or query surface for current process ancestry.
#[derive(Debug)]
pub struct ProcessTree {
    #[cfg(windows)]
    nodes: std::collections::BTreeMap<u32, ProcessNode>,
}

#[derive(Clone, Copy, Debug)]
struct ProcessNode {
    parent: u32,
    started: u64,
}

/// The operating system could not provide a complete bounded ancestry view.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ProcessTreeError {
    /// Windows refused or returned an incomplete process snapshot.
    #[error("cannot inspect the current process tree while {doing}: {detail}")]
    Snapshot {
        /// The exact structural operation that failed.
        doing: &'static str,
        /// The operating-system error or bounded refusal.
        detail: String,
    },
}

impl ProcessTree {
    /// Capture the platform state needed for bounded ancestry queries.
    ///
    /// Windows captures one process table because asking once per provider binding would repeat a whole-system walk.
    /// Linux and macOS keep no table and read only the short parent chain of each candidate when queried.
    ///
    /// # Errors
    ///
    /// [`ProcessTreeError`] when Windows cannot take or completely enumerate its process snapshot.
    pub fn capture() -> Result<Self, ProcessTreeError> {
        #[cfg(windows)]
        {
            capture_windows()
        }
        #[cfg(unix)]
        {
            Ok(Self {})
        }
    }

    /// Whether `candidate` is `root` itself or one of its descendants in the captured current process tree.
    ///
    /// A missing, vanished, cyclic, or deeper-than-bounded chain is not attributed.
    #[must_use]
    pub fn contains(&self, root: u32, candidate: u32) -> bool {
        #[cfg(windows)]
        {
            within(root, candidate, |pid| self.node_of(pid))
        }
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        {
            within(root, candidate, Self::node_of)
        }
    }

    /// Whether one exact process incarnation is the root or its current descendant.
    ///
    /// Both endpoint birth stamps are checked before any captured parent edge is trusted. This is the authority
    /// form of [`Self::contains`]: callers holding an authenticated peer or supervised process identity must use the
    /// complete tuple so a recycled endpoint PID cannot inherit authority.
    #[must_use]
    pub fn contains_identity(&self, root: ProcessIdentity, candidate: ProcessIdentity) -> bool {
        #[cfg(windows)]
        {
            within_identity(root, candidate, |pid| self.node_of(pid))
        }
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        {
            within_identity(root, candidate, Self::node_of)
        }
    }

    #[cfg(windows)]
    fn node_of(&self, pid: u32) -> Option<ProcessNode> {
        let captured = self.nodes.get(&pid).copied()?;
        // A PID can be reused after the ToolHelp snapshot. Rechecking the creation stamp prevents the captured
        // parent of the old occupant from being applied to the new process.
        (windows_process_start(pid)? == captured.started).then_some(captured)
    }

    #[cfg(target_os = "linux")]
    fn node_of(pid: u32) -> Option<ProcessNode> {
        let Ok(text) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
            return None;
        };
        let tail = text.rsplit_once(") ")?.1;
        let mut fields = tail.split_whitespace();
        // The tail begins at field 3. Field 4 is the parent process id and field 22 is the kernel start tick.
        fields.next()?;
        let Ok(parent) = fields.next()?.parse() else {
            return None;
        };
        let Ok(started) = fields.nth(17)?.parse() else {
            return None;
        };
        Some(ProcessNode { parent, started })
    }

    #[cfg(target_os = "macos")]
    #[expect(
        unsafe_code,
        reason = "macOS exposes a process parent only through proc_pidinfo"
    )]
    fn node_of(pid: u32) -> Option<ProcessNode> {
        let Ok(pid) = i32::try_from(pid) else {
            return None;
        };
        let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::uninit();
        let Ok(size) = i32::try_from(size_of::<libc::proc_bsdinfo>()) else {
            return None;
        };
        // SAFETY: `info` is writable for the exact size passed to the kernel and is read only after a complete-size
        // result states that the structure was initialized.
        let read = unsafe {
            libc::proc_pidinfo(
                pid,
                libc::PROC_PIDTBSDINFO,
                0,
                info.as_mut_ptr().cast(),
                size,
            )
        };
        if read != size {
            return None;
        }
        // SAFETY: the complete-size result above initialized every byte of `info`.
        let info = unsafe { info.assume_init() };
        let started = info
            .pbi_start_tvsec
            .checked_mul(1_000_000)?
            .checked_add(info.pbi_start_tvusec)?;
        Some(ProcessNode {
            parent: info.pbi_ppid,
            started,
        })
    }
}

/// Read the exact current incarnation of one process.
///
/// This performs one bounded kernel query and returns `None` when the PID is zero, has vanished, cannot be inspected,
/// or exposes no usable start stamp. It does not enumerate the process table.
#[must_use]
pub fn process_identity(pid: u32) -> Option<ProcessIdentity> {
    if pid == 0 {
        return None;
    }
    #[cfg(windows)]
    let started = windows_process_start(pid)?;
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    let started = ProcessTree::node_of(pid)?.started;
    ProcessIdentity::new(pid, started)
}

fn within(
    root: u32,
    mut candidate: u32,
    mut node_of: impl FnMut(u32) -> Option<ProcessNode>,
) -> bool {
    if root == 0 || candidate == 0 {
        return false;
    }
    for _ in 0..=MAX_ANCESTOR_DEPTH {
        if candidate == root {
            // Equality is meaningful only while the current PID still names a process. Otherwise a vanished root,
            // or a PID waiting to be reused, could turn a stale structural record into a false exact match.
            return node_of(candidate).is_some();
        }
        let Some(node) = node_of(candidate) else {
            return false;
        };
        if node.parent == 0 || node.parent == candidate {
            return false;
        }
        let Some(parent) = node_of(node.parent) else {
            return false;
        };
        // A process cannot be older than the process that created it. If a recorded parent PID has already exited
        // and been reused, the new occupant starts after the child and must not splice two unrelated trees together.
        if parent.started > node.started {
            return false;
        }
        candidate = node.parent;
    }
    false
}

fn within_identity(
    root: ProcessIdentity,
    candidate: ProcessIdentity,
    mut node_of: impl FnMut(u32) -> Option<ProcessNode>,
) -> bool {
    let root_node = node_of(root.pid());
    if root_node.is_none_or(|node| node.started != root.started()) {
        return false;
    }
    let candidate_node = node_of(candidate.pid());
    if candidate_node.is_none_or(|node| node.started != candidate.started()) {
        return false;
    }
    within(root.pid(), candidate.pid(), node_of)
}

#[cfg(windows)]
#[expect(
    unsafe_code,
    reason = "Windows exposes process creation identity through an opened process handle"
)]
fn windows_process_start(pid: u32) -> Option<u64> {
    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME};
    use windows_sys::Win32::System::Threading::{
        GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    struct Process(windows_sys::Win32::Foundation::HANDLE);
    impl Drop for Process {
        fn drop(&mut self) {
            // SAFETY: this guard owns the successful process handle and closes it exactly once.
            unsafe {
                _ = CloseHandle(self.0);
            }
        }
    }

    // SAFETY: arguments are plain values and the returned handle is checked before use.
    let raw = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if raw.is_null() {
        return None;
    }
    let process = Process(raw);
    let mut creation = FILETIME::default();
    let mut exit = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    // SAFETY: each pointer names a writable FILETIME and the checked handle has query rights.
    if unsafe {
        GetProcessTimes(
            process.0,
            &raw mut creation,
            &raw mut exit,
            &raw mut kernel,
            &raw mut user,
        )
    } == 0
    {
        return None;
    }
    Some((u64::from(creation.dwHighDateTime) << 32) | u64::from(creation.dwLowDateTime))
}

#[cfg(windows)]
#[expect(
    unsafe_code,
    reason = "Windows exposes the current process parent table through the ToolHelp snapshot API"
)]
fn capture_windows() -> Result<ProcessTree, ProcessTreeError> {
    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_NO_MORE_FILES, GetLastError, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };

    const MAX_PROCESSES: usize = 65_536;

    struct Snapshot(windows_sys::Win32::Foundation::HANDLE);
    impl Drop for Snapshot {
        fn drop(&mut self) {
            // SAFETY: this guard owns the successful snapshot handle and closes it exactly once.
            unsafe {
                _ = CloseHandle(self.0);
            }
        }
    }

    // SAFETY: the flags request a read-only process snapshot and the ignored process id is zero.
    let raw = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if raw == INVALID_HANDLE_VALUE {
        return Err(snapshot_failure("taking the process snapshot"));
    }
    let snapshot = Snapshot(raw);
    // SAFETY: the all-zero representation is the documented initialization for PROCESSENTRY32W. `dwSize` is set
    // before the structure reaches the API.
    let mut entry = unsafe { std::mem::zeroed::<PROCESSENTRY32W>() };
    entry.dwSize = u32::try_from(size_of::<PROCESSENTRY32W>()).map_err(|error| {
        ProcessTreeError::Snapshot {
            doing: "sizing the process snapshot entry",
            detail: error.to_string(),
        }
    })?;
    // SAFETY: the snapshot handle is live and `entry` is writable for the size declared in its first field.
    if unsafe { Process32FirstW(snapshot.0, &raw mut entry) } == 0 {
        return Err(snapshot_failure("reading the first process snapshot entry"));
    }
    let mut nodes = std::collections::BTreeMap::new();
    let mut seen = 0_usize;
    loop {
        seen += 1;
        if seen > MAX_PROCESSES {
            return Err(ProcessTreeError::Snapshot {
                doing: "bounding the process snapshot",
                detail: format!("the snapshot exceeds {MAX_PROCESSES} processes"),
            });
        }
        if let Some(started) = windows_process_start(entry.th32ProcessID) {
            nodes.insert(
                entry.th32ProcessID,
                ProcessNode {
                    parent: entry.th32ParentProcessID,
                    started,
                },
            );
        }
        // SAFETY: the snapshot handle and writable entry remain live for the complete enumeration.
        if unsafe { Process32NextW(snapshot.0, &raw mut entry) } != 0 {
            continue;
        }
        // SAFETY: read immediately after the failed ToolHelp call on this thread.
        let error = unsafe { GetLastError() };
        if error == ERROR_NO_MORE_FILES {
            break;
        }
        return Err(ProcessTreeError::Snapshot {
            doing: "reading the next process snapshot entry",
            detail: std::io::Error::from_raw_os_error(i32::try_from(error).unwrap_or(i32::MAX))
                .to_string(),
        });
    }
    Ok(ProcessTree { nodes })
}

#[cfg(windows)]
fn snapshot_failure(doing: &'static str) -> ProcessTreeError {
    ProcessTreeError::Snapshot {
        doing,
        detail: std::io::Error::last_os_error().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::process::{Child, Command, Stdio};
    use std::time::Duration;

    use super::*;

    const DESCENDANT_PROBE_ENV: &str = "RUNTROL_PROCESS_TREE_DESCENDANT_PROBE";

    struct ProbeChild(Child);

    impl Drop for ProbeChild {
        fn drop(&mut self) {
            drop(self.0.kill());
            drop(self.0.wait());
        }
    }

    #[test]
    fn nested_launcher_processes_belong_to_one_terminal_root() {
        let nodes = BTreeMap::from([
            (
                10,
                ProcessNode {
                    parent: 1,
                    started: 10,
                },
            ),
            (
                20,
                ProcessNode {
                    parent: 10,
                    started: 20,
                },
            ),
            (
                30,
                ProcessNode {
                    parent: 20,
                    started: 30,
                },
            ),
            (
                40,
                ProcessNode {
                    parent: 10,
                    started: 40,
                },
            ),
            (
                50,
                ProcessNode {
                    parent: 99,
                    started: 50,
                },
            ),
            (
                99,
                ProcessNode {
                    parent: 1,
                    started: 9,
                },
            ),
        ]);
        let relation = |candidate| nodes.get(&candidate).copied();

        assert!(within(10, 10, relation));
        assert!(within(10, 20, relation));
        assert!(within(10, 30, relation));
        assert!(within(10, 40, relation));
        assert!(!within(10, 50, relation));
    }

    #[test]
    fn missing_cycles_and_zero_are_never_attributed() {
        let nodes = BTreeMap::from([
            (
                20,
                ProcessNode {
                    parent: 30,
                    started: 20,
                },
            ),
            (
                30,
                ProcessNode {
                    parent: 20,
                    started: 10,
                },
            ),
        ]);
        let relation = |candidate| nodes.get(&candidate).copied();

        assert!(!within(10, 20, relation));
        assert!(!within(10, 99, relation));
        assert!(!within(99, 99, relation));
        assert!(!within(0, 10, relation));
        assert!(!within(10, 0, relation));
    }

    #[test]
    fn a_reused_parent_pid_cannot_splice_unrelated_process_trees() {
        let nodes = BTreeMap::from([
            (
                10,
                ProcessNode {
                    parent: 1,
                    started: 10,
                },
            ),
            // Process 30 remembers PID 20 as its parent, but the current PID 20 occupant started later than 30.
            (
                20,
                ProcessNode {
                    parent: 10,
                    started: 30,
                },
            ),
            (
                30,
                ProcessNode {
                    parent: 20,
                    started: 20,
                },
            ),
        ]);

        assert!(!within(10, 30, |candidate| nodes.get(&candidate).copied()));
    }

    #[test]
    fn exact_endpoint_stamps_close_pid_reuse_at_both_ends() {
        let nodes = BTreeMap::from([
            (
                10,
                ProcessNode {
                    parent: 1,
                    started: 10,
                },
            ),
            (
                20,
                ProcessNode {
                    parent: 10,
                    started: 20,
                },
            ),
        ]);
        let root = ProcessIdentity::new(10, 10).expect("the fixture root is usable");
        let candidate = ProcessIdentity::new(20, 20).expect("the fixture child is usable");
        let stale_root = ProcessIdentity::new(10, 9).expect("the stale fixture root is usable");
        let stale_candidate =
            ProcessIdentity::new(20, 19).expect("the stale fixture child is usable");

        assert!(within_identity(root, candidate, |pid| nodes
            .get(&pid)
            .copied()));
        assert!(!within_identity(stale_root, candidate, |pid| nodes
            .get(&pid)
            .copied()));
        assert!(!within_identity(root, stale_candidate, |pid| nodes
            .get(&pid)
            .copied()));
    }

    #[test]
    fn captured_tree_contains_the_current_process_itself() {
        let tree = ProcessTree::capture().expect("the current process tree is inspectable");
        let current = process_identity(std::process::id())
            .expect("the current process has an exact kernel identity");

        assert!(tree.contains(std::process::id(), std::process::id()));
        assert!(tree.contains_identity(current, current));
    }

    #[test]
    fn captured_tree_contains_a_real_child_process() {
        let child = Command::new(std::env::current_exe().expect("the test executable has a path"))
            .args([
                "--exact",
                "process_tree::tests::descendant_probe_helper",
                "--nocapture",
            ])
            .env(DESCENDANT_PROBE_ENV, "1")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("the owned descendant probe starts");
        let child = ProbeChild(child);
        let tree = ProcessTree::capture().expect("the current process tree is inspectable");
        let root = process_identity(std::process::id())
            .expect("the current process has an exact kernel identity");
        let descendant =
            process_identity(child.0.id()).expect("the child has an exact kernel identity");

        assert!(tree.contains(std::process::id(), child.0.id()));
        assert!(tree.contains_identity(root, descendant));
    }

    #[test]
    fn descendant_probe_helper() {
        if std::env::var_os(DESCENDANT_PROBE_ENV).is_some() {
            std::thread::sleep(Duration::from_secs(30));
        }
    }
}
