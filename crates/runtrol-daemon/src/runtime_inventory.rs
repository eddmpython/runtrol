//! Public inventory adapters over the registry and the single managed-session catalogue.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use runtrol_core::{BinFacts, ProbeCache, SessionManager, locate};
use runtrol_provider::{
    AbsPath, NativeSessionId, ProviderId as CoreProviderId, SessionId, TerminalId,
};
use runtrol_runtime_protocol::{
    InstallationObservation, InstallationState, ManagedSessionList, ProviderDescriptor,
    ProviderHelp, ProviderId, ProviderList, RuntimeSessionId, SessionDescriptor,
};
use runtrol_security::ProjectRootIdentity;
use runtrol_store::IntegrationRootRow;

use crate::Composed;
use crate::runtime_auth::AuthorizedIntegration;
use crate::runtime_control::public_waiting;

/// Coalesce one burst of identical provider list requests.
///
/// Runtime brackets provider operations with inventory publication, and Studio refreshes may be queued together.
/// A local installation cannot complete meaningfully inside this interval. With the official catalogue present, a
/// foreground restamp measured 314 to 320 ms on Windows, so list reads schedule it off their response path. Probe
/// writes bypass the floor through explicit invalidation.
/// How long one provider inventory answer stays good before the next ask may restamp the search path.
///
/// Ten seconds, not one. At one second the sidebar's 40-sample refresh window (~3 s) contained two to four
/// restamps, and a restamp that runs with cold file caches costs 85~130 ms on Windows, which is what the
/// refresh p95 measured (2026-08-30: 0.1.37 tree 33~48 ms because the activity observation of that build
/// happened to keep the walk warm every second; the build that stopped refreshing on activity measured
/// 84~129 ms on the same machine). At ten seconds a measurement window holds at most one walk, and a p95
/// over forty samples absorbs one. A newly installed service appears within this floor; starting or opening
/// a service resolves its executable at use and never waits on this cache.
const PROVIDER_INVENTORY_RECHECK_FLOOR: Duration = Duration::from_secs(10);

/// Safe reason a public session snapshot could not be authorized.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RuntimeInventoryFailure {
    /// The durable source catalogue could not be read.
    Unavailable,
    /// At least one granted root no longer names the approved filesystem object.
    RootAuthorityChanged,
    /// The session is absent or outside this integration's approved roots.
    SessionNotFound,
}

/// One public session plus the canonicalization input required for grant filtering.
pub(crate) struct RuntimeSessionRecord {
    session: runtrol_provider::SessionId,
    provider: CoreProviderId,
    native: Option<Box<str>>,
    descriptor: SessionDescriptor,
    workspace: Box<str>,
}

/// One immutable snapshot published by the session owner.
pub(crate) struct RuntimeSessionCatalogue {
    sessions: Vec<RuntimeSessionRecord>,
    unreadable: usize,
    available: bool,
}

/// One bounded provider snapshot and the local filesystem facts that make it current.
pub(crate) struct ProviderInventoryCache {
    current: Option<CachedProviderInventory>,
    revision: u64,
    background_revision: Option<u64>,
}

impl Default for ProviderInventoryCache {
    fn default() -> Self {
        Self {
            current: None,
            revision: 1,
            background_revision: None,
        }
    }
}

struct CachedProviderInventory {
    /// Whether the account reports were readable when this was built.
    ///
    /// A snapshot built while the probe held them carries no account on any row, which is a different
    /// picture from the truth and not one to keep. It is answered once and then thrown away.
    accounts_read: bool,
    checked_at: Instant,
    stamp: ProviderInventoryStamp,
    resolved_programs: Vec<PathBuf>,
    list: ProviderList,
}

#[derive(Debug, PartialEq, Eq)]
struct ProviderInventoryStamp {
    path: Option<OsString>,
    path_ext: Option<OsString>,
    files: Vec<PathFingerprint>,
}

#[derive(Debug, PartialEq, Eq)]
struct PathFingerprint {
    path: PathBuf,
    facts: Option<PathFacts>,
}

#[derive(Debug, PartialEq, Eq)]
struct PathFacts {
    directory: bool,
    bytes: u64,
    modified: Option<SystemTime>,
}

impl ProviderInventoryCache {
    fn begin_background_refresh(&mut self) -> Option<u64> {
        if self.background_revision.is_some()
            || self.current.as_ref().is_some_and(|current| {
                current.checked_at.elapsed() < PROVIDER_INVENTORY_RECHECK_FLOOR
            })
        {
            return None;
        }
        self.background_revision = Some(self.revision);
        Some(self.revision)
    }

    fn finish_background_refresh(&mut self, revision: u64) -> bool {
        if self.background_revision == Some(revision) {
            self.background_revision = None;
        }
        self.revision == revision
    }

    fn invalidate(&mut self) {
        self.revision = self.revision.wrapping_add(1).max(1);
        self.current = None;
        // The in-flight scan belongs to the previous revision. Detach it now so the authoritative account
        // write can immediately start a replacement; its eventual completion cannot clear the new revision.
        self.background_revision = None;
    }
}

