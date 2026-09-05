//! The daemon, running.
//!
//! # One owner of the sessions, and everything else beside it
//!
//! Every session lives in one [`SessionManager`], and exactly one task ever touches it. That task does two things and
//! races them against each other: it takes the next request that any connection has asked about, and it waits for the
//! next event that any session produces. Everything else about a connection (reading its frames, writing its answers,
//! relaying what it is watching) belongs to that connection's own task, because none of it needs the sessions.
//!
//! The alternative is a lock around the sessions and a task per connection that takes it. That would make the map,
//! the event numbering and the tier bound each a thing two tasks can be inside at once, and every rule the kernel
//! states about ordering would become a rule about lock ordering instead. One owner is what makes those rules true
//! without any locking at all.
//!
//! # Nothing one caller does may stop another
//!
//! The owner task holds the sessions while it answers, so a provider process wait there would stop every session's
//! output. Probes, model discovery, process open, and process cleanup therefore run in connection tasks. The owner
//! only reserves or commits a process slot and synchronously hands an agent to its connection for a command write.
//! [`Reply::Sending`], [`Reply::Stopping`] and [`Reply::Cleaning`] hand every provider wait back out.
//!
//! An operator watching one session while closing another is the case this is for, and it is what the tests here
//! check: a slow close does not stop a running session's events.
//!
//! Connection and cleanup tasks live in one `JoinSet`. Every returned serve outcome aborts and reaps that set. Dropping
//! the serve future drops the set and aborts its tasks; process containment remains the final child teardown boundary.
//!
//! # What is not decided here
//!
//! Who may connect. The endpoint is inside a directory only the operator can enter and remote clients are refused by
//! the transport; the scope wall that reads where a request came from belongs at the dispatch boundary, which is where
//! it goes. This file gets frames to that boundary and answers back.

use core::future::Future;
use core::time::Duration;
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use runtrol_core::{
    AgentLease, ClosingReservation, OpenReservation, ProviderUpdateReservation, ReservedOpen,
    SessionError, SessionManager, TakenAgent, WorkspaceClaim,
};
use runtrol_ipc::transport::{Connection, Listener, TransportError};
use runtrol_ipc::wire::{Request, Response, SessionListing, WireError};
use runtrol_provider::{
    AbsPath, AgentCommand, CloseMode, ProviderError, ProviderId, SessionId, WorkspaceAccess,
};
use runtrol_security::Caller;
use runtrol_transport::{
    CryptoError, LinkKind, NoiseUpgrade, NoiseWebSocket, PhoneHttp, PhoneHttpError, SessionBinding,
    StaticKeypair, WebSocketLinkError, response,
};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinSet;
use tokio::time::Instant;

use crate::compose::Composed;
use crate::dispatch::{
    Cleanup, CleanupReservation, Conversation, Discovered, Prepared, PreparedKind, Reply,
    answer_prepared, complete_prepare_for, discover, is_integration_admin, needs_driver,
    prepare_integration_admin, prepare_isolated_workspace, prepare_legacy,
    prepare_provider_updates, refuse,
};

/// How many answered requests may be waiting to reach the one task that answers them.
///
/// A bound rather than an unbounded queue, because an unbounded one is a way for a caller to make the daemon grow
/// without limit. A connection that finds it full waits, which is the correct thing for it to do: it has nothing else
/// to be doing until its request is answered.
pub const ASKED_QUEUE: usize = 64;

/// Blocking provider pipe operations the daemon can admit at once on Windows.
///
/// Every hot process may have one stdout read and one stdin write in flight. Discovery and model preparation share
/// one gate and add at most one stdout/stderr pair or model connection pair.
pub const MAX_BLOCKING_PROVIDER_OPERATIONS: usize = runtrol_core::session::MAX_HOT * 2 + 2;

/// Longest model discovery may hold the global provider preparation gate.
///
/// A cold discovery may issue thirteen sequential probes at fifteen seconds each. The remaining allowance covers a
/// normal catalogue response, while deliberately bounding multi-page enumeration rather than adding every internal
/// page timeout together. It also bounds a child that stops reading stdin or a driver that never returns.
pub const MODEL_PREPARATION_BUDGET_MS: u64 = 300_000;

/// How many native-catalogue listing children may run at once.
///
/// A few rather than one, and a few rather than unbounded: each listing spawns a whole CLI, so a cap keeps a
/// burst of discovery from becoming a process storm, while more than one slot keeps a single slow CLI from
/// starving every other provider's catalogue.
pub(crate) const NATIVE_LISTING_SLOTS: usize = 4;

/// Shared freshness window for the cheap external-process roster.
///
/// Studio asks every 250 ms. A shorter cache keeps one window's worst-case discovery at that same interval while
/// coalescing overlapping windows to at most five provider roster scans per second.
pub(crate) const NATIVE_ACTIVITY_CACHE_MS: u64 = 200;

/// The discovery gates, together because they bound the same class of work.
///
/// `lanes` holds one preparation mutex per registered provider. Preparation was one global mutex once, and a
/// cold start paid for it: five providers' first probes (300~900 ms each) queued single file, measured
/// 2026-08-19 as ~9.7 s before a cold window's first conversations against ~1.2 s warm. One lane per provider
/// keeps the two true requirements (the same CLI is never probed twice at once, and a Models request keeps
/// its provider call serialized with that provider's preparation) and drops the false one (different CLIs
/// waiting on each other). The probe-cache file itself stays consistent without a global hold: saves are
/// keyed merges guarded by [`crate::Composed::probe_cache_writing`], held for milliseconds.
///
/// A caller that must hold several lanes (a legacy read prepares every provider) takes them in identifier
/// order, so two such callers cannot deadlock.
///
/// `listing` bounds native-catalogue children instead of serializing them. It was one mutex once, held across
/// the whole listing, and that queued every provider's catalogue behind whichever CLI was slowest: measured
/// 2026-08-19, the conversations of a folder just opened into a live window took 13~17 seconds to arrive, and
/// ~1.2 seconds once the queueing was out of the way.
pub(crate) struct DiscoveryGates {
    known: Box<[ProviderId]>,
    lanes: tokio::sync::Mutex<BTreeMap<ProviderId, std::sync::Weak<tokio::sync::Mutex<()>>>>,
    /// The one lane for identities outside the registry, so an unknown provider still has exactly one.
    unknown: Arc<tokio::sync::Mutex<()>>,
    pub(crate) listing: tokio::sync::Semaphore,
    /// Latest content-free provider process roster, shared by every local window.
    native_activity: tokio::sync::Mutex<
        BTreeMap<ProviderId, (std::time::Instant, runtrol_provider::NativeProcessActivity)>,
    >,
}

impl DiscoveryGates {
    pub(crate) fn new(registry: &runtrol_core::registry::ProviderRegistry) -> Self {
        let mut known = registry
            .all()
            .map(runtrol_core::registry::Provider::id)
            .collect::<Vec<_>>();
        known.sort_unstable();
        Self {
            known: known.into_boxed_slice(),
            lanes: tokio::sync::Mutex::new(BTreeMap::new()),
            unknown: Arc::new(tokio::sync::Mutex::new(())),
            listing: tokio::sync::Semaphore::new(NATIVE_LISTING_SLOTS),
            native_activity: tokio::sync::Mutex::new(BTreeMap::new()),
        }
    }

    /// This provider's preparation lane.
    pub(crate) async fn lane(&self, provider: ProviderId) -> Arc<tokio::sync::Mutex<()>> {
        if self.known.binary_search(&provider).is_err() {
            return Arc::clone(&self.unknown);
        }
        let mut lanes = self.lanes.lock().await;
        if let Some(lane) = lanes.get(&provider).and_then(std::sync::Weak::upgrade) {
            return lane;
        }
        let lane = Arc::new(tokio::sync::Mutex::new(()));
        lanes.insert(provider, Arc::downgrade(&lane));
        lane
    }

    /// One still-fresh process roster previously measured for this provider.
    pub(crate) async fn cached_native_activity(
        &self,
        provider: ProviderId,
    ) -> Option<runtrol_provider::NativeProcessActivity> {
        let cache = self.native_activity.lock().await;
        let (measured_at, activity) = cache.get(&provider)?;
        (measured_at.elapsed() < Duration::from_millis(NATIVE_ACTIVITY_CACHE_MS))
            .then(|| activity.clone())
    }

    /// Publish one provider roster for all windows after its provider lane measured it.
    pub(crate) async fn remember_native_activity(
        &self,
        provider: ProviderId,
        activity: runtrol_provider::NativeProcessActivity,
    ) -> bool {
        let mut cache = self.native_activity.lock().await;
        let turn_ended = cache
            .get(&provider)
            .is_some_and(|(_, previous)| !previous.active.is_empty() && activity.active.is_empty());
        cache.insert(provider, (std::time::Instant::now(), activity));
        turn_ended
    }
}

/// Meet every installed and usable provider once, in the background, the moment the daemon is up.
///
/// A cold first meeting costs one CLI start of waiting (measured 2026-08-20: ~1.5 s for the Node-based
/// CLIs, with the probe's two questions already asked concurrently), and without this the person pays it
/// on their first real request. Started here, behind the boot, the probes are finished or in flight by
/// the time a request arrives; a racing request queues on its provider's lane and continues the moment
/// the same preparation completes, never duplicating it. Composing stays probe-free: this runs after the
/// listener is up, so it delays nothing.
async fn prewarm_providers(composed: Arc<Composed>, discovering: Arc<DiscoveryGates>) {
    // Windows can return only the complete working set. Do that after daemon assembly but before the deliberate
    // warm-up, so discarded startup pages stay out while the provider paths this task exists to warm remain hot.
    #[cfg(windows)]
    runtrol_childproc::footprint::release_unused_memory();

    // Two at a time, deliberately gentle: each meeting starts up to a few CLI processes, and measured
    // 2026-08-20, warming all five at once saturated the machine at the exact moment the operator's own
    // first request arrives, making that request slower than the cold it hides. A racing real request
    // still wins overall: it queues on its provider's lane and rides the same preparation.
    let gentle = Arc::new(tokio::sync::Semaphore::new(2));
    let providers = providers_to_prewarm(&composed.registry, |manifest| {
        runtrol_core::locate(manifest).is_ok()
    });
    let mut meetings = JoinSet::new();
    for provider in providers {
        let composed = Arc::clone(&composed);
        let discovering = Arc::clone(&discovering);
        let gentle = Arc::clone(&gentle);
        meetings.spawn(async move {
            let Ok(_breath) = gentle.acquire_owned().await else {
                // ok: the semaphore is never closed while this task set lives; if it ever were, skipping a
                // warm-up changes nothing the next real request cannot do itself.
                return;
            };
            let _lane = discovering.lane(provider).await.lock_owned().await;
            // ok: an absent or refusing CLI is a normal first answer here, not a condition to act on. The
            // next real request for this provider reports the same outcome to the person who asked for it,
            // through the path that owns that conversation.
            drop(crate::provider_prepare::prepared_driver(&composed, provider).await);
        });
    }
    while meetings.join_next().await.is_some() {}
    // Other platforms expose allocator-specific relief, so they can return discarded provider buffers without
    // evicting the live code this warm-up intentionally touched.
    #[cfg(not(windows))]
    runtrol_childproc::footprint::release_unused_memory();
}

/// Minimum distance between two provider recovery scans on an idle machine.
///
/// Operating-system notifications remain immediate. This clock only repairs a notification that was lost, and
/// one provider-neutral round robin owns it so independently started watchers cannot put several directory scans
/// into the same CPU-budget window. With the two shipped providers each one is still checked every 30 seconds.
const RECOVERY_SCAN_SPACING: Duration = Duration::from_secs(15);

/// How long a provider whose directory cannot be watched waits before trying to watch it again.
///
/// A CLI that has never run on this machine has no directory yet, and the person installing it should not have
/// to restart the Runtime for their first session to appear.
const RETRY_WATCH_AFTER: Duration = Duration::from_mins(1);

/// Find, bind and mirror provider sessions with no window involved.
///
/// Discovery used to happen only inside a window's activity question: the only callers of a provider's process
/// roster were one request handler and one open guard (measured 2026-08-30). A machine with no window open
/// therefore found nothing at all, and a window that had just opened watched its own list fill in. Each
/// provider is now waited on rather than asked, so a session started anywhere is bound and mirrored at once
/// and the first window to open finds the work already done.
async fn watch_native_sessions(composed: Arc<Composed>, discovering: Arc<DiscoveryGates>) {
    let providers = providers_to_prewarm(&composed.registry, |manifest| {
        runtrol_core::locate(manifest).is_ok()
    });
    let mut watching = JoinSet::new();
    let mut recoveries = Vec::with_capacity(providers.len());
    for provider in providers {
        let (request_recovery, recovery_requested) = mpsc::channel(1);
        recoveries.push(request_recovery);
        watching.spawn(watch_one_provider(
            Arc::clone(&composed),
            Arc::clone(&discovering),
            provider,
            recovery_requested,
        ));
    }
    if !recoveries.is_empty() {
        watching.spawn(schedule_native_recovery(Arc::clone(&composed), recoveries));
    }
    while watching.join_next().await.is_some() {}
}

/// Ask one installed provider at a time to repair a possibly missed directory notification.
async fn schedule_native_recovery(composed: Arc<Composed>, providers: Vec<mpsc::Sender<()>>) {
    if providers.is_empty() {
        return;
    }
    let mut next = 0;
    loop {
        tokio::time::sleep(RECOVERY_SCAN_SPACING).await;
        if composed.draining.load(Ordering::Acquire) {
            return;
        }
        // Capacity one coalesces a pulse with one already waiting behind that provider's current observation.
        if let Some(request) = providers.get(next) {
            match request.try_send(()) {
                Ok(())
                | Err(
                    mpsc::error::TrySendError::Full(()) | mpsc::error::TrySendError::Closed(()),
                ) => {}
            }
        }
        next = (next + 1) % providers.len();
    }
}

/// Wait on one provider's own statement that its open conversations changed, and act on each one.
async fn watch_one_provider(
    composed: Arc<Composed>,
    discovering: Arc<DiscoveryGates>,
    provider: ProviderId,
    mut recovery_requested: mpsc::Receiver<()>,
) {
    loop {
        // A draining generation is on its way out and opens nothing new; its successor is doing this now.
        if composed.draining.load(std::sync::atomic::Ordering::Acquire) {
            return;
        }
        let Some(mut changes) = watch_of(&composed, provider).await else {
            // A CLI whose directory does not exist yet still gets an initial observation. A recovery pulse both
            // observes it again and returns here to install the watch as soon as its first session creates it.
            look_again(&composed, &discovering, provider).await;
            tokio::select! {
                () = tokio::time::sleep(RETRY_WATCH_AFTER) => {}
                requested = recovery_requested.recv() => {
                    if requested.is_none() {
                        return;
                    }
                }
            }
            continue;
        };
        // Install the watch before the initial observation. Sessions already open at Runtime start are found by
        // this scan, and a session arriving during it leaves a notification for the receiver below.
        look_again(&composed, &discovering, provider).await;
        loop {
            tokio::select! {
                notice = changes.recv() => match notice {
                    Some(()) => look_again(&composed, &discovering, provider).await,
                    // The watch ended. Ask for a new one rather than going blind.
                    None => break,
                },
                requested = recovery_requested.recv() => match requested {
                    Some(()) => look_again(&composed, &discovering, provider).await,
                    None => return,
                },
            }
            if composed.draining.load(std::sync::atomic::Ordering::Acquire) {
                return;
            }
        }
    }
}

/// A watch on the directory this provider says names its open conversations, or nothing when it has none.
async fn watch_of(
    composed: &Arc<Composed>,
    provider: ProviderId,
) -> Option<tokio::sync::mpsc::Receiver<()>> {
    // A CLI that is absent or mid-update has nothing to watch yet, which the retry above tries again.
    let Ok(prepared) = crate::provider_prepare::prepared_driver(composed, provider).await else {
        return None;
    };
    let directory = prepared.driver.session_directory()?;
    runtrol_childproc::watch_directory(&directory)
}

/// Ask this provider what it has open and act on the answer.
async fn look_again(
    composed: &Arc<Composed>,
    discovering: &Arc<DiscoveryGates>,
    provider: ProviderId,
) {
    // A provider that cannot be prepared or will not answer leaves the last answer standing. Nothing is
    // reported here: this watch has no request waiting on it, and every failure it could meet is one a real
    // request meets again and reports to the person who asked.
    if let Ok(Ok(activity)) =
        crate::runtime_serve::observe_native_activity(composed, discovering, provider).await
    {
        crate::runtime_serve::reconcile_native_activity(composed, provider, &activity).await;
    }
}

/// Select only services with an executable on this machine for startup preparation.
///
/// The registry also carries installable catalogue entries so the sidebar can offer them. A known driver does not
/// make an absent program worth probing: creating preparation tasks for every catalogue entry made a no-client
/// daemon retain their startup allocations and delayed allocator relief beyond the idle-memory measurement.
fn providers_to_prewarm(
    registry: &runtrol_core::registry::ProviderRegistry,
    mut installed: impl FnMut(&runtrol_provider::Manifest) -> bool,
) -> Vec<ProviderId> {
    registry
        .usable()
        .filter(|provider| installed(&provider.manifest))
        .map(runtrol_core::registry::Provider::id)
        .collect()
}

/// Delay before the first automatic provider update check, outside activation and idle measurement windows.
const PROVIDER_UPDATE_INITIAL_DELAY: Duration = Duration::from_mins(5);

/// Frequency for checking providers after the first delayed pass.
const PROVIDER_UPDATE_INTERVAL: Duration = Duration::from_hours(1);

/// Maximum time an available provider update may remain blocked by its own live processes before surfacing a warning.
const PROVIDER_UPDATE_DEFER_LIMIT: Duration = Duration::from_hours(24);

/// Noise handshakes admitted by HTTP but not yet mapped to a paired device.
const PHONE_UPGRADE_QUEUE: usize = 16;

/// An explicit loopback phone listener used to join authenticated browser links to the same Core.
pub struct PhoneIngress {
    listener: TcpListener,
    admission: PhoneHttp,
}

impl PhoneIngress {
    /// Wrap an already-bound loopback listener with exact Host and Origin admission.
    ///
    /// Binding is kept outside so the caller can choose a fixed port or let the operating system assign one. The
    /// assigned address is then the single source for the accepted Host values.
    ///
    /// # Errors
    ///
    /// [`PhoneIngressError::Address`] when the listener address cannot be read,
    /// [`PhoneIngressError::NonLoopback`] when it is not loopback, or [`PhoneIngressError::Policy`] when an origin is
    /// invalid.
    pub fn loopback(
        listener: TcpListener,
        origins: impl IntoIterator<Item = impl AsRef<str>>,
    ) -> Result<Self, PhoneIngressError> {
        let address = listener.local_addr()?;
        if !address.ip().is_loopback() {
            return Err(PhoneIngressError::NonLoopback { address });
        }
        let admission = PhoneHttp::loopback(address.port(), origins, [])?;
        Ok(Self {
            listener,
            admission,
        })
    }

    /// The exact address assigned to this listener.
    ///
    /// # Errors
    ///
    /// An operating-system socket error if the listener no longer exposes its local address.
    pub fn local_addr(&self) -> Result<SocketAddr, std::io::Error> {
        self.listener.local_addr()
    }
}

