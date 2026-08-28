//! Transparent provider command launchers.
//!
//! A shim changes only process ownership: the same provider argv and working directory enter the local bridge, the
//! daemon creates the provider process, and the invoking terminal becomes its first viewer. Provider identities and
//! executable names are supplied from runtime manifests. This module contains no provider table.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use runtrol_provider::ProviderId;

const SHIM_MARKER: &str = "runtrol-provider-shim-v1";
const MAX_EXISTING_SHIM_BYTES: u64 = 16 * 1024;

/// Path-list environment entry containing Runtrol-owned provider shim directories.
///
/// Program resolution excludes these directories so a daemon started by any command cannot recursively resolve a
/// transparent launcher as the real provider executable.
pub const PROVIDER_SHIM_PATH_ENV: &str = "RUNTROL_PROVIDER_SHIM_PATH";

/// One runtime-discovered provider and the manifest names that can invoke it.
#[derive(Clone, Copy)]
pub struct ProviderShim<'provider> {
    /// Provider identity sent to the bridge.
    pub provider: &'provider ProviderId,
    /// Manifest-declared executable candidates.
    pub command_names: &'provider [Box<str>],
}

/// A provider shim directory could not be materialized safely.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ShimError {
    /// The requested shim directory is a symbolic link.
    #[error("refusing provider shim directory symlink {path}")]
    DirectorySymlink {
        /// Refused directory.
        path: String,
    },

    /// Filesystem operation failed.
    #[error("cannot {action} provider shim {path}: {detail}")]
    Io {
        /// Operation that failed.
        action: &'static str,
        /// Exact path.
        path: String,
        /// Platform detail.
        detail: String,
    },

    /// Two manifests claim the same shell command for different providers.
    #[error("provider command {command} is claimed by both {first} and {second}")]
    ProviderCollision {
        /// Shell command.
        command: String,
        /// First provider.
        first: String,
        /// Second provider.
        second: String,
    },

    /// An existing file in the destination is not owned by this feature.
    #[error("refusing to replace non-runtrol command at {path}")]
    ExistingCommand {
        /// Foreign command path.
        path: String,
    },

    /// The current runtime executable path cannot be represented in a native launcher.
    #[error("the runtime executable path cannot be represented in a provider shim: {path}")]
    RuntimePath {
        /// Unrepresentable executable path.
        path: String,
    },
}

/// Materialize provider-neutral command shims in one dedicated directory.
///
/// Existing files are replaced only when they carry this module's ownership marker. The directory must be placed at
/// the front of `PATH`; each launcher removes that leading directory before the runtime starts, so provider discovery
/// resolves the real executable rather than the shim itself.
///
/// # Errors
///
/// Returns [`ShimError`] for unsafe directories, command collisions, foreign destination files, or filesystem
/// failures.
pub fn materialize_provider_shims(
    directory: &Path,
    runtrol: &Path,
    providers: &[ProviderShim<'_>],
) -> Result<Vec<PathBuf>, ShimError> {
    if fs::symlink_metadata(directory).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return Err(ShimError::DirectorySymlink {
            path: directory.display().to_string(),
        });
    }
    fs::create_dir_all(directory).map_err(|error| io_error("create", directory, error))?;
    let mut commands: BTreeMap<String, &ProviderId> = BTreeMap::new();
    for provider in providers {
        for command in command_files(provider.command_names) {
            if let Some(first) = commands.insert(command.clone(), provider.provider)
                && first != provider.provider
            {
                return Err(ShimError::ProviderCollision {
                    command,
                    first: first.as_str().to_owned(),
                    second: provider.provider.as_str().to_owned(),
                });
            }
        }
    }
    remove_stale_owned(directory, &commands)?;
    let mut written = Vec::with_capacity(commands.len());
    for (command, provider) in commands {
        let path = directory.join(command);
        let body = shim_body(runtrol, provider)?;
        replace_owned(&path, body.as_bytes())?;
        written.push(path);
    }
    Ok(written)
}

fn remove_stale_owned(
    directory: &Path,
    commands: &BTreeMap<String, &ProviderId>,
) -> Result<(), ShimError> {
    let entries = fs::read_dir(directory).map_err(|error| io_error("list", directory, error))?;
    for entry in entries {
        let entry = entry.map_err(|error| io_error("list", directory, error))?;
        let Some(name) = entry.file_name().to_str().map(ToOwned::to_owned) else {
            continue;
        };
        #[cfg(windows)]
        let current = commands.contains_key(&name.to_ascii_lowercase());
        #[cfg(unix)]
        let current = commands.contains_key(&name);
        if current || !is_owned_provider_shim(&entry.path()) {
            continue;
        }
        fs::remove_file(entry.path())
            .map_err(|error| io_error("remove stale", &entry.path(), error))?;
    }
    Ok(())
}

