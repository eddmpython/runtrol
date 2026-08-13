//! Evidence-based update channel contract.

use std::fs;

use runtrol_core::{KindEntry, KindTable, ProviderRegistry, RegistryError};
use runtrol_provider::{AbsPath, ProviderId};
use runtrol_update::{
    ChannelId, ChannelObservation, ChannelVerdict, OwnershipError, RollbackVerdict,
    confirm_channel, discover_npm_ownership, select_rollback,
};
use semver::Version;

#[test]
fn only_matching_declaration_and_package_ownership_mint_an_executable_channel() {
    let package_root = std::env::temp_dir().join("runtrol-channel-root");
    let executable = package_root.join("node_modules/@scope/tool/bin/tool.exe");
    let observation = ChannelObservation {
        declared: ChannelId::Npm,
        package: "@scope/tool".to_owned(),
        package_root: package_root.clone(),
        executable,
    };
    let ChannelVerdict::Confirmed(confirmed) = confirm_channel(&observation) else {
        panic!("matching independent evidence must confirm the channel");
    };
    assert_eq!(confirmed.channel(), ChannelId::Npm);
    assert_eq!(confirmed.package(), "@scope/tool");
    assert_eq!(
        confirmed.install_argv(&Version::parse("2.3.4").expect("fixture version")),
        Some(vec![
            "install".to_owned(),
            "-g".to_owned(),
            "@scope/tool@2.3.4".to_owned(),
            "--no-audit".to_owned(),
            "--no-fund".to_owned(),
        ])
    );

    let mut ghost = observation.clone();
    ghost.executable = std::env::temp_dir().join("other-copy/tool.exe");
    assert_eq!(confirm_channel(&ghost), ChannelVerdict::GhostInstall);

    let mut injected = observation;
    injected.package = "--exec=other".to_owned();
    assert!(matches!(
        confirm_channel(&injected),
        ChannelVerdict::Unconfirmed(_)
    ));

    let self_managed = ChannelObservation {
        declared: ChannelId::SelfManaged,
        package: String::new(),
        package_root: package_root.clone(),
        executable: package_root.join("tool"),
    };
    assert_eq!(confirm_channel(&self_managed), ChannelVerdict::ObserveOnly);
}

#[test]
fn npm_ownership_comes_from_the_live_root_package_manifest_and_exact_bin_entry() {
    let root = std::env::temp_dir().join(format!("runtrol-npm-owner-{}", std::process::id()));
    if root.exists() {
        fs::remove_dir_all(&root).expect("clear npm ownership scratch");
    }
    let package = root.join("@scope/tool");
    let bin = package.join("bin");
    fs::create_dir_all(&bin).expect("create package tree");
    let entry = bin.join("tool.js");
    fs::write(&entry, "fixture").expect("write entry point");
    fs::write(
        package.join("package.json"),
        r#"{"name":"@scope/tool","version":"2.3.4","bin":{"tool":"bin/tool.js"},"ignored":true}"#,
    )
    .expect("write package manifest");

    let owned = discover_npm_ownership(&root, ["tool", "tool.cmd", "tool.exe"], [entry.as_path()])
        .expect("exact package ownership");
    assert_eq!(owned.package, "@scope/tool");
    assert_eq!(owned.version, Version::parse("2.3.4").expect("version"));
    assert_eq!(owned.entry_point, entry.canonicalize().expect("entry"));

    let unrelated = root.join("outside.js");
    fs::write(&unrelated, "fixture").expect("write unrelated file");
    assert!(matches!(
        discover_npm_ownership(&root, ["tool"], [unrelated.as_path()]),
        Err(OwnershipError::NotOwned)
    ));

    fs::write(
        package.join("package.json"),
        r#"{"name":"@scope/other","version":"2.3.4","bin":{"tool":"bin/tool.js"}}"#,
    )
    .expect("mutate package name");
    assert!(matches!(
        discover_npm_ownership(&root, ["tool"], [entry.as_path()]),
        Err(OwnershipError::Contradictory { .. })
    ));
    fs::remove_dir_all(root).expect("remove npm ownership scratch");
}

#[test]
fn rollback_restores_the_exact_installed_release_and_refuses_an_unowned_copy() {
    assert_eq!(
        select_rollback(
            ["0.145.0", "0.146.0", "0.147.0-alpha.4-win32-x64"],
            "0.146.0"
        ),
        RollbackVerdict::Available(Version::parse("0.146.0").expect("fixture version"))
    );
    assert_eq!(
        select_rollback(["1.0.0"], "1.0.0"),
        RollbackVerdict::Available(Version::parse("1.0.0").expect("fixture version"))
    );
    assert_eq!(
        select_rollback(["1.0.0", "2.0.0"], "3.0.0"),
        RollbackVerdict::Undetermined
    );
    assert_eq!(
        select_rollback(["1.0.0", "not-semver"], "1.0.0"),
        RollbackVerdict::Undetermined
    );
}

#[test]
fn an_on_disk_shadow_cannot_claim_the_compiled_updater_hint() {
    let root =
        std::env::temp_dir().join(format!("runtrol-channel-manifest-{}", std::process::id()));
    if root.exists() {
        fs::remove_dir_all(&root).expect("clear channel manifest scratch");
    }
    fs::create_dir_all(&root).expect("create channel manifest scratch");
    let operator = root.join("sample.toml");
    fs::write(
        &operator,
        "schema = 1\nid = \"sample\"\ndisplay_name = \"Shadow\"\nkind = \"example\"\n[bin]\nnames = [\"sample\"]\n[update]\nhint = \"npm\"\n",
    )
    .expect("write operator manifest");
    let root = AbsPath::canonicalize(root.to_str().expect("temporary path is UTF-8"))
        .expect("canonical scratch directory");
    let built_in = "schema = 1\nid = \"sample\"\ndisplay_name = \"Built in\"\nkind = \"example\"\n[bin]\nnames = [\"sample\"]\n[update]\nhint = \"self\"\n";
    let registry = ProviderRegistry::build(
        &[built_in],
        None,
        Some(&root),
        &KindTable::new([KindEntry {
            kind: "example",
            unavailable: None,
        }]),
    );
    let provider = registry
        .get(ProviderId::parse("sample").expect("fixture provider id"))
        .expect("the compiled provider remains available");
    assert_eq!(&*provider.manifest.display_name, "Built in");
    assert!(
        registry
            .rejected()
            .iter()
            .any(|rejected| matches!(rejected.why, RegistryError::UpdateAuthority))
    );
    fs::remove_dir_all(root.as_std_path()).expect("remove channel manifest scratch");
}
