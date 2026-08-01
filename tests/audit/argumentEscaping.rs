//! Gate: every argument is validated before the standard library quotes it for the operating system.
//!
//! This is the platform-independent half. The Windows integration test in
//! `crates/runtrol-childproc/tests/argument_escaping.rs` drives a real command file and proves the standard
//! library's quoting prevents metacharacter injection.

use runtrol_childproc::{MAX_ARGUMENT_LEN, check_all, check_one};

#[test]
fn metacharacters_reach_the_standard_library_and_controls_do_not() {
    let metacharacters = [
        "a&echo injected",
        "a|whoami",
        "a^b",
        "%PATH%",
        "a>out.txt",
        "$(whoami)",
        "`whoami`",
    ];
    assert!(check_all(&metacharacters).is_ok());

    for unsafe_argument in ["a\nb", "a\rb", "a\0b", "a\u{1b}[31m"] {
        assert!(check_one(0, unsafe_argument).is_err());
    }
    assert!(check_one(0, &"a".repeat(MAX_ARGUMENT_LEN + 1)).is_err());
}

#[test]
fn the_real_windows_command_file_smoke_is_registered() {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("the audit crate is inside the repository")
        .to_path_buf();
    let smoke = root.join("crates/runtrol-childproc/tests/argument_escaping.rs");
    assert!(smoke.is_file(), "{} is missing", smoke.display());
}