#[cfg(windows)]
fn command_files(names: &[Box<str>]) -> Vec<String> {
    names
        .iter()
        .filter_map(|name| {
            let name = Path::new(name.as_ref()).file_name()?.to_str()?;
            let lower = name.to_ascii_lowercase();
            let stem = [".cmd", ".exe", ".bat", ".com"]
                .iter()
                .find_map(|suffix| {
                    lower
                        .strip_suffix(suffix)
                        .map(|_| &name[..name.len() - suffix.len()])
                })
                .unwrap_or(name);
            (!stem.is_empty()).then(|| format!("{stem}.cmd").to_ascii_lowercase())
        })
        .collect()
}

#[cfg(unix)]
fn command_files(names: &[Box<str>]) -> Vec<String> {
    names
        .iter()
        .filter_map(|name| Path::new(name.as_ref()).file_name()?.to_str())
        // A name carrying one of Windows' launcher extensions is not a command file on this platform, and the
        // comparison is on the extension itself so that a name whose own text merely ends in those letters is
        // left alone.
        .filter(|name| {
            !Path::new(name)
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| {
                    ["cmd", "exe", "bat", "com"]
                        .iter()
                        .any(|launcher| extension.eq_ignore_ascii_case(launcher))
                })
        })
        .filter(|name| !name.is_empty() && *name != "." && *name != "..")
        .map(ToOwned::to_owned)
        .collect()
}

#[cfg(windows)]
fn shim_body(runtrol: &Path, provider: &ProviderId) -> Result<String, ShimError> {
    let executable = runtrol.to_str().ok_or_else(|| ShimError::RuntimePath {
        path: runtrol.display().to_string(),
    })?;
    let executable = executable.replace('%', "%%");
    Ok(format!(
        "@echo off\r\nrem {SHIM_MARKER}\r\nsetlocal\r\nset \"RUNTROL_PROVIDER_SHIM_PATH=%~dp0\"\r\n\"{executable}\" bridge {} %*\r\nexit /b %ERRORLEVEL%\r\n",
        provider.as_str(),
    ))
}

#[cfg(unix)]
fn shim_body(runtrol: &Path, provider: &ProviderId) -> Result<String, ShimError> {
    let executable = runtrol.to_str().ok_or_else(|| ShimError::RuntimePath {
        path: runtrol.display().to_string(),
    })?;
    Ok(format!(
        "#!/bin/sh\n# {SHIM_MARKER}\nshim_dir=$(CDPATH= cd -- \"$(dirname -- \"$0\")\" && pwd)\nRUNTROL_PROVIDER_SHIM_PATH=$shim_dir\nexport RUNTROL_PROVIDER_SHIM_PATH\nexec {} bridge {} \"$@\"\n",
        shell_word(executable),
        shell_word(provider.as_str()),
    ))
}

#[cfg(unix)]
fn shell_word(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn replace_owned(path: &Path, body: &[u8]) -> Result<(), ShimError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(ShimError::ExistingCommand {
                    path: path.display().to_string(),
                });
            }
            if metadata.len() > MAX_EXISTING_SHIM_BYTES {
                return Err(ShimError::ExistingCommand {
                    path: path.display().to_string(),
                });
            }
            let existing = fs::read(path).map_err(|error| io_error("read", path, error))?;
            if !existing
                .windows(SHIM_MARKER.len())
                .any(|window| window == SHIM_MARKER.as_bytes())
            {
                return Err(ShimError::ExistingCommand {
                    path: path.display().to_string(),
                });
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(io_error("inspect", path, error)),
    }
    let temporary = path.with_extension(format!("runtrol-new-{}", std::process::id()));
    fs::write(&temporary, body).map_err(|error| io_error("write", &temporary, error))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(&temporary, fs::Permissions::from_mode(0o755))
            .map_err(|error| io_error("set permissions on", &temporary, error))?;
    }
    if let Err(error) = fs::rename(&temporary, path) {
        if error.kind() != io::ErrorKind::AlreadyExists {
            drop(fs::remove_file(&temporary));
            return Err(io_error("replace", path, error));
        }
        fs::remove_file(path).map_err(|remove| io_error("replace", path, remove))?;
        fs::rename(&temporary, path).map_err(|rename| io_error("replace", path, rename))?;
    }
    Ok(())
}

