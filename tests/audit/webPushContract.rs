//! Web Push is a device-bound, content-free doorbell and never another conversation transport.

use std::path::PathBuf;

use base64ct::{Base64UrlUnpadded, Encoding as _};
use runtrol_transport::PushIdentity;

#[path = "productionSource.rs"]
mod production_source;

use production_source::without_tail_test_module;

fn repository() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("audit crate has a repository parent")
        .to_owned()
}

#[test]
fn a_subscription_is_bound_to_one_protected_pc_and_phone() {
    let identity = PushIdentity::derive(&[0x51; 32]).expect("protected machine identity");
    let other_pc = PushIdentity::derive(&[0x52; 32]).expect("different machine identity");
    let endpoint = "https://fcm.googleapis.com/fcm/send/bearer-capability";
    let phone = [0x61; 16];
    let sealed = identity
        .seal_endpoint(phone, endpoint)
        .expect("reviewed endpoint is sealed");

    assert!(
        !sealed
            .windows(endpoint.len())
            .any(|window| window == endpoint.as_bytes()),
        "the bearer capability was stored in plaintext"
    );
    identity
        .validate_stored_endpoint(phone, &sealed)
        .expect("same protected PC and phone restore the capability");
    assert!(
        identity
            .validate_stored_endpoint([0x62; 16], &sealed)
            .is_err()
    );
    assert!(other_pc.validate_stored_endpoint(phone, &sealed).is_err());

    let public_key = Base64UrlUnpadded::decode_vec(&identity.application_server_key())
        .expect("canonical VAPID application-server key");
    assert_eq!(public_key.len(), 65);
    assert_eq!(public_key.first(), Some(&4));
}

#[test]
fn the_production_wake_path_has_an_empty_request_and_a_generic_notification() {
    let repository = repository();
    let push = std::fs::read_to_string(repository.join("crates/runtrol-transport/src/push.rs"))
        .expect("read production push transport");
    let push = without_tail_test_module(&push);
    assert!(push.contains(".body(Full::new(Bytes::new()))"));
    assert!(!push.contains("CONTENT_ENCODING"));
    assert!(!push.contains("CONTENT_TYPE"));

    let worker = std::fs::read_to_string(repository.join("pwa/service-worker.js"))
        .expect("read production service worker");
    assert!(worker.contains("self.addEventListener(\"push\""));
    assert!(worker.contains("Runtrol needs attention"));
    assert!(worker.contains("Open Runtrol to check your PC."));
    assert!(!worker.contains("event.data"));
    for forbidden in ["session", "provider", "workspace", "approval", "prompt"] {
        assert!(
            !worker.to_ascii_lowercase().contains(forbidden),
            "service worker notification names {forbidden} content"
        );
    }
}
