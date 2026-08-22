//! Every path under `RUNTROL_HOME`, named exactly once.
//!
//! Nothing outside this file joins a path onto the runtrol home directory. The reason is a specific
//! bug rather than tidiness: when two call sites each compute where the daemon listens, the CLI
//! connects to one address while the daemon binds another, and on Windows that presents as "the
//! daemon will not start" with no error message anywhere. One resolution, computed once at startup
//! and handed around by reference, cannot disagree with itself.
//!
//! # What is here, and who reads it
//!
//! Each entry has exactly one reader, and each reader arrives in its own step. The layout is defined
//! as a whole because it is a single concern: the shape of a directory this program owns.
//!
//! | Entry | Reader |
//! |---|---|
//! | the database file | `runtrol-store`, the only code that opens it |
//! | `providers/` | the manifest loader, last in the discovery order and therefore able to shadow |
//! | `process-guards/` | child supervision, containing only bounded process identity records |
//! | the probe cache | the binary-identity cache of what each installed CLI can do |
//! | the Mission gate registry | fixed local Gate definitions, without process output |
//! | the isolated workspace registry | Core-owned chat worktree identities and cleanup state |
//! | the capability trust index | exact local digest approvals, without capability bodies |
//! | the provider update journal | verified version floors and rollback pins, never provider content |
//! | the machine identity vault | the per-user OS protector and Noise handshake assembly |
//! | `agent-tools/` | project-scoped public Runtime integration identities and grants |
//! | the daemon crash file | the detached daemon's panic hook writes it; the operator and gates read it |
//! | the endpoint | the daemon binds it, the CLI connects to it |
//! | the Runtime locator and endpoint | enrolled public SDK clients discover and connect to the separate surface |
//!
//! Deliberately absent: a general log directory. Where runtrol's ordinary diagnostics go has not
//! been decided. The crash file is the one decided exception: a detached daemon's streams go
//! nowhere, and one died with its reason unrecorded (measured three times), which is exactly the
//! silence the error rules forbid.
//!
//! # Why the endpoint is not simply a path
//!
//! On Unix the daemon listens on a socket file inside the home directory. On Windows it listens on a
//! named pipe, which is not a filesystem object at all and lives in a machine-global namespace. Two
//! home directories on one machine must not collide there, so the pipe name carries a fingerprint of
//! the home path. [`Endpoint`] is therefore a different shape per platform, constructed only here,
//! with the accessor each platform's listener actually needs.

use core::fmt;

use runtrol_provider::AbsPath;

use crate::home::HomeError;

/// The database. One file, opened by exactly one process at a time.
const DATABASE: &str = "runtrol.redb";

/// Separate bounded Mission evidence and recovery ledger.
const MISSION_LEDGER: &str = "mission-ledger.redb";

/// Fixed local Mission Gate definitions.
const MISSION_GATES: &str = "mission-gates.json";

/// Core-owned ordinary-chat worktrees and their exact cleanup state.
const ISOLATED_WORKSPACES: &str = "isolated-workspaces.json";

/// Exact local capability approvals and states.
const CAPABILITY_TRUST: &str = "capability-trust.json";

/// Provider manifests the operator wrote.
const PROVIDERS: &str = "providers";

/// Durable Unix child identities used only to recover process groups after an unclean daemon exit.
const PROCESS_GUARDS: &str = "process-guards";

/// What each installed CLI was found to support, keyed by its version.
const PROBE_CACHE: &str = "probe.json";

/// Verified provider versions and targets paused after an automatic rollback.
const PROVIDER_UPDATES: &str = "provider-updates.json";

/// The operating-system-protected long-lived machine identity.
const MACHINE_IDENTITY: &str = "machine-identity.vault";

/// Project-scoped Agent Tools Runtime credentials.
const AGENT_TOOLS: &str = "agent-tools";

/// Where the detached daemon's panic hook records why it died.
const DAEMON_CRASH_LOG: &str = "daemon-crash.log";

