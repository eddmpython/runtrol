//! Per-user operating-system protection for runtrol's fixed-size long-lived secrets.
//!
//! The vault owns no conversation data and each file contains exactly one fixed-size secret. On Windows the file contains a
//! DPAPI `CurrentUser` blob bound to runtrol-specific optional entropy. On macOS and Unix the file contains only a
//! path-bound lookup identifier for the current user's native Keychain or Secret Service entry. The plaintext exists
//! only in zeroizing memory while daemon assembly derives the Noise keypair. No platform writes a raw-key fallback.

mod platform;

use std::fs::OpenOptions;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

use runtrol_provider::AbsPath;
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

const MAGIC: &[u8; 8] = b"RTVAULT\0";
const FORMAT_VERSION: u8 = 1;
const SECRET_BYTES: usize = 32;
const HEADER_BYTES: usize = MAGIC.len() + 1;

/// A long-lived protected secret held only in zeroizing memory.
///
/// It implements neither `Debug`, `Display`, nor `Clone`. A diagnostic cannot print it and a caller cannot copy it
/// accidentally while passing it to the Noise constructor.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct ProtectedSecret([u8; SECRET_BYTES]);

impl ProtectedSecret {
    /// Open an existing per-user protected identity without creating a replacement.
    ///
    /// Use this when another durable record already names the identity. A missing envelope is an error because
    /// silently generating a different key would leave that record bound to authority this process cannot hold.
    ///
    /// # Errors
    ///
    /// [`VaultError`] when the envelope is missing, malformed, undecryptable, or cannot be read.
    pub fn load(path: &AbsPath) -> Result<Self, VaultError> {
        let encoded = std::fs::read(path.as_std_path()).map_err(|error| {
            VaultError::io("reading an existing protected identity", path, &error)
        })?;
        Self::decode(path, &encoded)
    }

    /// Open the per-user protected identity at `path`, or generate and durably create it once.
    ///
    /// Existing malformed or undecryptable files are refused and never replaced. Replacing one would silently sever
    /// every paired device whose Noise handshake pins the old public key.
    ///
    /// # Errors
    ///
    /// [`VaultError`] when the current user's native protector is unavailable, randomness is unavailable, the file
    /// is malformed, the native user protector refuses it, or durable file I/O fails.
    pub fn load_or_create(path: &AbsPath) -> Result<Self, VaultError> {
        match std::fs::read(path.as_std_path()) {
            Ok(encoded) => Self::decode(path, &encoded),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Self::create(path),
            Err(error) => Err(VaultError::io(
                "reading the machine identity vault",
                path,
                &error,
            )),
        }
    }

    /// Borrow the secret for the cryptographic constructor that immediately copies it into zeroizing storage.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; SECRET_BYTES] {
        &self.0
    }

    /// Delete one protected secret from both native storage and its Runtrol-owned envelope.
    ///
    /// Missing envelopes are already deleted. An existing malformed envelope is refused and preserved, because
    /// guessing which native credential it named could remove another identity.
    ///
    /// # Errors
    ///
    /// The envelope is malformed, native secret deletion is refused, or the file cannot be removed.
    pub fn delete(path: &AbsPath) -> Result<(), VaultError> {
        let encoded = match std::fs::read(path.as_std_path()) {
            Ok(encoded) => encoded,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => {
                return Err(VaultError::io(
                    "reading a protected secret for deletion",
                    path,
                    &error,
                ));
            }
        };
        let protected = decode_envelope(path, &encoded)?;
        #[cfg(windows)]
        platform::delete(path, protected);
        #[cfg(not(windows))]
        platform::delete(path, protected)?;
        std::fs::remove_file(path.as_std_path())
            .map_err(|error| VaultError::io("removing a protected secret envelope", path, &error))
    }

    fn create(path: &AbsPath) -> Result<Self, VaultError> {
        let mut secret = Self([0; SECRET_BYTES]);
        getrandom::fill(&mut secret.0).map_err(|_| VaultError::RandomUnavailable)?;
        let protected = platform::protect(path, &mut secret.0)?;
        let encoded = encode(&protected);
        persist_new(path, &encoded)?;
        Ok(secret)
    }

    fn decode(path: &AbsPath, encoded: &[u8]) -> Result<Self, VaultError> {
        let protected = decode_envelope(path, encoded)?;
        let plaintext = Zeroizing::new(platform::unprotect(path, protected)?);
        if plaintext.len() != SECRET_BYTES {
            return Err(VaultError::Malformed {
                path: path.clone(),
                why: "the protected payload is not a 32-byte machine identity",
            });
        }
        let mut secret = Self([0; SECRET_BYTES]);
        secret.0.copy_from_slice(&plaintext);
        Ok(secret)
    }
}

