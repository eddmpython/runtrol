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

use runtrol_core::{
    AgentLease, ClosingReservation, OpenReservation, ProviderUpdateReservation, ReservedOpen,
    SessionError, SessionManager, TakenAgent, WorkspaceClaim,
};
use runtrol_ipc::transport::{Connection, Listener, TransportError};
use runtrol_ipc::wire::{Request, Response, WireError};
use runtrol_provider::{
    AbsPath, AgentCommand, CloseMode, ProviderError, ProviderId, SessionId, WorkspaceAccess,
};
use runtrol_transport::{
    CryptoError, LinkKind, NoiseUpgrade, NoiseWebSocket, PhoneHttp, PhoneHttpError, SessionBinding,
    StaticKeypair, WebSocketLinkError, response,
};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Mutex, mpsc, oneshot, watch};
use tokio::task::JoinSet;
use tokio::time::Instant;

use crate::compose::Composed;
use crate::dispatch::{
    Cleanup, CleanupReservation, Conversation, Discovered, Prepared, PreparedKind, Reply,
    answer_prepared, complete_prepare_for, discover, is_integration_admin, needs_driver,
    prepare_consult, prepare_integration_admin, prepare_provider_updates, refuse,
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

enum SurfaceConnection {
    Local(Connection),
    Phone(Box<NoiseWebSocket>),
}

impl SurfaceConnection {
    async fn recv(&mut self) -> Result<Option<bytes::Bytes>, SurfaceError> {
        match self {
            Self::Local(connection) => connection.recv().await.map_err(SurfaceError::from),
            Self::Phone(connection) => connection.recv().await.map_err(SurfaceError::from),
        }
    }

    async fn send(&mut self, payload: &[u8]) -> Result<(), SurfaceError> {
        match self {
            Self::Local(connection) => connection.send(payload).await.map_err(SurfaceError::from),
            Self::Phone(connection) => connection.send(payload).await.map_err(SurfaceError::from),
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
        }
    }
}

#[derive(Debug, thiserror::Error)]
enum SurfaceError {
    #[error(transparent)]
    Local(#[from] TransportError),
    #[error(transparent)]
    Phone(#[from] WebSocketLinkError),
}

struct ConnectionServices {
    asking: mpsc::Sender<Asked>,
    reserving: mpsc::UnboundedSender<ReservationAsked>,
    returning: mpsc::UnboundedSender<AgentReturned>,
    composed: Arc<Composed>,
    discovering: Arc<Mutex<()>>,
    session_index: watch::Receiver<Arc<[u8]>>,
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
        claim: WorkspaceClaim,
        answered: oneshot::Sender<Result<ReservedOpen, SessionError>>,
    },
    ReserveProviderUpdate {
        provider: ProviderId,
        answered: oneshot::Sender<Result<ProviderUpdateReservation, SessionError>>,
    },
    CancelOpen(OpenReservation),
    ReleaseClosing(ClosingReservation),
    ReleaseProviderUpdate(ProviderUpdateReservation),
}

struct AutomaticUpdateNotice {
    provider: ProviderId,
    message: Box<str>,
}

/// Cancels a pending slot if connection preparation is abandoned.
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
    discovering: Arc<Mutex<()>>,
    reserving: mpsc::UnboundedSender<ReservationAsked>,
    notices: mpsc::UnboundedSender<AutomaticUpdateNotice>,
) {
    tokio::time::sleep(PROVIDER_UPDATE_INITIAL_DELAY).await;
    let mut deferred = BTreeMap::<ProviderId, (Instant, bool)>::new();
    let mut session_pins = BTreeMap::<ProviderId, Box<str>>::new();
    loop {
        let statuses = {
            let _gate = discovering.lock().await;
            crate::provider_update::inspect_all(&composed).await
        };
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

            let _gate = discovering.lock().await;
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
pub async fn serve(composed: Composed, mut listener: Listener) -> Result<(), ServeError> {
    serve_sessions(composed, &mut listener, SessionManager::new()).await
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
    mut listener: Listener,
    ingress: PhoneIngress,
) -> Result<(), ServeError> {
    let phone = phone_plane(&composed, ingress)?;
    serve_surfaces(composed, &mut listener, SessionManager::new(), Some(phone)).await
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
    listener: &mut Listener,
    sessions: SessionManager,
) -> Result<(), ServeError> {
    serve_surfaces(composed, listener, sessions, None).await
}

#[expect(
    clippy::too_many_lines,
    reason = "one owner loop keeps every way session state changes visible beside index publication"
)]
async fn serve_surfaces(
    composed: Composed,
    listener: &mut Listener,
    mut sessions: SessionManager,
    phone: Option<PhonePlane>,
) -> Result<(), ServeError> {
    let composed = Arc::new(composed);
    let runtime_instance =
        crate::runtime_locator::load_or_create_instance(composed.home.paths().runtime_instance())
            .map_err(|error| ServeError::RuntimeBootstrap(error.to_string()))?;
    let runtime_address = composed
        .home
        .paths()
        .runtime_endpoint()
        .address()
        .to_owned();
    let mut runtime_listener = Listener::bind_owner_only(&runtime_address).await?;
    let _runtime_locator = crate::runtime_locator::PublishedLocator::publish(
        composed.home.paths().runtime_locator(),
        &runtime_instance,
        &runtime_address,
    )
    .map_err(|error| ServeError::RuntimeBootstrap(error.to_string()))?;
    let (asking, mut asked) = mpsc::channel::<Asked>(ASKED_QUEUE);
    let (reserving, mut reservations) = mpsc::unbounded_channel::<ReservationAsked>();
    let (returning, mut returned) = mpsc::unbounded_channel::<AgentReturned>();
    let (runtime_asking, mut runtime_asked) =
        mpsc::channel::<crate::runtime_control::RuntimeAsked>(ASKED_QUEUE);
    let (runtime_returning, mut runtime_returned) =
        mpsc::unbounded_channel::<crate::runtime_control::RuntimeReturned>();
    let mut runtime_control = crate::runtime_control::RuntimeControl::new()
        .map_err(|error| ServeError::RuntimeBootstrap(error.message.to_owned()))?;
    let runtime_native_cursors = Arc::new(
        crate::runtime_native_sessions::NativeCursorCodec::new().map_err(|_| {
            ServeError::RuntimeBootstrap(
                "Runtime could not create native catalogue cursor authority".to_owned(),
            )
        })?,
    );
    let (noticing_updates, mut update_notices) = mpsc::unbounded_channel::<AutomaticUpdateNotice>();
    let mut provider_update_notices = BTreeMap::<ProviderId, Box<str>>::new();
    let initial_index = Arc::<[u8]>::from(encode_session_index(
        &composed,
        &sessions,
        &provider_update_notices,
    ));
    let (session_index, _initial_index_receiver) = watch::channel(initial_index);
    let initial_runtime_sessions =
        Arc::new(crate::runtime_inventory::sessions(&composed, &sessions)?);
    let (runtime_sessions, _initial_runtime_sessions_receiver) =
        watch::channel(initial_runtime_sessions);
    let (runtime_providers, _initial_runtime_providers_receiver) =
        watch::channel(Arc::new(crate::runtime_inventory::providers(&composed)));
    // ProbeCache replaces one file atomically but is deliberately not a database. Serializing provider preparation
    // keeps two connections from publishing stale snapshots over each other and bounds temporary provider processes.
    // A Models request holds this gate through its provider call. Opens release it after discovery because their
    // process slots are bounded separately by MAX_HOT.
    let discovering = Arc::new(Mutex::new(()));
    let mut connections = JoinSet::new();
    let (upgrading, mut upgrades) = mpsc::channel::<NoiseUpgrade>(PHONE_UPGRADE_QUEUE);
    connections.spawn(automatic_provider_updates(
        Arc::clone(&composed),
        Arc::clone(&discovering),
        reserving.clone(),
        noticing_updates,
    ));

    let outcome = loop {
        tokio::select! {
            arrived = runtime_listener.accept() => {
                let connection = match arrived {
                    Ok(connection) => connection,
                    Err(error) => break Err(error.into()),
                };
                connections.spawn(crate::runtime_serve::serve_connection(
                    connection,
                    runtime_instance.clone(),
                    Arc::clone(&composed),
                    Arc::clone(&discovering),
                    Arc::clone(&runtime_native_cursors),
                    runtime_providers.clone(),
                    runtime_sessions.subscribe(),
                    runtime_asking.clone(),
                    runtime_returning.clone(),
                ));
            }

            arrived = listener.accept() => {
                let connection = match arrived {
                    Ok(connection) => connection,
                    Err(error) => break Err(error.into()),
                };
                // The connection's own task. It reads, it writes, and it never touches a session.
                connections.spawn(converse(
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
                        .paired_devices
                        .iter()
                        .find(|paired| paired.remote_static_key == remote)
                        .map(|paired| paired.id)
                    else {
                        return;
                    };
                    let Ok(connection) = pending.approve(remote).await else {
                        return;
                    };
                    converse(
                        SurfaceConnection::Phone(Box::new(connection)),
                        Conversation::from_device(device),
                        services,
                    ).await;
                });
            }

            Some(reservation) = reservations.recv() => match reservation {
                ReservationAsked::Reserve { provider, session, claim, answered } => {
                    let reserved = sessions.reserve_open_for_provider(provider, session, claim);
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
                    }
                }
                ReservationAsked::ReserveProviderUpdate { provider, answered } => {
                    let reserved = sessions.reserve_provider_update(provider);
                    if let Err(Ok(abandoned)) = answered.send(reserved) {
                        sessions.release_provider_update(abandoned);
                    }
                }
                ReservationAsked::CancelOpen(reservation) => sessions.cancel_open(reservation),
                ReservationAsked::ReleaseClosing(reservation) => {
                    sessions.release_closing(reservation);
                    runtrol_childproc::footprint::release_unused_memory();
                }
                ReservationAsked::ReleaseProviderUpdate(reservation) => {
                    sessions.release_provider_update(reservation);
                    runtrol_childproc::footprint::release_unused_memory();
                }
            },

            Some(ask) = asked.recv() => {
                let Asked { mut conversation, request, prepared, reservation, answered } = ask;
                let changes_index = matches!(
                    &request,
                    Request::Start { .. }
                        | Request::Resume { .. }
                        | Request::Close { .. }
                        | Request::IntegrationApprovalFinish { .. }
                        | Request::IntegrationRevoke { .. }
                );
                let reservation = reservation.and_then(ReservationGuard::take);
                let reply = answer_prepared(
                    &mut conversation,
                    &composed,
                    &mut sessions,
                    request,
                    prepared,
                    reservation,
                );
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
                }
            }

            Some(ask) = runtime_asked.recv() => {
                let crate::runtime_control::RuntimeAsked {
                    integration,
                    request,
                    answered,
                } = ask;
                let reply = runtime_control.answer(
                    &composed.store,
                    &mut sessions,
                    integration,
                    request,
                );
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
                publish_session_index(
                    &session_index,
                    &runtime_sessions,
                    &composed,
                    &sessions,
                    &provider_update_notices,
                );
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
                if let Some(published) = pumped.published {
                    if let Err(error) = crate::dispatch::persist_live(&composed, &sessions, pumped.session) {
                        break Err(error.into());
                    }
                    if let Err(error) = composed.store.put_cursor(
                        pumped.session,
                        runtrol_store::Cursor {
                            src_end: published.event.src_end,
                            seq: published.event.seq,
                        },
                    ) {
                        break Err(error.into());
                    }
                }
                if pumped.index_changed {
                    publish_session_index(
                        &session_index,
                        &runtime_sessions,
                        &composed,
                        &sessions,
                        &provider_update_notices,
                    );
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

            Some(_finished) = connections.join_next(), if !connections.is_empty() => {}
        }
    };

    connections.abort_all();
    while connections.join_next().await.is_some() {}
    outcome
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
        Reply::One(_) | Reply::Watching(_) | Reply::WatchingSessions => false,
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
#[expect(
    clippy::too_many_lines,
    reason = "one connection lifecycle keeps reservation cancellation and request ownership visible together"
)]
async fn converse(
    mut connection: SurfaceConnection,
    mut conversation: Conversation,
    services: ConnectionServices,
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

        // The owner has already published the exact current listing as encoded wire bytes. A refresh that queued a
        // second reconstruction behind session work made every surface pay owner contention for immutable state.
        // Greeting and scope stay in front of the fast path, just as they are in answer_prepared.
        if matches!(request, Request::List) && conversation.greeted() {
            if let Err(refusal) =
                crate::scope::allowed(conversation.caller(), &request, &composed.granted)
            {
                if write(&mut connection, &refuse(&refusal.to_string()))
                    .await
                    .is_err()
                {
                    return;
                }
            } else {
                let current = Arc::clone(&session_index.borrow());
                if connection.send(current.as_ref()).await.is_err() {
                    return;
                }
            }
            continue;
        }

        let reservation = if matches!(request, Request::Start { .. } | Request::Resume { .. })
            && conversation.greeted()
            && crate::scope::allowed(conversation.caller(), &request, &composed.granted).is_ok()
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
            let (answered, hearing) = oneshot::channel();
            if reserving
                .send(ReservationAsked::Reserve {
                    provider,
                    session,
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
        let mut preparation_gate = if needs_driver(&request)
            || crate::consult::is_consult(&request)
            || matches!(
                request,
                Request::ProviderUpdates | Request::ProviderUpdate { .. }
            ) {
            Some(discovering.lock().await)
        } else {
            None
        };
        let reserved_session = reservation
            .as_ref()
            .and_then(|guard| guard.reservation.as_ref())
            .map(CleanupReservation::session);
        let prepared = if let Request::Models { provider } = &request {
            let preparing = async {
                let discovered = discover(&conversation, &composed, &request).await;
                complete_prepare_for(&request, discovered, reserved_session).await
            };
            finish_model_preparation(provider, preparing, model_preparation_budget()).await
        } else if crate::consult::is_consult(&request) {
            // The whole consult exchange runs here in the connection's own task, behind the same gate that
            // bounds temporary provider processes, so a toggle never stops a running session's events.
            prepare_consult(&conversation, &composed, &request).await
        } else if matches!(request, Request::ProviderUpdates) {
            prepare_provider_updates(&conversation, &composed, &request).await
        } else if is_integration_admin(&request) {
            prepare_integration_admin(&conversation, &composed, &request).await
        } else {
            let discovered = if preparation_gate.is_some() {
                discover(&conversation, &composed, &request).await
            } else {
                Discovered::None
            };
            if !provider_update {
                drop(preparation_gate.take());
            }
            complete_prepare_for(&request, discovered, reserved_session).await
        };
        // A provider update keeps the shared discovery gate through package mutation and verification. Its update
        // reservation blocks session processes, while this guard blocks short-lived probes that have no session slot.
        let _provider_update_gate = if provider_update {
            preparation_gate.take()
        } else {
            None
        };
        drop(preparation_gate);
        let (answered, hearing) = oneshot::channel();
        let ask = Asked {
            conversation,
            request,
            prepared,
            reservation,
            answered,
        };
        if asking.send(ask).await.is_err() {
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
                return;
            }

            Reply::WatchingSessions => {
                if write(&mut connection, &Response::WatchingSessions)
                    .await
                    .is_err()
                {
                    return;
                }
                relay_session_index(&mut connection, &mut session_index).await;
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
                let outcome = match agent.close(how).await {
                    Ok(()) => Response::Done,
                    Err(error) => refuse(&error.to_string()),
                };
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
    while let Some(item) = watching.recv().await {
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

/// Relay coalesced current session snapshots without copying one frame per connected surface.
async fn relay_session_index(
    connection: &mut SurfaceConnection,
    session_index: &mut watch::Receiver<Arc<[u8]>>,
) {
    let current = Arc::clone(&session_index.borrow_and_update());
    if connection.send(current.as_ref()).await.is_err() {
        return;
    }
    while session_index.changed().await.is_ok() {
        let current = Arc::clone(&session_index.borrow_and_update());
        if connection.send(current.as_ref()).await.is_err() {
            return;
        }
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

/// Publish only a changed current index. The encoded bytes are shared by every subscriber.
fn publish_session_index(
    session_index: &watch::Sender<Arc<[u8]>>,
    runtime_sessions: &watch::Sender<Arc<crate::runtime_inventory::RuntimeSessionCatalogue>>,
    composed: &Composed,
    sessions: &SessionManager,
    provider_update_notices: &BTreeMap<ProviderId, Box<str>>,
) {
    let next = Arc::<[u8]>::from(encode_session_index(
        composed,
        sessions,
        provider_update_notices,
    ));
    session_index.send_if_modified(|current| {
        if current.as_ref() == next.as_ref() {
            return false;
        }
        *current = next;
        true
    });
    let public = match crate::runtime_inventory::sessions(composed, sessions) {
        Ok(catalogue) => Arc::new(catalogue),
        Err(_) => Arc::new(crate::runtime_inventory::RuntimeSessionCatalogue::unavailable()),
    };
    runtime_sessions.send_replace(public);
}

fn encode_session_index(
    composed: &Composed,
    sessions: &SessionManager,
    provider_update_notices: &BTreeMap<ProviderId, Box<str>>,
) -> Vec<u8> {
    let mut response = crate::dispatch::list(composed, sessions);
    if let Response::Sessions(listing) = &mut response {
        listing
            .warnings
            .extend(provider_update_notices.values().cloned());
    }
    encode_response(&response)
}

/// Write one answer.
///
/// A response that cannot be serialized is a defect in this build rather than something a caller did, so what goes out
/// instead says exactly that. The alternative is writing nothing, which leaves the caller waiting on a daemon that is
/// working perfectly well.
async fn write(
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
    use core::future::Future;

    use async_trait::async_trait;
    use fastwebsockets::{Frame, OpCode, WebSocket, handshake};
    use http_body_util::Empty;
    use hyper::Request as HttpRequest;
    use hyper::upgrade::Upgraded;
    use hyper_util::rt::TokioIo;
    use runtrol_provider::{Agent, AgentCommand, Opaque, Produced, ProviderError, WallMs};
    use runtrol_security::{DeviceId, DeviceLabels, DeviceScope, GrantLedger};
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
        let reserved = sessions
            .reserve_open_for_tests(session)
            .expect("one process slot");
        let intent = runtrol_provider::OpenIntent {
            session,
            workspace: runtrol_provider::AbsPath::new(if cfg!(windows) {
                r"C:\work"
            } else {
                "/work"
            })
            .expect("valid test path"),
            disposition: runtrol_provider::Disposition::Fresh,
            model: None,
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
            let mut listener = Listener::bind(&address)
                .await
                .expect("the endpoint is free");
            let serving =
                tokio::spawn(
                    async move { serve_sessions(composed, &mut listener, sessions).await },
                );
            Self {
                address,
                home,
                serving,
            }
        }

        async fn start_with_phone(
            name: &str,
            sessions: SessionManager,
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
            composed.granted =
                GrantLedger::from_persisted([(device, vec![DeviceScope::SessionList])]);
            composed.paired_devices = vec![crate::compose::PairedDevice {
                id: device,
                remote_static_key: phone.public_key(),
                credential_fingerprint: token.fingerprint(),
                labels: DeviceLabels::new("Test phone", "Browser").expect("device labels"),
                paired_at: WallMs::from_millis(1_767_225_600_000),
            }];

            let address = composed.home.paths().endpoint().address().to_owned();
            let mut listener = Listener::bind(&address)
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
                serve_surfaces(composed, &mut listener, sessions, Some(phone_plane)).await
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

        async fn caller(&self) -> Connection {
            runtrol_ipc::transport::connect(&self.address)
                .await
                .expect("the daemon is listening")
        }

        fn stop(self) {
            self.serving.abort();
            drop(std::fs::remove_dir_all(&self.home));
        }
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

        let gate = Arc::new(Mutex::new(()));
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
            Running::start_with_phone("phone-same-owner", sessions).await;

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
        let phone_session = match phone.ask(&Request::List).await {
            Response::Sessions(listing) => listing.sessions.first().map(|line| line.session),
            other => panic!("expected a phone session index, got {other:?}"),
        };
        assert_eq!(phone_session, local_session);
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
                assert_eq!(
                    listing.sessions.first().map(|line| line.session),
                    Some(session)
                );
            }
            other => panic!("expected the initial phone watch snapshot, got {other:?}"),
        }

        assert!(matches!(
            ask(&mut local, &Request::Close { session, now: true }).await,
            Response::Done
        ));
        match watcher.receive().await {
            Response::Sessions(listing) => assert!(listing.sessions.is_empty()),
            other => panic!("expected the changed phone watch snapshot, got {other:?}"),
        }

        drop(phone);
        drop(watcher);
        drop(local);
        running.stop();
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