/// Atomic bootstrap record for the separate public Runtime endpoint.
const RUNTIME_LOCATOR: &str = "runtime.locator.json";

/// Durable identity of this installed Runtime home, regenerated after full uninstall.
const RUNTIME_INSTANCE: &str = "runtime-instance.json";

/// Directories runtrol creates when it opens a home.
///
/// Created up front rather than on first write, so that `rm -rf $RUNTROL_HOME` followed by a start
/// is a supported sequence rather than a race between whoever writes first.
const DIRECTORIES: [&str; 3] = [PROVIDERS, PROCESS_GUARDS, AGENT_TOOLS];

/// Every path runtrol uses inside its home directory, resolved.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Layout {
    /// The home directory itself, canonical.
    root: AbsPath,
    /// The database file.
    database: AbsPath,
    /// Separate Mission evidence ledger file.
    mission_ledger: AbsPath,
    /// Fixed local Mission Gate registry.
    mission_gates: AbsPath,
    /// Core-owned ordinary-chat worktree registry.
    isolated_workspaces: AbsPath,
    /// Exact local capability approval index.
    capability_trust: AbsPath,
    /// The operator's manifest directory.
    providers: AbsPath,
    /// Bounded durable process identities for restart recovery.
    process_guards: AbsPath,
    /// The probe cache file.
    probe_cache: AbsPath,
    /// Verified provider version floors and rollback pins.
    provider_updates: AbsPath,
    /// The operating-system-protected machine identity.
    machine_identity: AbsPath,
    /// Project-scoped Agent Tools Runtime credentials.
    agent_tools: AbsPath,
    /// Where the detached daemon's panic hook records why it died.
    daemon_crash_log: AbsPath,
    /// Where public SDK clients find the running Runtime instance.
    runtime_locator: AbsPath,
    /// Durable identity that binds locators and initialization across ordinary restarts.
    runtime_instance: AbsPath,
    /// Where the daemon listens.
    endpoint: Endpoint,
    /// Where enrolled public Runtime clients connect.
    runtime_endpoint: Endpoint,
}

impl Layout {
    /// Work out every path under an already-canonical `root`.
    ///
    /// Touches no filesystem: this is arithmetic on a path, which is what makes it testable for any
    /// root on any platform. Creating the directories belongs to [`super::RuntrolHome::open`].
    ///
    /// # Errors
    ///
    /// [`HomeError::Layout`] when a segment cannot be joined, and on Unix
    /// [`HomeError::SocketPathTooLong`] when the socket path would not fit the kernel's field.
    pub(crate) fn resolve(root: AbsPath) -> Result<Self, HomeError> {
        let entry = |segment: &'static str| {
            root.join(segment)
                .map_err(|source| HomeError::Layout { segment, source })
        };