impl PathFingerprint {
    #[expect(
        clippy::result_map_or_into_option,
        reason = "the workspace forbids Result::ok because silent errors are unsafe generally; this fingerprint deliberately records an unreadable timestamp as absent"
    )]
    fn read(path: PathBuf) -> Self {
        let facts = match std::fs::metadata(&path) {
            Ok(metadata) => Some(PathFacts {
                directory: metadata.is_dir(),
                bytes: metadata.len(),
                // An unsupported timestamp is an explicit part of the fingerprint. File presence and size still
                // participate, and the directory fingerprint covers entry creation and removal.
                modified: metadata.modified().map_or(None, Some),
            }),
            // Missing and unreadable are the same safe cache state: resolution cannot use either. If it becomes
            // readable, the next fingerprint changes to `Some` and invalidates the snapshot.
            Err(_) => None,
        };
        Self { path, facts }
    }
}

impl ProviderInventoryStamp {
    fn read(composed: &Composed, resolved_programs: &[PathBuf]) -> Self {
        let path = std::env::var_os("PATH");
        let mut watched: std::collections::BTreeSet<PathBuf> = path
            .as_deref()
            .into_iter()
            .flat_map(std::env::split_paths)
            .collect();
        watched.insert(
            composed
                .home
                .paths()
                .probe_cache()
                .as_std_path()
                .to_path_buf(),
        );
        watched.extend(resolved_programs.iter().cloned());
        for provider in composed.registry.all() {
            for candidate in &provider.manifest.bin.names {
                let candidate = Path::new(candidate.as_ref());
                if candidate.is_absolute() {
                    watched.insert(candidate.to_path_buf());
                    if let Some(parent) = candidate.parent() {
                        watched.insert(parent.to_path_buf());
                    }
                }
            }
        }
        Self {
            path,
            path_ext: std::env::var_os("PATHEXT"),
            files: watched.into_iter().map(PathFingerprint::read).collect(),
        }
    }
}

/// Project the supervisor's account gauges into the public usage list.
///
/// Structured fields only. The verbatim payload never reaches this list: it rides the session event stream under
/// session-output authority, and this list answers under provider authority.
/// The usage list with the windows the account probe read on request merged in.
///
/// A turn's report and a probe's report fill the same gauge. The newer of the two wins per service, so a
/// window read a minute ago never hides the one a turn reported just now, and a service that has had no
/// turn since the daemon started still shows where its account stands.
pub(crate) fn merge_probed_usage(
    current: &runtrol_runtime_protocol::ProviderUsageList,
    composed: &Composed,
) -> runtrol_runtime_protocol::ProviderUsageList {
    let probed = match composed.account_reports.try_lock() {
        Ok(reports) => reports.probed_gauges(),
        // The probe is writing a report this instant; the publish it triggers next reads them all.
        Err(_) => Vec::new(),
    };
    let probed = provider_usage(&probed);
    let mut merged = current.clone();
    for gauge in probed.providers {
        match merged
            .providers
            .iter_mut()
            .find(|known| known.provider_id == gauge.provider_id)
        {
            Some(known) if known.at_ms >= gauge.at_ms => {
                // The turn's report is newer, so it owns every window it named. A probe fills only the
                // windows that report never mentioned, which is how a service whose turn frame carries one
                // bucket still shows the buckets only its account surface publishes.
                for window in gauge.windows {
                    if known.windows.iter().any(|kept| kept.id == window.id) {
                        continue;
                    }
                    known.windows.push(window);
                }
                known
                    .windows
                    .sort_by_key(|window| window.window_minutes.unwrap_or(u32::MAX));
                // The cap is on what a surface is handed, not on what one reading held. Two readings of one
                // service are each bounded on their own, so joining them without this published twice the
                // number the memory-budget contract fixes.
                known.windows.truncate(runtrol_provider::MAX_LIMIT_WINDOWS);
                // Only a probe reads a daily token count, so a turn arriving after one would otherwise take
                // the number off the row until the next round put it back.
                if known.tokens_today.is_none() {
                    known.tokens_today = gauge.tokens_today;
                }
            }
            Some(known) => {
                let cost = known.cost.take();
                *known = gauge;
                known.cost = cost;
            }
            None => merged.providers.push(gauge),
        }
    }
    merged
        .providers
        .sort_by(|left, right| left.provider_id.as_str().cmp(right.provider_id.as_str()));
    merged
}

pub(crate) fn provider_usage(
    gauges: &[runtrol_core::ProviderGauge],
) -> runtrol_runtime_protocol::ProviderUsageList {
    let window =
        |window: &runtrol_provider::Window| runtrol_runtime_protocol::ProviderUsageWindow {
            id: window.id.to_string(),
            label: window.label.as_ref().map(ToString::to_string),
            scope: window.scope.as_ref().map(ToString::to_string),
            governing: window.governing,
            used_percent: window.used_percent,
            resets_at_ms: window.resets_at.map(runtrol_provider::WallMs::as_millis),
            window_minutes: window.window_minutes,
        };
    runtrol_runtime_protocol::ProviderUsageList {
        providers: gauges
            .iter()
            .map(|gauge| runtrol_runtime_protocol::ProviderUsageGauge {
                provider_id: ProviderId::new(gauge.provider.as_str()),
                reached: gauge.reached,
                windows: gauge.windows.iter().map(window).collect(),
                cost: gauge
                    .cost
                    .as_ref()
                    .map(|cost| runtrol_runtime_protocol::ProviderUsageCost {
                        amount: cost.amount,
                        currency: cost.currency.to_string(),
                    }),
                tokens_today: gauge.tokens_today,
                at_ms: gauge.at.as_millis(),
            })
            .collect(),
    }
}