/// Whether one resolved command is a bounded Runtrol-owned transparent provider launcher.
pub(crate) fn is_owned_provider_shim(path: &Path) -> bool {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return false;
    };
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() > MAX_EXISTING_SHIM_BYTES
    {
        return false;
    }
    fs::read(path).is_ok_and(|bytes| {
        bytes
            .windows(SHIM_MARKER.len())
            .any(|window| window == SHIM_MARKER.as_bytes())
    })
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "map_err closures transfer their owned error directly into this bounded public failure"
)]
fn io_error(action: &'static str, path: &Path, error: io::Error) -> ShimError {
    ShimError::Io {
        action,
        path: path.display().to_string(),
        detail: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    static NEXT_DIRECTORY: AtomicUsize = AtomicUsize::new(0);

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let serial = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "runtrol-provider-shims-{name}-{}-{serial}",
                std::process::id()
            ));
            fs::create_dir_all(&path).expect("the scratch directory is created");
            Self(path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            drop(fs::remove_dir_all(&self.0));
        }
    }

    fn provider(value: &str) -> ProviderId {
        ProviderId::parse(value).expect("the fixture provider identity parses")
    }

    #[test]
    fn declared_commands_become_owned_launchers_that_resolution_refuses_as_providers() {
        let scratch = Scratch::new("owned");
        let provider = provider("fixture-one");
        let command_names = vec!["fixture-one".into()];
        let written = materialize_provider_shims(
            &scratch.0,
            Path::new("/installed/runtrol"),
            &[ProviderShim {
                provider: &provider,
                command_names: &command_names,
            }],
        )
        .expect("the shim is materialized");

        assert_eq!(written.len(), 1);
        let shim = written.first().expect("one written shim");
        assert!(is_owned_provider_shim(shim));
        assert!(matches!(
            crate::resolve(shim.to_str().expect("the scratch path is UTF-8")),
            Err(crate::SpawnError::NotFound { .. })
        ));
    }

    #[test]
    fn a_foreign_command_is_never_replaced() {
        let scratch = Scratch::new("foreign");
        let provider = provider("fixture-two");
        let command_names = vec!["fixture-two".into()];
        let command = command_files(&command_names)
            .into_iter()
            .next()
            .expect("one command name");
        let path = scratch.0.join(command);
        fs::write(&path, b"foreign command").expect("the foreign command is written");

        let error = materialize_provider_shims(
            &scratch.0,
            Path::new("/installed/runtrol"),
            &[ProviderShim {
                provider: &provider,
                command_names: &command_names,
            }],
        )
        .expect_err("a foreign command must be preserved");
        assert!(matches!(error, ShimError::ExistingCommand { .. }));
        assert_eq!(
            fs::read(path).expect("the command remains readable"),
            b"foreign command"
        );
    }

    #[test]
    fn two_providers_cannot_claim_one_shell_command() {
        let scratch = Scratch::new("collision");
        let first = provider("fixture-first");
        let second = provider("fixture-second");
        let first_names = vec!["shared-command".into()];
        let second_names = vec!["shared-command".into()];

        let error = materialize_provider_shims(
            &scratch.0,
            Path::new("/installed/runtrol"),
            &[
                ProviderShim {
                    provider: &first,
                    command_names: &first_names,
                },
                ProviderShim {
                    provider: &second,
                    command_names: &second_names,
                },
            ],
        )
        .expect_err("ambiguous command ownership is refused");
        assert!(matches!(error, ShimError::ProviderCollision { .. }));
    }

    #[test]
    fn commands_no_longer_declared_are_removed_only_when_runtrol_owned() {
        let scratch = Scratch::new("stale");
        let provider = provider("fixture-stale");
        let command_names = vec!["old-command".into()];
        let written = materialize_provider_shims(
            &scratch.0,
            Path::new("/installed/runtrol"),
            &[ProviderShim {
                provider: &provider,
                command_names: &command_names,
            }],
        )
        .expect("the old shim is materialized");
        let foreign = scratch.0.join("keep-me.txt");
        fs::write(&foreign, b"foreign").expect("the foreign file is written");

        materialize_provider_shims(&scratch.0, Path::new("/installed/runtrol"), &[])
            .expect("stale owned shims are cleaned");
        assert!(!written.first().expect("one written shim").exists());
        assert_eq!(
            fs::read(foreign).expect("the foreign file remains"),
            b"foreign"
        );
    }
}
