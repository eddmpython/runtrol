//! Provider update status from independently confirmed package ownership.

use core::time::Duration;
use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::PathBuf;

use runtrol_childproc::{Program, capture, resolve};
use runtrol_ipc::wire::{
    ProviderUpdateLine, ProviderUpdateOutcome, ProviderUpdateResult, ProviderUpdateState, Response,
    WireError,
};
use runtrol_provider::{ProviderId, UpdateHint};
use runtrol_update::{
    ChannelId, ChannelObservation, ChannelVerdict, ConfirmedChannel, NpmOwnership, RollbackVerdict,
    UpdateAction, UpdateFinish, UpdateTransaction, confirm_channel, discover_npm_ownership,
    select_rollback,
};
use semver::Version;
use serde::{Deserialize, Serialize};

use crate::compose::Composed;

const PACKAGE_QUERY_DEADLINE: Duration = Duration::from_secs(30);
const PACKAGE_INSTALL_DEADLINE: Duration = Duration::from_mins(5);
const UPDATE_JOURNAL_SCHEMA: u32 = 1;
const MAX_UPDATE_JOURNAL_BYTES: u64 = 64 * 1024;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct JournalEntry {
    highest_verified: Option<String>,
    pinned_target: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct UpdateJournal {
    schema: u32,
    providers: BTreeMap<String, JournalEntry>,
}

impl Default for UpdateJournal {
    fn default() -> Self {
        Self {
            schema: UPDATE_JOURNAL_SCHEMA,
            providers: BTreeMap::new(),
        }
    }
}

struct ConfirmedNpm {
    ownership: NpmOwnership,
    channel: ConfirmedChannel,
}

/// Inspect every declared provider without starting it or reading provider prose.
pub(crate) async fn inspect_all(composed: &Composed) -> Vec<ProviderUpdateLine> {
    let npm = resolve("npm");
    let mut lines = Vec::with_capacity(composed.registry.len());
    for provider in composed.registry.all() {
        let line = match &npm {
            Ok(npm) => inspect_one(composed, npm, provider.id()).await,
            Err(error) => unavailable(
                provider.id(),
                ProviderUpdateState::Unconfirmed,
                &format!("the npm command is unavailable: {error}"),
            ),
        };
        lines.push(line);
    }
    lines
}

/// Apply the greatest confirmed plain release and restore the exact previous release when verification fails.
#[expect(
    clippy::too_many_lines,
    reason = "one update transaction keeps planning, preflight, mutation, verification, journal closure, and its one response together"
)]
pub(crate) async fn apply_latest(composed: &Composed, provider: ProviderId) -> Response {
    let npm = match resolve("npm") {
        Ok(npm) => npm,
        Err(error) => return failed(&format!("the npm command is unavailable: {error}"), false),
    };
    let status = inspect_one(composed, &npm, provider).await;
    if status.state == ProviderUpdateState::Current {
        let Some(installed) = status.installed else {
            return failed(
                "current provider status did not carry its installed version",
                false,
            );
        };
        return Response::ProviderUpdated(ProviderUpdateResult {
            provider: provider.as_str().into(),
            outcome: ProviderUpdateOutcome::AlreadyCurrent,
            from: installed.clone(),
            to: installed,
            why: None,
        });
    }
    if status.state != ProviderUpdateState::Available {
        return failed(
            status
                .why
                .as_deref()
                .unwrap_or("the provider update channel is not executable"),
            false,
        );
    }
    let (Some(installed), Some(target), Some(rollback)) =
        (status.installed, status.target, status.rollback)
    else {
        return failed(
            "the confirmed update has no exact installed, target, or rollback release",
            false,
        );
    };
    let Ok(target_version) = Version::parse(&target) else {
        return failed("the confirmed target stopped being semantic", false);
    };
    let Ok(rollback_version) = Version::parse(&rollback) else {
        return failed("the confirmed rollback stopped being semantic", false);
    };
    let confirmed = match confirmed_npm(composed, &npm, provider).await {
        Ok(confirmed) => confirmed,
        Err(line) => {
            return failed(
                line.why
                    .as_deref()
                    .unwrap_or("package ownership changed before update"),
                true,
            );
        }
    };
    if confirmed.ownership.version.to_string() != installed.as_ref() {
        return failed("the installed provider changed after update planning", true);
    }
    if rollback_version != confirmed.ownership.version {
        return failed(
            "the rollback release is not the exact installed provider",
            false,
        );
    }
    if let Err(why) = verify_provider(composed, provider).await {
        return failed(
            &format!("the installed provider did not pass its pre-update probe: {why}"),
            false,
        );
    }
    let mut journal = match load_journal(composed) {
        Ok(journal) => journal,
        Err(why) => return failed(&why, false),
    };
    if let Err(why) = journal.admit_installed(provider, &confirmed.ownership.version) {
        return failed(&why, false);
    }
    if let Err(why) = save_journal(composed, &journal) {
        return failed(
            &format!("the provider update safety journal could not be saved: {why}"),
            true,
        );
    }
    let target_for_journal = target_version.clone();
    let mut transaction = match UpdateTransaction::begin(rollback_version, target_version) {
        Ok(transaction) => transaction,
        Err(error) => return failed(&error.to_string(), false),
    };
    loop {
        let Some(action) = transaction.action() else {
            return failed(
                "the provider update transaction ended without a result",
                false,
            );
        };
        let result = execute_action(composed, &npm, provider, &confirmed.channel, action).await;
        let Some(finish) = transaction.advance(result) else {
            continue;
        };
        return match finish {
            UpdateFinish::Updated => {
                journal.record_verified(provider, &target_for_journal);
                journal.clear_pin(provider);
                let journal_warning = save_journal(composed, &journal).err();
                Response::ProviderUpdated(ProviderUpdateResult {
                    provider: provider.as_str().into(),
                    outcome: ProviderUpdateOutcome::Updated,
                    from: installed,
                    to: target,
                    why: journal_warning.map(|why| {
                        format!("the update succeeded but its safety journal was not saved: {why}")
                            .into_boxed_str()
                    }),
                })
            }
            UpdateFinish::RolledBack { why } => {
                journal.pin(provider, &target);
                let journal_warning = save_journal(composed, &journal).err();
                Response::ProviderUpdated(ProviderUpdateResult {
                    provider: provider.as_str().into(),
                    outcome: ProviderUpdateOutcome::RolledBack,
                    from: installed,
                    to: rollback,
                    why: Some(match journal_warning {
                        Some(journal_warning) => {
                            format!("{why}; the rollback pin was not saved: {journal_warning}")
                                .into_boxed_str()
                        }
                        None => why.into_boxed_str(),
                    }),
                })
            }
            UpdateFinish::Failed { why } => failed(&why, false),
        };
    }
}