/// Build the fast provider inventory without starting any provider process.
#[expect(
    clippy::result_map_or_into_option,
    reason = "the workspace forbids Result::ok generally; try-lock contention deliberately selects an uncached bounded projection instead of blocking the executor"
)]
pub(crate) fn providers(composed: &Composed) -> ProviderList {
    // This projection is synchronous and is called from both async and non-async paths. A non-blocking Tokio lock
    // reuses the cache when uncontended without blocking an executor thread. A simultaneous caller computes its own
    // bounded snapshot instead of waiting while another thread walks the filesystem.
    let mut inventory_cache = composed.provider_inventory.try_lock().map_or(None, Some);
    if let Some(cache) = inventory_cache.as_mut()
        && let Some(current) = cache.current.as_mut()
    {
        if current.checked_at.elapsed() < PROVIDER_INVENTORY_RECHECK_FLOOR {
            return current.list.clone();
        }
        let stamp = ProviderInventoryStamp::read(composed, current.resolved_programs.as_slice());
        if current.stamp == stamp {
            current.checked_at = Instant::now();
            return current.list.clone();
        }
    }

    let built = build_provider_inventory(composed);
    let list = built.list.clone();
    if let Some(cache) = inventory_cache.as_mut()
        && built.accounts_read
    {
        cache.current = Some(built);
    }
    list
}

/// Recheck the executable search surface away from the request that asked to refresh.
///
/// The current snapshot is already complete enough to answer a list read. One bounded task restamps the local
/// filesystem, and the provider watch receives a changed snapshot when an installation appeared or disappeared.
/// Revision binding prevents a probe-cache invalidation from being overwritten by an older scan.
pub(crate) async fn providers_in_background(
    composed: Arc<Composed>,
) -> Result<Option<ProviderList>, String> {
    let revision = {
        let mut cache = composed.provider_inventory.lock().await;
        let Some(revision) = cache.begin_background_refresh() else {
            return Ok(None);
        };
        revision
    };
    let building = Arc::clone(&composed);
    let built = match tokio::task::spawn_blocking(move || build_provider_inventory(&building)).await
    {
        Ok(built) => built,
        Err(error) => {
            composed
                .provider_inventory
                .lock()
                .await
                .finish_background_refresh(revision);
            return Err(format!(
                "provider inventory background task did not complete: {error}"
            ));
        }
    };
    let list = built.list.clone();
    let mut cache = composed.provider_inventory.lock().await;
    if !cache.finish_background_refresh(revision) {
        return Ok(None);
    }
    if built.accounts_read {
        cache.current = Some(built);
    }
    Ok(Some(list))
}

fn build_provider_inventory(composed: &Composed) -> CachedProviderInventory {
    let probe_cache = ProbeCache::open(composed.home.paths().probe_cache());
    let registered: Vec<&runtrol_core::registry::Provider> = composed.registry.all().collect();
    // Resolved concurrently because each entry walks the operator's search path for its own executable and
    // stats what it finds, and that cost is per provider rather than shared. Done in sequence it meant every
    // supported CLI added delay to the moment the window becomes usable, which is the one wait a person feels
    // on every single launch. Threads rather than tasks because this is blocking filesystem work and the
    // function is called from places that are not async.
    let installations: Vec<(InstallationObservation, Option<PathBuf>)> =
        std::thread::scope(|scope| {
            let resolving: Vec<_> = registered
                .iter()
                .map(|provider| scope.spawn(|| installation(provider, &probe_cache)))
                .collect();
            resolving
                .into_iter()
                .map(|handle| {
                    handle.join().unwrap_or_else(|_| {
                        (
                            InstallationObservation {
                                // A panic while resolving one provider must not lose the other three. Reported as
                                // unavailable rather than missing: something went wrong here, and claiming the CLI is
                                // absent would send the operator to install what they may already have.
                                state: InstallationState::Unavailable,
                                version: None,
                                why: Some(
                                    "resolving this provider's executable did not complete"
                                        .to_owned(),
                                ),
                            },
                            None,
                        )
                    })
                })
                .collect()
        });
    let resolved_programs = installations
        .iter()
        .filter_map(|(_, path)| path.clone())
        .collect::<Vec<_>>();
    // Contended only while the probe writes a fresh report, for microseconds; this projection then
    // reads no account and answers without one. What it must not do is keep that answer: a snapshot with no
    // accounts in it looks exactly like a machine where nobody is signed in, and caching one meant a single
    // unlucky instant took the plan, the sign-in state and the reason a service has no numbers off every row
    // until the cache aged out. So the miss is carried out of here and the caller declines to store it.
    let held = composed.account_reports.try_lock();
    let accounts: Option<&crate::account_probe::AccountReports> = match &held {
        Ok(reports) => Some(reports),
        Err(_) => None,
    };
    let accounts_read = accounts.is_some();
    let list = ProviderList {
        providers: registered
            .into_iter()
            .zip(installations)
            .map(|(provider, (installation, _))| ProviderDescriptor {
                provider_id: ProviderId::new(provider.id().as_str()),
                display_name: provider.manifest.display_name.to_string(),
                icon: provider.manifest.icon.as_ref().map(ToString::to_string),
                installation,
                command_names: provider
                    .manifest
                    .bin
                    .names
                    .iter()
                    .map(ToString::to_string)
                    .collect(),
                help: help(provider),
                switchable_modes: provider
                    .manifest
                    .modes
                    .switchable
                    .iter()
                    .map(ToString::to_string)
                    .collect(),
                account: accounts.and_then(|reports| reports.descriptor(provider.id())),
            })
            .collect(),
    };
    CachedProviderInventory {
        accounts_read,
        checked_at: Instant::now(),
        stamp: ProviderInventoryStamp::read(composed, &resolved_programs),
        resolved_programs,
        list,
    }
}