/// The daemon's long-lived Noise machine identity.
///
/// Kept as a type alias so existing pairing code retains its domain name while integrations can use the
/// same reviewed fixed-size operating-system protection without pretending their key is the daemon identity.
pub type MachineSecret = ProtectedSecret;

fn encode(protected: &[u8]) -> Vec<u8> {
    let mut encoded = Vec::with_capacity(HEADER_BYTES + protected.len());
    encoded.extend_from_slice(MAGIC);
    encoded.push(FORMAT_VERSION);
    encoded.extend_from_slice(protected);
    encoded
}

fn decode_envelope<'a>(path: &AbsPath, encoded: &'a [u8]) -> Result<&'a [u8], VaultError> {
    let Some(magic) = encoded.get(..MAGIC.len()) else {
        return Err(VaultError::Malformed {
            path: path.clone(),
            why: "the vault header is truncated",
        });
    };
    if magic != MAGIC {
        return Err(VaultError::Malformed {
            path: path.clone(),
            why: "the vault magic does not match",
        });
    }
    let Some(version) = encoded.get(MAGIC.len()).copied() else {
        return Err(VaultError::Malformed {
            path: path.clone(),
            why: "the vault version is missing",
        });
    };
    if version != FORMAT_VERSION {
        return Err(VaultError::Malformed {
            path: path.clone(),
            why: "the vault was written by an unsupported format version",
        });
    }
    let Some(protected) = encoded.get(HEADER_BYTES..) else {
        return Err(VaultError::Malformed {
            path: path.clone(),
            why: "the protected payload is missing",
        });
    };
    if protected.is_empty() {
        return Err(VaultError::Malformed {
            path: path.clone(),
            why: "the protected payload is empty",
        });
    }
    Ok(protected)
}

fn persist_new(path: &AbsPath, encoded: &[u8]) -> Result<(), VaultError> {
    let temporary = temporary_path(path)?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|error| {
            VaultError::io_path("creating a new machine identity vault", &temporary, &error)
        })?;
    file.write_all(encoded).map_err(|error| {
        VaultError::io_path("writing a new machine identity vault", &temporary, &error)
    })?;
    file.sync_all().map_err(|error| {
        VaultError::io_path("syncing a new machine identity vault", &temporary, &error)
    })?;
    drop(file);
    std::fs::rename(&temporary, path.as_std_path()).map_err(|error| {
        VaultError::io_path(
            "installing a new machine identity vault",
            &temporary,
            &error,
        )
    })
}

fn temporary_path(path: &AbsPath) -> Result<PathBuf, VaultError> {
    let mut suffix = [0_u8; 8];
    getrandom::fill(&mut suffix).map_err(|_| VaultError::RandomUnavailable)?;
    let mut hex = String::with_capacity(suffix.len() * 2);
    for byte in suffix {
        use core::fmt::Write as _;
        write!(&mut hex, "{byte:02x}").map_err(|_| VaultError::PathUnavailable)?;
    }
    let file_name = path
        .as_std_path()
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or(VaultError::PathUnavailable)?;
    Ok(path
        .as_std_path()
        .with_file_name(format!("{file_name}.new-{hex}")))
}

