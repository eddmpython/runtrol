//! Independent npm package ownership discovery.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use semver::Version;
use serde::Deserialize;

/// Maximum package manifest size accepted during ownership discovery.
pub const MAX_PACKAGE_JSON_BYTES: u64 = 64 * 1024;

/// One installed npm package proven to own a provider entry point.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NpmOwnership {
    /// Package identifier read from its own manifest.
    pub package: String,
    /// Installed version read from its own manifest.
    pub version: Version,
    /// Exact package directory below the independently queried global npm root.
    pub package_root: PathBuf,
    /// Exact entry point selected by the package's `bin` map.
    pub entry_point: PathBuf,
}

/// Why an npm installation could not be proved to own the provider invocation.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum OwnershipError {
    /// The package manager root is not an absolute existing directory.
    #[error("the npm package root is not an absolute existing directory")]
    InvalidRoot,
    /// None of the actual invocation files belongs to the queried npm root.
    #[error("the resolved provider invocation is not owned by the queried npm root")]
    NotOwned,
    /// More than one package claims an invocation file, so choosing would be arbitrary.
    #[error("more than one npm package claims the resolved provider invocation")]
    Ambiguous,
    /// A package manifest is absent, oversized, unreadable, or malformed.
    #[error("cannot read package ownership at {path}: {detail}")]
    Manifest {
        /// Manifest path.
        path: PathBuf,
        /// Bounded diagnostic.
        detail: String,
    },
    /// Package metadata did not agree with its directory or requested binary name.
    #[error("package ownership at {path} is contradictory: {detail}")]
    Contradictory {
        /// Package manifest path.
        path: PathBuf,
        /// Bounded diagnostic.
        detail: String,
    },
}

#[derive(Deserialize)]
struct PackageManifest {
    name: String,
    version: String,
    bin: BinField,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum BinField {
    One(String),
    Many(BTreeMap<String, String>),
}

/// Discover the one global npm package whose own `bin` map selects an actual invocation path.
///
/// `invocation_paths` comes from the resolved program, its unwrapped launchers, and absolute leading script
/// arguments. Provider and package names remain runtime data. The function neither searches `PATH` nor starts a
/// process.
///
/// # Errors
///
/// [`OwnershipError`] when ownership is absent, ambiguous, unreadable, or contradictory.
pub fn discover_npm_ownership<'a>(
    npm_root: &Path,
    binary_names: impl IntoIterator<Item = &'a str>,
    invocation_paths: impl IntoIterator<Item = &'a Path>,
) -> Result<NpmOwnership, OwnershipError> {
    let npm_root = npm_root
        .canonicalize()
        .map_err(|_| OwnershipError::InvalidRoot)?;
    if !npm_root.is_absolute() || !npm_root.is_dir() {
        return Err(OwnershipError::InvalidRoot);
    }
    let mut names: Vec<String> = binary_names
        .into_iter()
        .filter_map(normalized_binary_name)
        .collect();
    names.sort();
    names.dedup();

    let mut found: Vec<NpmOwnership> = Vec::new();
    for candidate in invocation_paths {
        let Ok(candidate) = candidate.canonicalize() else {
            continue;
        };
        let Ok(relative) = candidate.strip_prefix(&npm_root) else {
            continue;
        };
        let Some(package_root) = package_root_for(&npm_root, relative) else {
            continue;
        };
        let ownership = read_ownership(&package_root, &candidate, &names)?;
        if !found
            .iter()
            .any(|one| one.package_root == ownership.package_root)
        {
            found.push(ownership);
        }
    }

    match found.len() {
        0 => Err(OwnershipError::NotOwned),
        1 => found.pop().ok_or(OwnershipError::NotOwned),
        _ => Err(OwnershipError::Ambiguous),
    }
}

fn normalized_binary_name(name: &str) -> Option<String> {
    let file = Path::new(name).file_name()?.to_str()?;
    let stem = file
        .strip_suffix(".cmd")
        .or_else(|| file.strip_suffix(".exe"))
        .or_else(|| file.strip_suffix(".ps1"))
        .unwrap_or(file);
    (!stem.is_empty()).then(|| stem.to_owned())
}

