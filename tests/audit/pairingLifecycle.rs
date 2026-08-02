//! Pairing is short-lived, single-use, attempt-limited, and physically approved for the exact device.

use std::path::{Path, PathBuf};
use std::time::Duration;

use runtrol_provider::{AbsPath, WallMs};
use runtrol_security::{
    DeviceScope, GrantLedger, GrantRequest, LocalConsole, LocalScope, PairingIdentity,
    PresenceChallenge,
};
use runtrol_store::{DeviceKey, DeviceRow, Store};
use runtrol_transport::{
    AccessToken, CredentialFingerprint, CryptoError, InitiatorHandshake, PairingOffer,
    PairingSecret, StaticKeypair,
};

const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

struct Scratch {
    root: AbsPath,
    database: AbsPath,
}

impl Scratch {
    fn make() -> Self {
        let base = std::env::temp_dir().join("runtrol-pairing-lifecycle");
        if base.exists() {
            std::fs::remove_dir_all(&base).expect("clear previous pairing gate data");
        }
        std::fs::create_dir_all(&base).expect("create pairing gate directory");
        let root = AbsPath::canonicalize(base.to_str().expect("temporary path is UTF-8"))
            .expect("canonicalize pairing gate directory");
        let database = root.join("runtrol.redb").expect("valid database name");
        Self { root, database }
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_dir_all(self.root.as_std_path()) {
            eprintln!("could not clean up {}: {error}", self.root);
        }
    }
}

fn rust_sources_below(directory: &Path, found: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(directory).expect("read production source directory") {
        let entry = entry.expect("read production source entry");
        let path = entry.path();
        if path.is_dir() {
            rust_sources_below(&path, found);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            found.push(path);
        }
    }
}