        Ok(Self {
            database: entry(DATABASE)?,
            mission_ledger: entry(MISSION_LEDGER)?,
            mission_gates: entry(MISSION_GATES)?,
            isolated_workspaces: entry(ISOLATED_WORKSPACES)?,
            capability_trust: entry(CAPABILITY_TRUST)?,
            providers: entry(PROVIDERS)?,
            process_guards: entry(PROCESS_GUARDS)?,
            probe_cache: entry(PROBE_CACHE)?,
            provider_updates: entry(PROVIDER_UPDATES)?,
            machine_identity: entry(MACHINE_IDENTITY)?,
            agent_tools: entry(AGENT_TOOLS)?,
            daemon_crash_log: entry(DAEMON_CRASH_LOG)?,
            runtime_locator: entry(RUNTIME_LOCATOR)?,
            runtime_instance: entry(RUNTIME_INSTANCE)?,
            endpoint: Endpoint::of(&root)?,
            runtime_endpoint: Endpoint::runtime_of(&root)?,
            root,
        })
    }

    /// The home directory.
    #[must_use]
    pub const fn root(&self) -> &AbsPath {
        &self.root
    }

    /// The database file.
    #[must_use]
    pub const fn database(&self) -> &AbsPath {
        &self.database
    }

    /// The separate bounded Mission evidence ledger.
    #[must_use]
    pub const fn mission_ledger(&self) -> &AbsPath {
        &self.mission_ledger
    }

    /// Fixed local Mission Gate registry.
    #[must_use]
    pub const fn mission_gates(&self) -> &AbsPath {
        &self.mission_gates
    }

    /// Core-owned ordinary-chat worktree registry.
    #[must_use]
    pub const fn isolated_workspaces(&self) -> &AbsPath {
        &self.isolated_workspaces
    }

    /// Exact local capability approval index.
    #[must_use]
    pub const fn capability_trust(&self) -> &AbsPath {
        &self.capability_trust
    }

    /// The directory the operator puts their own provider manifests in.
    #[must_use]
    pub const fn providers(&self) -> &AbsPath {
        &self.providers
    }

    /// The bounded process-identity directory used for restart recovery.
    #[must_use]
    pub const fn process_guards(&self) -> &AbsPath {
        &self.process_guards
    }

    /// The probe cache file.
    #[must_use]
    pub const fn probe_cache(&self) -> &AbsPath {
        &self.probe_cache
    }

    /// The bounded provider update safety journal.
    #[must_use]
    pub const fn provider_updates(&self) -> &AbsPath {
        &self.provider_updates
    }

    /// The operating-system-protected long-lived machine identity.
    #[must_use]
    pub const fn machine_identity(&self) -> &AbsPath {
        &self.machine_identity
    }

    /// The directory containing project-scoped Agent Tools credentials.
    #[must_use]
    pub const fn agent_tools(&self) -> &AbsPath {
        &self.agent_tools
    }

    /// Resolve one digest-named Agent Tools credential slot.
    ///
    /// # Errors
    ///
    /// The slot name or fixed child names cannot be joined safely below the Agent Tools directory.
    pub fn agent_tool_slot(
        &self,
        slot: &str,
    ) -> Result<AgentToolSlot, runtrol_provider::PathError> {
        let directory = self.agent_tools.join(slot)?;
        Ok(AgentToolSlot {
            identity: directory.join("identity.vault")?,
            grant: directory.join("grant.json")?,
            directory,
        })
    }

    /// Where the detached daemon's panic hook records why it died.
    #[must_use]
    pub const fn daemon_crash_log(&self) -> &AbsPath {
        &self.daemon_crash_log
    }

    /// Atomic public Runtime bootstrap record.
    #[must_use]
    pub const fn runtime_locator(&self) -> &AbsPath {
        &self.runtime_locator
    }

    /// Durable identity of this installed Runtime home.
    #[must_use]
    pub const fn runtime_instance(&self) -> &AbsPath {
        &self.runtime_instance
    }

    /// Where the daemon listens and the CLI connects.
    #[must_use]
    pub const fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    /// Dedicated public Runtime endpoint, separate from private control IPC.
    #[must_use]
    pub const fn runtime_endpoint(&self) -> &Endpoint {
        &self.runtime_endpoint
    }

    /// The directories that have to exist before anything writes.
    pub(crate) const fn directories(&self) -> [&AbsPath; DIRECTORIES.len()] {
        [&self.providers, &self.process_guards, &self.agent_tools]
    }

    /// Every file and directory this layout names, for tests that must see the whole set.
    #[cfg(test)]
    fn everything(&self) -> Vec<&AbsPath> {
        vec![
            &self.database,
            &self.mission_ledger,
            &self.mission_gates,
            &self.isolated_workspaces,
            &self.capability_trust,
            &self.providers,
            &self.process_guards,
            &self.probe_cache,
            &self.provider_updates,
            &self.machine_identity,
            &self.agent_tools,
            &self.daemon_crash_log,
            &self.runtime_locator,
            &self.runtime_instance,
        ]
    }
}

/// Every path in one project-scoped Agent Tools credential slot.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct AgentToolSlot {
    directory: AbsPath,
    identity: AbsPath,
    grant: AbsPath,
}

