//! Closed update channels and evidence-based verdicts.

use std::path::{Path, PathBuf};

use semver::Version;

/// Every update adapter compiled into this build.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChannelId {
    /// The provider owns its updater and runtrol only observes it.
    SelfManaged,
    /// A global npm package owns the resolved executable.
    Npm,
}

/// Structured declaration and filesystem evidence for one installed copy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChannelObservation {
    /// What the provider's machine-readable surface declared.
    pub declared: ChannelId,
    /// Package identifier discovered from that surface.
    pub package: String,
    /// Package root claimed by the provider or package manager.
    pub package_root: PathBuf,
    /// Executable currently resolved on the operator's path.
    pub executable: PathBuf,
}

/// A channel that passed declaration, path ownership, and argv comparison.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfirmedChannel {
    channel: ChannelId,
    package: String,
}

impl ConfirmedChannel {
    /// Confirmed closed channel.
    #[must_use]
    pub const fn channel(&self) -> ChannelId {
        self.channel
    }

    /// Discovered and validated package identifier.
    #[must_use]
    pub fn package(&self) -> &str {
        &self.package
    }

    /// Build the owned npm argv for an exact target version.
    ///
    /// Returns `None` for a channel runtrol does not execute.
    #[must_use]
    pub fn install_argv(&self, target: &Version) -> Option<Vec<String>> {
        match self.channel {
            ChannelId::SelfManaged => None,
            ChannelId::Npm => Some(vec![
                "install".to_owned(),
                "-g".to_owned(),
                format!("{}@{target}", self.package),
                "--no-audit".to_owned(),
                "--no-fund".to_owned(),
            ]),
        }
    }
}

/// Whether runtrol may execute an update for the observed copy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChannelVerdict {
    /// Every independent fact agrees.
    Confirmed(ConfirmedChannel),
    /// The provider owns its own updater, so runtrol cannot execute it.
    ObserveOnly,
    /// The declaration points to a different installed copy than the resolved executable.
    GhostInstall,
    /// Evidence is incomplete or contradictory.
    Unconfirmed(String),
}

/// Compare a compiled channel declaration with independent filesystem ownership evidence.
#[must_use]
pub fn confirm_channel(observation: &ChannelObservation) -> ChannelVerdict {
    if observation.declared == ChannelId::SelfManaged {
        return ChannelVerdict::ObserveOnly;
    }
    if !valid_package(&observation.package) {
        return ChannelVerdict::Unconfirmed(
            "the discovered package identifier is not safe".to_owned(),
        );
    }
    if !observation.package_root.is_absolute() || !observation.executable.is_absolute() {
        return ChannelVerdict::Unconfirmed(
            "the package root and executable must be absolute".to_owned(),
        );
    }
    if !is_under(&observation.executable, &observation.package_root) {
        return ChannelVerdict::GhostInstall;
    }
    ChannelVerdict::Confirmed(ConfirmedChannel {
        channel: ChannelId::Npm,
        package: observation.package.clone(),
    })
}

/// Whether the registry proves that the exact installed release can be restored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RollbackVerdict {
    /// The exact installed release is still available for reinstallation.
    Available(Version),
    /// The installed version is not in this registry, so this channel may own another copy.
    Undetermined,
}

/// Confirm the exact installed plain release without trusting registry publication order.
#[must_use]
pub fn select_rollback<'a>(
    published: impl IntoIterator<Item = &'a str>,
    installed: &str,
) -> RollbackVerdict {
    let Ok(installed) = Version::parse(installed) else {
        return RollbackVerdict::Undetermined;
    };
    if !installed.pre.is_empty() || !installed.build.is_empty() {
        return RollbackVerdict::Undetermined;
    }
    let mut versions = Vec::new();
    for value in published {
        let Ok(version) = Version::parse(value) else {
            return RollbackVerdict::Undetermined;
        };
        if version.pre.is_empty() && version.build.is_empty() {
            versions.push(version);
        }
    }
    if !versions.iter().any(|version| version == &installed) {
        return RollbackVerdict::Undetermined;
    }
    RollbackVerdict::Available(installed)
}

fn valid_package(package: &str) -> bool {
    if package.is_empty() || package.starts_with('-') || package.len() > 214 {
        return false;
    }
    let mut parts = package.split('/');
    let first = parts.next();
    let second = parts.next();
    if parts.next().is_some() {
        return false;
    }
    match (first, second) {
        (Some(scope), Some(name)) => {
            scope.starts_with('@') && token(scope.trim_start_matches('@')) && token(name)
        }
        (Some(name), None) => token(name),
        _ => false,
    }
}

fn token(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_' | b'.')
        })
}

fn is_under(path: &Path, root: &Path) -> bool {
    #[cfg(windows)]
    {
        let path = path.to_string_lossy().to_ascii_lowercase();
        let root = root.to_string_lossy().to_ascii_lowercase();
        Path::new(&path).starts_with(Path::new(&root))
    }
    #[cfg(not(windows))]
    {
        path.starts_with(root)
    }
}
