//! The phone transport is a ciphertext-only egress boundary.
//!
//! This gate uses real loopback sockets and the production Noise implementation. The relay fixture records every
//! byte it would be able to observe, so the assertion is about the actual wire payload rather than a mock cipher.

use std::fs;
use std::net::{IpAddr, Ipv4Addr};
use std::path::{Path, PathBuf};
use std::time::Duration;

use runtrol_transport::{
    EgressPolicy, EncryptedRecord, InitiatorHandshake, LinkKind, MAX_NOISE_PLAINTEXT,
    PairingSecret, ResponderHandshake, SessionBinding, StaticKeypair,
};
use tokio::net::TcpListener;

#[path = "productionSource.rs"]
mod production_source;

use production_source::without_tail_test_module;

const LOOPBACK: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);

#[tokio::test]
async fn only_an_exact_approved_destination_can_be_dialed() {
    let allowed_listener = TcpListener::bind((LOOPBACK, 0))
        .await
        .expect("allowed listener");
    let blocked_listener = TcpListener::bind((LOOPBACK, 0))
        .await
        .expect("blocked listener");
    let allowed = allowed_listener.local_addr().expect("allowed address");
    let blocked = blocked_listener.local_addr().expect("blocked address");
    let policy = EgressPolicy::new([allowed]);

    let destination = policy.approve(allowed).expect("allowlisted address");
    let (connected, accepted) =
        tokio::join!(policy.connect(destination), allowed_listener.accept());
    assert!(connected.is_ok());
    assert!(accepted.is_ok());

    assert!(policy.approve(blocked).is_err());
    assert!(EgressPolicy::new([]).approve(allowed).is_err());
    assert!(
        tokio::time::timeout(Duration::from_millis(40), blocked_listener.accept())
            .await
            .is_err(),
        "a refused destination received a connection"
    );
}

#[test]
fn session_channel_is_mutually_authenticated_bound_and_ciphertext_only() {
    let phone = StaticKeypair::generate().expect("phone key");
    let pc = StaticKeypair::generate().expect("pc key");
    let binding = SessionBinding::direct(LinkKind::Lan, [7; 32]).expect("LAN binding");
    let wrong_binding =
        SessionBinding::relay("https://relay.runtrol.test", [7; 32]).expect("relay binding");

    let mut initiator =
        InitiatorHandshake::session(&phone, pc.public_key(), &binding).expect("session initiator");
    let first = initiator
        .write_first(b"subscribe from offset 41")
        .expect("first message");

    let wrong = ResponderHandshake::session(&pc, phone.public_key(), &wrong_binding)
        .expect("wrong-bound responder")
        .answer(&first, b"ack");
    assert!(
        wrong.is_err(),
        "a relay handshake was accepted as a LAN link"
    );

    let (mut pc_channel, reply, hello) =
        ResponderHandshake::session(&pc, phone.public_key(), &binding)
            .expect("session responder")
            .answer(&first, b"ack from pc")
            .expect("authenticated response");
    assert_eq!(hello, b"subscribe from offset 41");
    let (mut phone_channel, ack) = initiator.finish(&reply).expect("finish session");
    assert_eq!(ack, b"ack from pc");

    let secret = b"prompt body: rotate database credential before lunch";
    let mut frame = vec![0x05; MAX_NOISE_PLAINTEXT + 731];
    frame[97..97 + secret.len()].copy_from_slice(secret);
    let records = phone_channel.seal_frame(&frame).expect("chunk and seal");
    assert!(
        records.len() > 1,
        "the fixture must cross the Noise message cap"
    );
    assert!(records.iter().all(|record| {
        !record
            .as_ciphertext()
            .windows(secret.len())
            .any(|window| window == secret)
    }));
    assert!(!format!("{records:?}").contains("prompt body"));

    let mut relay_capture = Vec::new();
    for record in &records {
        record
            .append_wire(&mut relay_capture)
            .expect("length-prefix record");
    }
    assert!(
        !relay_capture
            .windows(secret.len())
            .any(|window| window == secret)
    );
    let mut cursor = 0;
    for expected in &records {
        let (decoded, consumed) =
            EncryptedRecord::decode_wire(&relay_capture[cursor..]).expect("decode wire record");
        assert_eq!(&decoded, expected);
        cursor += consumed;
    }
    assert_eq!(cursor, relay_capture.len());

    let mut opened = None;
    for record in &records {
        let next = pc_channel.open_record(record).expect("open ordered record");
        if next.is_some() {
            assert!(opened.is_none());
            opened = next;
        }
    }
    assert_eq!(opened.as_deref(), Some(frame.as_slice()));

    let rekey = phone_channel.request_rekey().expect("rekey signal");
    assert_eq!(
        pc_channel.open_record(&rekey).expect("authenticate rekey"),
        None
    );
    let after_rekey = phone_channel
        .seal_frame(b"after rekey")
        .expect("seal after rekey");
    assert_eq!(after_rekey.len(), 1);
    assert_eq!(
        pc_channel
            .open_record(&after_rekey[0])
            .expect("open after rekey")
            .as_deref(),
        Some(b"after rekey".as_slice())
    );
}