impl AgentToolSlot {
    /// The slot directory.
    #[must_use]
    pub const fn directory(&self) -> &AbsPath {
        &self.directory
    }

    /// The operating-system-protected integration identity.
    #[must_use]
    pub const fn identity(&self) -> &AbsPath {
        &self.identity
    }

    /// The public approved Runtime grant.
    #[must_use]
    pub const fn grant(&self) -> &AbsPath {
        &self.grant
    }
}

/// The socket file the daemon binds on Unix.
#[cfg(unix)]
const SOCKET: &str = "runtrol.sock";

/// The separate public Runtime socket file.
#[cfg(unix)]
const RUNTIME_SOCKET: &str = "runtrol-runtime.sock";

/// How many bytes of socket path the kernel's address field holds, including the terminating NUL.
///
/// Not a preference. `sockaddr_un::sun_path` is a fixed array, and a path that does not fit is
/// refused by `bind` with an error that says nothing about length. Checking it here turns a mystery
/// into a sentence naming the path and the limit.
#[cfg(target_os = "linux")]
const SUN_PATH_CAPACITY: usize = 108;

/// How many bytes of socket path the kernel's address field holds, including the terminating NUL.
#[cfg(all(unix, not(target_os = "linux")))]
const SUN_PATH_CAPACITY: usize = 104;

/// Where the daemon listens, in the form this platform's listener takes.
///
/// A socket file on Unix, a named pipe on Windows. Constructed only by [`Layout::resolve`], so the
/// two ends of the local connection cannot derive it differently.
#[cfg(unix)]
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Endpoint(AbsPath);

/// Where the daemon listens, in the form this platform's listener takes.
#[cfg(windows)]
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Endpoint(String);

#[cfg(unix)]
impl Endpoint {
    /// Where to listen or connect, as the transport takes it.
    ///
    /// One accessor on both platforms even though what it names is different on each. The alternative is every
    /// caller writing the same branch on the platform, and a branch repeated at every call site is a rule nobody
    /// owns: what an endpoint is belongs here, and everywhere else it is an address.
    #[must_use]
    pub fn address(&self) -> &str {
        self.0.as_str()
    }

    /// The socket file for a home directory.
    fn of(root: &AbsPath) -> Result<Self, HomeError> {
        Self::with_segment(root, SOCKET)
    }

    /// The public Runtime socket file for a home directory.
    fn runtime_of(root: &AbsPath) -> Result<Self, HomeError> {
        Self::with_segment(root, RUNTIME_SOCKET)
    }

    fn with_segment(root: &AbsPath, segment: &'static str) -> Result<Self, HomeError> {
        let path = root
            .join(segment)
            .map_err(|source| HomeError::Layout { segment, source })?;

        // The NUL the kernel stores after the path is part of the field, so the usable length is one
        // less than the capacity.
        if path.as_str().len() + 1 > SUN_PATH_CAPACITY {
            return Err(HomeError::SocketPathTooLong {
                path,
                limit: SUN_PATH_CAPACITY - 1,
            });
        }
        Ok(Self(path))
    }
}

#[cfg(windows)]
impl Endpoint {
    /// Where to listen or connect, as the transport takes it.
    ///
    /// One accessor on both platforms even though what it names is different on each. See the Unix half for why.
    #[must_use]
    pub fn address(&self) -> &str {
        &self.0
    }

