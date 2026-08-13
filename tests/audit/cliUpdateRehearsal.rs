//! Fixture rehearsal for exact provider update rollback.

use std::fs;
use std::path::Path;

use runtrol_update::{UpdateAction, UpdateFinish, UpdateTransaction};
use semver::Version;

fn version(value: &str) -> Version {
    Version::parse(value).expect("fixture version is semantic")
}

fn install(root: &Path, release: &Version) -> Result<(), String> {
    let healthy = release == &version("1.5.0");
    fs::write(
        root.join("package.json"),
        format!(r#"{{"name":"fixture-provider","version":"{release}"}}"#),
    )
    .map_err(|error| error.to_string())?;
    fs::write(
        root.join("provider.bin"),
        if healthy {
            "healthy fixture"
        } else {
            "broken fixture"
        },
    )
    .map_err(|error| error.to_string())
}

fn verify(root: &Path, release: &Version) -> Result<(), String> {
    let manifest =
        fs::read_to_string(root.join("package.json")).map_err(|error| error.to_string())?;
    if !manifest.contains(&format!(r#""version":"{release}""#)) {
        return Err("fixture package ownership names another release".to_owned());
    }
    let executable =
        fs::read_to_string(root.join("provider.bin")).map_err(|error| error.to_string())?;
    if executable != "healthy fixture" {
        return Err("fixture provider probe failed".to_owned());
    }
    Ok(())
}

#[test]
fn a_broken_target_is_replaced_by_the_exact_verified_starting_tree() {
    let root =
        std::env::temp_dir().join(format!("runtrol-update-rehearsal-{}", std::process::id()));
    if root.exists() {
        fs::remove_dir_all(&root).expect("clear update rehearsal scratch");
    }
    fs::create_dir_all(&root).expect("create update rehearsal scratch");
    install(&root, &version("1.5.0")).expect("install healthy baseline");
    let baseline_manifest = fs::read(root.join("package.json")).expect("read baseline manifest");
    let baseline_executable =
        fs::read(root.join("provider.bin")).expect("read baseline executable");

    let mut transaction =
        UpdateTransaction::begin(version("1.5.0"), version("2.0.0")).expect("valid fixture update");
    let finish = loop {
        let action = transaction
            .action()
            .expect("unfinished transaction has an action");
        let result = match action {
            UpdateAction::Install(release) => install(&root, &release),
            UpdateAction::Verify(release) => verify(&root, &release),
        };
        if let Some(finish) = transaction.advance(result) {
            break finish;
        }
    };

    assert!(matches!(finish, UpdateFinish::RolledBack { .. }));
    assert_eq!(
        fs::read(root.join("package.json")).expect("read restored manifest"),
        baseline_manifest
    );
    assert_eq!(
        fs::read(root.join("provider.bin")).expect("read restored executable"),
        baseline_executable
    );
    fs::remove_dir_all(root).expect("remove update rehearsal scratch");
}

#[test]
fn an_unrestorable_target_finishes_as_failure_instead_of_claiming_success() {
    let mut transaction =
        UpdateTransaction::begin(version("1.5.0"), version("2.0.0")).expect("valid fixture update");
    assert_eq!(transaction.advance(Ok(())), None);
    assert_eq!(
        transaction.advance(Err("target probe failed".to_owned())),
        None
    );
    let finish = transaction
        .advance(Err("registry no longer serves baseline".to_owned()))
        .expect("rollback installation failure is terminal");
    assert!(matches!(finish, UpdateFinish::Failed { .. }));
}