async fn execute_action(
    composed: &Composed,
    npm: &Program,
    provider: ProviderId,
    channel: &ConfirmedChannel,
    action: UpdateAction,
) -> Result<(), String> {
    match action {
        UpdateAction::Install(version) => {
            let argv = channel.install_argv(&version).ok_or_else(|| {
                "the confirmed channel cannot install an exact release".to_owned()
            })?;
            run_install(npm, &argv, composed).await
        }
        UpdateAction::Verify(version) => verify_installed(composed, npm, provider, &version).await,
    }
}

async fn verify_installed(
    composed: &Composed,
    npm: &Program,
    provider: ProviderId,
    expected: &Version,
) -> Result<(), String> {
    let after = confirmed_npm(composed, npm, provider)
        .await
        .map_err(|line| {
            line.why.map_or_else(
                || "package ownership is no longer confirmed".to_owned(),
                String::from,
            )
        })?;
    if &after.ownership.version != expected {
        return Err(format!(
            "the package manager did not install the expected release {expected}"
        ));
    }
    verify_provider(composed, provider).await
}

async fn verify_provider(composed: &Composed, provider: ProviderId) -> Result<(), String> {
    let declared = composed
        .registry
        .get(provider)
        .ok_or_else(|| "the provider declaration disappeared".to_owned())?;
    let driver = composed
        .driver_for(declared.manifest.kind.as_str())
        .ok_or_else(|| "this build cannot probe the provider kind".to_owned())?;
    let bound_flags = driver
        .flags
        .iter()
        .map(|flag| flag.flag)
        .collect::<Vec<_>>();
    let mut cache = runtrol_core::ProbeCache::open(composed.home.paths().probe_cache());
    runtrol_core::probe_program(
        &declared.manifest,
        &bound_flags,
        &mut cache,
        &composed.containment,
    )
    .await
    .map_err(|error| error.to_string())?;
    cache.save().map_err(|error| error.to_string())
}

