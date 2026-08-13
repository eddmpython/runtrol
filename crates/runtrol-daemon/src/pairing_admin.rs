//! Local phone pairing coordination around the relay's untrusted ciphertext stream.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;

use base64ct::{Base64UrlUnpadded, Encoding as _};
use runtrol_ipc::wire::{DeviceLine, PairingInvitationLine, PairingProposalLine, PairingUrl};
use runtrol_provider::WallMs;
use runtrol_security::{
    DeviceId, DeviceScope, GrantLedger, LocalConsole, PairingIdentity, PresenceChallenge,
};
use runtrol_store::{DeviceKey, DeviceRow};
use runtrol_transport::{
    AccessToken, Channel, EncryptedRecord, PairingOffer, PendingPairing, PublicKey, StaticKeypair,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use tokio::sync::Mutex;
use zeroize::Zeroizing;

use crate::Composed;

const PWA_PAIRING_ROUTE: &str = "https://eddmpython.github.io/runtrol/app/#pair=";
const MAX_PENDING_PAIRINGS: usize = 8;
const MAX_COMPLETED_PAIRINGS: usize = 8;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PhoneLabels {
    name: Box<str>,
    platform: Box<str>,
}

#[derive(Serialize)]
struct InvitationFragment<'a> {
    version: u8,
    relay_origin: &'a str,
    route: &'a str,
    credential: &'a str,
    pc_public_key: String,
    pairing_secret: String,
    expires_at_ms: u64,
}

#[derive(Serialize)]
struct PairingReply<'a> {
    credential: &'a str,
    scopes: Vec<String>,
}

struct ActiveOffer {
    offer: PairingOffer,
}

struct PendingApproval {
    challenge_id: [u8; 16],
    challenge: PresenceChallenge,
    scopes: Vec<DeviceScope>,
}

struct PendingProposal {
    link: u64,
    peer_id: [u8; 32],
    handshake: PendingPairing,
    identity: PairingIdentity,
    approval: Option<PendingApproval>,
}

#[derive(Default)]
struct PairingState {
    offer: Option<ActiveOffer>,
    proposals: BTreeMap<[u8; 16], PendingProposal>,
    peers: BTreeSet<(u64, [u8; 32])>,
    outcomes: VecDeque<PairingOutcome>,
}

/// One pairing result returned to the relay owner without exposing transport state to the command dispatcher.
pub(crate) enum PairingOutcome {
    Approved(Box<CompletedPairing>),
    Denied { link: u64, peer_id: [u8; 32] },
}

/// A durably authorized phone ready to receive Noise message two.
pub(crate) struct CompletedPairing {
    pub(crate) link: u64,
    pub(crate) peer_id: [u8; 32],
    pub(crate) device: DeviceId,
    pub(crate) channel: Channel,
    pub(crate) reply: EncryptedRecord,
}

/// Result of offering an unknown relay message to the active pairing attempt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Reception {
    AwaitingApproval,
    Refused,
}

/// Bounded one-use phone pairing state shared by the local VS Code plane and the relay owner.
#[derive(Clone, Default)]
pub(crate) struct PairingAdmin {
    state: Arc<Mutex<PairingState>>,
}

