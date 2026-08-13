//! Reconnecting relay supervision without moving session ownership out of the Core.
//!
//! One task owns the untrusted multiplexed WebSocket. A phone gets a surface only after a fresh relay-bound Noise IK
//! handshake authenticates one durable paired key. The task routes bounded ciphertext records and never sees an IPC
//! request or response. Each accepted surface owns its own Noise channel, so one slow phone cannot hold another
//! phone's cipher state or the Core session manager.

use core::time::Duration;
use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::sync::Arc;

use bytes::Bytes;
use runtrol_security::DeviceId;
use runtrol_transport::{
    Channel, CryptoError, EncryptedRecord, MAX_TRANSPORT_FRAME, RelayEndpoint, RelayError,
    RelayOrigin, RelaySeed, RelaySocket, SessionBinding, SessionResponder, StaticKeypair,
};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::{Instant, MissedTickBehavior, interval, sleep_until};

use crate::compose::DeviceAuthority;
use crate::pairing_admin::{PairingAdmin, PairingOutcome, Reception};

const ARRIVAL_QUEUE: usize = 8;
const RECORD_QUEUE_PER_PHONE: usize = 2;
const OUTBOUND_RECORD_QUEUE: usize = 16;
const RETRY_BASE_SECONDS: u64 = 1;
const RETRY_MAX_SECONDS: u64 = 30;
const RETRY_JITTER_MILLIS: u64 = 250;

/// Non-secret state of the optional remote relay path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RelayStatus {
    /// The VS Code surface has not configured a relay origin.
    Disabled,
    /// The daemon has not yet completed one authenticated relay connection.
    Connecting,
    /// The PC WebSocket is connected and ready to authenticate paired phones.
    Online,
    /// The last attempt failed at a named, non-secret boundary and is being retried.
    Offline(RelayStage),
}

#[derive(Clone)]
pub(crate) struct RelayControl {
    origin: watch::Sender<Option<Box<str>>>,
    status: watch::Sender<RelayStatus>,
}

impl RelayControl {
    pub(crate) fn new() -> Self {
        let (origin, _origin_initial) = watch::channel(None);
        let (status, _status_initial) = watch::channel(RelayStatus::Disabled);
        Self { origin, status }
    }

    pub(crate) fn configure(&self, origin: Option<&str>) -> Result<(), RelayError> {
        let origin = match origin {
            Some(origin) => Some(RelayOrigin::parse(origin)?.as_str().into()),
            None => None,
        };
        let status = if origin.is_some() {
            RelayStatus::Connecting
        } else {
            RelayStatus::Disabled
        };
        self.origin.send_replace(origin);
        self.status.send_replace(status);
        Ok(())
    }

    pub(crate) fn view(&self) -> (Option<Box<str>>, RelayStatus) {
        (self.origin.borrow().clone(), *self.status.borrow())
    }
}

/// Stable stage names suitable for a local status surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RelayStage {
    /// Public DNS resolution or exact destination admission failed.
    Discovery,
    /// Idempotent route registration was refused.
    Registration,
    /// Ticket exchange, TLS, or WebSocket connection failed.
    Connection,
    /// An established multiplexed ciphertext stream failed.
    Exchange,
}

/// Optional relay configuration passed explicitly to the daemon.
pub struct RelayIngress {
    endpoint: RelayEndpoint,
    status: watch::Sender<RelayStatus>,
}

impl RelayIngress {
    /// Configure an endpoint without opening a socket.
    #[must_use]
    pub fn new(endpoint: RelayEndpoint) -> Self {
        let (status, _initial) = watch::channel(RelayStatus::Connecting);
        Self { endpoint, status }
    }

    /// Observe connection state without receiving credentials or traffic.
    #[must_use]
    pub fn status(&self) -> watch::Receiver<RelayStatus> {
        self.status.subscribe()
    }
}

pub(crate) struct RelayArrival {
    pub(crate) device: DeviceId,
    pub(crate) surface: RelaySurface,
}

pub(crate) struct RelayHub {
    arrivals: mpsc::Receiver<RelayArrival>,
}