fn package_root_for(npm_root: &Path, relative: &Path) -> Option<PathBuf> {
    let mut components = relative.components();
    let first = components.next()?.as_os_str().to_str()?;
    if first.starts_with('@') {
        let second = components.next()?.as_os_str();
        components.next()?;
        Some(npm_root.join(first).join(second))
    } else {
        components.next()?;
        Some(npm_root.join(first))
    }
}

fn read_ownership(
    package_root: &Path,
    candidate: &Path,
    binary_names: &[String],
) -> Result<NpmOwnership, OwnershipError> {
    let manifest_path = package_root.join("package.json");
    let metadata = std::fs::metadata(&manifest_path).map_err(|error| OwnershipError::Manifest {
        path: manifest_path.clone(),
        detail: error.to_string(),
    })?;
    if metadata.len() > MAX_PACKAGE_JSON_BYTES {
        return Err(OwnershipError::Manifest {
            path: manifest_path,
            detail: format!("it exceeds the {MAX_PACKAGE_JSON_BYTES} byte limit"),
        });
    }
    let bytes = std::fs::read(&manifest_path).map_err(|error| OwnershipError::Manifest {
        path: manifest_path.clone(),
        detail: error.to_string(),
    })?;
    let manifest: PackageManifest =
        serde_json::from_slice(&bytes).map_err(|error| OwnershipError::Manifest {
            path: manifest_path.clone(),
            detail: error.to_string(),
        })?;
    let version =
        Version::parse(&manifest.version).map_err(|error| OwnershipError::Contradictory {
            path: manifest_path.clone(),
            detail: format!("version is not semantic: {error}"),
        })?;
    let expected_root = package_root_from_name(
        package_root
            .parent()
            .and_then(|parent| {
                if manifest.name.starts_with('@') {
                    parent.parent()
                } else {
                    Some(parent)
                }
            })
            .ok_or_else(|| OwnershipError::Contradictory {
                path: manifest_path.clone(),
                detail: "the package directory has no npm root".to_owned(),
            })?,
        &manifest.name,
    )
    .ok_or_else(|| OwnershipError::Contradictory {
        path: manifest_path.clone(),
        detail: "the package name is not safe".to_owned(),
    })?;
    if expected_root != package_root {
        return Err(OwnershipError::Contradictory {
            path: manifest_path,
            detail: "the package name does not match its directory".to_owned(),
        });
    }

    let selected = match manifest.bin {
        BinField::One(target) => (binary_names.len() == 1).then_some(target),
        BinField::Many(entries) => binary_names
            .iter()
            .find_map(|name| entries.get(name).cloned()),
    }
    .ok_or_else(|| OwnershipError::Contradictory {
        path: manifest_path.clone(),
        detail: "the package bin map does not name this provider binary".to_owned(),
    })?;
    let entry_point = package_root
        .join(selected)
        .canonicalize()
        .map_err(|error| OwnershipError::Contradictory {
            path: manifest_path.clone(),
            detail: format!("the package entry point cannot be resolved: {error}"),
        })?;
    if entry_point != candidate {
        return Err(OwnershipError::Contradictory {
            path: manifest_path,
            detail: "the resolved invocation is not the package bin entry point".to_owned(),
        });
    }

    Ok(NpmOwnership {
        package: manifest.name,
        version,
        package_root: package_root.to_owned(),
        entry_point,
    })
}

fn package_root_from_name(npm_root: &Path, name: &str) -> Option<PathBuf> {
    if name.is_empty() || name.starts_with('-') || name.len() > 214 {
        return None;
    }
    let mut parts = name.split('/');
    let first = parts.next()?;
    let second = parts.next();
    if parts.next().is_some() {
        return None;
    }
    match second {
        Some(package) if first.starts_with('@') && token(&first[1..]) && token(package) => {
            Some(npm_root.join(first).join(package))
        }
        None if token(first) => Some(npm_root.join(first)),
        _ => None,
    }
}

fn token(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_' | b'.')
        })
}
