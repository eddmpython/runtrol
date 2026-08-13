//! Evidence-based update channel contract.

use std::fs;

use runtrol_core::{KindEntry, KindTable, ProviderRegistry, RegistryError};
use runtrol_provider::{AbsPath, ProviderId};
use runtrol_update::{
    ChannelId, ChannelObservation, ChannelVerdict, RollbackVerdict, confirm_channel,
    select_rollback,
};
use semver::Version;

#[test]
fn only_matching_declaration_path_and_owned_argv_mint_an_executable_channel() {
    let package_root = std::env::temp_dir().join("runtrol-channel-root");
    let executable = package_root.join("node_modules/@scope/tool/bin/tool.exe");
    let observation = ChannelObservation {
        declared: ChannelId::Npm,
        package: "@scope/tool".to_owned(),
        package_root: package_root.clone(),
        executable,
        declared_argv: vec![
            "npm".to_owned(),
            "install".to_owned(),
            "-g".to_owned(),
            "@scope/tool".to_owned(),
        ],
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

    let mut command_mismatch = observation.clone();
    command_mismatch.declared_argv.push("--force".to_owned());
    assert!(matches!(
        confirm_channel(&command_mismatch),
        ChannelVerdict::Unconfirmed(_)
    ));

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
        declared_argv: Vec::new(),
    };
    assert_eq!(confirm_channel(&self_managed), ChannelVerdict::ObserveOnly);
}

#[test]
fn rollback_uses_semantic_order_and_refuses_a_registry_that_does_not_own_the_installed_copy() {
    assert_eq!(
        select_rollback(
            ["0.145.0", "0.146.0", "0.147.0-alpha.4-win32-x64"],
            "0.146.0"
        ),
        RollbackVerdict::Available(Version::parse("0.145.0").expect("fixture version"))
    );
    assert_eq!(
        select_rollback(["1.0.0"], "1.0.0"),
        RollbackVerdict::Unavailable
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