async fn run_install(npm: &Program, argv: &[String], composed: &Composed) -> Result<(), String> {
    let output = capture(npm, argv, PACKAGE_INSTALL_DEADLINE, &composed.containment)
        .await
        .map_err(|error| error.to_string())?;
    if output.succeeded() && !output.truncated {
        Ok(())
    } else {
        Err("npm did not complete the bounded package installation successfully".to_owned())
    }
}

fn failed(message: &str, retryable: bool) -> Response {
    Response::Failed(WireError {
        message: message.into(),
        retryable,
        needs_the_operator: false,
    })
}

async fn inspect_one(
    composed: &Composed,
    npm: &Program,
    provider: ProviderId,
) -> ProviderUpdateLine {
    let confirmed = match confirmed_npm(composed, npm, provider).await {
        Ok(confirmed) => confirmed,
        Err(line) => return line,
    };
    let ownership = confirmed.ownership;
    let journal = match load_journal(composed) {
        Ok(journal) => journal,
        Err(why) => return unavailable(provider, ProviderUpdateState::Unconfirmed, &why),
    };
    if let Err(why) = journal.admits(provider, &ownership.version) {
        return unavailable(provider, ProviderUpdateState::Unconfirmed, &why);
    }
    let published = match published_versions(npm, confirmed.channel.package(), composed).await {
        Ok(versions) => versions,
        Err(why) => return unavailable(provider, ProviderUpdateState::Unconfirmed, &why),
    };
    let rendered: Vec<String> = published.iter().map(ToString::to_string).collect();
    let rollback = match select_rollback(
        rendered.iter().map(String::as_str),
        &ownership.version.to_string(),
    ) {
        RollbackVerdict::Available(version) => Some(version.to_string().into_boxed_str()),
        RollbackVerdict::Undetermined => {
            return unavailable(
                provider,
                ProviderUpdateState::Unconfirmed,
                "the registry does not prove ownership of the installed version",
            );
        }
    };
    let target = published
        .into_iter()
        .filter(|version| version.pre.is_empty() && version.build.is_empty())
        .max();
    let Some(target) = target else {
        return unavailable(
            provider,
            ProviderUpdateState::Unconfirmed,
            "the registry reports no plain semantic release",
        );
    };
    let state = if target > ownership.version {
        ProviderUpdateState::Available
    } else {
        ProviderUpdateState::Current
    };
    ProviderUpdateLine {
        provider: provider.as_str().into(),
        state,
        package: Some(ownership.package.into_boxed_str()),
        installed: Some(ownership.version.to_string().into_boxed_str()),
        target: (target > ownership.version).then(|| target.to_string().into_boxed_str()),
        rollback,
        why: None,
    }
}

async fn confirmed_npm(
    composed: &Composed,
    npm: &Program,
    provider: ProviderId,
) -> Result<ConfirmedNpm, ProviderUpdateLine> {
    let Some(declared) = composed.registry.get(provider) else {
        return Err(unavailable(
            provider,
            ProviderUpdateState::Unconfirmed,
            "the provider is not declared",
        ));
    };
    let Some(update) = declared.manifest.update.as_ref() else {
        return Err(unavailable(
            provider,
            ProviderUpdateState::Unconfirmed,
            "no compiled update channel is declared",
        ));
    };
    match update.hint {
        UpdateHint::None => {
            return Err(unavailable(
                provider,
                ProviderUpdateState::Unconfirmed,
                "the compiled declaration has no update channel",
            ));
        }
        UpdateHint::SelfManaged => {
            return Err(unavailable(
                provider,
                ProviderUpdateState::ObserveOnly,
                "the provider owns this update channel",
            ));
        }
        UpdateHint::Npm => {}
    }

    let program = match runtrol_core::locate(&declared.manifest) {
        Ok(program) => program,
        Err(error) => {
            return Err(unavailable(
                provider,
                ProviderUpdateState::NotInstalled,
                &error.to_string(),
            ));
        }
    };
    let npm_root = match npm_root(npm, composed).await {
        Ok(root) => root,
        Err(why) => {
            return Err(unavailable(
                provider,
                ProviderUpdateState::Unconfirmed,
                &why,
            ));
        }
    };
    let invocation_paths = invocation_paths(&program);
    let ownership = match discover_npm_ownership(
        &npm_root,
        declared.manifest.bin.names.iter().map(AsRef::as_ref),
        invocation_paths.iter().map(PathBuf::as_path),
    ) {
        Ok(ownership) => ownership,
        Err(error) => {
            return Err(unavailable(
                provider,
                ProviderUpdateState::Unconfirmed,
                &error.to_string(),
            ));
        }
    };
    let observation = ChannelObservation {
        declared: ChannelId::Npm,
        package: ownership.package.clone(),
        package_root: ownership.package_root.clone(),
        executable: ownership.entry_point.clone(),
    };
    let ChannelVerdict::Confirmed(channel) = confirm_channel(&observation) else {
        return Err(unavailable(
            provider,
            ProviderUpdateState::Unconfirmed,
            "the compiled channel and package ownership do not agree",
        ));
    };
    Ok(ConfirmedNpm { ownership, channel })
}

