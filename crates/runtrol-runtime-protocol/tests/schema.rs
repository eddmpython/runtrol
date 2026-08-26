//! Checked public Runtime schema contract.

use runtrol_runtime_protocol::{PUBLIC_SCHEMA_NAME, public_schema};

#[test]
fn checked_schema_is_the_exact_generated_public_contract() {
    let generated = public_schema().expect("public schema must serialize");
    let mut rendered = serde_json::to_string_pretty(&generated).expect("public schema must render");
    rendered.push('\n');
    let checked = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("schema")
            .join(PUBLIC_SCHEMA_NAME),
    )
    .expect("checked schema must exist");
    assert_eq!(
        checked, rendered,
        "run the export_schema binary after a public DTO change"
    );
}

#[test]
fn schema_carries_the_revision_and_limit_inventory_used_by_generated_sdks() {
    let schema = public_schema().expect("generate public schema");
    assert_eq!(
        schema.get("x-runtrol-finalized-revisions"),
        Some(&serde_json::json!(["2026-08-27", "2026-08-13"]))
    );
    assert_eq!(
        schema
            .get("x-runtrol-limits")
            .and_then(|limits| limits.get("maxInputBytes")),
        Some(&serde_json::json!(
            runtrol_runtime_protocol::MAX_INPUT_BYTES
        ))
    );
}

#[test]
fn schema_contains_no_provider_enum_or_conversation_history_surface() {
    let schema = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("schema")
            .join(PUBLIC_SCHEMA_NAME),
    )
    .expect("checked schema must exist");
    for forbidden in [
        "transcript",
        "conversationHistory",
        "providerEnum",
        "apiKey",
    ] {
        assert!(
            !schema.contains(forbidden),
            "public schema contains {forbidden:?}"
        );
    }
}