impl RelayHub {
    pub(crate) async fn accept(&mut self) -> Option<RelayArrival> {
        self.arrivals.recv().await
    }
}

pub(crate) fn supervise(
    ingress: RelayIngress,
    identity: Arc<StaticKeypair>,
    device_authority: DeviceAuthority,
    pairing_admin: PairingAdmin,
) -> (RelayHub, impl Future<Output = ()> + Send + 'static) {
    let (arriving, arrivals) = mpsc::channel(ARRIVAL_QUEUE);
    let (outgoing, outbound) = mpsc::channel(OUTBOUND_RECORD_QUEUE);
    let supervisor = RelaySupervisor {
        ingress,
        identity,
        device_authority,
        pairing_admin,
        arriving,
        outgoing,
        outbound,
        attempts: 0,
        next_link: 1,
    };
    (RelayHub { arrivals }, supervisor.run())
}

pub(crate) fn supervise_controlled(
    control: RelayControl,
    seed: Arc<RelaySeed>,
    identity: Arc<StaticKeypair>,
    device_authority: DeviceAuthority,
    pairing_admin: PairingAdmin,
) -> (RelayHub, impl Future<Output = ()> + Send + 'static) {
    let (arriving, arrivals) = mpsc::channel(ARRIVAL_QUEUE);
    let manager = controlled_manager(
        control,
        seed,
        identity,
        device_authority,
        pairing_admin,
        arriving,
    );
    (RelayHub { arrivals }, manager)
}

async fn controlled_manager(
    control: RelayControl,
    seed: Arc<RelaySeed>,
    identity: Arc<StaticKeypair>,
    device_authority: DeviceAuthority,
    pairing_admin: PairingAdmin,
    arriving: mpsc::Sender<RelayArrival>,
) {
    let mut configured = control.origin.subscribe();
    loop {
        let origin = configured.borrow().clone();
        let Some(origin) = origin else {
            control.status.send_replace(RelayStatus::Disabled);
            if configured.changed().await.is_err() {
                return;
            }
            continue;
        };
        let endpoint = match seed.endpoint(&origin) {
            Ok(endpoint) => endpoint,
            Err(error) => {
                control
                    .status
                    .send_replace(RelayStatus::Offline(RelayStage::Discovery));
                drop(error);
                if configured.changed().await.is_err() {
                    return;
                }
                continue;
            }
        };
        let ingress = RelayIngress::new(endpoint);
        let mut status = ingress.status();
        let (mut hub, supervisor) = supervise(
            ingress,
            Arc::clone(&identity),
            device_authority.clone(),
            pairing_admin.clone(),
        );
        tokio::pin!(supervisor);
        loop {
            tokio::select! {
                changed = configured.changed() => {
                    if changed.is_err() {
                        return;
                    }
                    break;
                }
                changed = status.changed() => {
                    if changed.is_err() {
                        break;
                    }
                    control.status.send_replace(*status.borrow());
                }
                arrival = hub.accept() => {
                    let Some(arrival) = arrival else {
                        break;
                    };
                    if arriving.send(arrival).await.is_err() {
                        return;
                    }
                }
                () = &mut supervisor => break,
            }
        }
    }
}

pub(crate) struct RelaySurface {
    link: u64,
    peer_id: [u8; 32],
    channel: Channel,
    incoming: mpsc::Receiver<EncryptedRecord>,
    outgoing: mpsc::Sender<RelayOutbound>,
    closed: watch::Receiver<()>,
}

impl RelaySurface {
    pub(crate) async fn recv(&mut self) -> Result<Option<Bytes>, RelaySurfaceError> {
        loop {
            let record = tokio::select! {
                record = self.incoming.recv() => match record {
                    Some(record) => record,
                    None => return Ok(None),
                },
                closed = self.closed.changed() => {
                    match closed {
                        Ok(()) | Err(_) => return Ok(None),
                    }
                }
            };
            if let Some(frame) = self.channel.open_record(&record)? {
                return Ok(Some(Bytes::from(frame)));
            }
        }
    }