/// An invalid phone ingress boundary.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PhoneIngressError {
    /// The listener's local address could not be read.
    #[error("cannot read the phone listener address: {0}")]
    Address(#[from] std::io::Error),

    /// This constructor admits only a listener that cannot leave the machine.
    #[error("phone loopback listener is not loopback: {address}")]
    NonLoopback {
        /// The refused bound address.
        address: SocketAddr,
    },

    /// Exact Host or Origin admission could not be built.
    #[error(transparent)]
    Policy(#[from] PhoneHttpError),
}

/// The daemon could not keep serving.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ServeError {
    /// The endpoint could not be created or kept.
    #[error(transparent)]
    Transport(#[from] TransportError),

    /// Minimal session metadata could not be persisted.
    #[error(transparent)]
    Store(#[from] runtrol_store::StoreError),

    /// A remote listener was requested on a platform without a protected PC identity.
    #[error("phone ingress requires a protected PC identity")]
    PhoneIdentityUnavailable,

    /// The immutable Noise binding could not be constructed.
    #[error(transparent)]
    PhoneCrypto(#[from] CryptoError),

    /// The bound phone listener could not accept another TCP connection.
    #[error("phone listener failed while accepting a connection: {0}")]
    PhoneAccept(#[source] std::io::Error),

    /// Runtime instance identity or atomic locator publication failed.
    #[error("Runtime public bootstrap failed: {0}")]
    RuntimeBootstrap(String),
}

struct PhonePlane {
    ingress: PhoneIngress,
    identity: Arc<StaticKeypair>,
    binding: Arc<SessionBinding>,
}

pub(crate) enum SurfaceConnection {
    Local(Connection),
    Phone(Box<NoiseWebSocket>),
    Relay(Box<crate::relay::RelaySurface>),
}

impl SurfaceConnection {
    pub(crate) async fn recv(&mut self) -> Result<Option<bytes::Bytes>, SurfaceError> {
        match self {
            Self::Local(connection) => connection.recv().await.map_err(SurfaceError::from),
            Self::Phone(connection) => connection.recv().await.map_err(SurfaceError::from),
            Self::Relay(connection) => connection.recv().await.map_err(SurfaceError::from),
        }
    }

    pub(crate) async fn send(&mut self, payload: &[u8]) -> Result<(), SurfaceError> {
        match self {
            Self::Local(connection) => connection.send(payload).await.map_err(SurfaceError::from),
            Self::Phone(connection) => connection.send(payload).await.map_err(SurfaceError::from),
            Self::Relay(connection) => connection.send(payload).await.map_err(SurfaceError::from),
        }
    }

    async fn send_parts(&mut self, parts: &[&[u8]]) -> Result<(), SurfaceError> {
        match self {
            Self::Local(connection) => connection
                .send_parts(parts)
                .await
                .map_err(SurfaceError::from),
            Self::Phone(connection) => connection
                .send_parts(parts)
                .await
                .map_err(SurfaceError::from),
            Self::Relay(connection) => connection
                .send_parts(parts)
                .await
                .map_err(SurfaceError::from),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum SurfaceError {
    #[error(transparent)]
    Local(#[from] TransportError),
    #[error(transparent)]
    Phone(#[from] WebSocketLinkError),
    #[error(transparent)]
    Relay(#[from] crate::relay::RelaySurfaceError),
}

struct ConnectionServices {
    asking: mpsc::Sender<Box<Asked>>,
    reserving: mpsc::UnboundedSender<ReservationAsked>,
    returning: mpsc::UnboundedSender<AgentReturned>,
    composed: Arc<Composed>,
    discovering: Arc<DiscoveryGates>,
    session_index: watch::Receiver<PublishedIndex>,
}

/// One published session index: the full view encoded once, and the rows it was encoded from.
///
/// Local subscribers share `full_frame` verbatim, so the fast path stays one `Arc` clone. A device connection
/// must not receive that frame: it projects `listing` down to its own live workspace roots and encodes the
/// result itself, which keeps per-caller work on the caller's connection and off the session owner.
///
/// `listing` is `None` when the current index is a refusal (the store could not be read). A refusal carries no
/// session rows, so every caller shares it verbatim.
#[derive(Clone)]
struct PublishedIndex {
    full_frame: Arc<[u8]>,
    listing: Option<Arc<SessionListing>>,
}

/// One request, from a connection that is waiting for the answer.
struct Asked {
    /// The connection's own state, lent for the length of one answer and handed straight back.
    ///
    /// It travels with the request rather than living in the owner task, because it belongs to the connection: an
    /// owner task holding one entry per connection would be a second place a connection's life is recorded, and the
    /// two would disagree the moment one of them missed a disconnect.
    conversation: Conversation,
    /// What was asked.
    request: Request,
    /// Slow provider discovery completed by the connection task, outside the one session owner.
    prepared: Prepared,
    /// The bounded process slot reserved before an open, if this request opens one.
    reservation: Option<ReservationGuard>,
    /// Where the answer goes.
    answered: oneshot::Sender<Answered>,
}

/// A connection asking the session owner for a bounded process slot.
enum ReservationAsked {
    Reserve {
        provider: runtrol_provider::ProviderId,
        session: SessionId,
        native: Option<Box<str>>,
        workspace: Box<str>,
        claim: WorkspaceClaim,
        answered: oneshot::Sender<Result<ReservedOpen, OpenReservationFailure>>,
    },
    ReserveProviderUpdate {
        provider: ProviderId,
        answered: oneshot::Sender<Result<ProviderUpdateReservation, SessionError>>,
    },
    CancelOpen(OpenReservation),
    ReleaseClosing(ClosingReservation),
    ReleaseProviderUpdate(ProviderUpdateReservation),
}

#[derive(Debug, thiserror::Error)]
enum OpenReservationFailure {
    #[error(transparent)]
    Session(#[from] SessionError),
    #[error(transparent)]
    Claim(#[from] crate::native_claims::TerminalClaimError),
}

struct AutomaticUpdateNotice {
    provider: ProviderId,
    message: Box<str>,
}

/// Cancels a pending slot if connection preparation is abandoned.
/// A breadcrumb for a hanging close, printed only when the environment asks.
///
/// The Unix host harness times a `close --now` out at 15 seconds with the daemon's stderr empty, which says
/// nothing about where it stuck (measured 2026-08-27 on the CI runners; this machine has no Linux to attach a
/// debugger to). The harness sets `RUNTROL_CLOSE_TRACE=1`; production daemons never print these.
#[expect(
    clippy::print_stderr,
    reason = "the breadcrumb exists to reach the harness's captured stderr, and only when RUNTROL_CLOSE_TRACE=1 asks for it"
)]
pub(crate) fn close_trace(step: &str) {
    if std::env::var_os("RUNTROL_CLOSE_TRACE").is_some_and(|value| value == "1") {
        static BEGAN: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
        let elapsed = BEGAN
            .get_or_init(std::time::Instant::now)
            .elapsed()
            .as_millis();
        eprintln!("runtrol close trace: +{elapsed}ms {step}");
    }
}

/// Names the line that holds the runtime thread when the heartbeat stops, harness-only.
///
/// The heartbeat proved the thread wedges for 13 seconds at a time on the Linux CI hosts and every
/// breadcrumb placed by hand missed the culprit (2026-08-27). The detector itself lives in
/// `runtrol_childproc::stall` (the one crate allowed `unsafe`); this module only owns the beat clock.
mod stall_watchdog {
    use std::sync::atomic::{AtomicU64, Ordering};

    static LAST_BEAT_MS: AtomicU64 = AtomicU64::new(0);

    fn now_ms() -> u64 {
        static BEGAN: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
        u64::try_from(
            BEGAN
                .get_or_init(std::time::Instant::now)
                .elapsed()
                .as_millis(),
        )
        .unwrap_or(u64::MAX)
    }

    pub(super) fn beat() {
        LAST_BEAT_MS.store(now_ms(), Ordering::Release);
    }

    /// Called on the runtime thread, once, before serving.
    pub(super) fn arm() {
        if std::env::var_os("RUNTROL_CLOSE_TRACE").is_none_or(|value| value != "1") {
            return;
        }
        beat();
        runtrol_childproc::arm_stall_backtrace(|| {
            now_ms().saturating_sub(LAST_BEAT_MS.load(Ordering::Acquire)) > 3_000
        });
    }
}

struct ReservationGuard {
    reservation: Option<CleanupReservation>,
    cancelling: mpsc::UnboundedSender<ReservationAsked>,
}

impl ReservationGuard {
    fn take(mut self) -> Option<OpenReservation> {
        match self.reservation.take() {
            Some(CleanupReservation::Open(reservation)) => Some(reservation),
            Some(CleanupReservation::Closing(_)) | None => None,
        }
    }
}

impl Drop for ReservationGuard {
    fn drop(&mut self) {
        if let Some(reservation) = self.reservation.take() {
            let message = match reservation {
                CleanupReservation::Open(reservation) => ReservationAsked::CancelOpen(reservation),
                CleanupReservation::Closing(reservation) => {
                    ReservationAsked::ReleaseClosing(reservation)
                }
            };
            drop(self.cancelling.send(message));
        }
    }
}

/// Releases one provider's package-tree exclusion when an update finishes or is cancelled.
struct ProviderUpdateGuard {
    reservation: Option<ProviderUpdateReservation>,
    releasing: mpsc::UnboundedSender<ReservationAsked>,
}

impl Drop for ProviderUpdateGuard {
    fn drop(&mut self) {
        if let Some(reservation) = self.reservation.take() {
            drop(
                self.releasing
                    .send(ReservationAsked::ReleaseProviderUpdate(reservation)),
            );
        }
    }
}

async fn automatic_provider_updates(
    composed: Arc<Composed>,
    discovering: Arc<DiscoveryGates>,
    reserving: mpsc::UnboundedSender<ReservationAsked>,
    notices: mpsc::UnboundedSender<AutomaticUpdateNotice>,
) {
    tokio::time::sleep(PROVIDER_UPDATE_INITIAL_DELAY).await;
    let mut deferred = BTreeMap::<ProviderId, (Instant, bool)>::new();
    let mut session_pins = BTreeMap::<ProviderId, Box<str>>::new();
    loop {
        let statuses = crate::provider_update::inspect_all(&composed, &discovering).await;
        *composed.provider_update_status.lock().await = statuses.clone();
        for status in statuses {
            let Ok(provider) = ProviderId::parse(&status.provider) else {
                continue;
            };
            if status.state != runtrol_ipc::wire::ProviderUpdateState::Available {
                deferred.remove(&provider);
                continue;
            }
            let Some(target) = status.target.as_deref() else {
                continue;
            };
            match session_pins.get(&provider) {
                Some(pinned) if pinned.as_ref() == target => continue,
                Some(_) => {
                    session_pins.remove(&provider);
                }
                None => {}
            }
            match crate::provider_update::is_automatic_pinned(&composed, provider, target) {
                Ok(true) => continue,
                Ok(false) => {}
                Err(why) => {
                    drop(notices.send(AutomaticUpdateNotice {
                        provider,
                        message: format!(
                            "automatic provider update is paused because its safety journal cannot be read: {why}"
                        )
                        .into_boxed_str(),
                    }));
                    continue;
                }
            }

            let _gate = discovering.lane(provider).await.lock_owned().await;
            let (answered, hearing) = oneshot::channel();
            if reserving
                .send(ReservationAsked::ReserveProviderUpdate { provider, answered })
                .is_err()
            {
                return;
            }
            let reservation = match hearing.await {
                Ok(Ok(reservation)) => {
                    deferred.remove(&provider);
                    reservation
                }
                Ok(Err(
                    SessionError::ProviderBusyForUpdate { .. }
                    | SessionError::ProviderUpdating { .. },
                )) => {
                    let waiting = deferred.entry(provider).or_insert((Instant::now(), false));
                    if !waiting.1 && waiting.0.elapsed() >= PROVIDER_UPDATE_DEFER_LIMIT {
                        waiting.1 = true;
                        drop(notices.send(AutomaticUpdateNotice {
                            provider,
                            message: "a provider update has waited 24 hours for its sessions to close; close those sessions or run the VS Code update command when ready"
                                .into(),
                        }));
                    }
                    continue;
                }
                Ok(Err(error)) => {
                    drop(notices.send(AutomaticUpdateNotice {
                        provider,
                        message: format!("automatic provider update could not reserve its process boundary: {error}")
                            .into_boxed_str(),
                    }));
                    continue;
                }
                Err(_) => return,
            };
            let releasing = ProviderUpdateGuard {
                reservation: Some(reservation),
                releasing: reserving.clone(),
            };
            let response = crate::provider_update::apply_latest(&composed, provider).await;
            drop(releasing);
            track_automatic_pin(&mut session_pins, provider, target, &response);
            if let Some(message) = automatic_update_message(provider, &response) {
                drop(notices.send(AutomaticUpdateNotice { provider, message }));
            }
        }
        tokio::time::sleep(PROVIDER_UPDATE_INTERVAL).await;
    }
}

fn track_automatic_pin(
    pins: &mut BTreeMap<ProviderId, Box<str>>,
    provider: ProviderId,
    target: &str,
    response: &Response,
) {
    if let Response::ProviderUpdated(result) = response {
        match result.outcome {
            runtrol_ipc::wire::ProviderUpdateOutcome::RolledBack => {
                pins.insert(provider, target.into());
            }
            runtrol_ipc::wire::ProviderUpdateOutcome::AlreadyCurrent
            | runtrol_ipc::wire::ProviderUpdateOutcome::Updated => {
                pins.remove(&provider);
            }
        }
    }
}

fn automatic_update_message(provider: ProviderId, response: &Response) -> Option<Box<str>> {
    match response {
        Response::ProviderUpdated(result) => match result.outcome {
            runtrol_ipc::wire::ProviderUpdateOutcome::AlreadyCurrent => None,
            runtrol_ipc::wire::ProviderUpdateOutcome::Updated => Some(
                match &result.why {
                    Some(why) => format!(
                        "provider {provider} updated from {} to {}, but needs attention: {why}",
                        result.from, result.to
                    ),
                    None => format!(
                        "provider {provider} updated from {} to {}",
                        result.from, result.to
                    ),
                }
                .into_boxed_str(),
            ),
            runtrol_ipc::wire::ProviderUpdateOutcome::RolledBack => Some(
                format!(
                    "provider {provider} update failed and restored {}: {}",
                    result.to,
                    result.why.as_deref().unwrap_or("verification failed")
                )
                .into_boxed_str(),
            ),
        },
        Response::Failed(error) => {
            Some(format!("provider {provider} update failed: {}", error.message).into_boxed_str())
        }
        _ => Some(
            format!("provider {provider} update returned an unexpected result").into_boxed_str(),
        ),
    }
}

/// One answer, going back to the connection that asked.
struct Answered {
    /// The connection's state, as answering left it.
    conversation: Conversation,
    /// What to do about the request.
    reply: Reply,
}

enum AgentReturned {
    Finished {
        lease: AgentLease,
        agent: Box<dyn runtrol_provider::Agent>,
        outcome: Result<(), ProviderError>,
        answered: oneshot::Sender<Response>,
    },
    Abandoned(AgentLease),
}

struct AgentGuard {
    lease: Option<AgentLease>,
    returning: mpsc::UnboundedSender<AgentReturned>,
}

impl AgentGuard {
    fn take(mut self) -> Option<AgentLease> {
        self.lease.take()
    }
}

impl Drop for AgentGuard {
    fn drop(&mut self) {
        if let Some(lease) = self.lease.take() {
            drop(self.returning.send(AgentReturned::Abandoned(lease)));
        }
    }
}

/// Serve until the endpoint fails.
///
/// # Errors
///
/// [`ServeError::Transport`] when the endpoint cannot be created or cannot keep accepting. Not worked around: a
/// daemon nothing can reach is a daemon that does nothing, and staying up would hide that from the operator.
pub async fn serve(composed: Composed, listener: Listener) -> Result<(), ServeError> {
    serve_sessions(composed, listener, SessionManager::new()).await
}

/// Serve local surfaces and one explicit phone ingress through the same session owner.
///
/// The phone listener starts only after composition restored the PC identity, paired device keys, and grant ledger.
/// An unpaired Noise key is dropped before handshake message two and never reaches request dispatch.
///
/// # Errors
///
/// The same failures as [`serve`], plus [`ServeError::PhoneIdentityUnavailable`] when no protected PC key exists.
pub async fn serve_with_phone(
    composed: Composed,
    listener: Listener,
    ingress: PhoneIngress,
) -> Result<(), ServeError> {
    let phone = phone_plane(&composed, ingress)?;
    serve_surfaces(composed, listener, SessionManager::new(), Some(phone), None).await
}

/// Serve local surfaces and one reconnecting encrypted relay through the same session owner.
///
/// Relay unavailability never ends local service or a provider process. A remote surface is created only after a
/// relay-bound Noise handshake authenticates one restored paired device.
///
/// # Errors
///
/// The same failures as [`serve`], plus [`ServeError::PhoneIdentityUnavailable`] when no protected PC key exists.
pub async fn serve_with_relay(
    composed: Composed,
    listener: Listener,
    ingress: crate::relay::RelayIngress,
) -> Result<(), ServeError> {
    if composed.pc_identity.is_none() {
        return Err(ServeError::PhoneIdentityUnavailable);
    }
    serve_surfaces(
        composed,
        listener,
        SessionManager::new(),
        None,
        Some(ingress),
    )
    .await
}

fn phone_plane(composed: &Composed, ingress: PhoneIngress) -> Result<PhonePlane, ServeError> {
    let identity = composed
        .pc_identity
        .clone()
        .ok_or(ServeError::PhoneIdentityUnavailable)?;
    let binding = Arc::new(SessionBinding::direct(
        LinkKind::Loopback,
        identity.public_key().to_bytes(),
    )?);
    Ok(PhonePlane {
        ingress,
        identity,
        binding,
    })
}

async fn serve_sessions(
    composed: Composed,
    listener: Listener,
    sessions: SessionManager,
) -> Result<(), ServeError> {
    serve_surfaces(composed, listener, sessions, None, None).await
}

#[expect(
    clippy::too_many_lines,
    reason = "one owner loop keeps every way session state changes visible beside index publication"
)]
async fn serve_surfaces(
    composed: Composed,
    mut listener: Listener,
    mut sessions: SessionManager,
    phone: Option<PhonePlane>,
    relay: Option<crate::relay::RelayIngress>,
) -> Result<(), ServeError> {
    let composed = Arc::new(composed);
    let runtime_instance =
        crate::generations::load_or_create_instance(composed.home.paths().runtime_instance())
            .map_err(|error| ServeError::RuntimeBootstrap(error.to_string()))?;
    // This daemon is one generation: its public endpoint carries its own build digest, so a newer
    // build binds beside it rather than against it, and the locator lists both while both live.
    let identity = crate::generations::GenerationIdentity::of_this_executable()
        .map_err(|error| ServeError::RuntimeBootstrap(error.to_string()))?;
    let runtime_address = composed
        .home
        .paths()
        .generation_runtime_endpoint(identity.tag())
        .map_err(|error| ServeError::RuntimeBootstrap(error.to_string()))?
        .address()
        .to_owned();
    let mut runtime_listener = Listener::bind_owner_only(&runtime_address).await?;
    let mut generation = crate::generations::PublishedGeneration::publish(
        composed.home.paths(),
        &runtime_instance,
        &identity,
        &runtime_address,
        listener.address(),
    )
    .await
    .map_err(|error| ServeError::RuntimeBootstrap(error.to_string()))?;
    crate::generations::prime_generation_barrier(&composed, identity.digest());
    // Flipped once, by a successor's drain request, and never back. From then on the store belongs to
    // the successor, nothing new is opened here, and this loop ends when no turn is running.
    let mut draining = false;
    let mut runtime_audit_closing = false;
    // Everything a draining generation stops doing: warming providers, updating them, probing accounts, and
    // holding the relay, all of which the successor now does for this home.
    let mut background: Vec<tokio::task::AbortHandle> = Vec::new();
    // Both owner queues preserve the 64-request admission contract without preallocating 64 copies of their large
    // request envelopes while no client exists. Each active caller owns exactly one envelope allocation.
    let (asking, mut asked) = mpsc::channel::<Box<Asked>>(ASKED_QUEUE);
    let (reserving, mut reservations) = mpsc::unbounded_channel::<ReservationAsked>();
    let (returning, mut returned) = mpsc::unbounded_channel::<AgentReturned>();
    let (runtime_asking, mut runtime_asked) =
        mpsc::channel::<Box<crate::runtime_control::RuntimeAsked>>(ASKED_QUEUE);
    let (runtime_returning, mut runtime_returned) =
        mpsc::unbounded_channel::<crate::runtime_control::RuntimeReturned>();
    let mut runtime_control = crate::runtime_control::RuntimeControl::with_native_claims(
        Arc::clone(&composed.native_claims),
    )
    .map_err(|error| ServeError::RuntimeBootstrap(error.message.into_owned()))?;
    let runtime_native_cursors = Arc::new(
        crate::runtime_native_sessions::NativeCursorCodec::new().map_err(|_| {
            ServeError::RuntimeBootstrap(
                "Runtime could not create native catalogue cursor authority".to_owned(),
            )
        })?,
    );
    let (noticing_updates, mut update_notices) = mpsc::unbounded_channel::<AutomaticUpdateNotice>();
    let mut provider_update_notices = BTreeMap::<ProviderId, Box<str>>::new();
    let initial_index = build_session_index(&composed, &sessions, &provider_update_notices);
    let (session_index, _initial_index_receiver) = watch::channel(initial_index);
    let mut resident_memory_changes = crate::runtime_inventory::resident_session_changes();
    let initial_runtime_sessions =
        Arc::new(crate::runtime_inventory::sessions(&composed, &sessions)?);
    let (runtime_sessions, _initial_runtime_sessions_receiver) =
        watch::channel(initial_runtime_sessions);
    // Runtime refreshes this inventory before every method that consumes it. Keep the no-client daemon truly idle
    // instead of walking PATH, statting provider binaries, and retaining presentation strings before a surface asks.
    let (runtime_providers, _initial_runtime_providers_receiver) =
        watch::channel(Arc::new(runtrol_runtime_protocol::ProviderList {
            providers: Vec::new(),
        }));
    let (account_gauges, _initial_account_gauges_receiver) =
        watch::channel(Arc::new(crate::runtime_inventory::merge_probed_usage(
            &crate::runtime_inventory::provider_usage(&sessions.account_gauges()),
            &composed,
        )));
    let discovering = Arc::new(DiscoveryGates::new(&composed.registry));
    let mut connections = JoinSet::new();
    let (runtime_audit, runtime_audit_writer) =
        crate::runtime_audit::journal(Arc::clone(&composed));
    let mut runtime_audit_writer = tokio::spawn(runtime_audit_writer);
    let mut runtime_audit_writer_joined = None;
    let (generation_failed, mut generation_failures) = mpsc::unbounded_channel();
    {
        let composed = Arc::clone(&composed);
        let own_digest = identity.digest().to_owned();
        background.push(connections.spawn(async move {
            if let Err(error) =
                crate::generations::relay_generation_state(composed, own_digest).await
            {
                drop(generation_failed.send(error.to_string()));
            }
        }));
    }
    // The courier belongs to this logon and stays with this generation's live sessions while it drains.
    // Bind before any launch so a child's first courier call always finds its generation's endpoint.
    let courier_listener = Listener::bind_logon_only(composed.courier_gate.endpoint()).await?;
    connections.spawn(crate::courier_gate::serve::serve(
        Arc::clone(&composed.courier_gate),
        Arc::clone(&composed.containment),
        courier_listener,
        crate::courier_gate::serve::HELLO_WAIT,
    ));
    let push_wake_active = Arc::new(AtomicBool::new(false));
    let mut relay_hub = match relay {
        Some(relay) => {
            let identity = composed
                .pc_identity
                .clone()
                .ok_or(ServeError::PhoneIdentityUnavailable)?;
            let (hub, supervisor) = crate::relay::supervise(
                relay,
                identity,
                composed.device_authority.clone(),
                composed.pairing_admin.clone(),
            );
            background.push(connections.spawn(supervisor));
            Some(hub)
        }
        None => match (&composed.pc_identity, &composed.relay_seed) {
            (Some(identity), Some(seed)) => {
                let (hub, supervisor) = crate::relay::supervise_controlled(
                    composed.relay_control.clone(),
                    Arc::clone(seed),
                    Arc::clone(identity),
                    composed.device_authority.clone(),
                    composed.pairing_admin.clone(),
                );
                background.push(connections.spawn(supervisor));
                Some(hub)
            }
            _ => None,
        },
    };
    let (upgrading, mut upgrades) = mpsc::channel::<NoiseUpgrade>(PHONE_UPGRADE_QUEUE);
    background.push(connections.spawn(prewarm_providers(
        Arc::clone(&composed),
        Arc::clone(&discovering),
    )));
    // The Runtime's own eyes. Without this, a conversation opened in a terminal is invisible until some window
    // asks, and the mirror a person expects to click is built only after they look.
    background.push(connections.spawn(watch_native_sessions(
        Arc::clone(&composed),
        Arc::clone(&discovering),
    )));
    background.push(connections.spawn(automatic_provider_updates(
        Arc::clone(&composed),
        Arc::clone(&discovering),
        reserving.clone(),
        noticing_updates,
    )));
    // A once-a-second pulse on stderr, only under the harness trace switch: its silence in a captured log is
    // the direct picture of the runtime thread being wedged, which no per-request breadcrumb can draw (the CI
    // Unix hosts show connected clients whose greeting is never answered while the daemon says nothing).
    if std::env::var_os("RUNTROL_CLOSE_TRACE").is_some_and(|value| value == "1") {
        stall_watchdog::arm();
        connections.spawn(async {
            loop {
                close_trace("heartbeat");
                stall_watchdog::beat();
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }
        });
    }
    let (local_failed, mut local_failures) = mpsc::unbounded_channel();
    {
        let asking = asking.clone();
        let reserving = reserving.clone();
        let returning = returning.clone();
        let composed = Arc::clone(&composed);
        let discovering = Arc::clone(&discovering);
        let session_index = session_index.clone();
        connections.spawn(async move {
            let mut clients = JoinSet::new();
            loop {
                tokio::select! {
                    arrived = listener.accept() => match arrived {
                        Ok(connection) => {
                            close_trace("control: local connection accepted");
                            clients.spawn(converse(
                                SurfaceConnection::Local(connection),
                                Conversation::at_the_machine(),
                                ConnectionServices {
                                    asking: asking.clone(),
                                    reserving: reserving.clone(),
                                    returning: returning.clone(),
                                    composed: Arc::clone(&composed),
                                    discovering: Arc::clone(&discovering),
                                    session_index: session_index.subscribe(),
                                },
                            ));
                        }
                        Err(error) => {
                            close_trace("control: local accept failed");
                            drop(local_failed.send(error));
                            break;
                        }
                    },
                    Some(_finished) = clients.join_next(), if !clients.is_empty() => {}
                }
            }
            close_trace("control: local listener loop ended");
            clients.abort_all();
            while clients.join_next().await.is_some() {}
        });
    }
    background.push(connections.spawn(crate::account_probe::supervise(
        Arc::clone(&composed),
        runtime_providers.clone(),
        account_gauges.clone(),
    )));
    let (runtime_failed, mut runtime_failures) = mpsc::unbounded_channel();
    {
        let runtime_instance = runtime_instance.clone();
        let composed = Arc::clone(&composed);
        let runtime_audit = runtime_audit.clone();
        let discovering = Arc::clone(&discovering);
        let runtime_native_cursors = Arc::clone(&runtime_native_cursors);
        let runtime_providers = runtime_providers.clone();
        let runtime_sessions = runtime_sessions.clone();
        let account_gauges = account_gauges.clone();
        let runtime_asking = runtime_asking.clone();
        let runtime_returning = runtime_returning.clone();
        connections.spawn(async move {
            let mut clients = JoinSet::new();
            loop {
                tokio::select! {
                    arrived = runtime_listener.accept() => match arrived {
                        Ok(connection) => {
                            close_trace("runtime: connection accepted");
                            clients.spawn(crate::runtime_serve::serve_connection(
                                connection,
                                runtime_instance.clone(),
                                Arc::clone(&composed),
                                runtime_audit.clone(),
                                Arc::clone(&discovering),
                                Arc::clone(&runtime_native_cursors),
                                runtime_providers.clone(),
                                runtime_sessions.subscribe(),
                                account_gauges.subscribe(),
                                runtime_asking.clone(),
                                runtime_returning.clone(),
                            ));
                        }
                        Err(error) => {
                            close_trace("runtime: accept failed");
                            drop(runtime_failed.send(error));
                            break;
                        }
                    },
                    Some(_finished) = clients.join_next(), if !clients.is_empty() => {}
                }
            }
            close_trace("runtime: listener loop ended");
            clients.abort_all();
            while clients.join_next().await.is_some() {}
        });
    }

    // A draining generation sweeps its terminals on this clock, closing the ones nobody is watching so it can
    // finish and leave the locator instead of holding idle conversations for hours (operator, 2026-08-29).
    let mut drain_sweep = tokio::time::interval(std::time::Duration::from_secs(5));
    drain_sweep.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    let outcome = loop {
        tokio::select! {
            _ = drain_sweep.tick(), if draining => {
                crate::terminal_surface::close_idle_while_draining(&composed).await;
                generation.update(live_work_of(&sessions, &composed), true);
                begin_runtime_audit_shutdown_if_drained(
                    draining,
                    &sessions,
                    &composed,
                    &runtime_audit,
                    &mut runtime_audit_closing,
                );
            }

            () = wait_until_runtime_audit_relay_empty(
                &runtime_audit,
                &composed.audit_relay,
            ), if runtime_audit_closing => {
                break Ok(());
            }

            joined = &mut runtime_audit_writer => {
                let message = match &joined {
                    Ok(Ok(())) => "Runtime audit writer closed while the service was active".to_owned(),
                    Ok(Err(error)) => format!("Runtime audit writer failed: {error}"),
                    Err(error) => format!("Runtime audit writer task stopped: {error}"),
                };
                runtime_audit_writer_joined = Some(joined);
                break Err(ServeError::RuntimeBootstrap(message));
            }

            Some(error) = local_failures.recv() => {
                break Err(error.into());
            }

            Some(error) = runtime_failures.recv() => {
                break Err(error.into());
            }

            Some(error) = generation_failures.recv() => {
                break Err(ServeError::RuntimeBootstrap(format!(
                    "generation audit relay failed: {error}"
                )));
            }

            changed = resident_memory_changes.changed() => {
                if changed.is_ok() {
                    publish_runtime_session_catalogue(&runtime_sessions, &composed, &sessions);
                }
            }

            arrived = accept_phone(phone.as_ref()), if phone.is_some() => {
                let stream = match arrived {
                    Ok(stream) => stream,
                    Err(error) => break Err(ServeError::PhoneAccept(error)),
                };
                let Some(plane) = phone.as_ref() else {
                    continue;
                };
                let admission = plane.ingress.admission.clone();
                let identity = Arc::clone(&plane.identity);
                let binding = Arc::clone(&plane.binding);
                let upgrading = upgrading.clone();
                connections.spawn(async move {
                    drop(admission.serve_noise_connection(stream, move |admitted| {
                        let identity = Arc::clone(&identity);
                        let binding = Arc::clone(&binding);
                        let upgrading = upgrading.clone();
                        async move {
                            let Ok(permit) = upgrading.try_reserve() else {
                                return response(runtrol_transport::StatusCode::SERVICE_UNAVAILABLE, "busy");
                            };
                            match admitted.begin(&identity, &binding) {
                                Ok((answer, upgrade)) => {
                                    permit.send(upgrade);
                                    answer
                                }
                                Err(_) => response(runtrol_transport::StatusCode::BAD_REQUEST, "invalid upgrade"),
                            }
                        }
                    }).await);
                });
            }

            Some(upgrade) = upgrades.recv(), if phone.is_some() => {
                let services = ConnectionServices {
                    asking: asking.clone(),
                    reserving: reserving.clone(),
                    returning: returning.clone(),
                    composed: Arc::clone(&composed),
                    discovering: Arc::clone(&discovering),
                    session_index: session_index.subscribe(),
                };
                connections.spawn(async move {
                    let Ok(pending) = upgrade.receive().await else {
                        return;
                    };
                    let remote = pending.remote_public_key();
                    let Some(device) = services
                        .composed
                        .device_authority
                        .paired_device(remote)
                        .map(|paired| paired.id)
                    else {
                        return;
                    };
                    let Ok(connection) = pending.approve(remote).await else {
                        return;
                    };
                    // Boxed: the connection future carries the services snapshot, and clippy's size lint
                    // is right that one belongs on the heap rather than in every enclosing future.
                    Box::pin(converse(
                        SurfaceConnection::Phone(Box::new(connection)),
                        Conversation::from_device(device),
                        services,
                    ))
                    .await;
                });
            }

            arrived = accept_relay(relay_hub.as_mut()), if relay_hub.is_some() => {
                let Some(arrival) = arrived else {
                    relay_hub = None;
                    continue;
                };
                connections.spawn(converse(
                    SurfaceConnection::Relay(Box::new(arrival.surface)),
                    Conversation::from_device(arrival.device),
                    ConnectionServices {
                        asking: asking.clone(),
                        reserving: reserving.clone(),
                        returning: returning.clone(),
                        composed: Arc::clone(&composed),
                        discovering: Arc::clone(&discovering),
                        session_index: session_index.subscribe(),
                    },
                ));
            }

            Some(reservation) = reservations.recv() => match reservation {
                ReservationAsked::Reserve {
                    provider,
                    session,
                    native,
                    workspace,
                    claim,
                    answered,
                } => {
                    let reserved = composed
                        .native_claims
                        .reserve_structured(
                            session,
                            provider.as_str(),
                            native.as_deref(),
                            &workspace,
                        )
                        .map_err(OpenReservationFailure::from)
                        .and_then(|native_claim| {
                            let reserved = sessions
                                .reserve_open_for_provider(provider, session, claim)
                                .map_err(OpenReservationFailure::from)?;
                            native_claim.commit();
                            Ok(reserved)
                        });
                    publish_session_index(
                        &session_index,
                        &runtime_sessions,
                        &composed,
                        &sessions,
                        &provider_update_notices,
                    );
                    if let Err(Ok(abandoned)) = answered.send(reserved) {
                        abandon_reserved(
                            &mut sessions,
                            &mut connections,
                            &reserving,
                            abandoned,
                        );
                        composed.native_claims.replace_structured(&sessions);
                    }
                }
                ReservationAsked::ReserveProviderUpdate { provider, answered } => {
                    let reserved = sessions.reserve_provider_update(provider);
                    if let Err(Ok(abandoned)) = answered.send(reserved) {
                        sessions.release_provider_update(abandoned);
                    }
                }
                ReservationAsked::CancelOpen(reservation) => {
                    sessions.cancel_open(reservation);
                    composed.native_claims.replace_structured(&sessions);
                }
                ReservationAsked::ReleaseClosing(reservation) => {
                    sessions.release_closing(reservation);
                    // The native claim the closed session held is dropped here, the same as `CancelOpen`
                    // above does for an abandoned open. Without it the claim outlived the session, and a
                    // resume of the same provider-native conversation right after `close` was refused as
                    // "already live as a structured session" (measured 2026-08-27 by the ACP smoke gate on
                    // every platform). The reservation task is one FIFO, so a resume that follows the close's
                    // own reply is reserved only after this release is applied.
                    composed.native_claims.replace_structured(&sessions);
                    runtrol_childproc::footprint::release_unused_memory();
                }
                ReservationAsked::ReleaseProviderUpdate(reservation) => {
                    sessions.release_provider_update(reservation);
                    runtrol_childproc::footprint::release_unused_memory();
                }
            },

            Some(ask) = asked.recv() => {
                let Asked { mut conversation, request, prepared, reservation, answered } = *ask;
                let changes_index = matches!(
                    &request,
                    Request::Start { .. }
                        | Request::Resume { .. }
                        | Request::Close { .. }
                        | Request::IntegrationApprovalFinish { .. }
                        | Request::IntegrationRevoke { .. }
                        | Request::IntegrationGrantChange { .. }
                );
                let account_provider = match &request {
                    Request::Start { provider, .. } | Request::Resume { provider, .. } => {
                        ProviderId::parse(provider).map(Some)
                    }
                    Request::Close { session, .. } => Ok(sessions
                        .live_session(*session)
                        .map(|live| live.provider)),
                    _ => Ok(None),
                };
                let reservation = reservation.and_then(ReservationGuard::take);
                let reply = answer_prepared(
                    &mut conversation,
                    &composed,
                    &mut sessions,
                    request,
                    prepared,
                    reservation,
                ).await;
                if matches!(reply, Reply::Draining) && !draining {
                    draining = true;
                    begin_drain(&composed, &mut background, &mut relay_hub);
                    generation.update(live_work_of(&sessions, &composed), true);
                }
                // The connection stopped while its request was being answered. Nothing to report and nowhere to
                // report it: the caller is gone, and the sessions already record everything the request did.
                let abandoned_agent = deliver_answer(
                    answered,
                    Answered { conversation, reply },
                    &mut connections,
                    &reserving,
                    &mut sessions,
                );
                if changes_index || abandoned_agent {
                    publish_session_index(
                        &session_index,
                        &runtime_sessions,
                        &composed,
                        &sessions,
                        &provider_update_notices,
                    );
                    generation.update(live_work_of(&sessions, &composed), draining);
                    if let Ok(Some(provider)) = account_provider {
                        // A conversation opened or closed: ask only the service whose account could have moved.
                        composed.account_probe_wake.provider(provider).await;
                    }
                }
                begin_runtime_audit_shutdown_if_drained(
                    draining,
                    &sessions,
                    &composed,
                    &runtime_audit,
                    &mut runtime_audit_closing,
                );
            }

            // A hosted terminal ended. Only a draining owner cares: it may now be free to finish.
            () = composed.terminal_closed.notified(), if draining => {
                generation.update(live_work_of(&sessions, &composed), true);
                begin_runtime_audit_shutdown_if_drained(
                    draining,
                    &sessions,
                    &composed,
                    &runtime_audit,
                    &mut runtime_audit_closing,
                );
            }

            Some(ask) = runtime_asked.recv() => {
                let crate::runtime_control::RuntimeAsked {
                    integration,
                    request,
                    answered,
                } = *ask;
                let changes_index = !matches!(
                    &request,
                    crate::runtime_control::RuntimeControlRequest::Watch { .. }
                );
                let reply = if draining
                    && matches!(&request, crate::runtime_control::RuntimeControlRequest::PrepareOpen(_))
                {
                    crate::runtime_control::RuntimeControlReply::Failed(
                        crate::runtime_control::RuntimeControlFailure::new(
                            runtrol_runtime_protocol::RuntimeErrorKind::RuntimeUnavailable,
                            DRAINING_REFUSAL,
                        ),
                    )
                } else {
                    runtime_control.answer(
                        &composed.store,
                        &mut sessions,
                        integration,
                        request,
                    )
                };
                if let Err(reply) = answered.send(reply) {
                    match reply {
                        crate::runtime_control::RuntimeControlReply::Sending {
                            mutation: _,
                            taken,
                            command: _,
                        } => {
                            let runtrol_core::TakenAgent { agent, lease } = taken;
                            drop(agent);
                            sessions.abandon_agent(lease);
                        }
                        crate::runtime_control::RuntimeControlReply::Opening(opening) => {
                            let completion = crate::runtime_control::RuntimeControl::abandon_open(
                                &mut sessions,
                                *opening,
                            );
                            schedule_runtime_open_cleanup(
                                &mut connections,
                                &runtime_returning,
                                completion,
                            );
                        }
                        crate::runtime_control::RuntimeControlReply::Cooling(cooling) => {
                            schedule_abandoned_runtime_cool(
                                &mut connections,
                                &runtime_returning,
                                cooling,
                            );
                        }
                        _ => {}
                    }
                }
                if changes_index {
                    publish_session_index(
                        &session_index,
                        &runtime_sessions,
                        &composed,
                        &sessions,
                        &provider_update_notices,
                    );
                }
            }

            Some(returned_agent) = returned.recv() => match returned_agent {
                AgentReturned::Finished { lease, agent, outcome, answered } => {
                    let response = match sessions.return_agent(lease, agent) {
                        Ok(()) => match outcome {
                            Ok(()) => Response::Done,
                            Err(error) => Response::Failed(runtrol_ipc::wire::WireError::from_provider(&error)),
                        },
                        Err(agent) => {
                            drop(agent);
                            refuse("the session no longer accepts its completed provider command")
                        }
                    };
                    drop(answered.send(response));
                }
                AgentReturned::Abandoned(lease) => {
                    sessions.abandon_agent(lease);
                    publish_session_index(
                        &session_index,
                        &runtime_sessions,
                        &composed,
                        &sessions,
                        &provider_update_notices,
                    );
                }
            },

            Some(returned_agent) = runtime_returned.recv() => match returned_agent {
                crate::runtime_control::RuntimeReturned::Finished {
                    mutation,
                    taken,
                    outcome,
                    answered,
                } => {
                    let result = runtime_control.finish_command(
                        &composed.store,
                        &mut sessions,
                        mutation,
                        taken,
                        &outcome,
                    );
                    let _ignored = answered.send(result);
                }
                crate::runtime_control::RuntimeReturned::Abandoned { mutation, lease } => {
                    crate::runtime_control::RuntimeControl::abandon_command(
                        &mut sessions,
                        mutation,
                        lease,
                    );
                    publish_session_index(
                        &session_index,
                        &runtime_sessions,
                        &composed,
                        &sessions,
                        &provider_update_notices,
                    );
                }
                crate::runtime_control::RuntimeReturned::Cooled {
                    mutation,
                    reservation,
                    outcome,
                    answered,
                } => {
                    let result = runtime_control.finish_cool(
                        &composed.store,
                        &mut sessions,
                        mutation,
                        reservation,
                        &outcome,
                    );
                    let _ignored = answered.send(result);
                    publish_session_index(
                        &session_index,
                        &runtime_sessions,
                        &composed,
                        &sessions,
                        &provider_update_notices,
                    );
                }
                crate::runtime_control::RuntimeReturned::CoolAbandoned {
                    mutation,
                    reservation,
                } => {
                    crate::runtime_control::RuntimeControl::abandon_cool(
                        &mut sessions,
                        mutation,
                        reservation,
                    );
                    publish_session_index(
                        &session_index,
                        &runtime_sessions,
                        &composed,
                        &sessions,
                        &provider_update_notices,
                    );
                }
                crate::runtime_control::RuntimeReturned::Opened {
                    opening,
                    intent,
                    agent,
                    answered,
                } => {
                    let completion = runtime_control.finish_open(
                        &composed.store,
                        &mut sessions,
                        opening,
                        &intent,
                        agent,
                    ).await;
                    deliver_runtime_open_completion(
                        &mut connections,
                        &runtime_returning,
                        answered,
                        completion,
                    );
                    publish_session_index(
                        &session_index,
                        &runtime_sessions,
                        &composed,
                        &sessions,
                        &provider_update_notices,
                    );
                }
                crate::runtime_control::RuntimeReturned::OpenDenied {
                    opening,
                    failure,
                    answered,
                } => {
                    let completion = runtime_control.deny_open(
                        &composed.store,
                        &mut sessions,
                        opening,
                        failure,
                    );
                    deliver_runtime_open_completion(
                        &mut connections,
                        &runtime_returning,
                        answered,
                        completion,
                    );
                    publish_session_index(
                        &session_index,
                        &runtime_sessions,
                        &composed,
                        &sessions,
                        &provider_update_notices,
                    );
                }
                crate::runtime_control::RuntimeReturned::OpenUnknown { opening, answered } => {
                    let completion = crate::runtime_control::RuntimeControl::abandon_open(
                        &mut sessions,
                        opening,
                    );
                    deliver_runtime_open_completion(
                        &mut connections,
                        &runtime_returning,
                        answered,
                        completion,
                    );
                    publish_session_index(
                        &session_index,
                        &runtime_sessions,
                        &composed,
                        &sessions,
                        &provider_update_notices,
                    );
                }
                crate::runtime_control::RuntimeReturned::OpenAbandoned { opening } => {
                    let completion = crate::runtime_control::RuntimeControl::abandon_open(
                        &mut sessions,
                        opening,
                    );
                    schedule_runtime_open_cleanup(
                        &mut connections,
                        &runtime_returning,
                        completion,
                    );
                    publish_session_index(
                        &session_index,
                        &runtime_sessions,
                        &composed,
                        &sessions,
                        &provider_update_notices,
                    );
                }
                crate::runtime_control::RuntimeReturned::OpenCleaned {
                    reservation,
                    answered,
                } => {
                    let result = crate::runtime_control::RuntimeControl::finish_open_cleanup(
                        &mut sessions,
                        reservation,
                    );
                    drop(answered.send(result));
                    publish_session_index(
                        &session_index,
                        &runtime_sessions,
                        &composed,
                        &sessions,
                        &provider_update_notices,
                    );
                }
            },

            // Events reach watchers through the session's own fan-out. This arm keeps the provider stream moving.
            pumped = sessions.pump_any() => {
                let runtrol_core::Pumped {
                    session,
                    published,
                    index_changed,
                    gauges_changed,
                } = pumped;
                let release_oversize = published.as_ref().is_some_and(|published| {
                    published.event.body.payload_bytes()
                        > runtrol_core::events::MAX_LIVE_PAYLOAD_BYTES
                });
                if let Some(published) = published {
                    let should_wake = published.event.body.deserves_a_notification();
                    // A draining generation persists nothing: the store belongs to its successor now,
                    // and the provider's own transcript is where this conversation reopens from.
                    if !draining {
                        if let Err(error) = crate::dispatch::persist_live(&composed, &sessions, session).await {
                            break Err(error.into());
                        }
                        if let Err(error) = composed.store.put_cursor(
                            session,
                            runtrol_store::Cursor {
                                src_end: published.event.src_end,
                                seq: published.event.seq,
                            },
                        ) {
                            break Err(error.into());
                        }
                    }
                    if should_wake {
                        schedule_push_wakes(&mut connections, &composed, &push_wake_active);
                    }
                }
                if release_oversize {
                    // The fan-out and replay ring both reject this payload, so the positioned event above owned the
                    // last large allocation. Release allocator pages at that exact drop boundary instead of waiting
                    // for a later session close after smaller allocations may have fragmented them.
                    runtrol_childproc::footprint::release_unused_memory();
                }
                if index_changed {
                    publish_session_index(
                        &session_index,
                        &runtime_sessions,
                        &composed,
                        &sessions,
                        &provider_update_notices,
                    );
                    generation.update(live_work_of(&sessions, &composed), draining);
                    begin_runtime_audit_shutdown_if_drained(
                        draining,
                        &sessions,
                        &composed,
                        &runtime_audit,
                        &mut runtime_audit_closing,
                    );
                }
                if index_changed {
                    // A turn ended or a conversation changed state: the account probe asks the services
                    // again soon, so the limit the turn moved reaches the sidebar without waiting for a clock.
                    if let Some(live) = sessions.live_session(session) {
                        composed.account_probe_wake.provider(live.provider).await;
                    } else {
                        composed.account_probe_wake.all().await;
                    }
                }
                if gauges_changed {
                    account_gauges.send_replace(Arc::new(
                        crate::runtime_inventory::merge_probed_usage(
                            &crate::runtime_inventory::provider_usage(&sessions.account_gauges()),
                            &composed,
                        ),
                    ));
                    // The private index carries the same usage lines, so the phone's index watch draws
                    // the account's position from the push that moves its rows.
                    if !index_changed {
                        publish_session_index(
                            &session_index,
                            &runtime_sessions,
                            &composed,
                            &sessions,
                            &provider_update_notices,
                        );
                    }
                }
            }

            Some(notice) = update_notices.recv() => {
                provider_update_notices.insert(notice.provider, notice.message);
                publish_runtime_providers(&runtime_providers, &composed);
                publish_session_index(
                    &session_index,
                    &runtime_sessions,
                    &composed,
                    &sessions,
                    &provider_update_notices,
                );
                provider_update_notices.clear();
            }

            Some(_finished) = connections.join_next(), if !connections.is_empty() => {
                // JoinSet owns the completed task future until this exact branch removes it. Provider prewarming,
                // connection supervisors, and other bounded jobs may have already released their inner buffers,
                // but allocator relief before this point cannot return pages still referenced by the outer future.
                // On Windows this branch also sees startup preparation, where EmptyWorkingSet would purge live code
                // rather than only released buffers. Explicit session and oversized-watch boundaries still reclaim it.
                #[cfg(not(windows))]
                runtrol_childproc::footprint::release_unused_memory();
            }
        }
    };

    runtime_audit.begin_shutdown();
    connections.abort_all();
    while connections.join_next().await.is_some() {}
    drop(runtime_audit);
    let runtime_audit_writer = match runtime_audit_writer_joined {
        Some(joined) => joined,
        None => runtime_audit_writer.await,
    };
    // Removed before the process ends, so nothing reads an entry for a daemon that is gone.
    drop(generation);
    if outcome.is_ok() {
        match runtime_audit_writer {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                return Err(ServeError::RuntimeBootstrap(format!(
                    "Runtime audit writer failed during shutdown: {error}"
                )));
            }
            Err(error) => {
                return Err(ServeError::RuntimeBootstrap(format!(
                    "Runtime audit writer stopped during shutdown: {error}"
                )));
            }
        }
    }
    outcome
}

/// What a draining generation tells anything that would open a new conversation here.
const DRAINING_REFUSAL: &str =
    "this Runtrol Core generation is draining; the newer generation takes new conversations";

/// Hand this home to the successor: the store first, then everything only one generation should do.
///
/// The connections stay: watchers of running turns keep their streams, and the successor's liveness probe
/// still gets an answer on the control endpoint until this process ends.
fn begin_drain(
    composed: &Composed,
    background: &mut Vec<tokio::task::AbortHandle>,
    relay_hub: &mut Option<crate::relay::RelayHub>,
) {
    composed
        .draining
        .store(true, std::sync::atomic::Ordering::Release);
    // Freeze before releasing the database. Failure leaves the relay unavailable, which retires public
    // terminal authority instead of letting a draining generation keep an independent stale grant.
    composed
        .generation_authority
        .freeze(&composed.integration_authority);
    // The successor is retrying its open right now; this is the moment it succeeds. The session store is
    // the exclusive file it waits on, and holding it a moment longer than needed is the whole gap
    // generations exist to close.
    let released = composed.store.release();
    debug_assert!(
        released,
        "a drain request reached a store that was already released"
    );
    for task in background.drain(..) {
        task.abort();
    }
    *relay_hub = None;
}

/// How much work is live here: every supervised session plus every open terminal. What keeps a draining
/// generation alive, and what `runtrol status` shows.
///
/// # Why a session counts even when nothing is running in it
///
/// A supervised session is a provider process this generation started and owns. Between turns it is idle, not
/// finished: the person's conversation is sitting there with its context, waiting for the next thing they type.
/// This counted running turns instead, so a generation draining while every session happened to be between
/// turns saw zero work and left, taking those processes with it. Measured 2026-08-26: the upgrade journey
/// started one session, upgraded, and found the original daemon gone, which is exactly a person losing an open
/// conversation to an update they never noticed.
///
/// A hosted terminal counts for the same reason: it is a conversation somebody is looking at, and a generation
/// that ended under it would take the screen with it.
fn live_work_of(sessions: &SessionManager, composed: &Composed) -> u32 {
    let terminals = u32::try_from(
        composed
            .open_terminals
            .load(std::sync::atomic::Ordering::Acquire),
    )
    .unwrap_or(u32::MAX);
    let supervised = u32::try_from(sessions.live_sessions().count()).unwrap_or(u32::MAX);
    supervised.saturating_add(terminals)
}

fn begin_runtime_audit_shutdown_if_drained(
    draining: bool,
    sessions: &SessionManager,
    composed: &Composed,
    audit: &crate::runtime_audit::AuditJournal,
    closing: &mut bool,
) {
    if draining && !*closing && live_work_of(sessions, composed) == 0 {
        audit.begin_shutdown();
        *closing = true;
    }
}

async fn wait_until_runtime_audit_relay_empty(
    audit: &crate::runtime_audit::AuditJournal,
    relay: &crate::audit_relay::AuditRelay,
) {
    audit.wait_until_idle().await;
    relay.wait_until_empty().await;
}

fn schedule_push_wakes(
    tasks: &mut JoinSet<()>,
    composed: &Arc<Composed>,
    active: &Arc<AtomicBool>,
) {
    let Some(identity) = composed.push_identity.clone() else {
        return;
    };
    let targets = composed.device_authority.push_targets();
    if targets.is_empty()
        || active
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
    {
        return;
    }
    let guard = PushWakeGuard(Arc::clone(active));
    tasks.spawn(async move {
        let _guard = guard;
        for (device, endpoint) in targets {
            if identity.wake(*device.as_bytes(), &endpoint).await.is_err() {
                // Push is a redundant doorbell. The authoritative Core event remains in the bounded reconnect
                // stream, so a delivery failure must not stop the daemon or the provider session.
            }
        }
    });
}

struct PushWakeGuard(Arc<AtomicBool>);

impl Drop for PushWakeGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

fn schedule_abandoned_runtime_cool(
    tasks: &mut JoinSet<()>,
    returning: &mpsc::UnboundedSender<crate::runtime_control::RuntimeReturned>,
    cooling: crate::runtime_control::RuntimeCooling,
) {
    let returning = returning.clone();
    tasks.spawn(async move {
        let crate::runtime_control::RuntimeCooling {
            mutation,
            agent: handed_agent,
            reservation,
        } = cooling;
        let guard = crate::runtime_control::RuntimeCoolGuard::new(mutation, reservation, returning);
        let agent = handed_agent;
        drop(agent.close(CloseMode::graceful()).await);
        drop(guard);
    });
}

fn deliver_runtime_open_completion(
    tasks: &mut JoinSet<()>,
    returning: &mpsc::UnboundedSender<crate::runtime_control::RuntimeReturned>,
    answered: oneshot::Sender<crate::runtime_control::RuntimeOpenCompletion>,
    completion: crate::runtime_control::RuntimeOpenCompletion,
) {
    if let Err(completion) = answered.send(completion) {
        schedule_runtime_open_cleanup(tasks, returning, completion);
    }
}

fn schedule_runtime_open_cleanup(
    tasks: &mut JoinSet<()>,
    returning: &mpsc::UnboundedSender<crate::runtime_control::RuntimeReturned>,
    completion: crate::runtime_control::RuntimeOpenCompletion,
) {
    let crate::runtime_control::RuntimeOpenCompletion::Cleanup { agent, reservation } = completion
    else {
        return;
    };
    let returning = returning.clone();
    tasks.spawn(async move {
        drop(agent.close(CloseMode::Kill).await);
        let (answered, _hearing) = oneshot::channel();
        drop(
            returning.send(crate::runtime_control::RuntimeReturned::OpenCleaned {
                reservation,
                answered,
            }),
        );
    });
}

/// Release an unanswered reservation without exposing an extra live process during displaced cleanup.
fn abandon_reserved(
    sessions: &mut SessionManager,
    tasks: &mut JoinSet<()>,
    cancelling: &mpsc::UnboundedSender<ReservationAsked>,
    abandoned: ReservedOpen,
) {
    let ReservedOpen {
        reservation,
        displaced,
    } = abandoned;
    let Some(displaced) = displaced else {
        sessions.cancel_open(reservation);
        return;
    };
    let cancelling = cancelling.clone();
    tasks.spawn(async move {
        let releasing_open = ReservationGuard {
            reservation: Some(CleanupReservation::Open(reservation)),
            cancelling: cancelling.clone(),
        };
        let releasing_displaced = ReservationGuard {
            reservation: Some(CleanupReservation::Closing(displaced.reservation)),
            cancelling,
        };
        drop(
            displaced
                .agent
                .close(CloseMode::Graceful { grace_ms: 0 })
                .await,
        );
        drop(releasing_displaced);
        drop(releasing_open);
    });
}

/// Deliver an answer or finish any process handoff whose connection disappeared first.
fn deliver_answer(
    answered: oneshot::Sender<Answered>,
    answer: Answered,
    tasks: &mut JoinSet<()>,
    cancelling: &mpsc::UnboundedSender<ReservationAsked>,
    sessions: &mut SessionManager,
) -> bool {
    if let Err(abandoned) = answered.send(answer) {
        return abandon_reply(tasks, cancelling, sessions, abandoned.reply);
    }
    false
}

fn abandon_reply(
    tasks: &mut JoinSet<()>,
    cancelling: &mpsc::UnboundedSender<ReservationAsked>,
    sessions: &mut SessionManager,
    reply: Reply,
) -> bool {
    match reply {
        Reply::Stopping {
            agent,
            how,
            reservation,
        } => {
            spawn_abandoned_cleanup(
                tasks,
                cancelling,
                agent,
                how,
                Some(CleanupReservation::Closing(reservation)),
            );
            false
        }
        Reply::Cleaning { agents, .. } => {
            for Cleanup {
                agent,
                how,
                reservation,
            } in agents
            {
                spawn_abandoned_cleanup(tasks, cancelling, agent, how, reservation);
            }
            false
        }
        Reply::Sending { taken, .. } => {
            let TakenAgent { agent, lease } = taken;
            drop(agent);
            sessions.abandon_agent(lease);
            true
        }
        Reply::Updating { reservation, .. } => {
            drop(cancelling.send(ReservationAsked::ReleaseProviderUpdate(reservation)));
            false
        }
        Reply::One(_)
        | Reply::NotHere(_)
        | Reply::Watching(_)
        | Reply::WatchingSessions
        | Reply::Draining => false,
    }
}

fn spawn_abandoned_cleanup(
    tasks: &mut JoinSet<()>,
    cancelling: &mpsc::UnboundedSender<ReservationAsked>,
    agent: Box<dyn runtrol_provider::Agent>,
    how: CloseMode,
    reservation: Option<CleanupReservation>,
) {
    let cancelling = cancelling.clone();
    tasks.spawn(async move {
        let releasing = reservation.map(|reservation| ReservationGuard {
            reservation: Some(reservation),
            cancelling,
        });
        drop(agent.close(how).await);
        drop(releasing);
    });
}

fn canonical_workspace_claim(
    workspace: &str,
    access: WorkspaceAccess,
) -> Result<WorkspaceClaim, SessionError> {
    let workspace = AbsPath::canonicalize(workspace)
        .map_err(runtrol_core::ProjectError::from)
        .map_err(SessionError::from)?;
    WorkspaceClaim::discover(workspace, access).map_err(SessionError::from)
}

fn requested_workspace(request: &Request) -> Option<(&str, WorkspaceAccess)> {
    match request {
        Request::Start {
            workspace,
            workspace_access,
            ..
        }
        | Request::Resume {
            workspace,
            workspace_access,
            ..
        } => Some((workspace, *workspace_access)),
        _ => None,
    }
}

/// Which providers a request prepares, so a connection holds exactly those lanes.
///
/// A legacy read runs every registered provider's own configuration commands, so it holds every registered
/// lane; identifier order (the registry map's own) keeps multi-lane holders from deadlocking each other.
fn preparation_providers(
    request: &Request,
    registry: &runtrol_core::registry::ProviderRegistry,
) -> Vec<ProviderId> {
    match request {
        Request::Models { provider }
        | Request::Start { provider, .. }
        | Request::Resume { provider, .. }
        | Request::ProviderUpdate { provider, .. } => match ProviderId::parse(provider) {
            Ok(provider) => vec![provider],
            // A name no registry can hold is refused by name where the request is answered, and
            // nothing will be prepared for it, so there is no lane to hold.
            Err(_) => Vec::new(),
        },
        request if crate::legacy_mcp::is_legacy_mcp(request) => registry
            .all()
            .map(runtrol_core::registry::Provider::id)
            .collect(),
        _ => Vec::new(),
    }
}

fn requested_provider(request: &Request) -> Option<&str> {
    match request {
        Request::Start { provider, .. } | Request::Resume { provider, .. } => Some(provider),
        _ => None,
    }
}

/// One connection, for as long as it lasts.
///
/// Reads a request, asks the one task that owns the sessions, and writes back what it says. A connection that goes
/// away simply ends: it is not a failure the daemon has to act on.
async fn converse(
    connection: SurfaceConnection,
    conversation: Conversation,
    services: ConnectionServices,
) {
    close_trace("control: conversation began");
    let mut release_watch_memory = false;
    Box::pin(converse_inner(
        connection,
        conversation,
        services,
        &mut release_watch_memory,
    ))
    .await;
    if release_watch_memory {
        // The inner future owns the transport, relay state, and encoded frame. Awaiting it here drops those values
        // before allocator pressure relief runs, including when session close races with the peer disconnect.
        runtrol_childproc::footprint::release_unused_memory();
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "one connection lifecycle keeps reservation cancellation and request ownership visible together"
)]
async fn converse_inner(
    mut connection: SurfaceConnection,
    mut conversation: Conversation,
    services: ConnectionServices,
    release_watch_memory: &mut bool,
) {
    let ConnectionServices {
        asking,
        reserving,
        returning,
        composed,
        discovering,
        mut session_index,
    } = services;
    loop {
        // Deliberately untraced: this is the hottest line in the daemon, and a per-frame stderr write under the
        // harness switch measurably moved the refresh p95 ratchet (56.8 ms over a 50 ms budget, 2026-08-27).
        let frame = match connection.recv().await {
            Ok(Some(frame)) => frame,
            // The other end is gone. Ordinary, and the end of this task.
            Ok(None) => return,
            // The connection failed or sent something this build cannot carry. Said out loud if it can still be
            // written to, and then this connection is over: a stream that produced an unreadable frame cannot be
            // resynchronised, and reading on would act on whatever the next bytes happened to look like.
            Err(error) => {
                drop(write(&mut connection, &refuse(&error.to_string())).await);
                return;
            }
        };

        let request = match serde_json::from_slice::<Request>(&frame) {
            Ok(request) => request,
            // One unreadable request, not a broken connection. Refused by name so the caller can correct it, and
            // the connection stays open because the next request may well be fine.
            Err(error) => {
                if write(&mut connection, &refuse(&error.to_string()))
                    .await
                    .is_err()
                {
                    return;
                }
                continue;
            }
        };
        // The earliest point a close is a close: with the dispatch breadcrumbs, silence here means the CLI's
        // request never got read off the connection at all (the accept or read side is what is starved).
        if matches!(request, Request::Close { .. }) {
            close_trace("control: close request read");
        }

        if let Err(refusal) = crate::scope::allowed_with_authority(
            conversation.caller(),
            &request,
            &composed.device_authority,
        ) {
            if write(&mut connection, &refuse(&refusal.to_string()))
                .await
                .is_err()
            {
                return;
            }
            continue;
        }

        // A transparent provider shim is a terminal view from this point on. It is admitted only on the owner-local
        // pipe, registers the daemon-owned PTY before the provider process starts, and never enters the structured
        // session owner. The exact argv is local operator input, not a public or paired-device capability.
        if let Request::TerminalOpen {
            provider,
            arguments: Some(arguments),
            workspace,
            cols,
            rows,
            shell_ancestors,
            ..
        } = &request
            && conversation.greeted()
        {
            if !conversation.caller().is_at_the_machine() {
                drop(
                    write(
                        &mut connection,
                        &refuse("exact provider invocations are accepted only from a local owner terminal"),
                    )
                    .await,
                );
                return;
            }
            if composed.draining.load(std::sync::atomic::Ordering::Acquire) {
                drop(write(&mut connection, &refuse(DRAINING_REFUSAL)).await);
                return;
            }
            let Ok(provider) = ProviderId::parse(provider) else {
                drop(write(&mut connection, &refuse("the provider identity is invalid")).await);
                return;
            };
            let workspace = match AbsPath::canonicalize(workspace.as_ref()) {
                Ok(workspace) if workspace.as_std_path().is_dir() => workspace,
                Ok(_) | Err(_) => {
                    drop(
                        write(
                            &mut connection,
                            &refuse("the brokered provider working directory is not an existing directory"),
                        )
                        .await,
                    );
                    return;
                }
            };
            let arguments = arguments
                .iter()
                .map(|argument| String::from(argument.as_ref()))
                .collect();
            let opened = {
                let _provider_lane = discovering.lane(provider).await.lock_owned().await;
                let prepared =
                    match crate::provider_prepare::prepared_terminal_driver(&composed, provider)
                        .await
                    {
                        Ok(prepared) => prepared,
                        Err(error) => {
                            drop(write(&mut connection, &refuse(error.message())).await);
                            return;
                        }
                    };
                let Some(program) = prepared.terminal_program else {
                    drop(
                        write(
                            &mut connection,
                            &refuse("the provider publishes no prepared terminal program"),
                        )
                        .await,
                    );
                    return;
                };
                crate::terminal_surface::open_brokered(
                    &composed, provider, workspace, *cols, *rows, arguments, program,
                )
                .await
            };
            let (terminal_id, terminal, attachment) = match opened {
                Ok(opened) => opened,
                Err(error) => {
                    drop(write(&mut connection, &refuse(&error.to_string())).await);
                    return;
                }
            };
            if !shell_ancestors.is_empty() {
                crate::terminal_surface::brokered_by_shell(&composed, terminal_id, shell_ancestors)
                    .await;
            }
            let Some(hosted) = composed.terminals.lock().await.hosted(terminal_id) else {
                drop(
                    write(
                        &mut connection,
                        &refuse("the brokered terminal ended before its first viewer attached"),
                    )
                    .await,
                );
                return;
            };
            let control = crate::runtime_terminal::LocalTerminalControl::for_hosted(&hosted);
            relay_local_broker(
                &mut connection,
                &composed,
                terminal_id,
                terminal,
                attachment,
                Some(control),
            )
            .await;
            return;
        }

        // Refused before a slot is reserved or a provider process started: a draining generation opens
        // nothing new, and the successor is already listening for exactly this request.
        if matches!(request, Request::Start { .. } | Request::Resume { .. })
            && composed.draining.load(std::sync::atomic::Ordering::Acquire)
        {
            if write(&mut connection, &refuse(DRAINING_REFUSAL))
                .await
                .is_err()
            {
                return;
            }
            continue;
        }

        // The owner has already published the exact current listing as encoded wire bytes. A refresh that queued a
        // second reconstruction behind session work made every surface pay owner contention for immutable state.
        // Greeting and scope stay in front of the fast path, just as they are in answer_prepared.
        if matches!(request, Request::List) && conversation.greeted() {
            if let Err(refusal) = crate::scope::allowed_with_authority(
                conversation.caller(),
                &request,
                &composed.device_authority,
            ) {
                if write(&mut connection, &refuse(&refusal.to_string()))
                    .await
                    .is_err()
                {
                    return;
                }
            } else {
                // Cloned out of the borrow before projecting: a device projection reads the
                // filesystem to verify its roots, and a held watch borrow would block the owner's
                // next publish for exactly that long.
                let published = session_index.borrow().clone();
                let frame = index_frame_for(
                    &published,
                    conversation.caller(),
                    &composed.device_authority,
                );
                if connection.send(frame.as_ref()).await.is_err() {
                    return;
                }
            }
            continue;
        }

        let reservation = if matches!(request, Request::Start { .. } | Request::Resume { .. })
            && conversation.greeted()
            && crate::scope::allowed_with_authority(
                conversation.caller(),
                &request,
                &composed.device_authority,
            )
            .is_ok()
        {
            let Some((workspace, access)) = requested_workspace(&request) else {
                if write(
                    &mut connection,
                    &refuse("an opening request did not name a workspace"),
                )
                .await
                .is_err()
                {
                    return;
                }
                continue;
            };
            let claim = match canonical_workspace_claim(workspace, access) {
                Ok(claim) => claim,
                Err(error) => {
                    if write(&mut connection, &refuse(&error.to_string()))
                        .await
                        .is_err()
                    {
                        return;
                    }
                    continue;
                }
            };
            let Some(provider_text) = requested_provider(&request) else {
                continue;
            };
            let Ok(provider) = runtrol_provider::ProviderId::parse(provider_text) else {
                if write(
                    &mut connection,
                    &refuse(&format!(
                        "{provider_text:?} is not a provider name runtrol accepts"
                    )),
                )
                .await
                .is_err()
                {
                    return;
                }
                continue;
            };
            let session = SessionId::now();
            let native = match &request {
                Request::Resume { native, .. } => Some(native.clone()),
                _ => None,
            };
            let workspace = claim.identity().worktree().as_str().into();
            let (answered, hearing) = oneshot::channel();
            if reserving
                .send(ReservationAsked::Reserve {
                    provider,
                    session,
                    native,
                    workspace,
                    claim,
                    answered,
                })
                .is_err()
            {
                return;
            }
            let reserved = match hearing.await {
                Ok(Ok(reserved)) => reserved,
                Ok(Err(error)) => {
                    if write(&mut connection, &refuse(&error.to_string()))
                        .await
                        .is_err()
                    {
                        return;
                    }
                    continue;
                }
                Err(_) => return,
            };
            let ReservedOpen {
                reservation,
                displaced,
            } = reserved;
            let guard = ReservationGuard {
                reservation: Some(CleanupReservation::Open(reservation)),
                cancelling: reserving.clone(),
            };
            if let Some(displaced) = displaced {
                let releasing = ReservationGuard {
                    reservation: Some(CleanupReservation::Closing(displaced.reservation)),
                    cancelling: reserving.clone(),
                };
                drop(
                    displaced
                        .agent
                        .close(CloseMode::Graceful { grace_ms: 0 })
                        .await,
                );
                drop(releasing);
            }
            Some(guard)
        } else {
            None
        };

        let provider_update = matches!(&request, Request::ProviderUpdate { .. });
        // Exactly the lanes this request will prepare, in identifier order. ProviderUpdates holds none
        // here: the inspection takes each provider's lane itself as it reaches it.
        let mut preparation_gate = Vec::new();
        for provider in preparation_providers(&request, &composed.registry) {
            preparation_gate.push(discovering.lane(provider).await.lock_owned().await);
        }
        let reserved_session = reservation
            .as_ref()
            .and_then(|guard| guard.reservation.as_ref())
            .map(CleanupReservation::session);
        let prepared = if let Request::Models { provider } = &request {
            let preparing = async {
                let discovered = discover(&conversation, &composed, &request).await;
                complete_prepare_for(&request, discovered, reserved_session).await
            };
            Box::pin(finish_model_preparation(
                provider,
                preparing,
                model_preparation_budget(),
            ))
            .await
        } else if crate::legacy_mcp::is_legacy_mcp(&request) {
            // The whole legacy exchange runs here in the connection's own task, behind the same gate that
            // bounds temporary provider processes, so a cleanup never stops a running session's events.
            prepare_legacy(&conversation, &composed, &request).await
        } else if matches!(request, Request::ProviderUpdates) {
            prepare_provider_updates(&conversation, &composed, &request, &discovering).await
        } else if is_integration_admin(&request) {
            prepare_integration_admin(&conversation, &composed, &request).await
        } else if crate::dispatch::is_pairing_admin(&request) {
            crate::dispatch::prepare_pairing_admin(&conversation, &composed, &request).await
        } else if matches!(
            request,
            Request::WorkspaceIsolatePrepare { .. } | Request::WorkspaceIsolateRelease { .. }
        ) {
            prepare_isolated_workspace(&conversation, &composed, &request).await
        } else {
            // The old gate's presence doubled as this signal; named directly now that the lanes are a set.
            let discovered = if needs_driver(&request) || provider_update {
                discover(&conversation, &composed, &request).await
            } else {
                Discovered::None
            };
            if !provider_update {
                preparation_gate.clear();
            }
            complete_prepare_for(&request, discovered, reserved_session).await
        };
        // A provider update keeps its provider's lane through package mutation and verification. Its update
        // reservation blocks session processes, while this guard blocks short-lived probes that have no session slot.
        let _provider_update_gate = if provider_update {
            std::mem::take(&mut preparation_gate)
        } else {
            Vec::new()
        };
        drop(preparation_gate);
        let (answered, hearing) = oneshot::channel();
        // Kept for the one answer that can still be tried elsewhere: a session this generation does not hold.
        let asked_for = request.clone();
        let ask = Asked {
            conversation,
            request,
            prepared,
            reservation,
            answered,
        };
        if asking.send(Box::new(ask)).await.is_err() {
            // The daemon stopped serving. There is nothing left to ask and nothing that could answer.
            return;
        }
        let Ok(back) = hearing.await else {
            // Answering was abandoned, which happens only when the daemon is going away.
            return;
        };
        conversation = back.conversation;

        match back.reply {
            Reply::One(response) => {
                if write(&mut connection, &response).await.is_err() {
                    return;
                }
            }

            // The session is real and belongs to the generation draining beside this one, which is where a
            // conversation opened before an update still lives. Asked once, there; a draining generation never
            // asks anyone, so this cannot go round.
            Reply::NotHere(refusal) => {
                let elsewhere = if composed.draining.load(std::sync::atomic::Ordering::Acquire) {
                    None
                } else {
                    match crate::build_identity::build_digest() {
                        Some(own) => {
                            crate::generations::ask_draining_peer(
                                composed.home.paths(),
                                own,
                                &asked_for,
                            )
                            .await
                        }
                        // A build that cannot name itself cannot tell its own entry from a peer's, and asking
                        // blind could send the request back to this very generation.
                        None => None,
                    }
                };
                let answer = elsewhere.unwrap_or(refusal);
                if write(&mut connection, &answer).await.is_err() {
                    return;
                }
            }

            // The drain already happened in the owner: the store is released and nothing new opens.
            // This connection only acknowledges; the successor detects the handover by opening the
            // store, so a caller that vanished first changes nothing.
            Reply::Draining => {
                if write(&mut connection, &crate::generations::drained())
                    .await
                    .is_err()
                {
                    return;
                }
            }

            // This connection is a view of a session from here on. It stops when the session's stream ends or when
            // whoever is on the other end goes away, and either way it does not go back to reading requests: a
            // caller that wants both opens two connections, which costs it nothing and keeps this unambiguous.
            Reply::Watching(watching) => {
                // The acknowledgement is the subscription boundary. Without it, a caller can only sleep and guess
                // whether its Watch request arrived before the next prompt, which loses the very event it watches for
                // on a slow machine.
                let start = watching.start();
                let acknowledged = Response::Watching {
                    starts_at: start.starts_at,
                    live_at: start.live_at,
                    gap: start.gap.map(Box::new),
                };
                if write(&mut connection, &acknowledged).await.is_err() {
                    return;
                }
                relay(&mut connection, *watching).await;
                *release_watch_memory = true;
                return;
            }

            Reply::WatchingSessions => {
                if write(&mut connection, &Response::WatchingSessions)
                    .await
                    .is_err()
                {
                    return;
                }
                relay_session_index(
                    &mut connection,
                    &mut session_index,
                    conversation.caller(),
                    &composed.device_authority,
                )
                .await;
                return;
            }

            Reply::Updating {
                provider,
                reservation,
            } => {
                let releasing = ProviderUpdateGuard {
                    reservation: Some(reservation),
                    releasing: reserving.clone(),
                };
                let response = crate::provider_update::apply_latest(&composed, provider).await;
                drop(releasing);
                if write(&mut connection, &response).await.is_err() {
                    return;
                }
            }

            // The wait the owner task handed over. Done here so that closing one session does not stop every other
            // session's output, and answered truthfully when it is over rather than optimistically before.
            Reply::Stopping {
                agent,
                how,
                reservation,
            } => {
                let releasing = ReservationGuard {
                    reservation: Some(CleanupReservation::Closing(reservation)),
                    cancelling: reserving.clone(),
                };
                close_trace("stopping: agent.close begins");
                let outcome = match agent.close(how).await {
                    Ok(()) => Response::Done,
                    Err(error) => refuse(&error.to_string()),
                };
                close_trace("stopping: agent.close returned");
                drop(releasing);
                if write(&mut connection, &outcome).await.is_err() {
                    return;
                }
            }

            reply @ Reply::Cleaning { .. } => {
                let response = finish_connection_cleanup(reply, &reserving).await;
                if write(&mut connection, &response).await.is_err() {
                    return;
                }
            }

            Reply::Sending { taken, command } => {
                let Some(response) = perform_agent_command(taken, command, returning.clone()).await
                else {
                    return;
                };
                if write(&mut connection, &response).await.is_err() {
                    return;
                }
            }
        }
    }
}

/// Finish a model catalogue without allowing one provider to monopolize preparation forever.
async fn finish_model_preparation<F>(provider: &str, preparing: F, within: Duration) -> Prepared
where
    F: Future<Output = Prepared>,
{
    match tokio::time::timeout(within, preparing).await {
        Ok(prepared) => prepared,
        Err(_elapsed) => Prepared::Invalid {
            kind: PreparedKind::Models,
            provider: provider.into(),
            response: Response::Failed(WireError {
                message: format!(
                    "model discovery for {provider} did not finish within {} milliseconds",
                    within.as_millis()
                )
                .into(),
                retryable: true,
                needs_the_operator: false,
            }),
        },
    }
}

const fn model_preparation_budget() -> Duration {
    Duration::from_millis(MODEL_PREPARATION_BUDGET_MS)
}

/// Perform one provider command outside the session owner, then offer the agent back to it.
#[expect(
    clippy::manual_ok_err,
    reason = "the equivalent Result::ok is forbidden because channel loss must stay visible here"
)]
async fn perform_agent_command(
    taken: TakenAgent,
    command: AgentCommand,
    returning: mpsc::UnboundedSender<AgentReturned>,
) -> Option<Response> {
    let TakenAgent {
        agent: handed_agent,
        lease,
    } = taken;
    let guard = AgentGuard {
        lease: Some(lease),
        returning: returning.clone(),
    };
    // Declared after the guard so cancellation or panic drops the process owner before the guard tells the session
    // owner that its bounded slot may be released. Rust drops locals in reverse declaration order.
    let mut agent = handed_agent;
    let outcome = agent.send(command).await;
    let lease = guard.take()?;
    let (answered, hearing) = oneshot::channel();
    if returning
        .send(AgentReturned::Finished {
            lease,
            agent,
            outcome,
            answered,
        })
        .is_err()
    {
        return None;
    }
    match hearing.await {
        Ok(response) => Some(response),
        Err(_owner_stopped) => None,
    }
}

async fn finish_connection_cleanup(
    reply: Reply,
    cancelling: &mpsc::UnboundedSender<ReservationAsked>,
) -> Response {
    let Reply::Cleaning {
        mut response,
        agents,
    } = reply
    else {
        return refuse("connection cleanup received a reply with no process to stop");
    };
    let mut failures = Vec::new();
    for Cleanup {
        agent,
        how,
        reservation,
    } in agents
    {
        let releasing = reservation.map(|reservation| ReservationGuard {
            reservation: Some(reservation),
            cancelling: cancelling.clone(),
        });
        if let Err(error) = agent.close(how).await {
            failures.push(error.to_string());
        }
        drop(releasing);
    }
    if !failures.is_empty()
        && let Response::Failed(error) = &response
    {
        response = refuse(&format!(
            "{}; cleanup also failed: {}",
            error.message,
            failures.join("; ")
        ));
    }
    response
}

/// Relay a session's events to whoever is watching, until one end stops.
///
/// The event goes out as the provider wrote it. Encoded here and read by nobody in between: this is the last hop a
/// conversation takes inside runtrol, and the whole of what happens to it is being put in an envelope.
async fn relay(connection: &mut SurfaceConnection, mut watching: runtrol_core::SessionView) {
    let stream = watching.start().live_at.stream;
    loop {
        // A live session may be quiet indefinitely. Observe the peer in parallel so a closed watch surface drops its
        // subscription and receive buffer immediately instead of retaining both until the provider emits again or the
        // session closes. Watch connections are one-way after their acknowledgement, so any inbound result ends this
        // dedicated surface.
        let Some(item) = (tokio::select! {
            item = watching.recv() => item,
            _peer = connection.recv() => return,
        }) else {
            return;
        };
        let event = match item {
            runtrol_core::WatchItem::Event(event) => event,
            runtrol_core::WatchItem::Lagged(next_expected) => {
                drop(write(connection, &Response::Lagged { next_expected }).await);
                return;
            }
        };
        let encoded = match event.wire() {
            Ok(encoded) => encoded,
            // An event this build cannot write is a defect in this build, and it is about one event rather than
            // about the session. Said out loud in place of that event, because a watcher that silently skipped one
            // would show a conversation with a hole in it and no sign that anything was missing.
            Err(error) => {
                let detail = format!(
                    "cannot serialize {} event: {error}",
                    event.event().body.wire_name()
                );
                drop(write(connection, &refuse(&detail)).await);
                return;
            }
        };
        let positioned = event.event();
        let next_expected = runtrol_provider::WatchCursor {
            stream,
            epoch: positioned.epoch,
            seq: positioned.seq.wrapping_add(1),
        };
        let edges = match runtrol_ipc::event_response_edges(next_expected) {
            Ok(edges) => edges,
            Err(error) => {
                drop(
                    write(
                        connection,
                        &refuse(&format!("cannot serialize an event cursor: {error}")),
                    )
                    .await,
                );
                return;
            }
        };
        if connection
            .send_parts(&[edges.prefix(), encoded.as_str().as_bytes(), edges.suffix()])
            .await
            .is_err()
        {
            return;
        }
    }
}

/// Carry one owner-local terminal invocation as the first viewer of the daemon-owned PTY.
#[expect(
    clippy::too_many_lines,
    reason = "one local relay orders the screen, live bytes, lag replacement, exit, exact input, and resize"
)]
async fn relay_local_broker(
    connection: &mut SurfaceConnection,
    composed: &Composed,
    terminal_id: runtrol_provider::TerminalId,
    terminal: runtrol_core::terminal::Terminal,
    mut attachment: runtrol_core::terminal::Attachment,
    control: Option<crate::runtime_terminal::LocalTerminalControl>,
) {
    let writable = control.is_some();
    let relayed = async {
        if write(
            connection,
            &Response::TerminalOpened {
                terminal: terminal_id,
                pid: terminal.pid(),
                writable,
            },
        )
        .await
        .is_err()
        {
            return;
        }
        if write(
            connection,
            &Response::TerminalOutput {
                bytes: attachment.snapshot.to_vec().into(),
            },
        )
        .await
        .is_err()
        {
            return;
        }
        loop {
            let exit_code = *attachment.exited.borrow();
            if let Some(code) = exit_code {
                drop(write(connection, &Response::TerminalExited { code }).await);
                return;
            }
            tokio::select! {
                output = attachment.live.recv() => match output {
                    Ok(bytes) => {
                        if write(
                            connection,
                            &Response::TerminalOutput { bytes: bytes.bytes.to_vec().into() },
                        )
                        .await
                        .is_err()
                        {
                            return;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        attachment = terminal.attach().await;
                        if write(connection, &Response::TerminalLagged {}).await.is_err()
                            || write(
                                connection,
                                &Response::TerminalOutput {
                                    bytes: attachment.snapshot.to_vec().into(),
                                },
                            )
                            .await
                            .is_err()
                        {
                            return;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                },
                changed = attachment.exited.changed() => {
                    if changed.is_err() {
                        return;
                    }
                }
                incoming = connection.recv() => {
                    let Ok(Some(frame)) = incoming else {
                        return;
                    };
                    let request = match serde_json::from_slice::<Request>(&frame) {
                        Ok(request) => request,
                        Err(error) => {
                            if write(connection, &refuse(&error.to_string())).await.is_err() {
                                return;
                            }
                            continue;
                        }
                    };
                    let Some(control) = control.as_ref() else {
                        if write(
                            connection,
                            &refuse("this viewer is read-only while another terminal owns input"),
                        )
                        .await
                        .is_err()
                        {
                            return;
                        }
                        continue;
                    };
                    let result = match request {
                        Request::TerminalInput { bytes } => composed
                            .runtime_terminals
                            .write_local(composed, control, bytes.as_ref())
                            .await,
                        Request::TerminalResize { cols, rows } => composed
                            .runtime_terminals
                            .resize_local(composed, control, cols, rows)
                            .await,
                        _ => {
                            if write(
                                connection,
                                &refuse("a brokered terminal view accepts only input and resize frames"),
                            )
                            .await
                            .is_err()
                            {
                                return;
                            }
                            continue;
                        }
                    };
                    if let Err(error) = result
                        && write(connection, &refuse(error.message)).await.is_err()
                    {
                        return;
                    }
                }
            }
        }
    };
    relayed.await;
}

/// Relay coalesced current session snapshots, each projected to what this caller may see.
///
/// A local subscriber shares the one published frame, so nothing is copied per connected surface. A device
/// subscriber gets its own projection, and it also wakes on authority changes: revoking a workspace root must
/// shrink the phone's live view now, not at whatever moment a session next changes state. Identical frames are
/// not resent, so a wake that does not change this caller's view costs no wire traffic.
async fn relay_session_index(
    connection: &mut SurfaceConnection,
    session_index: &mut watch::Receiver<PublishedIndex>,
    caller: &Caller,
    authority: &crate::compose::DeviceAuthority,
) {
    let mut grants = matches!(caller, Caller::Device { .. }).then(|| authority.changes());
    let mut last_sent: Option<Arc<[u8]>> = None;
    loop {
        // Cloned out of the borrow before projecting, so root verification never holds the
        // watch read lock against the owner's next publish.
        let published = session_index.borrow_and_update().clone();
        let frame = index_frame_for(&published, caller, authority);
        if last_sent.as_deref() != Some(frame.as_ref()) {
            if connection.send(frame.as_ref()).await.is_err() {
                return;
            }
            last_sent = Some(frame);
        }
        tokio::select! {
            changed = session_index.changed() => {
                if changed.is_err() {
                    return;
                }
            }
            changed = authority_changed(&mut grants) => {
                if changed.is_err() {
                    return;
                }
            }
            _peer = connection.recv() => return,
        }
    }
}

/// Wait for a device's authority to change; wait forever for callers that have none to change.
async fn authority_changed(
    grants: &mut Option<watch::Receiver<crate::compose::DeviceAuthoritySnapshot>>,
) -> Result<(), watch::error::RecvError> {
    match grants {
        Some(grants) => grants.changed().await,
        None => std::future::pending().await,
    }
}

/// The exact index frame this caller may receive.
///
/// Somebody at the machine shares the published frame; a device gets its live-roots projection, encoded here on
/// its own connection. A refusal (no rows to project) is shared verbatim.
fn index_frame_for(
    published: &PublishedIndex,
    caller: &Caller,
    authority: &crate::compose::DeviceAuthority,
) -> Arc<[u8]> {
    match (&published.listing, caller) {
        (Some(listing), Caller::Device { .. }) => {
            let projected =
                crate::dispatch::sessions_visible_to((**listing).clone(), caller, authority);
            Arc::from(encode_response(&Response::Sessions(projected)))
        }
        _ => Arc::clone(&published.full_frame),
    }
}

fn publish_runtime_providers(
    providers: &watch::Sender<Arc<runtrol_runtime_protocol::ProviderList>>,
    composed: &Composed,
) {
    let next = Arc::new(crate::runtime_inventory::providers(composed));
    providers.send_if_modified(|current| {
        if current.as_ref() == next.as_ref() {
            return false;
        }
        *current = next;
        true
    });
}

/// Publish only a changed current index. The full frame is shared by every at-the-machine subscriber.
fn publish_session_index(
    session_index: &watch::Sender<PublishedIndex>,
    runtime_sessions: &watch::Sender<Arc<crate::runtime_inventory::RuntimeSessionCatalogue>>,
    composed: &Composed,
    sessions: &SessionManager,
    provider_update_notices: &BTreeMap<ProviderId, Box<str>>,
) {
    composed.native_claims.replace_structured(sessions);
    let next = build_session_index(composed, sessions, provider_update_notices);
    session_index.send_if_modified(|current| {
        if current.full_frame.as_ref() == next.full_frame.as_ref() {
            return false;
        }
        *current = next;
        true
    });
    publish_runtime_session_catalogue(runtime_sessions, composed, sessions);
}

/// Publish a memory or lifecycle change without rebuilding the legacy index frame.
fn publish_runtime_session_catalogue(
    runtime_sessions: &watch::Sender<Arc<crate::runtime_inventory::RuntimeSessionCatalogue>>,
    composed: &Composed,
    sessions: &SessionManager,
) {
    let public = match crate::runtime_inventory::sessions(composed, sessions) {
        Ok(catalogue) => Arc::new(catalogue),
        Err(_) => Arc::new(crate::runtime_inventory::RuntimeSessionCatalogue::unavailable()),
    };
    runtime_sessions.send_replace(public);
}

/// Build the current index once: the full at-the-machine view, encoded, plus the rows behind it.
fn build_session_index(
    composed: &Composed,
    sessions: &SessionManager,
    provider_update_notices: &BTreeMap<ProviderId, Box<str>>,
) -> PublishedIndex {
    let mut response = crate::dispatch::list(composed, sessions, &Caller::AtTheMachine);
    if let Response::Sessions(listing) = &mut response {
        listing
            .warnings
            .extend(provider_update_notices.values().cloned());
    }
    let full_frame = Arc::from(encode_response(&response));
    let listing = match response {
        Response::Sessions(listing) => Some(Arc::new(listing)),
        _ => None,
    };
    PublishedIndex {
        full_frame,
        listing,
    }
}

/// Write one answer.
///
/// A response that cannot be serialized is a defect in this build rather than something a caller did, so what goes out
/// instead says exactly that. The alternative is writing nothing, which leaves the caller waiting on a daemon that is
/// working perfectly well.
pub(crate) async fn write(
    connection: &mut SurfaceConnection,
    response: &Response,
) -> Result<(), SurfaceError> {
    connection.send(&encode_response(response)).await
}

async fn accept_phone(phone: Option<&PhonePlane>) -> Result<TcpStream, std::io::Error> {
    match phone {
        Some(phone) => phone
            .ingress
            .listener
            .accept()
            .await
            .map(|(stream, _)| stream),
        None => core::future::pending().await,
    }
}

async fn accept_relay(
    relay: Option<&mut crate::relay::RelayHub>,
) -> Option<crate::relay::RelayArrival> {
    match relay {
        Some(relay) => relay.accept().await,
        None => core::future::pending().await,
    }
}

fn encode_response(response: &Response) -> Vec<u8> {
    serde_json::to_vec(response).unwrap_or_else(|error| {
        let said = refuse(&format!("this daemon could not write its own answer: {error}"));
        serde_json::to_vec(&said).unwrap_or_else(|_| {
            // Two failures to serialize means the failure is in the vocabulary itself. This is that vocabulary,
            // written by hand, so that there is no third thing that could fail.
            br#"{"say":"failed","with":{"message":"this daemon cannot write its own answer","retryable":false,"needs_the_operator":false}}"#.to_vec()
        })
    })
}

#[cfg(test)]
mod tests {

    #[tokio::test]
    async fn a_session_between_turns_still_keeps_a_draining_generation_alive() {
        // The rule a generation drains by. An open session is a provider process this generation started and
        // owns; between turns it is idle, not finished. Counting running turns instead let an update end the
        // conversations a person had open simply because none of them happened to be mid-answer.
        let scratch =
            std::env::temp_dir().join(format!("runtrol-live-work-{}", std::process::id()));
        if scratch.exists() {
            std::fs::remove_dir_all(&scratch).expect("clear the previous run");
        }
        std::fs::create_dir(&scratch).expect("create the scratch home");
        let home = scratch.to_str().expect("UTF-8 scratch path");
        let composed = crate::Composed::for_tests(home, runtrol_drivers::builtin())
            .expect("a fresh home composes");
        let sessions = SessionManager::default();
        assert_eq!(
            live_work_of(&sessions, &composed),
            0,
            "a generation holding nothing has nothing to finish"
        );

        composed
            .open_terminals
            .store(2, std::sync::atomic::Ordering::Release);
        assert_eq!(
            live_work_of(&sessions, &composed),
            2,
            "a hosted terminal is a conversation somebody is looking at"
        );
        std::fs::remove_dir_all(&scratch).expect("clean the scratch home");
    }

    #[tokio::test]
    async fn preparation_lanes_are_per_provider_and_shared_per_provider() {
        // The cold-start fix in one assertion: holding one provider's lane must not make another
        // provider wait, while the same provider always meets the same lane.
        let scratch = std::env::temp_dir().join(format!("runtrol-lanes-{}", std::process::id()));
        if scratch.exists() {
            std::fs::remove_dir_all(&scratch).expect("clear the previous run");
        }
        std::fs::create_dir(&scratch).expect("create the scratch home");
        let home = scratch.to_str().expect("UTF-8 scratch path");
        let composed = crate::Composed::for_tests(home, runtrol_drivers::builtin())
            .expect("a fresh home composes");
        let gates = DiscoveryGates::new(&composed.registry);
        assert!(
            gates.lanes.lock().await.is_empty(),
            "provider lanes stay allocation-free until preparation actually starts"
        );

        let claude = runtrol_provider::ProviderId::parse("claude").expect("a builtin provider");
        let codex = runtrol_provider::ProviderId::parse("codex").expect("a builtin provider");
        let held = gates.lane(claude).await.lock_owned().await;
        assert!(
            gates.lane(codex).await.try_lock_owned().is_ok(),
            "another provider's preparation must not queue behind this one"
        );
        assert!(
            gates.lane(claude).await.try_lock_owned().is_err(),
            "the same provider's second preparation must wait for the first"
        );
        drop(held);

        let unknown = runtrol_provider::ProviderId::parse("nobody-ships-this").expect("parses");
        let held_unknown = gates.lane(unknown).await.lock_owned().await;
        assert!(
            gates.lane(unknown).await.try_lock_owned().is_err(),
            "identities outside the registry still share exactly one lane"
        );
        drop(held_unknown);

        drop(composed);
        std::fs::remove_dir_all(&scratch).expect("remove the scratch home");
    }

    #[tokio::test]
    async fn an_external_turn_ending_is_reported_once() {
        let scratch =
            std::env::temp_dir().join(format!("runtrol-native-activity-{}", std::process::id()));
        if scratch.exists() {
            std::fs::remove_dir_all(&scratch).expect("clear the previous run");
        }
        std::fs::create_dir(&scratch).expect("create the scratch home");
        let home = scratch.to_str().expect("UTF-8 scratch path");
        let composed = crate::Composed::for_tests(home, runtrol_drivers::builtin())
            .expect("a fresh home composes");
        let gates = DiscoveryGates::new(&composed.registry);
        let provider = runtrol_provider::ProviderId::parse("claude").expect("a builtin provider");
        let native = runtrol_provider::NativeSessionId::new("native-one").expect("native id");
        let active = runtrol_provider::NativeProcessActivity {
            live: vec![native.clone()],
            active: vec![native.clone()],
            processes: Vec::new(),
        };
        let quiet = runtrol_provider::NativeProcessActivity {
            live: vec![native],
            active: Vec::new(),
            processes: Vec::new(),
        };

        assert!(!gates.remember_native_activity(provider, active).await);
        assert!(
            gates
                .remember_native_activity(provider, quiet.clone())
                .await,
            "the busy-to-quiet edge is the provider-neutral external turn boundary"
        );
        assert!(
            !gates.remember_native_activity(provider, quiet).await,
            "re-reading the same quiet roster must not start another usage probe"
        );

        drop(gates);
        drop(composed);
        std::fs::remove_dir_all(&scratch).expect("remove the scratch home");
    }

    #[test]
    fn startup_preparation_excludes_catalogue_entries_without_an_executable() {
        let scratch =
            std::env::temp_dir().join(format!("runtrol-prewarm-selection-{}", std::process::id()));
        if scratch.exists() {
            std::fs::remove_dir_all(&scratch).expect("clear the previous run");
        }
        std::fs::create_dir(&scratch).expect("create the scratch home");
        let home = scratch.to_str().expect("UTF-8 scratch path");
        let composed = crate::Composed::for_tests(home, runtrol_drivers::builtin())
            .expect("a fresh home composes");
        let chosen = runtrol_provider::ProviderId::parse("codex").expect("a builtin provider");

        let selected = providers_to_prewarm(&composed.registry, |manifest| manifest.id == chosen);

        assert_eq!(selected, vec![chosen]);
        drop(composed);
        std::fs::remove_dir_all(&scratch).expect("remove the scratch home");
    }

    use core::future::Future;
    use std::collections::BTreeSet;
    use std::path::PathBuf;

    use async_trait::async_trait;
    use base64ct::{Base64UrlUnpadded, Encoding as _};
    use fastwebsockets::{Frame, OpCode, WebSocket, handshake};
    use http_body_util::Empty;
    use hyper::Request as HttpRequest;
    use hyper::upgrade::Upgraded;
    use hyper_util::rt::TokioIo;
    use runtrol_provider::{
        AbsPath, Agent, AgentCommand, Opaque, Produced, ProviderError, ProviderId, WallMs,
    };
    use runtrol_security::{
        DeviceId, DeviceLabels, DeviceScope, GrantLedger, ProjectRootIdentity, WorkspaceRootId,
    };
    use runtrol_store::{DeviceKey, DeviceRootRow, DeviceRow, StoreError};
    use runtrol_transport::{
        AccessToken, Channel, EncryptedRecord, InitiatorHandshake, MAX_ENCRYPTED_RECORD_WIRE,
        NOISE_LINK_PATH, NOISE_LINK_PROTOCOL, PublicKey,
    };

    use super::*;

    struct PendingClose {
        session: SessionId,
        started: oneshot::Sender<()>,
        release: oneshot::Receiver<()>,
        panic_after_release: bool,
    }

    struct PendingSend {
        session: SessionId,
        started: Option<oneshot::Sender<()>>,
        release: Option<oneshot::Receiver<()>>,
        panic_after_start: bool,
    }

    #[async_trait]
    impl Agent for PendingSend {
        fn session(&self) -> SessionId {
            self.session
        }

        fn native(&self) -> Option<&str> {
            None
        }

        async fn send(&mut self, _command: AgentCommand) -> Result<(), ProviderError> {
            if let Some(started) = self.started.take() {
                let _started = started.send(());
            }
            assert!(!self.panic_after_start, "scripted provider command panic");
            if let Some(release) = self.release.take() {
                drop(release.await);
            }
            Ok(())
        }

        async fn next(&mut self) -> Option<Result<Produced, ProviderError>> {
            core::future::pending().await
        }

        async fn close(self: Box<Self>, _how: CloseMode) -> Result<(), ProviderError> {
            Ok(())
        }
    }

    struct ReadyEvent {
        session: SessionId,
        ready: bool,
    }

    #[async_trait]
    impl Agent for ReadyEvent {
        fn session(&self) -> SessionId {
            self.session
        }

        fn native(&self) -> Option<&str> {
            None
        }

        async fn send(&mut self, _command: AgentCommand) -> Result<(), ProviderError> {
            Ok(())
        }

        async fn next(&mut self) -> Option<Result<Produced, ProviderError>> {
            if self.ready {
                self.ready = false;
                Some(Ok(Produced {
                    src_end: 1,
                    body: runtrol_provider::EventBody::Plan {
                        payload: Opaque::none(),
                    },
                }))
            } else {
                core::future::pending().await
            }
        }

        async fn close(self: Box<Self>, _how: CloseMode) -> Result<(), ProviderError> {
            Ok(())
        }
    }

    fn attach_test_agent(sessions: &mut SessionManager, session: SessionId, agent: Box<dyn Agent>) {
        attach_test_agent_in(
            sessions,
            session,
            agent,
            if cfg!(windows) { r"C:\work" } else { "/work" },
        );
    }

    /// Attach a test agent whose session works in an explicit directory, for tests about who may see it.
    fn attach_test_agent_in(
        sessions: &mut SessionManager,
        session: SessionId,
        agent: Box<dyn Agent>,
        workspace: &str,
    ) {
        let claimed = runtrol_provider::AbsPath::new(workspace).expect("valid test path");
        let claim = runtrol_core::WorkspaceClaim::discover(
            claimed,
            runtrol_provider::WorkspaceAccess::Shared,
        )
        .expect("the test workspace claims");
        let reserved = sessions
            .reserve_open(session, claim)
            .expect("one process slot");
        let intent = runtrol_provider::OpenIntent {
            session,
            workspace: runtrol_provider::AbsPath::new(workspace).expect("valid test path"),
            disposition: runtrol_provider::Disposition::Fresh,
            model: None,
            reasoning_effort: None,
            permission: None,
        };
        sessions
            .attach_opened(
                reserved.reservation,
                runtrol_provider::ProviderId::parse("test").expect("valid provider"),
                &intent,
                agent,
            )
            .expect("the test process attaches");
    }

    #[async_trait]
    impl Agent for PendingClose {
        fn session(&self) -> SessionId {
            self.session
        }

        fn native(&self) -> Option<&str> {
            None
        }

        async fn send(&mut self, _command: AgentCommand) -> Result<(), ProviderError> {
            Ok(())
        }

        async fn next(&mut self) -> Option<Result<Produced, ProviderError>> {
            core::future::pending().await
        }

        async fn close(self: Box<Self>, _how: CloseMode) -> Result<(), ProviderError> {
            let _sent = self.started.send(());
            drop(self.release.await);
            assert!(!self.panic_after_release, "scripted close panic");
            Ok(())
        }
    }

    /// A daemon serving at its own endpoint, and the address to reach it at.
    ///
    /// Every part of this is the real thing: a real endpoint, a real listener, real frames. The one substitution is
    /// the containment, which cannot be established in a test without terminating the runner on one platform.
    struct Running {
        address: String,
        home: String,
        serving: tokio::task::JoinHandle<Result<(), ServeError>>,
    }

    struct LivePhoneRestart {
        home: String,
        pc: Arc<StaticKeypair>,
        phone_address: SocketAddr,
    }

    impl Running {
        async fn start(name: &str) -> Self {
            Self::start_with_sessions(name, SessionManager::new()).await
        }

        async fn start_with_sessions(name: &str, sessions: SessionManager) -> Self {
            let root = std::env::temp_dir().join(format!("runtrol-serve-{name}"));
            if root.exists() {
                std::fs::remove_dir_all(&root).expect("clear the previous run");
            }
            let home = root
                .to_str()
                .expect("the temporary path is UTF-8")
                .to_owned();
            let composed = crate::compose::Composed::for_tests(&home, runtrol_drivers::builtin())
                .expect("a fresh home composes");
            let address = composed.home.paths().endpoint().address().to_owned();
            let listener = Listener::bind(&address)
                .await
                .expect("the endpoint is free");
            let serving =
                tokio::spawn(async move { serve_sessions(composed, listener, sessions).await });
            Self {
                address,
                home,
                serving,
            }
        }

        async fn start_with_phone(
            name: &str,
            sessions: SessionManager,
            granted_root: Option<&str>,
        ) -> (Self, SocketAddr, PublicKey, StaticKeypair) {
            let root = std::env::temp_dir().join(format!("runtrol-serve-{name}"));
            if root.exists() {
                std::fs::remove_dir_all(&root).expect("clear the previous run");
            }
            let home = root
                .to_str()
                .expect("the temporary path is UTF-8")
                .to_owned();
            let mut composed =
                crate::compose::Composed::for_tests(&home, runtrol_drivers::builtin())
                    .expect("a fresh home composes");
            let pc = StaticKeypair::generate().expect("PC key");
            let pc_public = pc.public_key();
            let phone = StaticKeypair::generate().expect("phone key");
            let device = DeviceId::now();
            let token = AccessToken::parse(
                "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            )
            .expect("canonical credential");
            composed.pc_identity = Some(Arc::new(pc));
            let mut scopes = vec![DeviceScope::SessionList];
            let mut roots = Vec::new();
            if let Some(granted) = granted_root {
                let root_id = WorkspaceRootId::now();
                let root_path =
                    AbsPath::canonicalize(granted).expect("granted test root is canonical");
                let identity = ProjectRootIdentity::read(&root_path)
                    .expect("granted test root has a filesystem identity");
                scopes.push(DeviceScope::Workspace(root_id));
                roots.push(crate::compose::PairedRoot {
                    id: root_id,
                    path: root_path,
                    identity,
                });
            }
            composed.device_authority.replace(
                GrantLedger::from_persisted([(device, scopes)]),
                vec![crate::compose::PairedDevice {
                    id: device,
                    remote_static_key: phone.public_key(),
                    credential_fingerprint: token.fingerprint(),
                    labels: DeviceLabels::new("Test phone", "Browser").expect("device labels"),
                    roots,
                    push_endpoint: None,
                    paired_at: WallMs::from_millis(1_767_225_600_000),
                }],
            );

            let address = composed.home.paths().endpoint().address().to_owned();
            let listener = Listener::bind(&address)
                .await
                .expect("the endpoint is free");
            let tcp = TcpListener::bind("127.0.0.1:0")
                .await
                .expect("phone listener");
            let ingress =
                PhoneIngress::loopback(tcp, ["https://phone.runtrol.test"]).expect("phone ingress");
            let phone_address = ingress.local_addr().expect("phone address");
            let phone_plane = phone_plane(&composed, ingress).expect("phone plane");
            let serving = tokio::spawn(async move {
                serve_surfaces(composed, listener, sessions, Some(phone_plane), None).await
            });
            (
                Self {
                    address,
                    home,
                    serving,
                },
                phone_address,
                pc_public,
                phone,
            )
        }

        async fn start_with_live_phone(
            name: &str,
            remote_static_key: PublicKey,
            workspace: &str,
            provider: ProviderId,
        ) -> (Self, SocketAddr, PublicKey) {
            let root =
                std::env::temp_dir().join(format!("runtrol-serve-{name}-{}", std::process::id()));
            if root.exists() {
                std::fs::remove_dir_all(&root).expect("clear the previous live phone run");
            }
            let home = root
                .to_str()
                .expect("the temporary path is UTF-8")
                .to_owned();
            let mut composed =
                crate::compose::Composed::for_tests(&home, runtrol_drivers::builtin())
                    .expect("a fresh home composes");
            let pc = StaticKeypair::generate().expect("PC key");
            let pc_public = pc.public_key();
            let device = DeviceId::now();
            let root_id = WorkspaceRootId::now();
            let root_path = AbsPath::canonicalize(workspace).expect("live workspace is canonical");
            let root_identity =
                ProjectRootIdentity::read(&root_path).expect("live workspace has stable identity");
            let token = AccessToken::parse(
                "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            )
            .expect("canonical credential");
            let scopes = vec![
                DeviceScope::SessionList,
                DeviceScope::SessionOutputRead,
                DeviceScope::SessionInputWrite,
                DeviceScope::SessionStart,
                DeviceScope::SessionStop,
                DeviceScope::SessionResume,
                DeviceScope::SessionDelete,
                DeviceScope::ApprovalRespondLow,
                DeviceScope::ApprovalRespondHigh,
                DeviceScope::ModeDefault,
                DeviceScope::Workspace(root_id),
                DeviceScope::Provider(provider),
            ];
            composed.pc_identity = Some(Arc::new(pc));
            composed.device_authority.replace(
                GrantLedger::from_persisted([(device, scopes)]),
                vec![crate::compose::PairedDevice {
                    id: device,
                    remote_static_key,
                    credential_fingerprint: token.fingerprint(),
                    labels: DeviceLabels::new("Headless phone", "Node WebCrypto")
                        .expect("device labels"),
                    roots: vec![crate::compose::PairedRoot {
                        id: root_id,
                        path: root_path,
                        identity: root_identity,
                    }],
                    push_endpoint: None,
                    paired_at: WallMs::from_millis(1_767_225_600_000),
                }],
            );

            let address = composed.home.paths().endpoint().address().to_owned();
            let listener = Listener::bind(&address)
                .await
                .expect("the endpoint is free");
            let tcp = TcpListener::bind("127.0.0.1:0")
                .await
                .expect("phone listener");
            let ingress =
                PhoneIngress::loopback(tcp, ["https://phone.runtrol.test"]).expect("phone ingress");
            let phone_address = ingress.local_addr().expect("phone address");
            let phone_plane = phone_plane(&composed, ingress).expect("phone plane");
            let serving = tokio::spawn(async move {
                serve_surfaces(
                    composed,
                    listener,
                    SessionManager::new(),
                    Some(phone_plane),
                    None,
                )
                .await
            });
            (
                Self {
                    address,
                    home,
                    serving,
                },
                phone_address,
                pc_public,
            )
        }

        async fn start_resilient_live_phone(
            remote_static_key: PublicKey,
            workspace: &str,
            provider: ProviderId,
        ) -> (Self, LivePhoneRestart) {
            let root = std::env::temp_dir().join(format!(
                "runtrol-serve-live-resilience-{}",
                std::process::id()
            ));
            if root.exists() {
                std::fs::remove_dir_all(&root).expect("clear the previous resilience run");
            }
            let home = root
                .to_str()
                .expect("the temporary path is UTF-8")
                .to_owned();
            let mut composed =
                crate::compose::Composed::for_tests(&home, runtrol_drivers::builtin())
                    .expect("a fresh resilience home composes");
            let pc = Arc::new(StaticKeypair::generate().expect("PC key"));
            let device = DeviceId::now();
            let root_id = WorkspaceRootId::now();
            let root_path = AbsPath::canonicalize(workspace).expect("live workspace is canonical");
            let root_identity =
                ProjectRootIdentity::read(&root_path).expect("live workspace has stable identity");
            let token = AccessToken::parse(
                "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            )
            .expect("canonical credential");
            let scopes = [
                DeviceScope::SessionList,
                DeviceScope::SessionOutputRead,
                DeviceScope::SessionInputWrite,
                DeviceScope::SessionStart,
                DeviceScope::SessionStop,
                DeviceScope::SessionResume,
                DeviceScope::SessionDelete,
                DeviceScope::ModeDefault,
                DeviceScope::Workspace(root_id),
                DeviceScope::Provider(provider),
            ];
            composed
                .store
                .put_device(
                    DeviceKey::from_bytes(*device.as_bytes()),
                    &DeviceRow {
                        remote_static_key: remote_static_key.to_bytes(),
                        credential_fingerprint: token.fingerprint().to_bytes(),
                        name: "Headless resilience phone".into(),
                        platform: "Node WebCrypto".into(),
                        scopes: scopes
                            .iter()
                            .map(|scope| scope.to_string().into())
                            .collect(),
                        roots: vec![DeviceRootRow {
                            id: *root_id.as_bytes(),
                            path: root_path.as_str().into(),
                            identity: root_identity.to_bytes(),
                        }],
                        push_endpoint: None,
                        paired_at: WallMs::from_millis(1_767_225_600_000),
                    },
                )
                .expect("persist the approved resilience phone");
            composed
                .reload_device_authority()
                .expect("restore the approved resilience phone");
            composed.pc_identity = Some(Arc::clone(&pc));

            let address = composed.home.paths().endpoint().address().to_owned();
            let listener = Listener::bind(&address)
                .await
                .expect("the endpoint is free");
            let tcp = TcpListener::bind("127.0.0.1:0")
                .await
                .expect("phone listener");
            let ingress =
                PhoneIngress::loopback(tcp, ["https://phone.runtrol.test"]).expect("phone ingress");
            let phone_address = ingress.local_addr().expect("phone address");
            let phone_plane = phone_plane(&composed, ingress).expect("phone plane");
            let serving = tokio::spawn(async move {
                serve_surfaces(
                    composed,
                    listener,
                    SessionManager::new(),
                    Some(phone_plane),
                    None,
                )
                .await
            });
            (
                Self {
                    address,
                    home: home.clone(),
                    serving,
                },
                LivePhoneRestart {
                    home,
                    pc,
                    phone_address,
                },
            )
        }

        async fn restart_resilient_live_phone(seed: &LivePhoneRestart) -> Self {
            let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
            let mut composed = loop {
                match crate::compose::Composed::for_tests(&seed.home, runtrol_drivers::builtin()) {
                    Ok(composed) => break composed,
                    Err(crate::compose::ComposeError::Store(StoreError::AlreadyOpen {
                        ..
                    })) if tokio::time::Instant::now() < deadline => {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                    Err(error) => panic!("the resilience home does not recompose: {error}"),
                }
            };
            composed.pc_identity = Some(Arc::clone(&seed.pc));
            assert_eq!(
                composed.device_authority.paired_devices().len(),
                1,
                "the paired phone must restore from durable state"
            );
            let address = composed.home.paths().endpoint().address().to_owned();
            let listener = Listener::bind(&address)
                .await
                .expect("the restarted endpoint is free");
            let tcp = TcpListener::bind(seed.phone_address)
                .await
                .expect("the restarted phone listener is free");
            let ingress =
                PhoneIngress::loopback(tcp, ["https://phone.runtrol.test"]).expect("phone ingress");
            let phone_plane = phone_plane(&composed, ingress).expect("phone plane");
            let serving = tokio::spawn(async move {
                serve_surfaces(
                    composed,
                    listener,
                    SessionManager::new(),
                    Some(phone_plane),
                    None,
                )
                .await
            });
            Self {
                address,
                home: seed.home.clone(),
                serving,
            }
        }

        async fn crash_preserving(self) {
            self.serving.abort();
            let stopped = self.serving.await;
            assert!(stopped.is_err_and(|error| error.is_cancelled()));
        }

        async fn caller(&self) -> Connection {
            runtrol_ipc::transport::connect(&self.address)
                .await
                .expect("the daemon is listening")
        }

        fn stop(self) {
            self.serving.abort();
            drop(std::fs::remove_dir_all(&self.home));
        }

        async fn close_live_sessions(&self) {
            let mut caller = self.caller().await;
            assert!(matches!(
                ask(
                    &mut caller,
                    &Request::Hello {
                        wire: runtrol_ipc::WIRE_VERSION,
                    },
                )
                .await,
                Response::Welcome { .. }
            ));
            let Response::Sessions(listing) = ask(&mut caller, &Request::List).await else {
                panic!("the live cleanup did not receive a session listing");
            };
            for line in listing.sessions {
                assert!(matches!(
                    ask(
                        &mut caller,
                        &Request::Close {
                            session: line.session,
                            now: true,
                        },
                    )
                    .await,
                    Response::Done
                ));
            }
        }
    }

    #[derive(serde::Deserialize)]
    struct LivePhoneIdentity {
        phone_public: Box<str>,
    }

    #[derive(Debug, serde::Deserialize)]
    struct LivePhoneEvidence {
        facts: BTreeSet<Box<str>>,
    }

    #[derive(serde::Deserialize)]
    struct LivePhoneControl {
        control: Box<str>,
    }

    type BrowserSocket = WebSocket<TokioIo<Upgraded>>;

    struct TokioExecutor;

    impl<F> hyper::rt::Executor<F> for TokioExecutor
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        fn execute(&self, future: F) {
            tokio::spawn(future);
        }
    }

    struct BrowserPhone {
        socket: BrowserSocket,
        channel: Channel,
    }

    impl BrowserPhone {
        async fn connect(address: SocketAddr, pc: PublicKey, phone: &StaticKeypair) -> Self {
            let mut socket = browser_socket(address).await;
            let binding = SessionBinding::direct(LinkKind::Loopback, pc.to_bytes())
                .expect("phone link binding");
            let mut initiator =
                InitiatorHandshake::session(phone, pc, &binding).expect("phone Noise initiator");
            let first = initiator.write_first(&[]).expect("Noise message one");
            write_browser_record(&mut socket, &first).await;
            let reply = read_browser_record(&mut socket).await;
            let (channel, payload) = initiator.finish(&reply).expect("Noise message two");
            assert!(payload.is_empty());
            Self { socket, channel }
        }

        async fn ask(&mut self, request: &Request) -> Response {
            let payload = serde_json::to_vec(request).expect("phone request encoding");
            for record in self
                .channel
                .seal_frame(&payload)
                .expect("phone request frame")
            {
                write_browser_record(&mut self.socket, &record).await;
            }
            self.receive().await
        }

        async fn receive(&mut self) -> Response {
            let payload = loop {
                let record = read_browser_record(&mut self.socket).await;
                if let Some(frame) = self
                    .channel
                    .open_record(&record)
                    .expect("phone answer record")
                {
                    break frame;
                }
            };
            serde_json::from_slice(&payload).expect("phone answer encoding")
        }
    }

    async fn browser_socket(address: SocketAddr) -> BrowserSocket {
        const ORIGIN: &str = "https://phone.runtrol.test";
        let stream = TcpStream::connect(address).await.expect("phone TCP");
        let request = HttpRequest::builder()
            .method("GET")
            .uri(format!("ws://{address}{NOISE_LINK_PATH}"))
            .header("Host", address.to_string())
            .header("Origin", ORIGIN)
            .header("Sec-Fetch-Site", "same-origin")
            .header("Upgrade", "websocket")
            .header("Connection", "Upgrade")
            .header("Sec-WebSocket-Key", handshake::generate_key())
            .header("Sec-WebSocket-Version", "13")
            .header("Sec-WebSocket-Protocol", NOISE_LINK_PROTOCOL)
            .body(Empty::<bytes::Bytes>::new())
            .expect("phone upgrade request");
        let (mut socket, switched) = handshake::client(&TokioExecutor, request, stream)
            .await
            .expect("phone WebSocket");
        assert_eq!(
            switched.status(),
            runtrol_transport::StatusCode::SWITCHING_PROTOCOLS
        );
        assert_eq!(
            switched
                .headers()
                .get("Sec-WebSocket-Protocol")
                .map(|value| value.to_str().expect("ASCII subprotocol")),
            Some(NOISE_LINK_PROTOCOL)
        );
        socket.set_max_message_size(MAX_ENCRYPTED_RECORD_WIRE);
        socket
    }

    async fn write_browser_record(socket: &mut BrowserSocket, record: &EncryptedRecord) {
        let mut encoded = Vec::new();
        record
            .append_wire(&mut encoded)
            .expect("canonical Noise record");
        socket
            .write_frame(Frame::binary(encoded.into()))
            .await
            .expect("browser record write");
    }

    async fn read_browser_record(socket: &mut BrowserSocket) -> EncryptedRecord {
        loop {
            let frame = socket.read_frame().await.expect("browser record read");
            match frame.opcode {
                OpCode::Binary => {
                    let (record, consumed) = EncryptedRecord::decode_wire(&frame.payload)
                        .expect("Noise record envelope");
                    assert_eq!(consumed, frame.payload.len());
                    return record;
                }
                OpCode::Ping | OpCode::Pong => {}
                OpCode::Close => panic!("phone link closed before its answer"),
                OpCode::Text | OpCode::Continuation => {
                    panic!("phone link returned a non-binary message")
                }
            }
        }
    }

    /// Ask, and read the answer.
    async fn ask(connection: &mut Connection, request: &Request) -> Response {
        let frame = serde_json::to_vec(request).expect("writable");
        connection.send(&frame).await.expect("the daemon is there");
        let answer = connection
            .recv()
            .await
            .expect("the connection holds")
            .expect("every request produces an answer");
        serde_json::from_slice(&answer).expect("the answer is readable")
    }

    async fn receive(connection: &mut Connection) -> Response {
        let answer = connection
            .recv()
            .await
            .expect("the connection holds")
            .expect("the watch remains open");
        serde_json::from_slice(&answer).expect("the event answer is readable")
    }

    async fn greeted_caller(running: &Running) -> Connection {
        let mut caller = running.caller().await;
        assert!(matches!(
            ask(
                &mut caller,
                &Request::Hello {
                    wire: runtrol_ipc::WIRE_VERSION,
                },
            )
            .await,
            Response::Welcome { .. }
        ));
        caller
    }

    fn live_phone_public_key(identity_line: &str) -> PublicKey {
        let identity: LivePhoneIdentity =
            serde_json::from_str(identity_line).expect("phone identity contract");
        let decoded = Base64UrlUnpadded::decode_vec(&identity.phone_public)
            .expect("phone public key is canonical base64url");
        let remote_bytes: [u8; 32] = decoded
            .try_into()
            .expect("phone public key has the X25519 length");
        PublicKey::from_bytes(remote_bytes)
    }

    async fn start_live_phone_runtime(
        mode: &str,
        phone: PublicKey,
        workspace: &str,
        provider: ProviderId,
    ) -> (Running, SocketAddr, PublicKey, Option<LivePhoneRestart>) {
        if mode == "resilience" {
            let (running, restart) =
                Running::start_resilient_live_phone(phone, workspace, provider).await;
            let address = restart.phone_address;
            let public = restart.pc.public_key();
            (running, address, public, Some(restart))
        } else {
            let (running, address, public) =
                Running::start_with_live_phone(&format!("live-{mode}"), phone, workspace, provider)
                    .await;
            (running, address, public, None)
        }
    }

    fn live_phone_config(
        phone_address: SocketAddr,
        pc_public: PublicKey,
        workspace: &str,
        provider: &str,
    ) -> Vec<u8> {
        let config = serde_json::json!({
            "address": phone_address.to_string(),
            "pc_public": Base64UrlUnpadded::encode_string(&pc_public.to_bytes()),
            "workspace": workspace,
            "provider": provider,
        });
        format!("{config}\n").into_bytes()
    }

    fn live_phone_enabled(mode: &str) -> bool {
        matches!(std::env::var("RUNTROL_PHONE_LIVE_MODE"), Ok(enabled) if enabled == mode)
    }

    fn live_phone_inputs() -> (String, String, ProviderId, std::ffi::OsString, PathBuf) {
        let workspace = std::env::var("RUNTROL_PHONE_LIVE_WORKSPACE")
            .expect("the live gate supplies its isolated workspace");
        let provider_text = std::env::var("RUNTROL_PHONE_LIVE_PROVIDER")
            .expect("the live gate supplies its discovered provider");
        let provider =
            ProviderId::parse(&provider_text).expect("the live provider identity is valid");
        let node = std::env::var_os("RUNTROL_PHONE_LIVE_NODE").unwrap_or_else(|| "node".into());
        let script =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../pwa/test/live-phone.mjs");
        (workspace, provider_text, provider, node, script)
    }

    async fn live_phone_journey(mode: &str) {
        if !live_phone_enabled(mode) {
            return;
        }
        let (workspace, provider_text, provider, node, script) = live_phone_inputs();
        let mut child = tokio::process::Command::new(node)
            .arg(script)
            .arg(mode)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .expect("start the headless phone process");
        let stdout = child.stdout.take().expect("phone stdout");
        let mut lines = tokio::io::AsyncBufReadExt::lines(tokio::io::BufReader::new(stdout));
        let stderr = child.stderr.take().expect("phone stderr");
        let stderr_task = tokio::spawn(async move {
            let mut bytes = Vec::new();
            let mut reader = tokio::io::BufReader::new(stderr);
            tokio::io::AsyncReadExt::read_to_end(&mut reader, &mut bytes)
                .await
                .expect("read phone diagnostics");
            String::from_utf8_lossy(&bytes).into_owned()
        });
        let identity_line = tokio::time::timeout(Duration::from_secs(10), lines.next_line())
            .await
            .expect("phone identity timed out")
            .expect("read phone identity")
            .expect("phone exited before its identity");
        let (mut running, phone_address, pc_public, restart) = start_live_phone_runtime(
            mode,
            live_phone_public_key(&identity_line),
            &workspace,
            provider,
        )
        .await;
        let mut stdin = child.stdin.take().expect("phone stdin");
        tokio::io::AsyncWriteExt::write_all(
            &mut stdin,
            &live_phone_config(phone_address, pc_public, &workspace, &provider_text),
        )
        .await
        .expect("send live phone config");
        if let Some(restart) = restart.as_ref() {
            let control_line = tokio::time::timeout(Duration::from_mins(1), lines.next_line())
                .await
                .expect("phone restart request timed out")
                .expect("read phone restart request")
                .expect("phone exited before requesting restart");
            let control: LivePhoneControl =
                serde_json::from_str(&control_line).expect("phone restart request contract");
            assert_eq!(control.control.as_ref(), "restart");
            running.crash_preserving().await;
            running = Running::restart_resilient_live_phone(restart).await;
        }
        tokio::io::AsyncWriteExt::shutdown(&mut stdin)
            .await
            .expect("close live phone config");

        let evidence_line = tokio::time::timeout(Duration::from_secs(90), lines.next_line()).await;
        if evidence_line.is_err() {
            child.kill().await.expect("stop timed-out phone process");
        }
        let waited = tokio::time::timeout(Duration::from_secs(10), child.wait()).await;
        let exit_timed_out = waited.is_err();
        let status = if let Ok(status) = waited {
            status.expect("wait for phone process")
        } else {
            child
                .kill()
                .await
                .expect("stop phone process after exit timeout");
            child
                .wait()
                .await
                .expect("reap phone process after exit timeout")
        };
        let diagnostics = stderr_task.await.expect("phone diagnostics task");
        running.close_live_sessions().await;
        running.stop();

        assert!(
            !exit_timed_out,
            "headless phone did not exit: {diagnostics}"
        );
        assert!(status.success(), "headless phone failed: {diagnostics}");
        let evidence_line = evidence_line
            .expect("phone evidence timed out")
            .expect("read phone evidence")
            .expect("phone exited before its evidence");
        let evidence: LivePhoneEvidence =
            serde_json::from_str(&evidence_line).expect("phone evidence contract");
        assert_live_phone_evidence(&evidence, mode);
    }

    fn assert_live_phone_evidence(evidence: &LivePhoneEvidence, mode: &str) {
        let common = [
            "started",
            "prompted",
            "output_seen",
            "provider_ended",
            "close_confirmed",
        ];
        for fact in common {
            assert!(evidence.facts.contains(fact), "{evidence:?}");
        }
        if mode == "approval" {
            for fact in [
                "approval_seen",
                "attention_listed",
                "subject_complete",
                "reject_once",
                "answered",
                "attention_cleared",
            ] {
                assert!(evidence.facts.contains(fact), "{evidence:?}");
            }
        }
        if mode == "resilience" {
            for fact in [
                "network_cut",
                "exact_replay",
                "no_duplicate",
                "reconnected",
                "native_preserved",
                "explicit_gap",
                "resumed_turn",
            ] {
                assert!(evidence.facts.contains(fact), "{evidence:?}");
            }
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn phone_drives_pc_through_a_real_cli() {
        live_phone_journey("drive").await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn phone_approval_resumes_a_real_cli() {
        live_phone_journey("approval").await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn phone_survives_network_and_core_restart() {
        live_phone_journey("resilience").await;
    }

    #[tokio::test]
    async fn a_stuck_model_provider_releases_global_preparation() {
        struct DropSignal(Option<oneshot::Sender<()>>);

        impl Drop for DropSignal {
            fn drop(&mut self) {
                if let Some(dropped) = self.0.take() {
                    let _dropped = dropped.send(());
                }
            }
        }

        let gate = Arc::new(tokio::sync::Mutex::new(()));
        let first_gate = Arc::clone(&gate);
        let (held, holding) = oneshot::channel();
        let (dropped, dropping) = oneshot::channel();
        let first = tokio::spawn(async move {
            let _guard = first_gate.lock().await;
            held.send(()).expect("the test observes the held gate");
            let preparing = async move {
                let _cleanup = DropSignal(Some(dropped));
                core::future::pending::<Prepared>().await
            };
            finish_model_preparation("fixture", preparing, Duration::from_millis(25)).await
        });
        holding.await.expect("the first preparation holds the gate");

        let second_gate = Arc::clone(&gate);
        let second = tokio::spawn(async move {
            let _guard = second_gate.lock().await;
        });

        let prepared = first.await.expect("the bounded preparation task finishes");
        match prepared {
            Prepared::Invalid {
                kind: PreparedKind::Models,
                provider,
                response: Response::Failed(error),
            } => {
                assert_eq!(&*provider, "fixture");
                assert!(error.retryable);
                assert!(!error.needs_the_operator);
                assert!(error.message.contains("did not finish"));
            }
            _ => panic!("the stuck model preparation must become a bound refusal"),
        }
        dropping
            .await
            .expect("timeout drops provider preparation before releasing its gate");
        tokio::time::timeout(Duration::from_secs(1), second)
            .await
            .expect("the next preparation acquired the released gate")
            .expect("the next preparation task finishes");
    }

    #[tokio::test]
    async fn a_request_over_a_real_endpoint_is_answered() {
        // Not a unit of the loop: an actual listener, an actual connection, actual frames. Everything below is the
        // same daemon an operator reaches.
        let running = Running::start("answered").await;
        let mut caller = running.caller().await;

        let welcome = ask(
            &mut caller,
            &Request::Hello {
                wire: runtrol_ipc::WIRE_VERSION,
            },
        )
        .await;
        assert!(matches!(welcome, Response::Welcome { .. }), "{welcome:?}");

        match ask(&mut caller, &Request::List).await {
            Response::Sessions(listing) => {
                assert!(listing.sessions.is_empty(), "nothing has been started");
                assert!(listing.warnings.is_empty(), "a fresh store has no warnings");
            }
            other => panic!("expected a listing, got {other:?}"),
        }
        running.stop();
    }

    #[tokio::test]
    async fn a_paired_phone_and_local_surface_share_one_session_owner() {
        let session = SessionId::now();
        let mut sessions = SessionManager::new();
        attach_test_agent(
            &mut sessions,
            session,
            Box::new(PendingSend {
                session,
                started: None,
                release: None,
                panic_after_start: false,
            }),
        );
        let (running, phone_address, pc, phone_key) =
            Running::start_with_phone("phone-same-owner", sessions, None).await;

        let stranger = StaticKeypair::generate().expect("unpaired phone key");
        let binding = SessionBinding::direct(LinkKind::Loopback, pc.to_bytes())
            .expect("unpaired link binding");
        let mut stranger_handshake =
            InitiatorHandshake::session(&stranger, pc, &binding).expect("unpaired Noise initiator");
        let first = stranger_handshake
            .write_first(&[])
            .expect("unpaired message one");
        let mut stranger_socket = browser_socket(phone_address).await;
        write_browser_record(&mut stranger_socket, &first).await;
        let closed = tokio::time::timeout(Duration::from_secs(1), stranger_socket.read_frame())
            .await
            .expect("unpaired identity is closed without a handshake reply");
        if let Ok(frame) = closed {
            assert_ne!(frame.opcode, OpCode::Binary);
        }
        drop(stranger_socket);

        let mut local = greeted_caller(&running).await;
        let local_session = match ask(&mut local, &Request::List).await {
            Response::Sessions(listing) => listing.sessions.first().map(|line| line.session),
            other => panic!("expected a local session index, got {other:?}"),
        };
        assert_eq!(local_session, Some(session));

        let mut phone = BrowserPhone::connect(phone_address, pc, &phone_key).await;
        assert!(matches!(
            phone
                .ask(&Request::Hello {
                    wire: runtrol_ipc::WIRE_VERSION,
                })
                .await,
            Response::Welcome { .. }
        ));
        // The phone holds session.list and not one workspace root, so the same daemon that just showed the
        // local surface a session answers the phone with nothing: which projects exist on this machine is not
        // something the listing scope alone may learn.
        match phone.ask(&Request::List).await {
            Response::Sessions(listing) => {
                assert!(
                    listing.sessions.is_empty(),
                    "a phone with no workspace root must not see the machine's sessions"
                );
                assert!(
                    listing.warnings.is_empty(),
                    "storage warnings are operator information"
                );
            }
            other => panic!("expected a phone session index, got {other:?}"),
        }
        match phone.ask(&Request::Close { session, now: true }).await {
            Response::Failed(error) => assert!(error.message.contains("session.delete")),
            other => panic!("ungranted phone close was not refused: {other:?}"),
        }

        let mut watcher = BrowserPhone::connect(phone_address, pc, &phone_key).await;
        assert!(matches!(
            watcher
                .ask(&Request::Hello {
                    wire: runtrol_ipc::WIRE_VERSION,
                })
                .await,
            Response::Welcome { .. }
        ));
        assert!(matches!(
            watcher.ask(&Request::WatchSessions).await,
            Response::WatchingSessions
        ));
        match watcher.receive().await {
            Response::Sessions(listing) => {
                assert!(
                    listing.sessions.is_empty(),
                    "the watch snapshot is projected exactly like the listing"
                );
            }
            other => panic!("expected the initial phone watch snapshot, got {other:?}"),
        }

        assert!(matches!(
            ask(&mut local, &Request::Close { session, now: true }).await,
            Response::Done
        ));
        // The close changed the machine's index but not this phone's empty view, so no frame is resent: an
        // ungranted phone cannot even observe that something changed.
        let silence = tokio::time::timeout(Duration::from_millis(500), watcher.receive()).await;
        assert!(
            silence.is_err(),
            "an unchanged projected view must not produce a frame"
        );

        drop(phone);
        drop(watcher);
        drop(local);
        running.stop();
    }

    #[tokio::test]
    async fn a_phone_watches_exactly_the_sessions_of_its_granted_root() {
        let granted_dir =
            std::env::temp_dir().join(format!("runtrol-phone-granted-{}", std::process::id()));
        if granted_dir.exists() {
            std::fs::remove_dir_all(&granted_dir).expect("clear the previous granted root");
        }
        std::fs::create_dir(&granted_dir).expect("create the granted root");
        // One canonical spelling for the grant and the session workspace alike, so the test never
        // depends on how the OS spells its temporary directory.
        let granted_text =
            AbsPath::canonicalize(granted_dir.to_str().expect("the granted path is UTF-8"))
                .expect("the granted root canonicalizes")
                .as_str()
                .to_owned();

        let visible = SessionId::now();
        let hidden = SessionId::now();
        let mut sessions = SessionManager::new();
        attach_test_agent_in(
            &mut sessions,
            visible,
            Box::new(ReadyEvent {
                session: visible,
                ready: true,
            }),
            &granted_text,
        );
        attach_test_agent(
            &mut sessions,
            hidden,
            Box::new(ReadyEvent {
                session: hidden,
                ready: true,
            }),
        );
        let (running, phone_address, pc, phone_key) =
            Running::start_with_phone("phone-granted-root", sessions, Some(&granted_text)).await;

        let mut phone = BrowserPhone::connect(phone_address, pc, &phone_key).await;
        assert!(matches!(
            phone
                .ask(&Request::Hello {
                    wire: runtrol_ipc::WIRE_VERSION,
                })
                .await,
            Response::Welcome { .. }
        ));
        match phone.ask(&Request::List).await {
            Response::Sessions(listing) => {
                let seen: Vec<_> = listing.sessions.iter().map(|line| line.session).collect();
                assert_eq!(
                    seen,
                    vec![visible],
                    "exactly the granted root's session, and never its neighbour"
                );
            }
            other => panic!("expected the granted phone listing, got {other:?}"),
        }

        assert!(matches!(
            phone.ask(&Request::WatchSessions).await,
            Response::WatchingSessions
        ));
        match phone.receive().await {
            Response::Sessions(listing) => {
                assert_eq!(
                    listing.sessions.first().map(|line| line.session),
                    Some(visible)
                );
            }
            other => panic!("expected the granted watch snapshot, got {other:?}"),
        }

        // Closing the granted session changes what this phone may see, so the watch pushes the shrunk view.
        let mut local = greeted_caller(&running).await;
        assert!(matches!(
            ask(
                &mut local,
                &Request::Close {
                    session: visible,
                    now: true
                }
            )
            .await,
            Response::Done
        ));
        match phone.receive().await {
            Response::Sessions(listing) => {
                assert!(
                    listing.sessions.is_empty(),
                    "the hidden neighbour must not appear once the granted session is gone"
                );
            }
            other => panic!("expected the shrunk granted watch snapshot, got {other:?}"),
        }

        drop(phone);
        drop(local);
        running.stop();
        std::fs::remove_dir_all(&granted_dir).expect("remove the granted root");
    }

    #[tokio::test]
    async fn a_session_index_watch_pushes_only_the_new_current_snapshot() {
        let session = SessionId::now();
        let mut sessions = SessionManager::new();
        attach_test_agent(
            &mut sessions,
            session,
            Box::new(PendingSend {
                session,
                started: None,
                release: None,
                panic_after_start: false,
            }),
        );
        let running = Running::start_with_sessions("session-index", sessions).await;
        let mut watcher = greeted_caller(&running).await;

        assert!(matches!(
            ask(&mut watcher, &Request::WatchSessions).await,
            Response::WatchingSessions
        ));
        match receive(&mut watcher).await {
            Response::Sessions(listing) => {
                assert_eq!(listing.sessions.len(), 1);
                assert_eq!(
                    listing.sessions.first().map(|line| line.session),
                    Some(session)
                );
            }
            other => panic!("expected the current session index, got {other:?}"),
        }

        for _ in 0..40 {
            let mut refresh = greeted_caller(&running).await;
            assert!(matches!(
                ask(&mut refresh, &Request::List).await,
                Response::Sessions(_)
            ));
        }

        let mut control = greeted_caller(&running).await;
        assert!(matches!(
            ask(&mut control, &Request::Close { session, now: true }).await,
            Response::Done
        ));
        match receive(&mut watcher).await {
            Response::Sessions(listing) => assert!(listing.sessions.is_empty()),
            other => panic!("expected the changed session index, got {other:?}"),
        }

        running.stop();
    }

    #[tokio::test]
    async fn a_real_endpoint_acknowledges_replays_and_resumes_at_the_exact_cursor() {
        let session = SessionId::now();
        let mut sessions = SessionManager::new();
        attach_test_agent(
            &mut sessions,
            session,
            Box::new(ReadyEvent {
                session,
                ready: true,
            }),
        );
        let running = Running::start_with_sessions("watch-cursor", sessions).await;

        let mut first = greeted_caller(&running).await;
        let start = ask(
            &mut first,
            &Request::Watch {
                session,
                after: None,
            },
        )
        .await;
        let starts_at = match start {
            Response::Watching {
                starts_at,
                gap: None,
                ..
            } => starts_at,
            other => panic!("expected a watch acknowledgement, got {other:?}"),
        };
        let next_expected = match receive(&mut first).await {
            Response::Event {
                payload,
                next_expected,
            } => {
                assert!(payload.as_str().contains("\"seq\":0"));
                assert_eq!(starts_at.seq, 0);
                next_expected
            }
            other => panic!("expected one replayed event, got {other:?}"),
        };
        drop(first);

        let mut exact = greeted_caller(&running).await;
        match ask(
            &mut exact,
            &Request::Watch {
                session,
                after: Some(next_expected),
            },
        )
        .await
        {
            Response::Watching {
                starts_at,
                live_at,
                gap: None,
            } => {
                assert_eq!(starts_at, next_expected);
                assert_eq!(live_at, next_expected);
            }
            other => panic!("expected an exact reconnect acknowledgement, got {other:?}"),
        }
        assert!(
            tokio::time::timeout(Duration::from_millis(50), exact.recv())
                .await
                .is_err(),
            "an exact reconnect must not duplicate the replayed event"
        );
        drop(exact);

        let mut mismatched = greeted_caller(&running).await;
        let wrong_stream = runtrol_provider::WatchCursor {
            stream: runtrol_provider::StreamId::now(),
            ..next_expected
        };
        match ask(
            &mut mismatched,
            &Request::Watch {
                session,
                after: Some(wrong_stream),
            },
        )
        .await
        {
            Response::Watching {
                starts_at,
                live_at,
                gap: Some(gap),
            } => {
                // The gap names the unreachable request, and delivery still resumes at the retained
                // suffix rather than skipping to live: the ring holds the session's one event, so
                // the acknowledgement starts there and the event itself follows.
                assert_eq!(gap.requested, wrong_stream);
                assert_eq!(gap.live_at, starts_at);
                assert_eq!(starts_at.seq, 0);
                assert_eq!(live_at.seq, 1);
            }
            other => panic!("expected a visible stream gap, got {other:?}"),
        }
        match receive(&mut mismatched).await {
            Response::Event { payload, .. } => {
                assert!(payload.as_str().contains("\"seq\":0"));
            }
            other => panic!("expected the retained suffix to replay after the gap, got {other:?}"),
        }

        running.stop();
    }

    #[tokio::test]
    async fn the_greeting_is_enforced_on_the_wire_and_not_only_in_the_dispatcher() {
        // The rule exists in the dispatcher, and this checks it survives the trip: a connection is refused because
        // of what it has not done, not because of anything this file remembered about it.
        let running = Running::start("ungreeted").await;
        let mut caller = running.caller().await;

        match ask(&mut caller, &Request::List).await {
            Response::Failed(failure) => assert!(
                failure.message.contains("wire format"),
                "{}",
                failure.message
            ),
            other => panic!("expected a refusal, got {other:?}"),
        }
        running.stop();
    }

    #[tokio::test]
    async fn every_connection_greets_for_itself() {
        // A second place a connection's state could live is a second place it could be wrong. One connection having
        // greeted must say nothing about another, or a caller inherits permission it never asked for.
        let running = Running::start("separate").await;
        let mut greeted = running.caller().await;
        drop(
            ask(
                &mut greeted,
                &Request::Hello {
                    wire: runtrol_ipc::WIRE_VERSION,
                },
            )
            .await,
        );

        let mut fresh = running.caller().await;
        match ask(&mut fresh, &Request::List).await {
            Response::Failed(failure) => assert!(
                failure.message.contains("wire format"),
                "{}",
                failure.message
            ),
            other => panic!("a fresh connection inherited a greeting: {other:?}"),
        }
        running.stop();
    }

    #[tokio::test]
    async fn a_frame_that_is_not_a_request_is_refused_and_the_connection_survives() {
        // One bad request is not a broken connection. Closing on it would make a caller's typo look like the daemon
        // dying, and the next request is usually fine.
        let running = Running::start("garbage").await;
        let mut caller = running.caller().await;

        caller
            .send(b"this is not a request")
            .await
            .expect("the daemon is there");
        let answer = caller
            .recv()
            .await
            .expect("the connection holds")
            .expect("even nonsense is answered");
        let read: Response = serde_json::from_slice(&answer).expect("the answer is readable");
        assert!(matches!(read, Response::Failed(_)), "{read:?}");

        let welcome = ask(
            &mut caller,
            &Request::Hello {
                wire: runtrol_ipc::WIRE_VERSION,
            },
        )
        .await;
        assert!(
            matches!(welcome, Response::Welcome { .. }),
            "the connection had to survive one unreadable frame: {welcome:?}"
        );
        running.stop();
    }

    #[tokio::test]
    async fn one_caller_waiting_does_not_stop_another_from_being_answered() {
        // Several connections at once is the ordinary case (a terminal watching, a phone listing), and the whole
        // arrangement of this file is for it. A daemon that answered one connection at a time would be a daemon
        // that freezes whenever anything is slow.
        let running = Running::start("concurrent").await;
        let mut first = running.caller().await;
        let mut second = running.caller().await;

        for caller in [&mut first, &mut second] {
            let welcome = ask(
                caller,
                &Request::Hello {
                    wire: runtrol_ipc::WIRE_VERSION,
                },
            )
            .await;
            assert!(matches!(welcome, Response::Welcome { .. }));
        }

        // Interleaved on purpose: each answer has to come back on the connection that asked for it.
        assert!(matches!(
            ask(&mut second, &Request::List).await,
            Response::Sessions(_)
        ));
        assert!(matches!(
            ask(&mut first, &Request::List).await,
            Response::Sessions(_)
        ));
        running.stop();
    }

    #[tokio::test]
    async fn a_quiet_watch_releases_as_soon_as_its_peer_disconnects() {
        let root = std::env::temp_dir().join(format!("runtrol-quiet-watch-{}", SessionId::now()));
        let home = root.to_str().expect("the temporary path is UTF-8");
        let composed = crate::compose::Composed::for_tests(home, runtrol_drivers::builtin())
            .expect("a fresh home composes");
        let address = composed.home.paths().endpoint().address().to_owned();
        let mut listener = Listener::bind(&address)
            .await
            .expect("the isolated watch endpoint is free");
        let accepting =
            tokio::spawn(
                async move { listener.accept().await.expect("the watch peer is accepted") },
            );
        let peer = runtrol_ipc::connect(&address)
            .await
            .expect("the watch peer connects");
        let connection = accepting.await.expect("the accept task completes");

        let mut hub = runtrol_core::SessionHub::new(SessionId::now());
        let view = hub.view(None);
        assert_eq!(hub.watchers(), 1);
        let relaying = tokio::spawn(async move {
            relay(&mut SurfaceConnection::Local(connection), view).await;
        });
        drop(peer);
        tokio::time::timeout(Duration::from_secs(2), relaying)
            .await
            .expect("the disconnected quiet watch is released")
            .expect("the relay task does not panic");

        drop(hub.publish(
            1,
            runtrol_provider::EventBody::Plan {
                payload: Opaque::none(),
            },
        ));
        assert_eq!(hub.watchers(), 0);
        drop(composed);
        std::fs::remove_dir_all(root).expect("the isolated watch home is removed");
    }

    #[tokio::test]
    async fn a_pending_provider_write_does_not_block_another_event_or_owner_request() {
        let mut sessions = SessionManager::new();
        let command_session = SessionId::now();
        let event_session = SessionId::now();
        let (started, starting) = oneshot::channel();
        let (release, releasing) = oneshot::channel();
        attach_test_agent(
            &mut sessions,
            command_session,
            Box::new(PendingSend {
                session: command_session,
                started: Some(started),
                release: Some(releasing),
                panic_after_start: false,
            }),
        );
        attach_test_agent(
            &mut sessions,
            event_session,
            Box::new(ReadyEvent {
                session: event_session,
                ready: true,
            }),
        );

        let taken = sessions
            .take_agent(command_session)
            .expect("the command is handed to its connection");
        let (returning, mut returned) = mpsc::unbounded_channel();
        let command = tokio::spawn(perform_agent_command(
            taken,
            AgentCommand::Interrupt,
            returning,
        ));
        tokio::time::timeout(core::time::Duration::from_secs(2), starting)
            .await
            .expect("provider command start did not time out")
            .expect("provider command started");

        assert!(matches!(
            sessions.take_agent(command_session),
            Err(SessionError::AgentInFlight { session }) if session == command_session
        ));
        let pumped = tokio::time::timeout(
            core::time::Duration::from_secs(2),
            sessions.pump_once(event_session),
        )
        .await
        .expect("another event pump did not time out")
        .expect("the other session remains live");
        assert!(pumped.is_some(), "the other session's event was published");

        let mut reservations = Vec::new();
        for _ in 0..runtrol_core::session::MAX_HOT - 2 {
            reservations.push(
                sessions
                    .reserve_open_for_tests(SessionId::now())
                    .expect("an unrelated owner request progresses"),
            );
        }
        assert!(matches!(
            sessions.reserve_open_for_tests(SessionId::now()),
            Err(SessionError::OpeningCapacityReserved)
        ));

        release.send(()).expect("the provider command may finish");
        let returned_agent =
            tokio::time::timeout(core::time::Duration::from_secs(2), returned.recv())
                .await
                .expect("provider return did not time out")
                .expect("the provider returned its agent");
        let AgentReturned::Finished {
            lease,
            agent,
            outcome,
            answered,
        } = returned_agent
        else {
            panic!("a completed provider command was expected");
        };
        assert!(outcome.is_ok());
        assert!(
            sessions.return_agent(lease, agent).is_ok(),
            "the owner restores the exact agent"
        );
        answered
            .send(Response::Done)
            .expect("the worker is waiting");
        assert!(matches!(
            tokio::time::timeout(core::time::Duration::from_secs(2), command)
                .await
                .expect("command completion did not time out")
                .expect("command task completed"),
            Some(Response::Done)
        ));

        for reserved in reservations {
            sessions.cancel_open(reserved.reservation);
        }
    }

    #[tokio::test]
    async fn a_cancelled_or_panicking_provider_command_never_reattaches_its_agent() {
        for panic_after_start in [false, true] {
            let mut sessions = SessionManager::new();
            let session = SessionId::now();
            let (started, starting) = oneshot::channel();
            let (_release, releasing) = oneshot::channel();
            attach_test_agent(
                &mut sessions,
                session,
                Box::new(PendingSend {
                    session,
                    started: Some(started),
                    release: Some(releasing),
                    panic_after_start,
                }),
            );
            let taken = sessions
                .take_agent(session)
                .expect("the agent is handed out");
            let (returning, mut returned) = mpsc::unbounded_channel();
            let command = tokio::spawn(perform_agent_command(
                taken,
                AgentCommand::Interrupt,
                returning,
            ));
            tokio::time::timeout(core::time::Duration::from_secs(2), starting)
                .await
                .expect("provider command start did not time out")
                .expect("provider command started");
            let joined = if panic_after_start {
                tokio::time::timeout(core::time::Duration::from_secs(2), command)
                    .await
                    .expect("panicking command join did not time out")
            } else {
                command.abort();
                tokio::time::timeout(core::time::Duration::from_secs(2), command)
                    .await
                    .expect("cancelled command join did not time out")
            };
            if panic_after_start {
                assert!(joined.expect_err("the command panics").is_panic());
            } else {
                assert!(joined.expect_err("the command is cancelled").is_cancelled());
            }

            let abandoned =
                tokio::time::timeout(core::time::Duration::from_secs(2), returned.recv())
                    .await
                    .expect("abandoned handoff did not time out")
                    .expect("the guard reports its abandoned lease");
            let AgentReturned::Abandoned(lease) = abandoned else {
                panic!("an abandoned provider command was expected");
            };
            sessions.abandon_agent(lease);
            assert!(!sessions.is_live(session));
            assert!(
                sessions.reserve_open_for_tests(SessionId::now()).is_ok(),
                "cleanup returns the process slot"
            );
        }
    }

    #[test]
    fn abandoning_connection_preparation_requests_reservation_cancellation() {
        let mut sessions = SessionManager::new();
        let reserved = sessions
            .reserve_open_for_tests(SessionId::now())
            .expect("one bounded slot");
        let (cancelling, mut cancelled) = mpsc::unbounded_channel();
        drop(ReservationGuard {
            reservation: Some(CleanupReservation::Open(reserved.reservation)),
            cancelling,
        });

        let ReservationAsked::CancelOpen(reservation) = cancelled
            .try_recv()
            .expect("dropping preparation reports its reservation")
        else {
            panic!("a cancellation message was expected");
        };
        sessions.cancel_open(reservation);
        for _ in 0..runtrol_core::session::MAX_HOT {
            sessions
                .reserve_open_for_tests(SessionId::now())
                .expect("the abandoned slot was returned");
        }
    }

    #[tokio::test]
    async fn an_unanswered_reservation_stays_occupied_while_displaced_cleanup_is_pending() {
        let mut sessions = SessionManager::new();
        let (started, closing) = oneshot::channel();
        let (release, released) = oneshot::channel();
        let victim = SessionId::now();
        attach_test_agent(
            &mut sessions,
            victim,
            Box::new(PendingClose {
                session: victim,
                started,
                release: released,
                panic_after_release: false,
            }),
        );
        for _ in 1..runtrol_core::session::MAX_HOT {
            let session = SessionId::now();
            attach_test_agent(
                &mut sessions,
                session,
                Box::new(ReadyEvent {
                    session,
                    ready: true,
                }),
            );
            drop(
                sessions
                    .pump_once(session)
                    .await
                    .expect("the newer session can publish"),
            );
        }
        let abandoned = sessions
            .reserve_open_for_tests(SessionId::now())
            .expect("the oldest idle process is displaced");
        assert_eq!(
            abandoned
                .displaced
                .as_ref()
                .map(|displaced| displaced.reservation.session()),
            Some(victim)
        );
        let (cancelling, mut cancelled) = mpsc::unbounded_channel();
        let mut tasks = JoinSet::new();

        abandon_reserved(&mut sessions, &mut tasks, &cancelling, abandoned);
        tokio::time::timeout(core::time::Duration::from_secs(2), closing)
            .await
            .expect("cleanup start did not time out")
            .expect("cleanup started");
        assert!(matches!(
            sessions.reserve_open_for_tests(SessionId::now()),
            Err(SessionError::OpeningCapacityReserved)
        ));

        release.send(()).expect("cleanup may finish");
        tokio::time::timeout(core::time::Duration::from_secs(2), tasks.join_next())
            .await
            .expect("cleanup task join did not time out")
            .expect("cleanup task joined")
            .expect("cleanup task completed");
        for _ in 0..2 {
            match tokio::time::timeout(core::time::Duration::from_secs(2), cancelled.recv())
                .await
                .expect("slot release did not time out")
                .expect("cleanup releases both reservations")
            {
                ReservationAsked::CancelOpen(reservation) => sessions.cancel_open(reservation),
                ReservationAsked::ReleaseClosing(reservation) => {
                    sessions.release_closing(reservation);
                }
                _ => panic!("a slot release was expected"),
            }
        }
        assert!(sessions.reserve_open_for_tests(SessionId::now()).is_ok());
    }

    enum DroppedCleanup {
        Stopping,
        Cleaning,
    }

    async fn dropped_answer_holds_slot_until_cleanup(
        kind: DroppedCleanup,
        panic_after_release: bool,
    ) {
        let mut sessions = SessionManager::new();
        let mut held = Vec::new();
        for _ in 0..runtrol_core::session::MAX_HOT {
            held.push(
                sessions
                    .reserve_open_for_tests(SessionId::now())
                    .expect("fills one bounded slot"),
            );
        }
        let reserved = held.pop().expect("one reserved slot").reservation;
        let (started, closing) = oneshot::channel();
        let (release, released) = oneshot::channel();
        let agent: Box<dyn Agent> = Box::new(PendingClose {
            session: reserved.session(),
            started,
            release: released,
            panic_after_release,
        });
        let reply = match kind {
            DroppedCleanup::Stopping => {
                let intent = runtrol_provider::OpenIntent {
                    session: reserved.session(),
                    workspace: runtrol_provider::AbsPath::new(if cfg!(windows) {
                        r"C:\work"
                    } else {
                        "/work"
                    })
                    .expect("valid test path"),
                    disposition: runtrol_provider::Disposition::Fresh,
                    model: None,
                    reasoning_effort: None,
                    permission: None,
                };
                sessions
                    .attach_opened(
                        reserved,
                        runtrol_provider::ProviderId::parse("test").expect("valid provider"),
                        &intent,
                        agent,
                    )
                    .expect("the cleanup fixture attaches");
                let closing = sessions
                    .close(intent.session)
                    .expect("the cleanup fixture starts closing");
                Reply::Stopping {
                    agent: closing.agent,
                    how: CloseMode::Kill,
                    reservation: closing.reservation,
                }
            }
            DroppedCleanup::Cleaning => Reply::Cleaning {
                response: Response::Done,
                agents: vec![Cleanup {
                    agent,
                    how: CloseMode::Kill,
                    reservation: Some(CleanupReservation::Open(reserved)),
                }],
            },
        };
        let answer = Answered {
            conversation: Conversation::at_the_machine(),
            reply,
        };
        let (answered, hearing) = oneshot::channel();
        drop(hearing);
        let (cancelling, mut cancelled) = mpsc::unbounded_channel();
        let mut tasks = JoinSet::new();

        deliver_answer(answered, answer, &mut tasks, &cancelling, &mut sessions);
        tokio::time::timeout(core::time::Duration::from_secs(2), closing)
            .await
            .expect("abandoned cleanup start did not time out")
            .expect("abandoned reply cleanup started");
        assert!(matches!(
            sessions.reserve_open_for_tests(SessionId::now()),
            Err(SessionError::OpeningCapacityReserved)
        ));

        release.send(()).expect("cleanup may finish");
        let joined = tokio::time::timeout(core::time::Duration::from_secs(2), tasks.join_next())
            .await
            .expect("cleanup task join did not time out")
            .expect("cleanup task joined");
        if panic_after_release {
            assert!(joined.is_err(), "the scripted cleanup had to panic");
        } else {
            joined.expect("cleanup task completed");
        }
        let released = tokio::time::timeout(core::time::Duration::from_secs(2), cancelled.recv())
            .await
            .expect("cleanup slot release did not time out")
            .expect("cleanup releases its slot");
        match released {
            ReservationAsked::CancelOpen(reservation) => sessions.cancel_open(reservation),
            ReservationAsked::ReleaseClosing(reservation) => {
                sessions.release_closing(reservation);
            }
            _ => panic!("a slot release was expected"),
        }
        assert!(sessions.reserve_open_for_tests(SessionId::now()).is_ok());
    }

    #[tokio::test]
    async fn a_dropped_stopping_answer_keeps_its_slot_until_cleanup() {
        dropped_answer_holds_slot_until_cleanup(DroppedCleanup::Stopping, false).await;
    }

    #[tokio::test]
    async fn a_dropped_cleaning_answer_keeps_its_slot_until_cleanup() {
        dropped_answer_holds_slot_until_cleanup(DroppedCleanup::Cleaning, false).await;
    }

    #[tokio::test]
    async fn a_panicking_abandoned_cleanup_still_releases_its_slot() {
        dropped_answer_holds_slot_until_cleanup(DroppedCleanup::Stopping, true).await;
    }

    #[test]
    fn a_refusal_this_file_writes_is_readable_by_the_surface_that_reads_answers() {
        // Written by this file rather than by the vocabulary, so its shape is worth checking rather than assuming.
        let said = refuse("something went wrong");
        let bytes = serde_json::to_vec(&said).expect("writable");
        let read: Response = serde_json::from_slice(&bytes).expect("readable");
        match read {
            Response::Failed(error) => assert_eq!(&*error.message, "something went wrong"),
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    #[test]
    fn the_last_resort_answer_is_a_refusal_and_not_something_unreadable() {
        // The one answer written by hand. If it were wrong, the case it exists for would be a caller waiting
        // forever, which is the outcome this whole file is arranged to prevent.
        let bytes = br#"{"say":"failed","with":{"message":"this daemon cannot write its own answer","retryable":false,"needs_the_operator":false}}"#;
        let read: Response =
            serde_json::from_slice(bytes).expect("the last resort must be readable");
        assert!(matches!(read, Response::Failed(_)));
    }

    #[test]
    fn an_automatic_rollback_stays_pinned_when_the_journal_cannot_be_saved() {
        let provider = ProviderId::parse("test").expect("valid provider");
        let mut pins = BTreeMap::new();
        let rolled_back = Response::ProviderUpdated(runtrol_ipc::wire::ProviderUpdateResult {
            provider: provider.as_str().into(),
            outcome: runtrol_ipc::wire::ProviderUpdateOutcome::RolledBack,
            from: "1.0.0".into(),
            to: "1.0.0".into(),
            why: Some("the rollback pin was not saved".into()),
        });

        track_automatic_pin(&mut pins, provider, "2.0.0", &rolled_back);
        assert_eq!(pins.get(&provider).map(AsRef::as_ref), Some("2.0.0"));

        let updated = Response::ProviderUpdated(runtrol_ipc::wire::ProviderUpdateResult {
            provider: provider.as_str().into(),
            outcome: runtrol_ipc::wire::ProviderUpdateOutcome::Updated,
            from: "1.0.0".into(),
            to: "2.0.0".into(),
            why: None,
        });
        track_automatic_pin(&mut pins, provider, "2.0.0", &updated);
        assert!(!pins.contains_key(&provider));
    }
}