#[test]
fn a_pairing_finishes_only_after_exact_pc_presence() {
    let phone = StaticKeypair::generate().expect("phone key");
    let pc = StaticKeypair::generate().expect("pc key");
    let (mut offer, invitation) = PairingOffer::generate().expect("pairing offer");
    assert_eq!(PairingOffer::valid_for(), Duration::from_secs(120));
    assert!(invitation.expires_at_unix_ms() > 0);

    let secret = invitation.noise_secret().expect("QR secret");
    let mut initiator =
        InitiatorHandshake::pairing(&phone, pc.public_key(), &secret).expect("pairing initiator");
    let first = initiator
        .write_first(br#"{"name":"Pixel 9","platform":"Android"}"#)
        .expect("pairing message");
    let pending = offer.receive(&pc, &first).expect("authenticated proposal");
    assert!(
        offer.is_spent(),
        "the QR remained reusable after message one"
    );
    assert_eq!(pending.remote_public_key(), phone.public_key());

    let identity = pending
        .identity("Pixel 9", "Android")
        .expect("display identity");
    let request = pending.approval_request(&identity).expect("bound request");
    let console = LocalConsole::claim().expect("one local surface");
    let challenge = PresenceChallenge::issue(&console, request).expect("presence challenge");
    let prompt = challenge.prompt();
    assert!(prompt.contains("with key"));
    assert!(prompt.contains("Android"));
    assert!(prompt.contains("Pixel 9"));
    let phrase = prompt.rsplit_once("type: ").expect("challenge phrase").1;
    let witness = challenge.answer(phrase).expect("physical approval");

    let approved = pending
        .approve(&identity, &witness, b"pc identity")
        .expect("approved pairing");
    assert_eq!(approved.remote_public_key(), phone.public_key());
    assert_ne!(approved.device_id().as_bytes(), &[0; 16]);
    assert_eq!(
        approved.initiator_payload(),
        br#"{"name":"Pixel 9","platform":"Android"}"#
    );

    let scratch = Scratch::make();
    let device = approved.device_id();
    let token = AccessToken::parse(TOKEN).expect("canonical bearer token");
    let approved_scopes = [DeviceScope::SessionList, DeviceScope::ApprovalRespondLow];
    let row = DeviceRow {
        remote_static_key: approved.remote_public_key().to_bytes(),
        credential_fingerprint: token.fingerprint().to_bytes(),
        name: identity.name().into(),
        platform: identity.platform().into(),
        scopes: approved_scopes
            .iter()
            .map(|scope| scope.to_string().into())
            .collect(),
        paired_at: WallMs::now(),
    };
    let store = Store::open(&scratch.database).expect("open authorization store");
    store
        .put_device(DeviceKey::from_bytes(*device.as_bytes()), &row)
        .expect("persist exact approved identity and scopes");
    drop(store);

    let reopened = Store::open(&scratch.database).expect("reopen after daemon restart");
    let listed = reopened.list_devices().expect("list restored devices");
    assert!(listed.unreadable.is_empty());
    let (stored_key, stored_row) = listed.devices.first().expect("one restored device");
    assert_eq!(stored_key.to_bytes(), *device.as_bytes());
    assert_eq!(stored_row.remote_static_key, phone.public_key().to_bytes());
    let restored_scopes: Vec<DeviceScope> = stored_row
        .scopes
        .iter()
        .map(|scope| DeviceScope::from_stored(scope).expect("known approved scope"))
        .collect();
    let ledger = GrantLedger::from_persisted([(device, restored_scopes)]);
    for scope in approved_scopes {
        assert!(ledger.holds(device, scope));
    }
    let fingerprint = CredentialFingerprint::from_bytes(stored_row.credential_fingerprint);
    assert_eq!(fingerprint, token.fingerprint());
    drop(reopened);
    let database_bytes =
        std::fs::read(scratch.database.as_std_path()).expect("read database bytes");
    assert!(
        !database_bytes
            .windows(TOKEN.len())
            .any(|window| window == TOKEN.as_bytes()),
        "the bearer token was copied into the durable store"
    );

    let (mut phone_channel, response) = initiator
        .finish(approved.reply())
        .expect("phone finishes pairing");
    assert_eq!(response, b"pc identity");
    let mut pc_channel = approved.into_channel();
    let record = phone_channel
        .seal_frame(b"first paired message")
        .expect("seal after approval")
        .remove(0);
    assert_eq!(
        pc_channel
            .open_record(&record)
            .expect("open after approval")
            .as_deref(),
        Some(b"first paired message".as_slice())
    );

    assert!(matches!(
        offer.receive(&pc, &first),
        Err(CryptoError::PairingSpent)
    ));

    let (mut unrelated_offer, unrelated_invitation) =
        PairingOffer::generate().expect("unrelated offer");
    let unrelated_secret = unrelated_invitation
        .noise_secret()
        .expect("unrelated secret");
    let mut unrelated_initiator =
        InitiatorHandshake::pairing(&phone, pc.public_key(), &unrelated_secret)
            .expect("unrelated initiator");
    let unrelated_first = unrelated_initiator
        .write_first(b"same labels, different attempt")
        .expect("unrelated first message");
    let unrelated_pending = unrelated_offer
        .receive(&pc, &unrelated_first)
        .expect("unrelated pending");
    let unrelated_identity = unrelated_pending
        .identity("Pixel 9", "Android")
        .expect("unrelated identity");
    let generic = PresenceChallenge::issue(&console, GrantRequest::Local(LocalScope::DevicePair))
        .expect("generic challenge");
    let generic_prompt = generic.prompt();
    let generic_phrase = generic_prompt
        .rsplit_once("type: ")
        .expect("generic phrase")
        .1;
    let generic_witness = generic.answer(generic_phrase).expect("generic presence");
    assert!(matches!(
        unrelated_pending.approve(&unrelated_identity, &generic_witness, b"must not send"),
        Err(CryptoError::PairingNeedsPcApproval)
    ));
}

#[test]
fn five_bad_messages_lock_the_offer() {
    let phone = StaticKeypair::generate().expect("phone key");
    let pc = StaticKeypair::generate().expect("pc key");
    let (mut offer, _) = PairingOffer::generate().expect("pairing offer");
    let wrong = PairingSecret::from_qr([0xa5; 16]).expect("wrong secret");
    let mut initiator = InitiatorHandshake::pairing(&phone, pc.public_key(), &wrong)
        .expect("wrong pairing initiator");
    let first = initiator.write_first(b"attacker").expect("bad message");

    for remaining in (0..5).rev() {
        assert!(offer.receive(&pc, &first).is_err());
        assert_eq!(offer.remaining_attempts(), remaining);
    }
    assert!(matches!(
        offer.receive(&pc, &first),
        Err(CryptoError::PairingLocked)
    ));
}

#[test]
fn untrusted_device_labels_cannot_rewrite_the_presence_prompt() {
    assert!(PairingIdentity::new([1; 16], [2; 32], "", "Android").is_err());
    assert!(PairingIdentity::new([1; 16], [2; 32], "phone\nDo something else", "Android").is_err());
    assert!(PairingIdentity::new([1; 16], [2; 32], "phone", "Android\rhidden").is_err());
    assert!(PairingIdentity::new([1; 16], [2; 32], "phone\u{202e}txt", "Android").is_err());
    assert!(PairingIdentity::new([1; 16], [2; 32], "x".repeat(65), "Android").is_err());
}

#[test]
fn persisted_grants_enter_only_through_daemon_assembly() {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("canonical repository root");
    let crates = repository.join("crates");
    let mut sources = Vec::new();
    rust_sources_below(&crates, &mut sources);
    sources.sort();

    let mut hits = Vec::new();
    for source in sources {
        let text = std::fs::read_to_string(&source).expect("read production Rust source");
        let count = text.matches("GrantLedger::from_persisted(").count();
        if count > 0 {
            let relative = source
                .strip_prefix(&repository)
                .expect("source is below repository")
                .to_string_lossy()
                .replace('\\', "/");
            hits.push((relative, count));
        }
    }

    assert_eq!(
        hits,
        vec![
            ("crates/runtrol-daemon/src/compose.rs".to_owned(), 1),
            ("crates/runtrol-security/src/grant.rs".to_owned(), 1),
        ],
        "persisted authority gained another production entry point"
    );
}

#[cfg(windows)]
#[test]
fn the_pc_noise_identity_survives_restart_only_as_a_dpapi_blob() {
    let scratch = Scratch::make();
    let vault_path = scratch
        .root
        .join("machine-identity.vault")
        .expect("valid vault file name");
    let first = runtrol_vault::MachineSecret::load_or_create(&vault_path)
        .expect("create DPAPI-protected identity");
    let raw = *first.as_bytes();
    let public = StaticKeypair::from_private(first.as_bytes())
        .expect("derive first PC identity")
        .public_key();
    let on_disk = std::fs::read(vault_path.as_std_path()).expect("read protected vault");
    assert!(
        !on_disk.windows(raw.len()).any(|window| window == raw),
        "the PC private key appeared in the vault file"
    );
    drop(first);

    let restored = runtrol_vault::MachineSecret::load_or_create(&vault_path)
        .expect("restore DPAPI-protected identity");
    assert_eq!(
        StaticKeypair::from_private(restored.as_bytes())
            .expect("derive restored PC identity")
            .public_key(),
        public
    );
}