    /// The pipe name for a home directory.
    ///
    /// A pipe name cannot contain a path separator and the namespace is machine-global, so the home
    /// directory goes in as a fingerprint. Two homes on one machine (an operator's real one and a
    /// test's temporary one) therefore get two pipes, and one home gets the same pipe name from
    /// every process that asks.
    ///
    /// Cannot fail: a fingerprint is a fixed sixteen characters, so unlike a socket path there is no
    /// length for it to run over. The fallible signature is the Unix one, kept identical so that
    /// [`Layout::resolve`] calls one function rather than branching on the platform itself.
    #[expect(
        clippy::unnecessary_wraps,
        reason = "one signature for both platforms keeps the cfg out of Layout::resolve"
    )]
    fn of(root: &AbsPath) -> Result<Self, HomeError> {
        /// The namespace Windows requires for a pipe on this machine.
        const PREFIX: &str = r"\\.\pipe\runtrol-";

        Ok(Self(format!("{PREFIX}{:016x}", fingerprint(root))))
    }

    /// The separate public Runtime pipe name for a home directory.
    #[expect(
        clippy::unnecessary_wraps,
        reason = "one signature for both platforms keeps the cfg out of Layout::resolve"
    )]
    fn runtime_of(root: &AbsPath) -> Result<Self, HomeError> {
        /// The dedicated public endpoint cannot share a method table with private control IPC.
        const PREFIX: &str = r"\\.\pipe\runtrol-runtime-";

        Ok(Self(format!("{PREFIX}{:016x}", fingerprint(root))))
    }
}