impl PairingAdmin {
    pub(crate) async fn begin(
        &self,
        composed: &Composed,
    ) -> Result<PairingInvitationLine, AdminError> {
        let (origin, status) = composed.relay_control.view();
        let origin = origin.ok_or_else(|| AdminError::invalid("remote connection is disabled"))?;
        if status != crate::RelayStatus::Online {
            return Err(AdminError::unavailable(
                "the relay connection is not online yet",
            ));
        }
        let seed = composed.relay_seed.as_ref().ok_or_else(|| {
            AdminError::unavailable("the protected relay identity is unavailable")
        })?;
        let identity = composed
            .pc_identity
            .as_ref()
            .ok_or_else(|| AdminError::unavailable("the protected PC identity is unavailable"))?;
        let material = seed
            .pairing_material(&origin)
            .map_err(|_| AdminError::state())?;
        let (offer, invitation) = PairingOffer::generate().map_err(|_| AdminError::state())?;
        let fragment = InvitationFragment {
            version: 1,
            relay_origin: material.origin(),
            route: material.route(),
            credential: material.pairing_credential(),
            pc_public_key: Base64UrlUnpadded::encode_string(&identity.public_key().to_bytes()),
            pairing_secret: Base64UrlUnpadded::encode_string(invitation.qr_value()),
            expires_at_ms: invitation.expires_at_unix_ms(),
        };
        let encoded =
            Zeroizing::new(serde_json::to_vec(&fragment).map_err(|_| AdminError::state())?);
        let fragment = Zeroizing::new(Base64UrlUnpadded::encode_string(&encoded));
        let mut pairing_url = String::with_capacity(PWA_PAIRING_ROUTE.len() + fragment.len());
        pairing_url.push_str(PWA_PAIRING_ROUTE);
        pairing_url.push_str(&fragment);
        let pairing_url = PairingUrl::new(pairing_url);
        self.state.lock().await.offer = Some(ActiveOffer { offer });
        Ok(PairingInvitationLine {
            pairing_url,
            expires_at_ms: invitation.expires_at_unix_ms(),
            pc_key_fingerprint: fingerprint(identity.public_key()).into(),
        })
    }

    pub(crate) async fn receive(
        &self,
        identity: &StaticKeypair,
        link: u64,
        peer_id: [u8; 32],
        first: &EncryptedRecord,
    ) -> Reception {
        let mut state = self.state.lock().await;
        if state.proposals.len() >= MAX_PENDING_PAIRINGS || state.peers.contains(&(link, peer_id)) {
            return Reception::Refused;
        }
        let Some(active) = state.offer.as_mut() else {
            return Reception::Refused;
        };
        let Ok(pending) = active.offer.receive(identity, first) else {
            return Reception::Refused;
        };
        let labels: PhoneLabels = match serde_json::from_slice(pending.initiator_payload()) {
            Ok(labels) => labels,
            Err(_) => return Reception::Refused,
        };
        let Ok(pairing_identity) = pending.identity(&labels.name, &labels.platform) else {
            return Reception::Refused;
        };
        let Ok(Some(proposal_id)) = unused_id(&state.proposals) else {
            return Reception::Refused;
        };
        state.offer = None;
        state.peers.insert((link, peer_id));
        state.proposals.insert(
            proposal_id,
            PendingProposal {
                link,
                peer_id,
                handshake: pending,
                identity: pairing_identity,
                approval: None,
            },
        );
        Reception::AwaitingApproval
    }

    pub(crate) async fn proposals(&self) -> Vec<PairingProposalLine> {
        let state = self.state.lock().await;
        state
            .proposals
            .iter()
            .map(|(id, proposal)| PairingProposalLine {
                proposal_id: opaque("pp_", id).into(),
                name: proposal.identity.name().into(),
                platform: proposal.identity.platform().into(),
                key_fingerprint: fingerprint(proposal.handshake.remote_public_key()).into(),
                available_scopes: DeviceScope::EVERY_PLAIN
                    .iter()
                    .map(|scope| scope.to_string().into())
                    .collect(),
            })
            .collect()
    }

    pub(crate) async fn begin_approval(
        &self,
        proposal_id: &str,
        scope_names: &[Box<str>],
    ) -> Result<(Box<str>, Box<str>), AdminError> {
        let proposal_id = parse_opaque(proposal_id, "pp_")?;
        let scopes = parse_scopes(scope_names)?;
        let mut state = self.state.lock().await;
        let proposal = state
            .proposals
            .get_mut(&proposal_id)
            .ok_or_else(|| AdminError::invalid("the phone proposal does not exist"))?;
        if proposal.approval.is_some() {
            return Err(AdminError::invalid(
                "the phone proposal already has an active approval challenge",
            ));
        }
        let request = proposal
            .handshake
            .approval_request(&proposal.identity, &scopes)
            .map_err(|_| AdminError::state())?;
        let console = LocalConsole::claim().ok_or_else(|| {
            AdminError::unavailable("the local approval surface is already in use")
        })?;
        let challenge = PresenceChallenge::issue(&console, request)
            .map_err(|_| AdminError::unavailable("a local challenge could not be generated"))?;
        let prompt = challenge.prompt().into();
        let challenge_id = random_id()?;
        proposal.approval = Some(PendingApproval {
            challenge_id,
            challenge,
            scopes,
        });
        Ok((opaque("pac_", &challenge_id).into(), prompt))
    }