async fn npm_root(npm: &Program, composed: &Composed) -> Result<PathBuf, String> {
    let args = ["root".to_owned(), "-g".to_owned()];
    let output = capture(npm, &args, PACKAGE_QUERY_DEADLINE, &composed.containment)
        .await
        .map_err(|error| error.to_string())?;
    if !output.succeeded() || output.truncated {
        return Err("npm did not return one complete global package root".to_owned());
    }
    one_absolute_path(&output.stdout)
}

fn one_absolute_path(bytes: &[u8]) -> Result<PathBuf, String> {
    let text = std::str::from_utf8(bytes)
        .map_err(|_| "npm returned a package root that is not UTF-8".to_owned())?;
    let mut lines = text.lines().map(str::trim).filter(|line| !line.is_empty());
    let Some(line) = lines.next() else {
        return Err("npm returned no global package root".to_owned());
    };
    if lines.next().is_some() {
        return Err("npm returned more than one global package root".to_owned());
    }
    let path = PathBuf::from(line);
    if !path.is_absolute() {
        return Err("npm returned a relative global package root".to_owned());
    }
    Ok(path)
}

async fn published_versions(
    npm: &Program,
    package: &str,
    composed: &Composed,
) -> Result<Vec<Version>, String> {
    let args = [
        "view".to_owned(),
        package.to_owned(),
        "versions".to_owned(),
        "--json".to_owned(),
    ];
    let output = capture(npm, &args, PACKAGE_QUERY_DEADLINE, &composed.containment)
        .await
        .map_err(|error| error.to_string())?;
    if !output.succeeded() || output.truncated {
        return Err("npm did not return one complete version catalogue".to_owned());
    }
    let value: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|_| "npm returned a malformed version catalogue".to_owned())?;
    let values = match value {
        serde_json::Value::String(value) => vec![value],
        serde_json::Value::Array(values) => values
            .into_iter()
            .map(|value| match value {
                serde_json::Value::String(value) => Ok(value),
                _ => Err("npm returned a non-string version catalogue".to_owned()),
            })
            .collect::<Result<Vec<_>, _>>()?,
        _ => return Err("npm returned a malformed version catalogue".to_owned()),
    };
    let mut versions = Vec::with_capacity(values.len());
    for value in values {
        versions.push(
            Version::parse(&value)
                .map_err(|_| "npm returned a non-semantic version catalogue".to_owned())?,
        );
    }
    Ok(versions)
}

fn invocation_paths(program: &Program) -> Vec<PathBuf> {
    let mut paths = vec![program.path().as_std_path().to_owned()];
    paths.extend(
        program
            .via()
            .iter()
            .map(|path| path.as_std_path().to_owned()),
    );
    paths.extend(
        program
            .leading()
            .iter()
            .map(PathBuf::from)
            .filter(|path| path.is_absolute()),
    );
    paths
}

fn unavailable(provider: ProviderId, state: ProviderUpdateState, why: &str) -> ProviderUpdateLine {
    ProviderUpdateLine {
        provider: provider.as_str().into(),
        state,
        package: None,
        installed: None,
        target: None,
        rollback: None,
        why: Some(why.into()),
    }
}

impl UpdateJournal {
    fn admits(&self, provider: ProviderId, installed: &Version) -> Result<(), String> {
        let Some(floor) = self
            .providers
            .get(provider.as_str())
            .and_then(|entry| entry.highest_verified.as_deref())
        else {
            return Ok(());
        };
        let floor = Version::parse(floor)
            .map_err(|_| "the provider update journal has a malformed version floor".to_owned())?;
        if installed < &floor {
            return Err(format!(
                "installed release {installed} is below the verified version floor {floor}"
            ));
        }
        Ok(())
    }

    fn admit_installed(&mut self, provider: ProviderId, installed: &Version) -> Result<(), String> {
        self.admits(provider, installed)?;
        self.record_verified(provider, installed);
        Ok(())
    }

