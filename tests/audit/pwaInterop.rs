//! Cross-implementation proof for the production WebCrypto phone transport.

use std::io::{BufRead as _, BufReader, Write as _};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use base64ct::{Base64UrlUnpadded, Encoding as _};
use runtrol_transport::{
    EncryptedRecord, PairingSecret, PublicKey, ResponderHandshake, SessionBinding, StaticKeypair,
};
use serde::Deserialize;
use serde_json::json;

const ORIGIN: &str = "https://relay.runtrol.test";
const PAIRING_REQUEST: &[u8] = br#"{"name":"Interop phone","platform":"WebCrypto"}"#;
const PAIRING_RESPONSE: &[u8] = br#"{"credential":"1111111111111111111111111111111111111111111111111111111111111111","scopes":["session.list"]}"#;

struct NodePeer {
    child: Child,
    input: Option<ChildStdin>,
    output: BufReader<ChildStdout>,
}

impl NodePeer {
    fn start() -> Self {
        let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(std::path::Path::parent)
            .expect("audit crate has a repository parent")
            .to_owned();
        let mut child = Command::new("node")
            .arg("pwa/test/rust-interop.mjs")
            .current_dir(repository)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("Node starts the checked WebCrypto peer");
        let input = child.stdin.take().expect("Node stdin is piped");
        let output = BufReader::new(child.stdout.take().expect("Node stdout is piped"));
        Self {
            child,
            input: Some(input),
            output,
        }
    }

    fn send(&mut self, value: serde_json::Value) {
        let input = self.input.as_mut().expect("Node stdin remains open");
        serde_json::to_writer(&mut *input, &value).expect("interop command is JSON");
        input.write_all(b"\n").expect("write interop line");
        input.flush().expect("flush interop line");
    }

    fn receive<T: for<'de> Deserialize<'de>>(&mut self) -> T {
        let mut line = String::new();
        let read = self
            .output
            .read_line(&mut line)
            .expect("read WebCrypto interop line");
        assert_ne!(read, 0, "WebCrypto peer ended before its answer");
        serde_json::from_str(&line).expect("WebCrypto peer emits JSON lines")
    }

    fn finish(mut self) {
        drop(self.input.take());
        let status = self.child.wait().expect("wait for WebCrypto peer");
        assert!(status.success(), "WebCrypto peer failed with {status}");
    }
}

impl Drop for NodePeer {
    fn drop(&mut self) {
        let stopped = matches!(self.child.try_wait(), Ok(Some(_)));
        if !stopped {
            if let Err(_kill_error) = self.child.kill() {
                // Drop cannot report a cleanup failure without hiding an earlier test failure.
            }
            if let Err(_wait_error) = self.child.wait() {
                // The operating system still owns final process cleanup when waiting fails.
            }
        }
    }
}

#[derive(Deserialize)]
struct FirstMessage {
    phone_public_key: String,
    first: String,
}

#[derive(Deserialize)]
struct CiphertextMessage {
    record: String,
}

#[derive(Deserialize)]
struct Finished {
    ok: bool,
}

fn decode_key(value: &str) -> PublicKey {
    let mut bytes = [0_u8; 32];
    Base64UrlUnpadded::decode(value, &mut bytes).expect("canonical public key");
    PublicKey::from_bytes(bytes)
}

fn decode_record(value: &str) -> EncryptedRecord {
    let bytes = Base64UrlUnpadded::decode_vec(value).expect("canonical record encoding");
    let (record, consumed) = EncryptedRecord::decode_wire(&bytes).expect("canonical record wire");
    assert_eq!(consumed, bytes.len(), "one exact record");
    record
}

fn encode_record(record: &EncryptedRecord) -> String {
    let mut wire = Vec::new();
    record.append_wire(&mut wire).expect("record wire encoding");
    Base64UrlUnpadded::encode_string(&wire)
}

#[test]
fn webcrypto_and_rust_pair_then_open_a_fresh_noise_session() {
    let pc = StaticKeypair::from_private(&[7_u8; 32]).expect("fixed test PC identity");
    let qr = [9_u8; 16];
    let secret = PairingSecret::from_qr(qr).expect("pairing secret");
    let peer_id = [5_u8; 32];
    let mut node = NodePeer::start();
    node.send(json!({
        "pc_public_key": Base64UrlUnpadded::encode_string(&pc.public_key().to_bytes()),
        "pairing_secret": Base64UrlUnpadded::encode_string(&qr),
        "relay_origin": ORIGIN,
        "peer_id": Base64UrlUnpadded::encode_string(&peer_id),
    }));

    let pairing: FirstMessage = node.receive();
    let phone_public = decode_key(&pairing.phone_public_key);
    let first = decode_record(&pairing.first);
    let (mut pairing_channel, reply, request) =
        ResponderHandshake::pairing(&pc, phone_public, &secret)
            .expect("Rust pairing responder")
            .answer(&first, PAIRING_RESPONSE)
            .expect("WebCrypto pairing message authenticates");
    assert_eq!(request, PAIRING_REQUEST);
    node.send(json!({ "reply": encode_record(&reply) }));

    let encrypted: CiphertextMessage = node.receive();
    let pairing_frame = pairing_channel
        .open_record(&decode_record(&encrypted.record))
        .expect("Rust opens pairing transport record")
        .expect("pairing transport record completes a frame");
    assert_eq!(pairing_frame, b"paired transport from WebCrypto");
    let pairing_reply = pairing_channel
        .seal_frame(b"paired transport from Rust")
        .expect("Rust seals pairing transport reply")
        .remove(0);
    node.send(json!({ "record": encode_record(&pairing_reply) }));

    let session: FirstMessage = node.receive();
    assert_eq!(decode_key(&session.phone_public_key), phone_public);
    let binding = SessionBinding::relay(ORIGIN, peer_id).expect("relay binding");
    let (mut session_channel, reply, request) =
        ResponderHandshake::session(&pc, phone_public, &binding)
            .expect("Rust session responder")
            .answer(&decode_record(&session.first), &[])
            .expect("WebCrypto session message authenticates");
    assert!(request.is_empty());
    node.send(json!({ "reply": encode_record(&reply) }));

    let encrypted: CiphertextMessage = node.receive();
    let session_frame = session_channel
        .open_record(&decode_record(&encrypted.record))
        .expect("Rust opens session transport record")
        .expect("session transport record completes a frame");
    assert_eq!(session_frame, b"session transport from WebCrypto");
    let session_reply = session_channel
        .seal_frame(b"session transport from Rust")
        .expect("Rust seals session transport reply")
        .remove(0);
    node.send(json!({ "record": encode_record(&session_reply) }));

    let encrypted: CiphertextMessage = node.receive();
    let session_frame = session_channel
        .open_record(&decode_record(&encrypted.record))
        .expect("Rust opens the second session transport record")
        .expect("the second session transport record completes a frame");
    assert_eq!(session_frame, b"session nonce one from WebCrypto");
    let session_reply = session_channel
        .seal_frame(b"session nonce one from Rust")
        .expect("Rust seals the second session transport reply")
        .remove(0);
    node.send(json!({ "record": encode_record(&session_reply) }));

    let finished: Finished = node.receive();
    assert!(finished.ok, "WebCrypto peer verified both Rust replies");
    node.finish();
}