    pub(crate) async fn finish_approval(
        &self,
        composed: &Composed,
        challenge_id: &str,
        answer: &str,
    ) -> Result<DeviceId, AdminError> {
        let challenge_id = parse_opaque(challenge_id, "pac_")?;
        let mut state = self.state.lock().await;
        let proposal_id = state
            .proposals
            .iter()
            .find_map(|(id, proposal)| {
                proposal
                    .approval
                    .as_ref()
                    .filter(|approval| approval.challenge_id == challenge_id)
                    .map(|_| *id)
            })
            .ok_or_else(|| AdminError::invalid("the pairing challenge does not exist"))?;
        if state.outcomes.len() >= MAX_COMPLETED_PAIRINGS {
            return Err(AdminError::unavailable(
                "the relay has not consumed earlier pairing decisions",
            ));
        }
        let pending = state
            .proposals
            .remove(&proposal_id)
            .ok_or_else(AdminError::state)?;
        let link = pending.link;
        let peer_id = pending.peer_id;
        state.peers.remove(&(link, peer_id));
        match complete_approval(composed, pending, answer) {
            Ok(completed) => {
                let device = completed.device;
                state
                    .outcomes
                    .push_back(PairingOutcome::Approved(Box::new(completed)));
                Ok(device)
            }
            Err(error) => {
                state
                    .outcomes
                    .push_back(PairingOutcome::Denied { link, peer_id });
                Err(error)
            }
        }
    }

    pub(crate) async fn deny(&self, proposal_id: &str) -> Result<(), AdminError> {
        let proposal_id = parse_opaque(proposal_id, "pp_")?;
        let mut state = self.state.lock().await;
        if state.outcomes.len() >= MAX_COMPLETED_PAIRINGS {
            return Err(AdminError::unavailable(
                "the relay has not consumed earlier pairing decisions",
            ));
        }
        let pending = state
            .proposals
            .remove(&proposal_id)
            .ok_or_else(|| AdminError::invalid("the phone proposal does not exist"))?;
        state.peers.remove(&(pending.link, pending.peer_id));
        state.outcomes.push_back(PairingOutcome::Denied {
            link: pending.link,
            peer_id: pending.peer_id,
        });
        Ok(())
    }

    pub(crate) async fn take_outcomes(&self, link: u64) -> Vec<PairingOutcome> {
        let mut state = self.state.lock().await;
        let mut selected = Vec::new();
        let mut retained = VecDeque::new();
        while let Some(outcome) = state.outcomes.pop_front() {
            let outcome_link = match &outcome {
                PairingOutcome::Approved(completed) => completed.link,
                PairingOutcome::Denied { link, .. } => *link,
            };
            if outcome_link == link {
                selected.push(outcome);
            } else {
                retained.push_back(outcome);
            }
        }
        state.outcomes = retained;
        selected
    }

    pub(crate) async fn disconnect(&self, link: u64) {
        let mut state = self.state.lock().await;
        let removed: Vec<(u64, [u8; 32])> = state
            .proposals
            .values()
            .filter(|proposal| proposal.link == link)
            .map(|proposal| (proposal.link, proposal.peer_id))
            .collect();
        state.proposals.retain(|_, proposal| proposal.link != link);
        for peer in removed {
            state.peers.remove(&peer);
        }
        state.outcomes.retain(|outcome| match outcome {
            PairingOutcome::Approved(completed) => completed.link != link,
            PairingOutcome::Denied { link: outcome, .. } => *outcome != link,
        });
    }

