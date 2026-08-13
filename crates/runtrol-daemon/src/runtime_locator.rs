//! Durable Runtime instance identity and atomic public locator publication.

use std::path::{Path, PathBuf};

use runtrol_provider::AbsPath;
use serde::{Deserialize, Serialize};

const SCHEMA: u32 = 1;
const MAX_RECORD_BYTES: u64 = 8 * 1024;

/// Runtime identity, locator publication, or cleanup failed closed.
#[derive(Debug, thiserror::Error)]
pub(crate) enum RuntimeBootstrapError {
    #[error("cannot read Runtime bootstrap state at {path}: {detail}")]
    Read { path: String, detail: String },
    #[error("Runtime bootstrap state at {path} is unsafe: {why}")]
    Unsafe { path: String, why: &'static str },
    #[error("Runtime bootstrap state at {path} is malformed: {detail}")]
    Malformed { path: String, detail: String },
    #[error("cannot write Runtime bootstrap state at {path}: {detail}")]
    Write { path: String, detail: String },
    #[error("operating-system randomness is unavailable for the Runtime instance identity")]
    Random,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct InstanceRecord {
    schema: u32,
    instance_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LocatorRecord {
    schema: u32,
    instance_id: String,
    endpoint_kind: String,
    endpoint: String,
    runtime_version: String,
    process_id: u32,
}

/// Load the durable installed-Runtime identity or mint it exactly once for a new home.
pub(crate) fn load_or_create_instance(path: &AbsPath) -> Result<String, RuntimeBootstrapError> {
    if path.as_std_path().exists() {
        let record: InstanceRecord = read_bounded(path.as_std_path())?;
        if record.schema != SCHEMA || !valid_instance(&record.instance_id) {
            return Err(RuntimeBootstrapError::Malformed {
                path: path.as_str().to_owned(),
                detail: "unsupported schema or invalid instance identity".to_owned(),
            });
        }
        return Ok(record.instance_id);
    }

    let mut random = [0_u8; 16];
    getrandom::fill(&mut random).map_err(|_| RuntimeBootstrapError::Random)?;
    let mut instance_id = String::with_capacity(4 + random.len() * 2);
    instance_id.push_str("rtm_");
    for byte in random {
        use core::fmt::Write as _;
        write!(&mut instance_id, "{byte:02x}").map_err(|error| RuntimeBootstrapError::Write {
            path: path.as_str().to_owned(),
            detail: error.to_string(),
        })?;
    }
    write_new(
        path.as_std_path(),
        &InstanceRecord {
            schema: SCHEMA,
            instance_id: instance_id.clone(),
        },
    )?;
    Ok(instance_id)
}

/// Locator lifetime guard. Dropping it removes only the record published by this Runtime instance.
pub(crate) struct PublishedLocator {
    path: PathBuf,
    instance_id: String,
}

impl PublishedLocator {
    /// Publish after the public endpoint is already bound.
    pub(crate) fn publish(
        path: &AbsPath,
        instance_id: &str,
        endpoint: &str,
    ) -> Result<Self, RuntimeBootstrapError> {
        ensure_replaceable(path.as_std_path())?;
        let record = LocatorRecord {
            schema: SCHEMA,
            instance_id: instance_id.to_owned(),
            endpoint_kind: endpoint_kind().to_owned(),
            endpoint: endpoint.to_owned(),
            runtime_version: env!("CARGO_PKG_VERSION").to_owned(),
            process_id: std::process::id(),
        };
        write_new(path.as_std_path(), &record)?;
        Ok(Self {
            path: path.as_std_path().to_owned(),
            instance_id: instance_id.to_owned(),
        })
    }
}

impl Drop for PublishedLocator {
    fn drop(&mut self) {
        let Ok(record) = read_bounded::<LocatorRecord>(&self.path) else {
            return;
        };
        if record.instance_id == self.instance_id {
            drop(std::fs::remove_file(&self.path));
        }
    }
}

fn read_bounded<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, RuntimeBootstrapError> {
    let metadata =
        std::fs::symlink_metadata(path).map_err(|error| RuntimeBootstrapError::Read {
            path: path.display().to_string(),
            detail: error.to_string(),
        })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(RuntimeBootstrapError::Unsafe {
            path: path.display().to_string(),
            why: "the record is not a regular file",
        });
    }
    if metadata.len() > MAX_RECORD_BYTES {
        return Err(RuntimeBootstrapError::Unsafe {
            path: path.display().to_string(),
            why: "the record exceeds its byte limit",
        });
    }
    let bytes = std::fs::read(path).map_err(|error| RuntimeBootstrapError::Read {
        path: path.display().to_string(),
        detail: error.to_string(),
    })?;
    serde_json::from_slice(&bytes).map_err(|error| RuntimeBootstrapError::Malformed {
        path: path.display().to_string(),
        detail: error.to_string(),
    })
}

fn write_new<T: Serialize>(path: &Path, value: &T) -> Result<(), RuntimeBootstrapError> {
    let encoded = serde_json::to_vec(value).map_err(|error| RuntimeBootstrapError::Write {
        path: path.display().to_string(),
        detail: error.to_string(),
    })?;
    if u64::try_from(encoded.len()).map_or(true, |length| length > MAX_RECORD_BYTES) {
        return Err(RuntimeBootstrapError::Write {
            path: path.display().to_string(),
            detail: "the encoded record exceeds its byte limit".to_owned(),
        });
    }
    let pending = random_sibling(path)?;
    runtrol_ipc::transport::create_owner_only_file(&pending, &encoded).map_err(|error| {
        RuntimeBootstrapError::Write {
            path: pending.display().to_string(),
            detail: error.to_string(),
        }
    })?;
    let outcome = std::fs::rename(&pending, path);
    if let Err(error) = outcome {
        drop(std::fs::remove_file(&pending));
        return Err(RuntimeBootstrapError::Write {
            path: path.display().to_string(),
            detail: error.to_string(),
        });
    }
    Ok(())
}

fn ensure_replaceable(path: &Path) -> Result<(), RuntimeBootstrapError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => Ok(()),
        Ok(_) => Err(RuntimeBootstrapError::Unsafe {
            path: path.display().to_string(),
            why: "the stale locator is not a regular file",
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(RuntimeBootstrapError::Read {
            path: path.display().to_string(),
            detail: error.to_string(),
        }),
    }
}

fn random_sibling(path: &Path) -> Result<PathBuf, RuntimeBootstrapError> {
    let Some(name) = path.file_name() else {
        return Err(RuntimeBootstrapError::Write {
            path: path.display().to_string(),
            detail: "the record path has no file name".to_owned(),
        });
    };
    let mut random = [0_u8; 8];
    getrandom::fill(&mut random).map_err(|_| RuntimeBootstrapError::Random)?;
    let mut suffix = String::with_capacity(random.len() * 2);
    for byte in random {
        use core::fmt::Write as _;
        write!(&mut suffix, "{byte:02x}").map_err(|error| RuntimeBootstrapError::Write {
            path: path.display().to_string(),
            detail: error.to_string(),
        })?;
    }
    let mut pending = name.to_os_string();
    pending.push(format!(".{suffix}.new"));
    Ok(path.with_file_name(pending))
}

fn valid_instance(instance_id: &str) -> bool {
    instance_id.len() == 36
        && instance_id.starts_with("rtm_")
        && instance_id
            .bytes()
            .skip(4)
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

#[cfg(windows)]
const fn endpoint_kind() -> &'static str {
    "namedPipe"
}

#[cfg(unix)]
const fn endpoint_kind() -> &'static str {
    "unixSocket"
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Scratch(PathBuf);

    impl Scratch {
        fn make(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "runtrol-runtime-locator-{name}-{}",
                std::process::id()
            ));
            drop(std::fs::remove_dir_all(&path));
            std::fs::create_dir_all(&path).expect("create scratch");
            Self(path)
        }