#[test]
fn ciphertext_tampering_and_wrong_static_key_are_rejected() {
    let phone = StaticKeypair::generate().expect("phone key");
    let pc = StaticKeypair::generate().expect("pc key");
    let stranger = StaticKeypair::generate().expect("stranger key");
    let binding = SessionBinding::direct(LinkKind::PeerToPeer, [11; 32]).expect("binding");

    let mut initiator =
        InitiatorHandshake::session(&phone, pc.public_key(), &binding).expect("session initiator");
    let first = initiator.write_first(b"hello").expect("first message");
    let rejected = ResponderHandshake::session(&pc, stranger.public_key(), &binding)
        .expect("responder")
        .answer(&first, b"ack");
    assert!(rejected.is_err(), "an unpaired initiator was accepted");

    let (mut receiver, reply, _) = ResponderHandshake::session(&pc, phone.public_key(), &binding)
        .expect("session responder")
        .answer(&first, b"ack")
        .expect("response");
    let (mut sender, _) = initiator.finish(&reply).expect("finish");
    let record = sender
        .seal_frame(b"approval response")
        .expect("seal")
        .remove(0);
    let mut tampered = record.as_ciphertext().to_vec();
    let last = tampered.len() - 1;
    tampered[last] ^= 0x80;
    let tampered = EncryptedRecord::from_ciphertext(tampered).expect("record shape");
    assert!(receiver.open_record(&tampered).is_err());
}

#[test]
fn pairing_uses_the_one_time_qr_secret() {
    let phone = StaticKeypair::generate().expect("phone key");
    let pc = StaticKeypair::generate().expect("pc key");
    let psk = PairingSecret::from_qr([0x31; 16]).expect("pairing secret");
    let wrong_psk = PairingSecret::from_qr([0x32; 16]).expect("wrong secret");

    let mut initiator =
        InitiatorHandshake::pairing(&phone, pc.public_key(), &psk).expect("pairing initiator");
    let first = initiator
        .write_first(b"phone identity")
        .expect("pairing message");
    let rejected = ResponderHandshake::pairing(&pc, phone.public_key(), &wrong_psk)
        .expect("wrong-psk responder")
        .answer(&first, b"pc identity");
    assert!(rejected.is_err(), "pairing accepted the wrong QR secret");

    let (_, reply, phone_payload) = ResponderHandshake::pairing(&pc, phone.public_key(), &psk)
        .expect("pairing responder")
        .answer(&first, b"pc identity")
        .expect("pairing response");
    assert_eq!(phone_payload, b"phone identity");
    let (_, pc_payload) = initiator.finish(&reply).expect("finish pairing");
    assert_eq!(pc_payload, b"pc identity");
}

#[test]
fn product_socket_dials_exist_only_inside_the_egress_policy() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .to_path_buf();
    let mut callers = Vec::new();
    visit_rs(&root.join("crates"), &mut |path, source| {
        if without_tail_test_module(source).contains("TcpStream::connect") {
            callers.push(
                path.strip_prefix(&root)
                    .expect("relative path")
                    .to_path_buf(),
            );
        }
    });
    assert_eq!(
        callers,
        [PathBuf::from("crates/runtrol-transport/src/egress.rs")]
    );

    for crate_name in ["runtrol-drivers", "runtrol-store"] {
        visit_rs(
            &root.join("crates").join(crate_name),
            &mut |path, source| {
                assert!(
                    !source.contains(".claude/projects")
                        && !source.contains(".codex/sessions")
                        && !source.contains("session.jsonl"),
                    "{} names a provider-owned transcript path",
                    path.display()
                );
            },
        );
    }

    visit_rs(
        &root.join("crates/runtrol-transport/src"),
        &mut |path, source| {
            for forbidden in [
                "std::fs",
                "tokio::fs",
                "File::open",
                "OpenOptions",
                "tracing::",
                "log::",
                "println!",
                "eprintln!",
            ] {
                assert!(
                    !source.contains(forbidden),
                    "{} can persist or log transport content through `{forbidden}`",
                    path.display()
                );
            }
        },
    );
}

fn visit_rs(directory: &Path, visit: &mut impl FnMut(&Path, &str)) {
    for entry in fs::read_dir(directory).expect("read directory") {
        let entry = entry.expect("directory entry");
        let path = entry.path();
        if path.is_dir() {
            visit_rs(&path, visit);
        } else if path.extension().and_then(|extension| extension.to_str()) == Some("rs") {
            let source = fs::read_to_string(&path).expect("UTF-8 Rust source");
            visit(&path, &source);
        }
    }
}