    pub(crate) fn devices(composed: &Composed) -> Vec<DeviceLine> {
        let grants = composed.device_authority.grants();
        composed
            .device_authority
            .paired_devices()
            .iter()
            .map(|device| DeviceLine {
                device_id: device.id.to_string().into(),
                name: device.labels.name().into(),
                platform: device.labels.platform().into(),
                key_fingerprint: fingerprint(device.remote_static_key).into(),
                scopes: grants
                    .scopes_of(device.id)
                    .into_iter()
                    .map(|scope| scope.to_string().into())
                    .collect(),
                paired_at_ms: device.paired_at.as_millis(),
            })
            .collect()
    }

    pub(crate) fn revoke(composed: &Composed, device_id: &str) -> Result<(), AdminError> {
        let device = DeviceId::parse(device_id)
            .ok_or_else(|| AdminError::invalid("the device identity is malformed"))?;
        let removed = composed
            .store
            .remove_device(DeviceKey::from_bytes(*device.as_bytes()))
            .map_err(|_| AdminError::state())?;
        if !removed {
            return Err(AdminError::invalid("the paired device does not exist"));
        }
        composed
            .reload_device_authority()
            .map_err(|_| AdminError::state())
    }
}

fn complete_approval(
    composed: &Composed,
    pending: PendingProposal,
    answer: &str,
) -> Result<CompletedPairing, AdminError> {
    let approval = pending.approval.ok_or_else(AdminError::state)?;
    let witness = approval
        .challenge
        .answer(answer)
        .map_err(|_| AdminError::invalid("the local approval phrase was wrong or expired"))?;
    let token = AccessToken::generate().map_err(|_| AdminError::state())?;
    let token_value = Zeroizing::new(token.pairing_value());
    let reply = PairingReply {
        credential: &token_value,
        scopes: approval.scopes.iter().map(ToString::to_string).collect(),
    };
    let reply_payload =
        Zeroizing::new(serde_json::to_vec(&reply).map_err(|_| AdminError::state())?);
    let approved = pending
        .handshake
        .approve(
            &pending.identity,
            &approval.scopes,
            &witness,
            &reply_payload,
        )
        .map_err(|_| AdminError::invalid("the pairing approval no longer matches the proposal"))?;
    let (device, remote_public, channel, reply) = approved.into_relay_parts();
    let mut grants: GrantLedger = composed.device_authority.grants().as_ref().clone();
    grants
        .grant_pairing(device, &approval.scopes, &pending.identity, &witness)
        .map_err(|_| AdminError::state())?;
    let row = DeviceRow {
        remote_static_key: remote_public.to_bytes(),
        credential_fingerprint: token.fingerprint().to_bytes(),
        name: pending.identity.name().into(),
        platform: pending.identity.platform().into(),
        scopes: approval
            .scopes
            .iter()
            .map(|scope| scope.to_string().into())
            .collect(),
        paired_at: WallMs::now(),
    };
    composed
        .store
        .put_device(DeviceKey::from_bytes(*device.as_bytes()), &row)
        .map_err(|_| AdminError::state())?;
    composed
        .reload_device_authority()
        .map_err(|_| AdminError::state())?;
    Ok(CompletedPairing {
        link: pending.link,
        peer_id: pending.peer_id,
        device,
        channel,
        reply,
    })
}

fn parse_scopes(names: &[Box<str>]) -> Result<Vec<DeviceScope>, AdminError> {
    if names.len() > DeviceScope::EVERY_PLAIN.len() {
        return Err(AdminError::invalid("too many device scopes were selected"));
    }
    let mut unique = BTreeSet::new();
    names
        .iter()
        .map(|name| {
            let scope = DeviceScope::from_stored(name)
                .map_err(|_| AdminError::invalid("a selected device scope is unknown"))?;
            if matches!(scope, DeviceScope::Workspace(_) | DeviceScope::Provider(_))
                || !unique.insert(scope)
            {
                return Err(AdminError::invalid(
                    "selected device scopes must be unique plain scope names",
                ));
            }
            Ok(scope)
        })
        .collect()
}

