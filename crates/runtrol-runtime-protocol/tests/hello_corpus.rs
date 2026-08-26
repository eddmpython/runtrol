//! Every hello that ever shipped must stay readable, and every new hello shape must join the corpus.
//!
//! The corpus in `hello_corpus/` holds one `InitializeResult` per shipped shape of the hello,
//! extracted from the exact source of the release that first sent it. A client that cannot read
//! one of these would brick against an installed daemon of that age, which happened on
//! 2026-08-20: required limits fields joined a finalized revision and every installed daemon
//! failed the new client's schema at hello. These tests are that incident, made permanent.

use runtrol_runtime_protocol::{
    InitializeResult, REVISION_2026_08_27, RuntimeCapabilities, RuntimeInstance, RuntimeLimits,
};

#[expect(
    clippy::expect_used,
    reason = "a test helper outside #[test] bodies, which the allow-expect-in-tests escape does not cover"
)]
fn corpus() -> Vec<(String, serde_json::Value)> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("hello_corpus");
    let mut fixtures = Vec::new();
    for entry in std::fs::read_dir(&root).expect("the hello corpus directory exists") {
        let path = entry.expect("corpus entry").path();
        if path
            .extension()
            .is_some_and(|extension| extension == "json")
        {
            let body = std::fs::read_to_string(&path).expect("corpus fixture is readable");
            let name = path
                .file_name()
                .expect("fixture name")
                .to_string_lossy()
                .into_owned();
            fixtures.push((
                name,
                serde_json::from_str(&body).expect("corpus fixture is JSON"),
            ));
        }
    }
    assert!(!fixtures.is_empty(), "an empty corpus guards nothing");
    fixtures.sort_by(|a, b| a.0.cmp(&b.0));
    fixtures
}

/// Every field path in a JSON object tree, so shapes compare by structure and not by values.
fn key_paths(value: &serde_json::Value, prefix: &str, into: &mut Vec<String>) {
    if let serde_json::Value::Object(map) = value {
        for (key, child) in map {
            let path = if prefix.is_empty() {
                key.clone()
            } else {
                format!("{prefix}.{key}")
            };
            key_paths(child, &path, into);
            into.push(path);
        }
    }
}

#[test]
fn every_shipped_hello_still_deserializes() {
    for (name, fixture) in corpus() {
        let parsed: Result<InitializeResult, _> = serde_json::from_value(fixture.clone());
        assert!(
            parsed.is_ok(),
            "the shipped hello {name} no longer deserializes: {:?}. A field became required or \
             was removed inside a finalized revision; make it `serde(default)` instead.",
            parsed.err(),
        );
    }
}

#[test]
fn the_current_hello_shape_is_in_the_corpus() {
    let current = InitializeResult {
        selected_revision: REVISION_2026_08_27,
        runtime: RuntimeInstance {
            instance_id: "rtm_fixture".to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            platform: "test".to_owned(),
            build_digest: Some("0".repeat(64)),
        },
        server_capabilities: RuntimeCapabilities {
            integration_enrollment: true,
            provider_inventory: true,
            managed_session_list: true,
            model_discovery: true,
            native_session_catalogue: true,
            session_control: true,
            session_events: true,
            terminal_surface: true,
        },
        limits: RuntimeLimits::default(),
        grant: None,
    };
    let serialized = serde_json::to_value(&current).expect("the current hello serializes");
    let mut current_shape = Vec::new();
    key_paths(&serialized, "", &mut current_shape);
    current_shape.sort();

    let mut shapes = Vec::new();
    for (name, fixture) in corpus() {
        let mut shape = Vec::new();
        key_paths(&fixture, "", &mut shape);
        shape.sort();
        if shape == current_shape {
            return;
        }
        shapes.push(name);
    }
    panic!(
        "the current hello shape is not in the corpus (checked {shapes:?}). Add a fixture for it \
         to hello_corpus/ so future clients are forced to keep reading this exact shape.",
    );
}