    fn record_verified(&mut self, provider: ProviderId, release: &Version) {
        let entry = self
            .providers
            .entry(provider.as_str().to_owned())
            .or_default();
        let replace = match entry.highest_verified.as_deref() {
            Some(current) => match Version::parse(current) {
                Ok(current) => release > &current,
                Err(_) => true,
            },
            None => true,
        };
        if replace {
            entry.highest_verified = Some(release.to_string());
        }
    }

    fn pin(&mut self, provider: ProviderId, target: &str) {
        self.providers
            .entry(provider.as_str().to_owned())
            .or_default()
            .pinned_target = Some(target.to_owned());
    }

    fn clear_pin(&mut self, provider: ProviderId) {
        if let Some(entry) = self.providers.get_mut(provider.as_str()) {
            entry.pinned_target = None;
        }
    }

    fn is_pinned(&self, provider: ProviderId, target: &str) -> bool {
        self.providers
            .get(provider.as_str())
            .and_then(|entry| entry.pinned_target.as_deref())
            == Some(target)
    }
}

pub(crate) fn is_automatic_pinned(
    composed: &Composed,
    provider: ProviderId,
    target: &str,
) -> Result<bool, String> {
    load_journal(composed).map(|journal| journal.is_pinned(provider, target))
}

fn load_journal(composed: &Composed) -> Result<UpdateJournal, String> {
    let path = composed.home.paths().provider_updates().as_std_path();
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(UpdateJournal::default());
        }
        Err(error) => return Err(format!("cannot read the provider update journal: {error}")),
    };
    if metadata.len() > MAX_UPDATE_JOURNAL_BYTES {
        return Err("the provider update journal exceeds its byte bound".to_owned());
    }
    let bytes = std::fs::read(path)
        .map_err(|error| format!("cannot read the provider update journal: {error}"))?;
    let journal = serde_json::from_slice::<UpdateJournal>(&bytes)
        .map_err(|_| "the provider update journal is malformed".to_owned())?;
    if journal.schema != UPDATE_JOURNAL_SCHEMA {
        return Err(format!(
            "provider update journal schema {} is not the supported schema {UPDATE_JOURNAL_SCHEMA}",
            journal.schema
        ));
    }
    Ok(journal)
}

fn save_journal(composed: &Composed, journal: &UpdateJournal) -> Result<(), String> {
    let path = composed.home.paths().provider_updates();
    let bytes = serde_json::to_vec(journal).map_err(|error| error.to_string())?;
    if u64::try_from(bytes.len()).map_or(true, |size| size > MAX_UPDATE_JOURNAL_BYTES) {
        return Err("the bounded provider update journal is full".to_owned());
    }
    let parent = path
        .parent()
        .ok_or_else(|| "the provider update journal has no parent directory".to_owned())?;
    let temporary = parent
        .join("provider-updates.json.writing")
        .map_err(|error| error.to_string())?;
    let mut file =
        std::fs::File::create(temporary.as_std_path()).map_err(|error| error.to_string())?;
    file.write_all(&bytes).map_err(|error| error.to_string())?;
    file.sync_all().map_err(|error| error.to_string())?;
    drop(file);
    std::fs::rename(temporary.as_std_path(), path.as_std_path()).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn npm_root_output_is_one_absolute_path() {
        let expected = if cfg!(windows) {
            PathBuf::from(r"C:\packages\node_modules")
        } else {
            PathBuf::from("/packages/node_modules")
        };
        assert_eq!(
            one_absolute_path(format!("{}\n", expected.display()).as_bytes()),
            Ok(expected)
        );
        assert!(one_absolute_path(b"").is_err());
        assert!(one_absolute_path(b"relative\n").is_err());
        assert!(one_absolute_path(b"/one\n/two\n").is_err());
    }

    #[test]
    fn the_verified_floor_never_moves_down_and_a_rollback_target_stays_pinned() {
        let provider = ProviderId::parse("fixture").expect("fixture provider id");
        let one = Version::parse("1.0.0").expect("fixture version");
        let two = Version::parse("2.0.0").expect("fixture version");
        let mut journal = UpdateJournal::default();

        journal
            .admit_installed(provider, &two)
            .expect("the first verified release establishes the floor");
        assert!(journal.admit_installed(provider, &one).is_err());
        journal.pin(provider, "3.0.0");
        assert!(journal.is_pinned(provider, "3.0.0"));
        journal.clear_pin(provider);
        assert!(!journal.is_pinned(provider, "3.0.0"));
    }
}