fn fingerprint(key: PublicKey) -> String {
    let digest = Sha256::digest(key.to_bytes());
    let short: Vec<u8> = digest.iter().take(8).copied().collect();
    Base64UrlUnpadded::encode_string(&short)
}

fn unused_id<T>(existing: &BTreeMap<[u8; 16], T>) -> Result<Option<[u8; 16]>, AdminError> {
    for _ in 0..4 {
        let candidate = random_id()?;
        if !existing.contains_key(&candidate) {
            return Ok(Some(candidate));
        }
    }
    Ok(None)
}

fn random_id() -> Result<[u8; 16], AdminError> {
    let mut id = [0_u8; 16];
    getrandom::fill(&mut id).map_err(|_| AdminError::state())?;
    Ok(id)
}

fn opaque(prefix: &str, id: &[u8; 16]) -> String {
    format!("{prefix}{}", Base64UrlUnpadded::encode_string(id))
}

fn parse_opaque(value: &str, prefix: &str) -> Result<[u8; 16], AdminError> {
    let encoded = value
        .strip_prefix(prefix)
        .ok_or_else(|| AdminError::invalid("the pairing identity is malformed"))?;
    let mut id = [0_u8; 16];
    Base64UrlUnpadded::decode(encoded, &mut id)
        .map_err(|_| AdminError::invalid("the pairing identity is malformed"))?;
    Ok(id)
}

