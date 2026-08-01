//! Gate: background children never create a visible Windows console.
//!
//! A GUI launch is not complete when only the GUI process hides its own console. Provider sessions and
//! discovery probes are console programs too, and Windows may create a new console window for each one unless
//! every spawn boundary explicitly opts out. That window is brief for probes, so the defect presents as a flash.

use std::path::{Path, PathBuf};
use std::process::Command;

/// The repository root above this audit crate.
fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("the audit crate lives below the repository root")
        .to_path_buf()
}

/// Read one product source file.
fn source(relative: &str) -> String {
    let path = root().join(relative);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()))
}

#[test]
fn child_processes_have_one_cross_platform_console_policy() {
    let mut command = Command::new("not-started");
    runtrol_childproc::hide_console_window(&mut command);
}

#[test]
fn every_product_spawn_boundary_applies_the_policy() {
    let async_spawns = [
        "crates/runtrol-childproc/src/run.rs",
        "crates/runtrol-drivers/src/claude/agent.rs",
        "crates/runtrol-drivers/src/codex/conn.rs",
        "crates/runtrol-drivers/src/acp/agent.rs",
    ];

    for relative in async_spawns {
        let text = source(relative);
        assert!(
            text.contains("hide_console_window(command.as_std_mut())"),
            "{relative} starts a background child without suppressing a Windows console window"
        );
    }

    let daemon = source("crates/runtrol-cli/src/link.rs");
    assert!(
        daemon.contains("CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP"),
        "the detached daemon can still create a visible Windows console"
    );
    assert!(
        !daemon.contains("const DETACHED_PROCESS"),
        "DETACHED_PROCESS makes CREATE_NO_WINDOW ineffective and must not be used here"
    );
}