/// Force the next provider projection to observe a probe-cache write immediately.
pub(crate) async fn invalidate_provider_inventory(composed: &Composed) {
    composed.provider_inventory.lock().await.invalidate();
}

/// This service's own help commands, assembled into lines a person can read and run.
///
/// Uses the first declared binary name rather than a resolved path. Three reasons, and the third is the
/// deciding one: it is what an operator types in their own terminal, it needs no quoting on any platform,
/// and the install line is wanted exactly when nothing resolved at all, so depending on resolution would
/// withhold the one command that is still useful when the CLI is absent.
///
/// Returns `None` rather than an empty structure so that a client shows nothing instead of an action that
/// leads nowhere.
fn help(provider: &runtrol_core::registry::Provider) -> Option<ProviderHelp> {
    let declared = &provider.manifest.help;
    let command = provider.manifest.bin.names.first()?;
    let assembled = ProviderHelp {
        sign_in: help_line(command, &declared.sign_in),
        diagnose: help_line(command, &declared.diagnose),
        install: declared.install.as_ref().map(ToString::to_string),
    };
    (!assembled.is_empty()).then_some(assembled)
}

/// This service's own sign-out command, for the private admin surface only.
///
/// Not on the public `ProviderHelp`. Every shipped client validates the public inventory against a closed
/// schema (`additionalProperties: false`, and the Rust client's `deny_unknown_fields`), so a field added there
/// makes every window still running an older build reject the new daemon's whole provider list (measured
/// 2026-08-29 against the 0.1.36 client). The admin wire is additive by contract, and it is the only reader.
pub(crate) fn sign_out_line(composed: &Composed, provider_id: &str) -> Option<String> {
    let provider = composed
        .registry
        .all()
        .find(|provider| provider.id().as_str() == provider_id)?;
    let command = provider.manifest.bin.names.first()?;
    help_line(command, &provider.manifest.help.sign_out)
}

/// One of the service's own commands as a person would type it: the binary's first name, then the arguments.
fn help_line(command: &str, arguments: &[Box<str>]) -> Option<String> {
    (!arguments.is_empty()).then(|| {
        let mut text = command.to_string();
        for argument in arguments {
            text.push(' ');
            text.push_str(argument);
        }
        text
    })
}

fn installation(
    provider: &runtrol_core::registry::Provider,
    cache: &ProbeCache,
) -> (InstallationObservation, Option<PathBuf>) {
    if !provider.is_usable() {
        return (
            InstallationObservation {
                state: InstallationState::Unavailable,
                version: None,
                why: Some(
                    "this Runtime build has no driver for the declared provider kind".to_owned(),
                ),
            },
            None,
        );
    }
    let Ok(program) = locate(&provider.manifest) else {
        return (
            InstallationObservation {
                state: InstallationState::Missing,
                version: None,
                why: Some("no registered executable candidate is installed".to_owned()),
            },
            None,
        );
    };
    let resolved = Some(program.path().as_std_path().to_path_buf());
    let Ok(facts) = BinFacts::of_program(&program) else {
        return (
            InstallationObservation {
                state: InstallationState::Unavailable,
                version: None,
                why: Some("the installed executable identity could not be verified".to_owned()),
            },
            resolved,
        );
    };
    let observation = match cache.get(provider.id(), &facts) {
        Some(entry) => InstallationObservation {
            state: InstallationState::Usable,
            version: Some(entry.version.clone()),
            why: None,
        },
        None => InstallationObservation {
            state: InstallationState::Unavailable,
            version: None,
            why: Some("the installed executable has not completed a verified probe".to_owned()),
        },
    };
    (observation, resolved)
}

type ResidentSamples = std::collections::HashMap<ResidentKey, ResidentSample>;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct ResidentKey {
    pid: u32,
    owner: ResidentOwner,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum ResidentOwner {
    Session(SessionId, u64),
    Terminal(TerminalId, u64),
}

struct ResidentSample {
    taken: Instant,
    bytes: Option<u64>,
    refreshing: bool,
    refresh_generation: u64,
    notify_sessions: bool,
    terminal_notifiers: Vec<tokio::sync::watch::Sender<u64>>,
}

static RESIDENT_SAMPLES: std::sync::LazyLock<tokio::sync::Mutex<ResidentSamples>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(ResidentSamples::new()));
static RESIDENT_SESSION_CHANGES: std::sync::LazyLock<tokio::sync::watch::Sender<u64>> =
    std::sync::LazyLock::new(|| {
        let (changes, _initial) = tokio::sync::watch::channel(0);
        changes
    });