/// A stable 64-bit fingerprint of a home directory.
///
/// Hand-written FNV-1a rather than `DefaultHasher`, which documents that its output may change
/// between releases. The CLI and the daemon are two processes that must agree on this value, so
/// "stable across processes and builds" is the whole requirement.
///
/// ASCII case is folded, because Windows treats two spellings of one directory as the same directory
/// and would otherwise give them two pipes. Only ASCII, matching how `AbsPath` compares components:
/// full case folding needs the OS's own table, and this only has to be consistent, not linguistic.
#[cfg(windows)]
fn fingerprint(root: &AbsPath) -> u64 {
    /// FNV-1a's 64-bit offset basis.
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    /// FNV-1a's 64-bit prime.
    const PRIME: u64 = 0x0000_0100_0000_01b3;

    let mut hash = OFFSET;
    for byte in root.as_str().bytes() {
        hash ^= u64::from(byte.to_ascii_lowercase());
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}

impl fmt::Display for Endpoint {
    /// The address as a person would type it, so an error can name where it went looking.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        #[cfg(unix)]
        return f.write_str(self.0.as_str());
        #[cfg(windows)]
        return f.write_str(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An absolute path for the platform the test is running on.
    fn abs(tail: &str) -> AbsPath {
        let text = if cfg!(windows) {
            format!("C:\\{tail}")
        } else {
            format!("/{tail}")
        };
        AbsPath::new(&text).expect("the test's own path must be valid")
    }

    #[test]
    fn every_named_path_is_inside_the_home_and_distinct_from_the_others() {
        // Two entries resolving to one path would mean two owners writing one file. Distinctness is
        // the property this file exists to hold, so it is checked rather than assumed from reading.
        let root = abs("state/runtrol");
        let layout = Layout::resolve(root.clone()).expect("a short root must resolve");

        let all = layout.everything();
        for path in &all {
            assert!(path.is_under(&root), "{path:?} escaped {root:?}");
            assert_ne!(*path, &root, "an entry must not be the root itself");
        }
        for (index, path) in all.iter().enumerate() {
            for other in all.iter().skip(index + 1) {
                assert_ne!(path, other, "two entries resolve to the same path");
            }
        }
        assert_eq!(layout.root(), &root);
    }

    #[test]
    fn the_directories_to_create_are_the_ones_declared() {
        let layout = Layout::resolve(abs("state/runtrol")).expect("resolve");
        let created = layout.directories();
        assert_eq!(created.len(), DIRECTORIES.len());
        for (path, segment) in created.iter().zip(DIRECTORIES) {
            assert_eq!(path.file_name(), Some(segment));
        }
    }

    #[test]
    fn one_root_always_resolves_to_the_same_endpoint() {
        // The CLI and the daemon are separate processes computing this independently. If it were not
        // a function of the root alone, they would miss each other and neither would say why.
        let first = Layout::resolve(abs("state/runtrol")).expect("resolve");
        let second = Layout::resolve(abs("state/runtrol")).expect("resolve");
        assert_eq!(first.endpoint(), second.endpoint());
        assert_eq!(first.runtime_endpoint(), second.runtime_endpoint());
    }

    #[test]
    fn two_roots_resolve_to_two_endpoints() {
        // A test's temporary home and the operator's real home run at the same time. Sharing an
        // address would let one connect to the other.
        let first = Layout::resolve(abs("state/runtrol")).expect("resolve");
        let second = Layout::resolve(abs("other/runtrol")).expect("resolve");
        assert_ne!(first.endpoint(), second.endpoint());
        assert_ne!(first.runtime_endpoint(), second.runtime_endpoint());
    }

    #[test]
    fn private_control_and_public_runtime_have_distinct_endpoints() {
        let layout = Layout::resolve(abs("state/runtrol")).expect("resolve");
        assert_ne!(layout.endpoint(), layout.runtime_endpoint());
    }

    #[test]
    fn the_endpoint_can_name_itself_in_an_error_message() {
        let layout = Layout::resolve(abs("state/runtrol")).expect("resolve");
        assert!(!layout.endpoint().to_string().is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn the_socket_lives_in_the_home_directory() {
        let root = abs("state/runtrol");
        let layout = Layout::resolve(root.clone()).expect("resolve");
        // Checked as the text the transport is given, because that is what an endpoint is once it leaves here.
        let socket = AbsPath::new(layout.endpoint().address()).expect("the address is a path here");
        assert!(socket.is_under(&root));
        assert_eq!(socket.file_name(), Some(SOCKET));
        let runtime_socket = AbsPath::new(layout.runtime_endpoint().address())
            .expect("the Runtime address is a path here");
        assert!(runtime_socket.is_under(&root));
        assert_eq!(runtime_socket.file_name(), Some(RUNTIME_SOCKET));
    }

    #[cfg(unix)]
    #[test]
    fn a_socket_path_the_kernel_cannot_hold_is_refused_by_name() {
        // `bind` would refuse this with an error that never mentions length, which is how it becomes
        // "the daemon will not start" with nothing in any log.
        let deep = abs(&"d".repeat(SUN_PATH_CAPACITY));
        match Layout::resolve(deep) {
            Err(HomeError::SocketPathTooLong { path, limit }) => {
                assert_eq!(limit, SUN_PATH_CAPACITY - 1);
                assert!(path.as_str().len() > limit);
            }
            other => panic!("expected a refusal that names the path, got {other:?}"),
        }
    }

    #[cfg(windows)]
    #[test]
    fn the_pipe_is_in_the_local_namespace_and_carries_no_separator() {
        // A path separator past the prefix would name a directory in the pipe namespace, which is
        // not what a listener accepts.
        let layout = Layout::resolve(abs("state/runtrol")).expect("resolve");
        let name = layout.endpoint().address();
        let tail = name
            .strip_prefix(r"\\.\pipe\")
            .expect("a local pipe name must carry the local namespace prefix");
        assert!(!tail.contains('\\'), "{name}");
        assert!(tail.starts_with("runtrol-"), "{name}");
        let runtime_name = layout.runtime_endpoint().address();
        let runtime_tail = runtime_name
            .strip_prefix(r"\\.\pipe\")
            .expect("a local Runtime pipe must carry the local namespace prefix");
        assert!(!runtime_tail.contains('\\'), "{runtime_name}");
        assert!(
            runtime_tail.starts_with("runtrol-runtime-"),
            "{runtime_name}"
        );
    }

    #[cfg(windows)]
    #[test]
    fn two_spellings_of_one_directory_get_one_pipe() {
        // Windows opens the same directory for either spelling, so two pipes would mean two daemons
        // over one database, and the second would find the file locked with no explanation.
        let upper =
            Layout::resolve(AbsPath::new(r"C:\State\Runtrol").expect("valid")).expect("resolve");
        let lower =
            Layout::resolve(AbsPath::new(r"c:\state\runtrol").expect("valid")).expect("resolve");
        assert_eq!(upper.endpoint(), lower.endpoint());
    }
}