        fn path(&self, name: &str) -> AbsPath {
            AbsPath::new(self.0.join(name).to_str().expect("UTF-8 scratch"))
                .expect("absolute scratch")
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            drop(std::fs::remove_dir_all(&self.0));
        }
    }

    #[test]
    fn installed_instance_survives_restart_and_full_removal_mints_another() {
        let scratch = Scratch::make("instance");
        let path = scratch.path("instance.json");
        let first = load_or_create_instance(&path).expect("mint instance");
        let second = load_or_create_instance(&path).expect("restore instance");
        assert_eq!(first, second);
        std::fs::remove_file(path.as_std_path()).expect("simulate uninstall");
        let third = load_or_create_instance(&path).expect("mint after reinstall");
        assert_ne!(first, third);
    }

    #[test]
    fn locator_is_published_after_readiness_and_removed_by_its_owner() {
        let scratch = Scratch::make("publication");
        let path = scratch.path("locator.json");
        let instance = "rtm_0123456789abcdef0123456789abcdef";
        let guard =
            PublishedLocator::publish(&path, instance, "local-endpoint").expect("publish locator");
        let record: LocatorRecord = read_bounded(path.as_std_path()).expect("read locator");
        assert_eq!(record.instance_id, instance);
        drop(guard);
        assert!(!path.as_std_path().exists());
    }

    #[test]
    fn a_new_locator_atomically_replaces_a_verified_regular_record() {
        let scratch = Scratch::make("replacement");
        let path = scratch.path("locator.json");
        let instance = "rtm_0123456789abcdef0123456789abcdef";
        write_new(
            path.as_std_path(),
            &LocatorRecord {
                schema: SCHEMA,
                instance_id: instance.to_owned(),
                endpoint_kind: endpoint_kind().to_owned(),
                endpoint: "first".to_owned(),
                runtime_version: env!("CARGO_PKG_VERSION").to_owned(),
                process_id: std::process::id(),
            },
        )
        .expect("first locator");
        let second = PublishedLocator::publish(&path, instance, "second").expect("replace locator");
        let record: LocatorRecord = read_bounded(path.as_std_path()).expect("read replacement");
        assert_eq!(record.endpoint, "second");
        drop(second);
    }
}