const RESIDENT_SAMPLE_FRESH_FOR: Duration = Duration::from_secs(2);
const RESIDENT_SAMPLE_DEADLINE: Duration = Duration::from_secs(1);
const MAX_RESIDENT_SAMPLES: usize = 32;
const MAX_RESIDENT_TERMINAL_NOTIFIERS: usize = 8;

/// Return the latest provider-process memory sample and refresh it outside the async executor when stale.
///
/// Absence is the honest answer, not a failure to report: the process may have ended between the listing and
/// the question, or the platform may refuse the question for a process this daemon may not open. Either way
/// a surface draws no number, never a zero.
///
/// A listing never opens a process handle. The first read returns the last bounded sample or `None` and
/// schedules one blocking-pool refresh. A figure a couple of seconds old is still the figure a person reads.
pub(crate) fn resident_bytes_for_session(
    pid: u32,
    session: SessionId,
    generation: u64,
) -> Option<u64> {
    resident_bytes_cached(
        ResidentKey {
            pid,
            owner: ResidentOwner::Session(session, generation),
        },
        None,
        true,
    )
}

/// Session catalogue publication wake for a completed background process-memory sample.
pub(crate) fn resident_session_changes() -> tokio::sync::watch::Receiver<u64> {
    RESIDENT_SESSION_CHANGES.subscribe()
}

/// Terminal memory sampling also rings the index when a background value changes.
pub(crate) fn resident_bytes_for_terminal(
    pid: u32,
    terminal: TerminalId,
    generation: u64,
    changed: &tokio::sync::watch::Sender<u64>,
) -> Option<u64> {
    resident_bytes_cached(
        ResidentKey {
            pid,
            owner: ResidentOwner::Terminal(terminal, generation),
        },
        Some(changed),
        false,
    )
}

fn resident_bytes_cached(
    key: ResidentKey,
    changed: Option<&tokio::sync::watch::Sender<u64>>,
    notify_sessions: bool,
) -> Option<u64> {
    let now = Instant::now();
    let Ok(mut samples) = RESIDENT_SAMPLES.try_lock() else {
        notify_resident_retry(changed, notify_sessions);
        return None;
    };
    if let Some(sample) = samples.get_mut(&key) {
        if sample.refreshing && now.duration_since(sample.taken) < RESIDENT_SAMPLE_DEADLINE {
            sample.notify_sessions |= notify_sessions;
            remember_terminal_notifier(&mut sample.terminal_notifiers, changed);
            return sample.bytes;
        }
        if now.duration_since(sample.taken) < RESIDENT_SAMPLE_FRESH_FOR {
            return sample.bytes;
        }
    }
    samples.retain(|sample_key, sample| {
        *sample_key == key
            || sample.refreshing
            || now.duration_since(sample.taken) < RESIDENT_SAMPLE_FRESH_FOR
    });
    let previous = samples.get(&key).and_then(|sample| sample.bytes);
    if !samples.contains_key(&key) && samples.len() >= MAX_RESIDENT_SAMPLES {
        return previous;
    }
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        return previous;
    };
    let refresh_generation = samples
        .get(&key)
        .map_or(1, |sample| sample.refresh_generation.wrapping_add(1).max(1));
    let mut terminal_notifiers = Vec::new();
    remember_terminal_notifier(&mut terminal_notifiers, changed);
    samples.insert(
        key,
        ResidentSample {
            taken: now,
            bytes: previous,
            refreshing: true,
            refresh_generation,
            notify_sessions,
            terminal_notifiers,
        },
    );
    drop(samples);
    drop(handle.spawn(async move {
        let observation = tokio::task::spawn_blocking(move || sample_resident_bytes(key.pid)).await;
        let now = Instant::now();
        let mut samples = RESIDENT_SAMPLES.lock().await;
        let Some(sample) = samples.get_mut(&key) else {
            return;
        };
        if sample.refresh_generation != refresh_generation {
            return;
        }
        let Ok(bytes) = observation else {
            sample.taken = now;
            sample.refreshing = false;
            sample.notify_sessions = false;
            sample.terminal_notifiers.clear();
            return;
        };
        let value_changed = sample.bytes != bytes;
        let notify_sessions = std::mem::replace(&mut sample.notify_sessions, false);
        let terminal_notifiers = std::mem::take(&mut sample.terminal_notifiers);
        sample.taken = now;
        sample.bytes = bytes;
        sample.refreshing = false;
        samples.retain(|sample_key, sample| {
            *sample_key == key
                || sample.refreshing
                || now.duration_since(sample.taken) < RESIDENT_SAMPLE_FRESH_FOR
        });
        drop(samples);
        if value_changed {
            if notify_sessions {
                RESIDENT_SESSION_CHANGES
                    .send_modify(|revision| *revision = revision.wrapping_add(1));
            }
            for changed in terminal_notifiers {
                changed.send_modify(|revision| *revision = revision.wrapping_add(1));
            }
        }
    }));
    previous
}

fn notify_resident_retry(changed: Option<&tokio::sync::watch::Sender<u64>>, notify_sessions: bool) {
    if notify_sessions {
        RESIDENT_SESSION_CHANGES.send_modify(|revision| *revision = revision.wrapping_add(1));
    }
    if let Some(changed) = changed {
        changed.send_modify(|revision| *revision = revision.wrapping_add(1));
    }
}

