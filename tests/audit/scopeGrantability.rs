//! Gate: capabilities reserved for somebody at the machine cannot enter a device grant.
//!
//! The compiler proves the negative claim in `GrantLedger::grant`'s `compile_fail` example. This gate keeps
//! that proof attached to the public method and keeps the method's accepted type on the device side of the
//! wall. `cargo test --all` runs both this test and the documentation test.

use std::fs;
use std::path::PathBuf;

fn source(relative: &str) -> String {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("the audit crate is inside the repository")
        .to_path_buf();
    fs::read_to_string(root.join(relative)).expect("the security contract source must be readable")
}

#[test]
fn the_device_grant_carries_a_compiler_checked_negative_example() {
    let grant = source("crates/runtrol-security/src/grant.rs");
    assert!(
        grant.contains("```compile_fail")
            && grant.contains("&[LocalScope::ConfigWrite]")
            && grant.contains("scopes: &[DeviceScope]"),
        "GrantLedger::grant must prove that a LocalScope is not a DeviceScope"
    );
}

#[test]
fn no_conversion_can_turn_a_local_scope_into_a_device_scope() {
    let scope = source("crates/runtrol-security/src/scope.rs");
    for forbidden in [
        "impl From<LocalScope> for DeviceScope",
        "impl TryFrom<LocalScope> for DeviceScope",
        "impl Into<DeviceScope> for LocalScope",
    ] {
        assert!(
            !scope.contains(forbidden),
            "the type wall was opened by `{forbidden}`"
        );
    }
}