/// The machine identity could not be protected, restored, or installed durably.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum VaultError {
    /// The operating system would not provide fresh secret material or a unique temporary name.
    #[error("the operating system could not generate fresh machine-identity material")]
    RandomUnavailable,

    /// The fixed layout path could not form a temporary sibling name.
    #[error("the machine identity vault path cannot form a temporary sibling")]
    PathUnavailable,

    /// The on-disk envelope is not one this build can trust.
    #[error("machine identity vault {path} is malformed: {why}")]
    Malformed {
        /// The vault file.
        path: AbsPath,
        /// A stable reason that never contains protected bytes.
        why: &'static str,
    },

    /// The platform protector refused an operation.
    #[error("the per-user protector failed while {doing}: {detail}")]
    Platform {
        /// Protecting, unprotecting, or releasing platform memory.
        doing: &'static str,
        /// The operating-system error, which contains no input bytes.
        detail: String,
    },

    /// Durable file I/O failed.
    #[error("machine identity vault I/O failed while {doing} at {path}: {detail}")]
    Io {
        /// The exact operation.
        doing: &'static str,
        /// The final or temporary file.
        path: String,
        /// The I/O category.
        kind: io::ErrorKind,
        /// The operating-system detail.
        detail: String,
    },
}

impl VaultError {
    fn io(doing: &'static str, path: &AbsPath, error: &io::Error) -> Self {
        Self::io_path(doing, path.as_std_path(), error)
    }

    fn io_path(doing: &'static str, path: &Path, error: &io::Error) -> Self {
        Self::Io {
            doing,
            path: path.display().to_string(),
            kind: error.kind(),
            detail: error.to_string(),
        }
    }

    #[cfg(windows)]
    pub(crate) fn platform(doing: &'static str) -> Self {
        Self::Platform {
            doing,
            detail: io::Error::last_os_error().to_string(),
        }
    }

    #[cfg(not(windows))]
    pub(crate) fn platform_detail(doing: &'static str, detail: impl Into<String>) -> Self {
        Self::Platform {
            doing,
            detail: detail.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(name: &str) -> AbsPath {
        let root = std::env::temp_dir().join(format!("runtrol-vault-{name}"));
        std::fs::create_dir_all(&root).expect("create vault test directory");
        AbsPath::canonicalize(root.to_str().expect("temporary path is UTF-8"))
            .expect("canonical test directory")
            .join("identity.vault")
            .expect("valid vault file name")
    }

    #[test]
    fn envelope_rejects_wrong_magic_version_and_empty_payload() {
        let file = path("envelope");
        assert!(decode_envelope(&file, b"short").is_err());

        let mut wrong_magic = encode(b"protected");
        *wrong_magic.first_mut().expect("encoded magic") ^= 1;
        assert!(decode_envelope(&file, &wrong_magic).is_err());

        let mut wrong_version = encode(b"protected");
        *wrong_version.get_mut(MAGIC.len()).expect("encoded version") = FORMAT_VERSION + 1;
        assert!(decode_envelope(&file, &wrong_version).is_err());

        assert!(decode_envelope(&file, &encode(&[])).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn windows_vault_restores_the_same_secret_without_plaintext_on_disk() {
        let file = path("windows-roundtrip");
        if file.as_std_path().exists() {
            std::fs::remove_file(file.as_std_path()).expect("clear previous vault");
        }
        let first = MachineSecret::load_or_create(&file).expect("create protected identity");
        let expected = *first.as_bytes();
        let on_disk = std::fs::read(file.as_std_path()).expect("read protected file");
        assert!(
            !on_disk
                .windows(expected.len())
                .any(|window| window == expected),
            "the raw machine identity appeared in the vault file"
        );
        drop(first);

        let restored = MachineSecret::load_or_create(&file).expect("restore protected identity");
        assert_eq!(restored.as_bytes(), &expected);
    }

    #[cfg(windows)]
    #[test]
    fn an_existing_secret_can_be_deleted_without_leaving_an_envelope() {
        let file = path("windows-delete");
        if file.as_std_path().exists() {
            std::fs::remove_file(file.as_std_path()).expect("clear previous vault");
        }
        drop(ProtectedSecret::load_or_create(&file).expect("create protected identity"));

        ProtectedSecret::delete(&file).expect("delete protected identity");

        assert!(!file.as_std_path().exists());
        ProtectedSecret::delete(&file).expect("repeated deletion is settled");
        assert!(ProtectedSecret::load(&file).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn a_damaged_existing_vault_is_refused_and_never_replaced() {
        let file = path("windows-damaged");
        std::fs::write(file.as_std_path(), b"damaged").expect("plant damaged vault");
        assert!(MachineSecret::load_or_create(&file).is_err());
        assert_eq!(
            std::fs::read(file.as_std_path()).expect("read unchanged damage"),
            b"damaged"
        );
    }
}