fn remember_terminal_notifier(
    notifiers: &mut Vec<tokio::sync::watch::Sender<u64>>,
    changed: Option<&tokio::sync::watch::Sender<u64>>,
) {
    let Some(changed) = changed else {
        return;
    };
    if notifiers.iter().any(|known| known.same_channel(changed))
        || notifiers.len() >= MAX_RESIDENT_TERMINAL_NOTIFIERS
    {
        return;
    }
    notifiers.push(changed.clone());
}

#[expect(
    clippy::manual_ok_err,
    reason = "`Result::ok` is disallowed workspace-wide so that a dropped error is always written out; here the absent number is the answer, and the arm says so"
)]
fn sample_resident_bytes(pid: u32) -> Option<u64> {
    match runtrol_childproc::footprint::resident_bytes(pid) {
        Ok(bytes) => Some(bytes),
        // A process that ended between the listing and the question, or one this daemon may not open, has no
        // number to report, and the descriptor says so by leaving the field absent.
        Err(_gone_or_refused) => None,
    }
}

/// Read the one session owner into an immutable public projection.
pub(crate) fn sessions(
    composed: &Composed,
    sessions: &SessionManager,
) -> Result<RuntimeSessionCatalogue, runtrol_store::StoreError> {
    let catalogue = crate::session_catalogue::read(composed, sessions)?;
    Ok(RuntimeSessionCatalogue {
        sessions: catalogue
            .sessions
            .into_iter()
            .map(|session| RuntimeSessionRecord {
                session: session.session,
                provider: session.provider,
                native: session.native.clone(),
                descriptor: SessionDescriptor {
                    session_id: RuntimeSessionId::new(session.session.to_string()),
                    provider_id: ProviderId::new(session.provider.as_str()),
                    native_session_id: session.native.as_deref().map(str::to_owned),
                    workspace: session.workspace.to_string(),
                    hot: session.hot,
                    lifecycle: session.lifecycle.public(session.hot),
                    // Retired: silence is not a proved state (`SessionDescriptor::looks_stuck`).
                    looks_stuck: false,
                    waiting_on: session.waiting.map(public_waiting),
                    session_generation: session.generation,
                    label: session.label.map(Into::into),
                    memory_bytes: session.pid.and_then(|pid| {
                        resident_bytes_for_session(pid, session.session, session.generation)
                    }),
                },
                workspace: session.workspace,
            })
            .collect(),
        unreadable: catalogue.warnings.len(),
        available: true,
    })
}

impl RuntimeSessionCatalogue {
    /// Publish an explicit unavailable snapshot after a durable catalogue read fails.
    pub(crate) const fn unavailable() -> Self {
        Self {
            sessions: Vec::new(),
            unreadable: 0,
            available: false,
        }
    }

    #[cfg(test)]
    pub(crate) const fn empty_for_tests() -> Self {
        Self {
            sessions: Vec::new(),
            unreadable: 0,
            available: true,
        }
    }

    #[cfg(test)]
    pub(crate) fn one_for_tests(
        provider: CoreProviderId,
        native: &str,
        workspace: &AbsPath,
    ) -> Self {
        let session = runtrol_provider::SessionId::now();
        Self {
            sessions: vec![RuntimeSessionRecord {
                session,
                provider,
                native: Some(native.into()),
                descriptor: SessionDescriptor {
                    session_id: RuntimeSessionId::new(session.to_string()),
                    provider_id: ProviderId::new(provider.as_str()),
                    native_session_id: Some(native.to_owned()),
                    workspace: workspace.to_string(),
                    hot: false,
                    lifecycle: runtrol_runtime_protocol::LifecycleState::Cold,
                    looks_stuck: false,
                    waiting_on: None,
                    session_generation: 0,
                    label: None,
                    memory_bytes: None,
                },
                workspace: workspace.as_str().into(),
            }],
            unreadable: 0,
            available: true,
        }
    }

    /// The first session's identity, for tests that need to address the one row they built.
    #[cfg(test)]
    pub(crate) fn first_session_id_for_tests(&self) -> Option<runtrol_provider::SessionId> {
        self.sessions.first().map(|row| row.session)
    }

    /// Every managed session on the machine, for a caller that already passed the scope wall.
    ///
    /// Deliberately not root-bounded (the folderless rule in `docs/runtimeProtocol.md`): the Runtime
    /// endpoint is owner-only local, and a local process already holds machine-wide authority through the
    /// admin wire, so bounding this second local wire by enrollment roots protected nothing while it broke
    /// the product's one promise: every conversation on the machine in one list, controllable before any
    /// window is moved. Root bounds remain exactly where they are security: the phone wire.
    pub(crate) fn authorized(
        &self,
        _authority: &AuthorizedIntegration,
    ) -> Result<ManagedSessionList, RuntimeInventoryFailure> {
        if !self.available {
            return Err(RuntimeInventoryFailure::Unavailable);
        }
        let sessions = self
            .sessions
            .iter()
            .map(|session| session.descriptor.clone())
            .collect();
        let warnings = if self.unreadable == 0 {
            Vec::new()
        } else {
            vec![format!(
                "{} stored session rows were unreadable and omitted",
                self.unreadable
            )]
        };
        Ok(ManagedSessionList { sessions, warnings })
    }