    pub(crate) async fn send(&mut self, payload: &[u8]) -> Result<(), RelaySurfaceError> {
        for record in self.channel.seal_frame(payload)? {
            self.send_record(record).await?;
        }
        Ok(())
    }

    pub(crate) async fn send_parts(&mut self, parts: &[&[u8]]) -> Result<(), RelaySurfaceError> {
        let length = parts.iter().try_fold(0_usize, |total, part| {
            total
                .checked_add(part.len())
                .ok_or(CryptoError::FrameTooLarge {
                    length: usize::MAX,
                    max: MAX_TRANSPORT_FRAME,
                })
        })?;
        if length > MAX_TRANSPORT_FRAME {
            return Err(CryptoError::FrameTooLarge {
                length,
                max: MAX_TRANSPORT_FRAME,
            }
            .into());
        }
        let mut payload = Vec::with_capacity(length);
        for part in parts {
            payload.extend_from_slice(part);
        }
        self.send(&payload).await
    }

    async fn send_record(&mut self, record: EncryptedRecord) -> Result<(), RelaySurfaceError> {
        let (answered, answer) = oneshot::channel();
        let outbound = RelayOutbound {
            link: self.link,
            peer_id: self.peer_id,
            record,
            answered,
        };
        tokio::select! {
            sent = self.outgoing.send(outbound) => {
                if sent.is_err() {
                    return Err(RelaySurfaceError::Closed);
                }
            }
            closed = self.closed.changed() => {
                match closed {
                    Ok(()) | Err(_) => return Err(RelaySurfaceError::Closed),
                }
            }
        }
        tokio::select! {
            result = answer => result.unwrap_or(Err(RelaySurfaceError::Closed)),
            closed = self.closed.changed() => {
                match closed {
                    Ok(()) | Err(_) => Err(RelaySurfaceError::Closed),
                }
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum RelaySurfaceError {
    #[error("relay phone link closed")]
    Closed,
    #[error(transparent)]
    Crypto(#[from] CryptoError),
}

struct RelayOutbound {
    link: u64,
    peer_id: [u8; 32],
    record: EncryptedRecord,
    answered: oneshot::Sender<Result<(), RelaySurfaceError>>,
}

struct RelayPeer {
    incoming: mpsc::Sender<EncryptedRecord>,
    _lifetime: watch::Sender<()>,
}

struct RelaySupervisor {
    ingress: RelayIngress,
    identity: Arc<StaticKeypair>,
    device_authority: DeviceAuthority,
    pairing_admin: PairingAdmin,
    arriving: mpsc::Sender<RelayArrival>,
    outgoing: mpsc::Sender<RelayOutbound>,
    outbound: mpsc::Receiver<RelayOutbound>,
    attempts: u32,
    next_link: u64,
}

impl RelaySupervisor {
    async fn run(mut self) {
        loop {
            self.ingress.status.send_replace(RelayStatus::Connecting);
            let resolved = match self.ingress.endpoint.resolve().await {
                Ok(resolved) => resolved,
                Err(error) => {
                    self.report(RelayStage::Discovery, error);
                    self.wait_before_retry().await;
                    continue;
                }
            };
            if let Err(error) = resolved.register().await {
                self.report(RelayStage::Registration, error);
                self.wait_before_retry().await;
                continue;
            }
            let socket = match resolved.connect_pc().await {
                Ok(socket) => socket,
                Err(error) => {
                    self.report(RelayStage::Connection, error);
                    self.wait_before_retry().await;
                    continue;
                }
            };
            self.attempts = 0;
            self.ingress.status.send_replace(RelayStatus::Online);
            let link = self.next_link;
            self.next_link = self.next_link.wrapping_add(1).max(1);
            let origin: Box<str> = resolved.origin().as_str().into();
            let driven = self.drive(socket, link, &origin).await;
            self.pairing_admin.disconnect(link).await;
            match driven {
                Ok(()) => return,
                Err(DriveError::Transport(error)) => {
                    self.report(RelayStage::Exchange, error);
                    self.wait_before_retry().await;
                }
                Err(DriveError::Disconnected) => {
                    self.ingress
                        .status
                        .send_replace(RelayStatus::Offline(RelayStage::Exchange));
                    self.wait_before_retry().await;
                }
            }
        }
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one relay owner loop keeps socket routing, pairing decisions, and backpressure teardown in one place"
    )]
    async fn drive(
        &mut self,
        mut socket: RelaySocket,
        link: u64,
        origin: &str,
    ) -> Result<(), DriveError> {
        let mut peers = BTreeMap::<[u8; 32], RelayPeer>::new();
        let mut refused = BTreeSet::<[u8; 32]>::new();
        let mut awaiting = BTreeSet::<[u8; 32]>::new();
        let mut pairing_tick = interval(Duration::from_millis(100));
        pairing_tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                received = socket.recv() => {
                    let Some(envelope) = received.map_err(DriveError::Transport)? else {
                        return Err(DriveError::Disconnected);
                    };
                    let peer_id = envelope.peer_id();
                    let Some(record) = envelope.into_record() else {
                        peers.remove(&peer_id);
                        refused.remove(&peer_id);
                        awaiting.remove(&peer_id);
                        continue;
                    };
                    if let Some(peer) = peers.get(&peer_id) {
                        if peer.incoming.try_send(record).is_err() {
                            peers.remove(&peer_id);
                            refused.insert(peer_id);
                        }
                        continue;
                    }
                    if refused.contains(&peer_id) {
                        continue;
                    }
                    if awaiting.contains(&peer_id) {
                        continue;
                    }
                    let admitted = admit_peer(
                        &self.identity,
                        &self.device_authority,
                        origin,
                        peer_id,
                        &record,
                    );
                    let Some(admitted) = admitted else {
                        match self
                            .pairing_admin
                            .receive(&self.identity, link, peer_id, &record)
                            .await
                        {
                            Reception::AwaitingApproval => {
                                awaiting.insert(peer_id);
                            }
                            Reception::Refused => {
                                refused.insert(peer_id);
                            }
                        }
                        continue;
                    };
                    socket
                        .send(peer_id, &admitted.reply)
                        .await
                        .map_err(DriveError::Transport)?;
                    let (incoming, records) = mpsc::channel(RECORD_QUEUE_PER_PHONE);
                    let (lifetime, closed) = watch::channel(());
                    let arrival = RelayArrival {
                        device: admitted.device,
                        surface: RelaySurface {
                            link,
                            peer_id,
                            channel: admitted.channel,
                            incoming: records,
                            outgoing: self.outgoing.clone(),
                            closed,
                        },
                    };
                    if self.arriving.send(arrival).await.is_err() {
                        return Ok(());
                    }
                    peers.insert(peer_id, RelayPeer { incoming, _lifetime: lifetime });
                }
                _ = pairing_tick.tick() => {
                    for outcome in self.pairing_admin.take_outcomes(link).await {
                        match outcome {
                            PairingOutcome::Denied { peer_id, .. } => {
                                awaiting.remove(&peer_id);
                                refused.insert(peer_id);
                            }
                            PairingOutcome::Approved(completed) => {
                                if !awaiting.remove(&completed.peer_id) {
                                    continue;
                                }
                                socket
                                    .send(completed.peer_id, &completed.reply)
                                    .await
                                    .map_err(DriveError::Transport)?;
                                let (incoming, records) = mpsc::channel(RECORD_QUEUE_PER_PHONE);
                                let (lifetime, closed) = watch::channel(());
                                let arrival = RelayArrival {
                                    device: completed.device,
                                    surface: RelaySurface {
                                        link,
                                        peer_id: completed.peer_id,
                                        channel: completed.channel,
                                        incoming: records,
                                        outgoing: self.outgoing.clone(),
                                        closed,
                                    },
                                };
                                if self.arriving.send(arrival).await.is_err() {
                                    return Ok(());
                                }
                                peers.insert(
                                    completed.peer_id,
                                    RelayPeer {
                                        incoming,
                                        _lifetime: lifetime,
                                    },
                                );
                            }
                        }
                    }
                }
                outbound = self.outbound.recv() => {
                    let Some(outbound) = outbound else {
                        return Ok(());
                    };
                    if outbound.link != link || !peers.contains_key(&outbound.peer_id) {
                        let _ignored = outbound.answered.send(Err(RelaySurfaceError::Closed));
                        continue;
                    }
                    match socket.send(outbound.peer_id, &outbound.record).await {
                        Ok(()) => {
                            let _ignored = outbound.answered.send(Ok(()));
                        }
                        Err(error) => {
                            let _ignored = outbound.answered.send(Err(RelaySurfaceError::Closed));
                            return Err(DriveError::Transport(error));
                        }
                    }
                }
            }
        }
    }

    fn report(&self, stage: RelayStage, _error: RelayError) {
        self.ingress
            .status
            .send_replace(RelayStatus::Offline(stage));
    }

    async fn wait_before_retry(&mut self) {
        let exponent = self.attempts.min(5);
        let seconds = RETRY_BASE_SECONDS
            .saturating_mul(1_u64 << exponent)
            .min(RETRY_MAX_SECONDS);
        let process = u64::from(std::process::id());
        let attempt = u64::from(self.attempts);
        let jitter = process
            .wrapping_mul(1_103_515_245)
            .wrapping_add(attempt.wrapping_mul(12_345))
            % (RETRY_JITTER_MILLIS + 1);
        self.attempts = self.attempts.saturating_add(1);
        let deadline =
            Instant::now() + Duration::from_secs(seconds) + Duration::from_millis(jitter);
        loop {
            tokio::select! {
                () = sleep_until(deadline) => return,
                outbound = self.outbound.recv() => {
                    let Some(outbound) = outbound else {
                        return;
                    };
                    let _ignored = outbound.answered.send(Err(RelaySurfaceError::Closed));
                }
            }
        }
    }
}

struct AdmittedPeer {
    device: DeviceId,
    channel: Channel,
    reply: EncryptedRecord,
}

enum DriveError {
    Transport(RelayError),
    Disconnected,
}

fn admit_peer(
    identity: &StaticKeypair,
    device_authority: &DeviceAuthority,
    origin: &str,
    peer_id: [u8; 32],
    first: &EncryptedRecord,
) -> Option<AdmittedPeer> {
    let binding = match SessionBinding::relay(origin, peer_id) {
        Ok(binding) => binding,
        Err(_invalid_binding) => return None,
    };
    let responder = match SessionResponder::new(identity, &binding) {
        Ok(responder) => responder,
        Err(_invalid_responder) => return None,
    };
    let pending = match responder.receive(first) {
        Ok(pending) => pending,
        Err(_invalid_handshake) => return None,
    };
    let remote = pending.remote_public_key();
    let device = device_authority.paired_device(remote)?;
    let (channel, reply, payload) = match pending.approve(remote, &[]) {
        Ok(approved) => approved,
        Err(_invalid_identity) => return None,
    };
    if !payload.is_empty() {
        return None;
    }
    Some(AdmittedPeer {
        device: device.id,
        channel,
        reply,
    })
}

#[cfg(test)]
mod tests {
    use runtrol_provider::WallMs;
    use runtrol_security::{DeviceLabels, GrantLedger};
    use runtrol_transport::{CredentialFingerprint, InitiatorHandshake};

    use super::*;
    use crate::compose::PairedDevice;

    const ORIGIN: &str = "https://relay.runtrol.test";

    fn paired_device(remote_static_key: runtrol_transport::PublicKey) -> PairedDevice {
        PairedDevice {
            id: DeviceId::now(),
            remote_static_key,
            credential_fingerprint: CredentialFingerprint::from_bytes([7; 32]),
            labels: DeviceLabels::new("Pocket", "Test OS").expect("valid labels"),
            paired_at: WallMs::from_millis(1_767_225_600_000),
        }
    }

    fn authority(paired_devices: Vec<PairedDevice>) -> DeviceAuthority {
        DeviceAuthority::new(GrantLedger::new(), paired_devices)
    }

    fn handshake(
        pc: &StaticKeypair,
        phone: &StaticKeypair,
        peer_id: [u8; 32],
        payload: &[u8],
    ) -> (InitiatorHandshake, EncryptedRecord) {
        let binding = SessionBinding::relay(ORIGIN, peer_id).expect("relay binding");
        let mut initiator =
            InitiatorHandshake::session(phone, pc.public_key(), &binding).expect("initiator");
        let first = initiator.write_first(payload).expect("first message");
        (initiator, first)
    }

    #[test]
    fn only_a_stored_key_with_an_empty_handshake_payload_gets_a_surface() {
        let pc = StaticKeypair::generate().expect("PC identity");
        let phone = StaticKeypair::generate().expect("phone identity");
        let stranger = StaticKeypair::generate().expect("stranger identity");
        let paired = paired_device(phone.public_key());
        let authority = authority(vec![paired]);
        let peer_id = [3; 32];

        let (_initiator, first) = handshake(&pc, &stranger, peer_id, &[]);
        assert!(admit_peer(&pc, &authority, ORIGIN, peer_id, &first).is_none());

        let (_initiator, first) = handshake(&pc, &phone, peer_id, b"early request");
        assert!(admit_peer(&pc, &authority, ORIGIN, peer_id, &first).is_none());
    }

    #[test]
    fn live_control_accepts_only_an_exact_origin_and_clears_without_restart() {
        let control = RelayControl::new();
        assert_eq!(control.view(), (None, RelayStatus::Disabled));
        assert!(
            control
                .configure(Some("https://Relay.example.com"))
                .is_err()
        );
        assert_eq!(control.view(), (None, RelayStatus::Disabled));

        control
            .configure(Some("https://relay.example.com"))
            .expect("canonical origin");
        assert_eq!(
            control.view(),
            (
                Some("https://relay.example.com".into()),
                RelayStatus::Connecting
            )
        );
        control.configure(None).expect("disable");
        assert_eq!(control.view(), (None, RelayStatus::Disabled));
    }

    #[tokio::test]
    async fn an_admitted_relay_surface_carries_frames_in_both_directions() {
        let pc = StaticKeypair::generate().expect("PC identity");
        let phone = StaticKeypair::generate().expect("phone identity");
        let paired = paired_device(phone.public_key());
        let device = paired.id;
        let authority = authority(vec![paired]);
        let peer_id = [5; 32];
        let (initiator, first) = handshake(&pc, &phone, peer_id, &[]);
        let admitted =
            admit_peer(&pc, &authority, ORIGIN, peer_id, &first).expect("paired phone admitted");
        assert_eq!(admitted.device, device);
        let (mut phone_channel, payload) = initiator
            .finish(&admitted.reply)
            .expect("phone completes handshake");
        assert!(payload.is_empty());

        let (incoming, records) = mpsc::channel(RECORD_QUEUE_PER_PHONE);
        let (outgoing, mut outbound) = mpsc::channel(OUTBOUND_RECORD_QUEUE);
        let (lifetime, closed) = watch::channel(());
        let mut surface = RelaySurface {
            link: 9,
            peer_id,
            channel: admitted.channel,
            incoming: records,
            outgoing,
            closed,
        };

        for record in phone_channel
            .seal_frame(b"list from phone")
            .expect("encrypted request")
        {
            incoming.send(record).await.expect("routed to PC");
        }
        assert_eq!(
            surface.recv().await.expect("PC receives").expect("frame"),
            Bytes::from_static(b"list from phone")
        );

        let sending = surface.send_parts(&[b"same ", b"Core"]);
        let routing = async {
            loop {
                let outbound = outbound.recv().await.expect("outbound record");
                assert_eq!(outbound.link, 9);
                assert_eq!(outbound.peer_id, peer_id);
                let frame = phone_channel
                    .open_record(&outbound.record)
                    .expect("phone decrypts");
                outbound.answered.send(Ok(())).expect("surface waits");
                if let Some(frame) = frame {
                    return frame;
                }
            }
        };
        let (sent, response) = tokio::join!(sending, routing);
        sent.expect("PC sends");
        assert_eq!(response, b"same Core");

        drop(lifetime);
        assert!(surface.recv().await.expect("clean close").is_none());
    }
}
