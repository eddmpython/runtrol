//! The publishable Runtime surface remains provider-neutral and cannot import private control or Core authority.

use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .unwrap_or_else(|| panic!("audit manifest has no repository root"))
        .to_owned()
}

fn rust_sources(relative: &str) -> String {
    let source_root = root().join(relative).join("src");
    let mut paths: Vec<PathBuf> = std::fs::read_dir(&source_root)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", source_root.display()))
        .map(|entry| entry.unwrap_or_else(|error| panic!("cannot read a source entry: {error}")))
        .flat_map(|entry| {
            let path = entry.path();
            if path.is_dir() {
                std::fs::read_dir(path)
                    .into_iter()
                    .flatten()
                    .map(|entry| {
                        entry.unwrap_or_else(|error| {
                            panic!("cannot read a nested source entry: {error}")
                        })
                    })
                    .map(|nested| nested.path())
                    .collect::<Vec<_>>()
            } else {
                vec![path]
            }
        })
        .filter(|path| path.extension().and_then(|one| one.to_str()) == Some("rs"))
        .collect();
    paths.sort();
    paths
        .into_iter()
        .map(|path| {
            std::fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn text_sources(relative: &str, extensions: &[&str]) -> String {
    fn collect(path: &Path, extensions: &[&str], found: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(path)
            .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()))
        {
            let path = entry
                .unwrap_or_else(|error| panic!("cannot read source entry: {error}"))
                .path();
            if path.is_dir() {
                collect(&path, extensions, found);
            } else if path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extensions.contains(&extension))
            {
                found.push(path);
            }
        }
    }

    let source_root = root().join(relative);
    let mut paths = Vec::new();
    collect(&source_root, extensions, &mut paths);
    paths.sort();
    paths
        .into_iter()
        .map(|path| {
            std::fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn publishable_crates_import_no_private_runtime_authority() {
    let protocol = rust_sources("crates/runtrol-runtime-protocol");
    let client = rust_sources("crates/runtrol-runtime-client");
    let manifest = std::fs::read_to_string(root().join("crates/runtrol-runtime-client/Cargo.toml"))
        .unwrap_or_else(|error| panic!("cannot read Runtime client manifest: {error}"));
    for forbidden in [
        "runtrol_core",
        "runtrol_daemon",
        "runtrol_drivers",
        "runtrol_ipc",
        "runtrol_store",
        "runtrol_vault",
        "runtrol-core",
        "runtrol-daemon",
        "runtrol-drivers",
        "runtrol-ipc",
        "runtrol-store",
        "runtrol-vault",
    ] {
        assert!(
            !protocol.contains(forbidden),
            "public protocol imports private authority {forbidden:?}"
        );
        assert!(
            !client.contains(forbidden),
            "public client imports private authority {forbidden:?}"
        );
        assert!(
            !manifest.contains(forbidden),
            "public client declares private authority {forbidden:?}"
        );
    }
}

#[test]
fn public_method_table_has_no_private_control_vocabulary_or_provider_enum() {
    let method =
        std::fs::read_to_string(root().join("crates/runtrol-runtime-protocol/src/method.rs"))
            .unwrap_or_else(|error| panic!("cannot read public method table: {error}"));
    let schema = std::fs::read_to_string(
        root().join("crates/runtrol-runtime-protocol/schema/runtime.schema.json"),
    )
    .unwrap_or_else(|error| panic!("cannot read public schema: {error}"));
    for private in [
        "Request::",
        "Response::",
        "providerUpdate",
        "consultWire",
        "consultUnwire",
        "WIRE_VERSION",
    ] {
        assert!(
            !method.contains(private),
            "public methods contain {private:?}"
        );
        assert!(
            !schema.contains(private),
            "public schema contains {private:?}"
        );
    }
    for provider_specific in ["claude", "codex", "gemini"] {
        assert!(
            !schema.to_ascii_lowercase().contains(provider_specific),
            "public schema contains provider-specific enum or branch {provider_specific:?}"
        );
    }
}

#[test]
fn public_daemon_dispatch_cannot_deserialize_private_control_requests() {
    let source = std::fs::read_to_string(root().join("crates/runtrol-daemon/src/runtime_serve.rs"))
        .unwrap_or_else(|error| panic!("cannot read public Runtime dispatcher: {error}"));
    for forbidden in ["runtrol_ipc::wire", "crate::dispatch", "SessionManager"] {
        assert!(
            !source.contains(forbidden),
            "public Runtime dispatcher reaches private boundary {forbidden:?}"
        );
    }
}

#[test]
fn publishable_typescript_client_imports_no_private_runtime_authority() {
    let source = text_sources("clients/typescript/src", &["ts"]);
    for forbidden in [
        "extensions/runtrol-vscode",
        "runtrol-core",
        "runtrol-daemon",
        "runtrol-drivers",
        "runtrol-ipc",
        "runtrol-store",
        "runtrol-vault",
    ] {
        assert!(
            !source.contains(forbidden),
            "public TypeScript client reaches private authority {forbidden:?}"
        );
    }
}

#[test]
fn studio_public_read_modules_use_the_package_and_not_private_ipc() {
    for relative in ["runtimeClient.ts", "runtimeProjection.ts"] {
        let path = root().join("extensions/runtrol-vscode/src").join(relative);
        let source = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
        assert!(
            source.contains("@runtrol/runtime-client"),
            "Studio public read module {relative} does not import the public package"
        );
        for forbidden in [
            "./core/client",
            "./protocol",
            "FrameTransport",
            "WIRE_VERSION",
        ] {
            assert!(
                !source.contains(forbidden),
                "Studio public read module {relative} imports private IPC declaration {forbidden:?}"
            );
        }
    }
}