    /// Resolve one public session identity. Machine-wide for the same reason [`Self::authorized`] is.
    pub(crate) fn authorized_session(
        &self,
        _authority: &AuthorizedIntegration,
        requested: &runtrol_runtime_protocol::RuntimeSessionId,
    ) -> Result<runtrol_provider::SessionId, RuntimeInventoryFailure> {
        if !self.available {
            return Err(RuntimeInventoryFailure::Unavailable);
        }
        let session = self
            .sessions
            .iter()
            .find(|session| session.descriptor.session_id.as_str() == requested.as_str())
            .ok_or(RuntimeInventoryFailure::SessionNotFound)?;
        Ok(session.session)
    }

    /// Which provider owns one already-authorized session.
    ///
    /// For policy reads after [`Self::authorized_session`] succeeded; answers nothing about sessions this
    /// catalogue does not hold.
    pub(crate) fn provider_of(
        &self,
        session: runtrol_provider::SessionId,
    ) -> Option<CoreProviderId> {
        self.sessions
            .iter()
            .find(|row| row.session == session)
            .map(|row| row.provider)
    }

    /// Read one public descriptor. Machine-wide for the same reason the listing is.
    pub(crate) fn authorized_descriptor(
        &self,
        authority: &AuthorizedIntegration,
        requested: &runtrol_runtime_protocol::RuntimeSessionId,
    ) -> Result<SessionDescriptor, RuntimeInventoryFailure> {
        self.authorized_managed_session(authority, requested)
            .map(|session| session.descriptor)
    }

    /// Resolve the provider pointer and exact current workspace needed to heat one managed session.
    pub(crate) fn authorized_managed_session(
        &self,
        _authority: &AuthorizedIntegration,
        requested: &runtrol_runtime_protocol::RuntimeSessionId,
    ) -> Result<AuthorizedManagedSession, RuntimeInventoryFailure> {
        if !self.available {
            return Err(RuntimeInventoryFailure::Unavailable);
        }
        let session = self
            .sessions
            .iter()
            .find(|session| session.descriptor.session_id.as_str() == requested.as_str())
            .ok_or(RuntimeInventoryFailure::SessionNotFound)?;
        let workspace = AbsPath::canonicalize(&session.workspace)
            .map_err(|_| RuntimeInventoryFailure::SessionNotFound)?;
        Ok(AuthorizedManagedSession {
            session: session.session,
            provider: session.provider,
            native: session.native.clone(),
            descriptor: session.descriptor.clone(),
            workspace,
        })
    }

    /// Find an authorized managed pointer by the only safe native merge key.
    pub(crate) fn managed_as(
        &self,
        _authority: &AuthorizedIntegration,
        provider: CoreProviderId,
        native: &NativeSessionId,
    ) -> Result<Option<RuntimeSessionId>, RuntimeInventoryFailure> {
        if !self.available {
            return Err(RuntimeInventoryFailure::Unavailable);
        }
        Ok(self.sessions.iter().find_map(|session| {
            if session.provider != provider || session.native.as_deref() != Some(native.as_str()) {
                return None;
            }
            Some(session.descriptor.session_id.clone())
        }))
    }

    /// One live structured owner for an exact provider-native conversation.
    ///
    /// This is structural admission state only. It never reads provider output or storage to invent an identity.
    pub(crate) fn live_native_owner(
        &self,
        provider: CoreProviderId,
        native: &str,
    ) -> Option<RuntimeSessionId> {
        self.sessions.iter().find_map(|session| {
            (session.provider == provider
                && session.native.as_deref() == Some(native)
                && session.descriptor.hot)
                .then(|| session.descriptor.session_id.clone())
        })
    }
}

/// One exact currently valid approved root and its filesystem identity.
#[derive(Clone)]
pub(crate) struct AuthorizedRoot {
    pub(crate) path: AbsPath,
    pub(crate) identity: [u8; 24],
}

/// One current canonical workspace proven to remain below a locally approved root.
pub(crate) struct AuthorizedWorkspace {
    pub(crate) path: AbsPath,
}

/// One managed session resolved without disclosing anything outside the caller's roots.
pub(crate) struct AuthorizedManagedSession {
    pub(crate) session: runtrol_provider::SessionId,
    pub(crate) provider: CoreProviderId,
    pub(crate) native: Option<Box<str>>,
    pub(crate) descriptor: SessionDescriptor,
    pub(crate) workspace: AbsPath,
}

/// Revalidate every approved root before provider-supplied paths are filtered.
pub(crate) fn authorized_roots(
    authority: &AuthorizedIntegration,
) -> Result<Vec<AuthorizedRoot>, RuntimeInventoryFailure> {
    approved_roots(authority)
}

/// Resolve any exact current workspace on the machine.
///
/// Machine-wide for the same reason session reads are (`docs/runtimeProtocol.md`): the local surface starts
/// and resumes conversations wherever they live, without moving the window there first. The path still has
/// to exist and canonicalize, so a session cannot be aimed at a name that resolves elsewhere later.
pub(crate) fn authorized_workspace(
    _authority: &AuthorizedIntegration,
    requested: &str,
) -> Result<AuthorizedWorkspace, RuntimeInventoryFailure> {
    let workspace = AbsPath::canonicalize(requested)
        .map_err(|_| RuntimeInventoryFailure::RootAuthorityChanged)?;
    Ok(AuthorizedWorkspace { path: workspace })
}

