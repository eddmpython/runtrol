//! Pairing is short-lived, single-use, attempt-limited, and physically approved for the exact device.

use std::time::Duration;

use runtrol_security::{
    GrantRequest, LocalConsole, LocalScope, PairingIdentity, PresenceChallenge,
};
use runtrol_transport::{
    CryptoError, InitiatorHandshake, PairingOffer, PairingSecret, StaticKeypair,
};

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