/// Closed local administration failure safe for the existing wire refusal shape.
#[derive(Debug, thiserror::Error)]
pub(crate) enum AdminError {
    #[error("{0}")]
    Invalid(&'static str),
    #[error("{0}")]
    Unavailable(&'static str),
    #[error("paired-device authority could not be updated safely")]
    State,
}

impl AdminError {
    fn invalid(message: &'static str) -> Self {
        Self::Invalid(message)
    }

    fn unavailable(message: &'static str) -> Self {
        Self::Unavailable(message)
    }

    fn state() -> Self {
        Self::State
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use runtrol_security::DeviceScope;
    use runtrol_transport::{InitiatorHandshake, PairingOffer, StaticKeypair};

    use super::*;

    static NEXT_SCRATCH: AtomicU64 = AtomicU64::new(0);

    struct Scratch {
        path: std::path::PathBuf,
    }

    impl Scratch {
        fn make() -> Self {
            let sequence = NEXT_SCRATCH.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "runtrol-pairing-admin-{}-{sequence}",
                std::process::id()
            ));
            std::fs::create_dir(&path).expect("create pairing scratch");
            Self { path }
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ignored = std::fs::remove_dir_all(&self.path);
        }
    }

    #[tokio::test]
    async fn exact_local_approval_persists_then_releases_the_noise_channel() {
        let scratch = Scratch::make();
        let home = scratch.path.to_str().expect("UTF-8 scratch");
        let mut composed = Composed::for_tests(home, runtrol_drivers::builtin()).expect("compose");
        let pc = StaticKeypair::generate().expect("PC key");
        let phone = StaticKeypair::generate().expect("phone key");
        let pc_public = pc.public_key();
        composed.pc_identity = Some(Arc::new(pc));
        let pc = composed.pc_identity.as_ref().expect("installed PC key");
        let (offer, invitation) = PairingOffer::generate().expect("pairing offer");
        composed.pairing_admin.state.lock().await.offer = Some(ActiveOffer { offer });
        let secret = invitation.noise_secret().expect("pairing secret");
        let mut initiator =
            InitiatorHandshake::pairing(&phone, pc_public, &secret).expect("phone initiator");
        let first = initiator
            .write_first(br#"{"name":"Pocket","platform":"Test OS"}"#)
            .expect("first pairing message");

        assert_eq!(
            composed.pairing_admin.receive(pc, 7, [9; 32], &first).await,
            Reception::AwaitingApproval
        );
        let proposals = composed.pairing_admin.proposals().await;
        let proposal = proposals.first().expect("one authenticated proposal");
        let scopes = vec![
            DeviceScope::SessionList.to_string().into(),
            DeviceScope::SessionOutputRead.to_string().into(),
        ];
        let (challenge_id, prompt) = composed
            .pairing_admin
            .begin_approval(&proposal.proposal_id, &scopes)
            .await
            .expect("local approval challenge");
        let phrase = prompt.rsplit_once("type: ").expect("challenge phrase").1;
        let device = composed
            .pairing_admin
            .finish_approval(&composed, &challenge_id, phrase)
            .await
            .expect("durable local approval");

        let outcomes = composed.pairing_admin.take_outcomes(7).await;
        let PairingOutcome::Approved(completed) = outcomes.into_iter().next().expect("one outcome")
        else {
            panic!("approval produced denial");
        };
        assert_eq!(completed.device, device);
        let (mut phone_channel, payload) = initiator
            .finish(&completed.reply)
            .expect("phone accepts PC");
        let reply: serde_json::Value = serde_json::from_slice(&payload).expect("pairing response");
        assert_eq!(
            reply.get("scopes").expect("scopes"),
            &serde_json::json!(["session.list", "session.output.read"])
        );
        assert_eq!(
            reply
                .get("credential")
                .expect("credential field")
                .as_str()
                .expect("credential")
                .len(),
            64
        );
        let grants = composed.device_authority.grants();
        assert!(grants.holds(device, DeviceScope::SessionList));
        assert!(grants.holds(device, DeviceScope::SessionOutputRead));
        let stored = composed
            .store
            .get_device(DeviceKey::from_bytes(*device.as_bytes()))
            .expect("read device")
            .expect("durable device");
        assert_eq!(stored.remote_static_key, phone.public_key().to_bytes());

        let records = phone_channel
            .seal_frame(b"hello Core")
            .expect("phone frame");
        let mut pc_channel = completed.channel;
        assert_eq!(
            pc_channel
                .open_record(records.first().expect("one record"))
                .expect("PC decrypts")
                .as_deref(),
            Some(b"hello Core".as_slice())
        );
    }

    #[tokio::test]
    async fn a_wrong_local_phrase_releases_a_denial_to_the_waiting_relay_peer() {
        let scratch = Scratch::make();
        let home = scratch.path.to_str().expect("UTF-8 scratch");
        let mut composed = Composed::for_tests(home, runtrol_drivers::builtin()).expect("compose");
        let pc = StaticKeypair::generate().expect("PC key");
        let phone = StaticKeypair::generate().expect("phone key");
        let pc_public = pc.public_key();
        composed.pc_identity = Some(Arc::new(pc));
        let pc = composed.pc_identity.as_ref().expect("installed PC key");
        let (offer, invitation) = PairingOffer::generate().expect("pairing offer");
        composed.pairing_admin.state.lock().await.offer = Some(ActiveOffer { offer });
        let secret = invitation.noise_secret().expect("pairing secret");
        let mut initiator =
            InitiatorHandshake::pairing(&phone, pc_public, &secret).expect("phone initiator");
        let first = initiator
            .write_first(br#"{"name":"Pocket","platform":"Test OS"}"#)
            .expect("first pairing message");
        assert_eq!(
            composed
                .pairing_admin
                .receive(pc, 11, [12; 32], &first)
                .await,
            Reception::AwaitingApproval
        );
        let proposal = composed
            .pairing_admin
            .proposals()
            .await
            .into_iter()
            .next()
            .expect("proposal");
        let (challenge_id, _prompt) = composed
            .pairing_admin
            .begin_approval(&proposal.proposal_id, &["session.list".into()])
            .await
            .expect("challenge");
        assert!(
            composed
                .pairing_admin
                .finish_approval(&composed, &challenge_id, "wrong phrase here")
                .await
                .is_err()
        );
        let outcomes = composed.pairing_admin.take_outcomes(11).await;
        assert!(matches!(
            outcomes.first(),
            Some(PairingOutcome::Denied { peer_id, .. }) if *peer_id == [12; 32]
        ));
        assert!(composed.pairing_admin.proposals().await.is_empty());
    }
}