fn approved_roots(
    authority: &AuthorizedIntegration,
) -> Result<Vec<AuthorizedRoot>, RuntimeInventoryFailure> {
    approved_root_rows(&authority.roots)
}

/// Revalidate exact stored roots without requiring a connection-owned authority value.
pub(crate) fn approved_root_rows(
    approved_rows: &[IntegrationRootRow],
) -> Result<Vec<AuthorizedRoot>, RuntimeInventoryFailure> {
    let mut roots = Vec::with_capacity(approved_rows.len());
    for root in approved_rows {
        let approved =
            AbsPath::new(&root.path).map_err(|_| RuntimeInventoryFailure::RootAuthorityChanged)?;
        // Approval stores the canonical path and kernel identity atomically. Re-canonicalizing that same trusted
        // path on every keystroke performs an avoidable filesystem traversal and stalls Windows terminals under
        // ordinary metadata jitter. Reading through the exact stored path still follows a replacement link or
        // junction to its new object, whose kernel identity cannot match the approved directory.
        let identity = ProjectRootIdentity::read(&approved)
            .map_err(|_| RuntimeInventoryFailure::RootAuthorityChanged)?;
        if identity.to_bytes() != root.identity {
            return Err(RuntimeInventoryFailure::RootAuthorityChanged);
        }
        roots.push(AuthorizedRoot {
            path: approved,
            identity: root.identity,
        });
    }
    Ok(roots)
}

#[cfg(test)]
mod tests {
    use runtrol_runtime_protocol::{AppScope, IntegrationGrant, IntegrationId, LifecycleState};
    use runtrol_store::{IntegrationKey, IntegrationRootRow};

    use super::*;

    #[test]
    fn a_background_provider_scan_cannot_overwrite_a_direct_invalidation() {
        let mut cache = ProviderInventoryCache::default();
        let first = cache
            .begin_background_refresh()
            .expect("an empty cache starts one background scan");
        assert!(
            cache.begin_background_refresh().is_none(),
            "a second list read coalesces behind the same scan"
        );
        cache.invalidate();
        let second = cache
            .begin_background_refresh()
            .expect("an invalidation can replace an obsolete in-flight scan immediately");
        assert_ne!(second, first);
        assert!(
            !cache.finish_background_refresh(first),
            "a probe write makes the older filesystem answer stale"
        );
        assert_eq!(
            cache.background_revision,
            Some(second),
            "the obsolete completion cannot clear the replacement scan"
        );
    }

    #[test]
    fn the_local_surface_sees_the_machine_even_when_an_enrollment_root_was_replaced() {
        let base = std::env::temp_dir().join(format!(
            "runtrol-runtime-root-replacement-{}",
            std::process::id()
        ));
        drop(std::fs::remove_dir_all(&base));
        let project_path = base.join("project");
        std::fs::create_dir_all(&project_path).expect("create project root");
        let project = AbsPath::canonicalize(project_path.to_str().expect("UTF-8 path"))
            .expect("canonical project root");
        let identity = ProjectRootIdentity::read(&project).expect("read root identity");
        let authority = AuthorizedIntegration {
            key: IntegrationKey::from_bytes([4; 16]),
            grant: IntegrationGrant {
                integration_id: IntegrationId::new("int_fixture"),
                scopes: vec![AppScope::SessionList],
                roots: vec![project.to_string()],
                key_generation: 1,
                grant_generation: 1,
            },
            roots: vec![IntegrationRootRow {
                path: project.as_str().into(),
                identity: identity.to_bytes(),
            }],
        };
        let catalogue = RuntimeSessionCatalogue {
            sessions: vec![RuntimeSessionRecord {
                session: runtrol_provider::SessionId::now(),
                provider: runtrol_provider::ProviderId::parse("provider-fixture")
                    .expect("valid provider"),
                native: Some("native_fixture".into()),
                descriptor: SessionDescriptor {
                    session_id: RuntimeSessionId::new("session_fixture"),
                    provider_id: ProviderId::new("provider_fixture"),
                    native_session_id: Some("native_fixture".to_owned()),
                    workspace: project.to_string(),
                    hot: false,
                    lifecycle: LifecycleState::Cold,
                    looks_stuck: false,
                    waiting_on: None,
                    session_generation: 0,
                    label: None,
                    memory_bytes: None,
                },
                workspace: project.as_str().into(),
            }],
            unreadable: 0,
            available: true,
        };
        assert_eq!(
            catalogue
                .authorized(&authority)
                .expect("original root remains authorized")
                .sessions
                .len(),
            1
        );

        let retired = base.join("retired");
        std::fs::rename(&project_path, &retired).expect("retire approved directory");
        std::fs::create_dir(&project_path).expect("replace directory at same path");
        // The folderless rule (`docs/runtimeProtocol.md`): the owner-only local wire is machine-wide, so
        // enrollment-root drift never hides a managed session here. Root identity keeps mattering exactly
        // where it is security: the phone wire, whose own tests pin that a replaced directory disappears.
        assert_eq!(
            catalogue
                .authorized(&authority)
                .expect("the local surface stays machine-wide")
                .sessions
                .len(),
            1,
            "a replaced enrollment root must not hide the machine from its owner"
        );
        std::fs::remove_dir_all(&base).expect("clean root replacement fixture");
    }
}
