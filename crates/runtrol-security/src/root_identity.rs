//! Stable filesystem identity for an approved project root.

use std::io;

use runtrol_provider::AbsPath;

/// Kernel-issued identity of the directory that occupied an approved path.
///
/// Paths are operator-facing labels. This value is the authority binding that makes deleting or
/// replacing a directory at the same path invalidate an existing grant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProjectRootIdentity([u8; 24]);

/// Live Windows directory handle that detects whether an approved root was renamed or replaced.
///
/// The guard is intentionally unavailable on Unix, where an open directory descriptor does not pin
/// the pathname. Unix callers continue to compare the current identity before each mutation.
#[cfg(windows)]
#[derive(Debug)]
pub struct ProjectRootGuard {
    inner: platform::RootGuard,
}

impl ProjectRootIdentity {
    /// Read the current directory identity from the filesystem.
    ///
    /// # Errors
    ///
    /// The operating system could not open or identify the path.
    pub fn read(path: &AbsPath) -> Result<Self, io::Error> {
        platform::read(path)
    }

    /// Restore a durable identity previously emitted by [`Self::to_bytes`].
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 24]) -> Self {
        Self(bytes)
    }

    /// Stable bounded representation for durable authority rows.
    #[must_use]
    pub const fn to_bytes(self) -> [u8; 24] {
        self.0
    }
}

#[cfg(windows)]
impl ProjectRootGuard {
    /// Open and pin the exact directory only when its kernel identity still matches the durable grant.
    ///
    /// # Errors
    ///
    /// The path cannot be opened, cannot be identified, or names another object.
    pub fn acquire(path: &AbsPath, expected: ProjectRootIdentity) -> Result<Self, io::Error> {
        platform::guard(path, expected).map(|inner| Self { inner })
    }

    /// Confirm that the guarded directory still has the same final filesystem name.
    ///
    /// # Errors
    ///
    /// The directory was renamed, unlinked, or the operating system can no longer identify its name.
    pub fn validate(&mut self) -> Result<(), io::Error> {
        self.inner.validate()
    }
}

#[cfg(unix)]
mod platform {
    use std::os::unix::fs::MetadataExt as _;
    use std::time::UNIX_EPOCH;

    use sha2::{Digest as _, Sha256};

    use super::{AbsPath, ProjectRootIdentity, io};

    pub(super) fn read(path: &AbsPath) -> Result<ProjectRootIdentity, io::Error> {
        let metadata = std::fs::metadata(path.as_std_path())?;
        let created = metadata.created().map_err(|error| {
            io::Error::new(
                error.kind(),
                format!("the filesystem cannot report a stable directory creation time: {error}"),
            )
        })?;
        let created = created.duration_since(UNIX_EPOCH).map_err(|error| {
            io::Error::other(format!(
                "the directory creation time precedes the Unix epoch: {error}"
            ))
        })?;
        let mut digest = Sha256::new();
        digest.update(b"runtrol/project-root-identity/unix/2");
        digest.update(metadata.dev().to_le_bytes());
        digest.update(metadata.ino().to_le_bytes());
        digest.update(created.as_secs().to_le_bytes());
        digest.update(created.subsec_nanos().to_le_bytes());
        let digest = digest.finalize();
        let mut bytes = [0_u8; 24];
        for (target, source) in bytes.iter_mut().zip(digest) {
            *target = source;
        }
        Ok(ProjectRootIdentity(bytes))
    }
}

#[cfg(windows)]
mod platform {
    use std::fs::{File, OpenOptions};
    use std::mem::size_of;
    use std::os::windows::fs::OpenOptionsExt as _;
    use std::os::windows::io::AsRawHandle as _;

    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_ID_INFO, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE,
        FILE_SHARE_READ, FILE_SHARE_WRITE, FileIdInfo, GetFileInformationByHandleEx,
        GetFinalPathNameByHandleW, VOLUME_NAME_DOS,
    };

    use super::{AbsPath, ProjectRootIdentity, io};

    #[derive(Debug)]
    pub(super) struct RootGuard {
        file: File,
        expected_name: Vec<u16>,
        name_buffer: Vec<u16>,
    }

    const MAX_FINAL_PATH_UNITS: usize = 32_768;

    pub(super) fn read(path: &AbsPath) -> Result<ProjectRootIdentity, io::Error> {
        let file = OpenOptions::new()
            .access_mode(FILE_READ_ATTRIBUTES)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(path.as_std_path())?;
        identity(&file)
    }

    pub(super) fn guard(
        path: &AbsPath,
        expected: ProjectRootIdentity,
    ) -> Result<RootGuard, io::Error> {
        let file = OpenOptions::new()
            .access_mode(FILE_READ_ATTRIBUTES)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(path.as_std_path())?;
        if identity(&file)? != expected {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "the directory identity no longer matches the approved root",
            ));
        }
        let expected_name = final_name(&file)?;
        let name_buffer = vec![0; expected_name.len().saturating_add(1)];
        Ok(RootGuard {
            file,
            expected_name,
            name_buffer,
        })
    }

    impl RootGuard {
        pub(super) fn validate(&mut self) -> Result<(), io::Error> {
            let written = read_final_name(&self.file, &mut self.name_buffer)?;
            let current = self.name_buffer.get(..written).ok_or_else(root_moved)?;
            if current == self.expected_name {
                Ok(())
            } else {
                Err(root_moved())
            }
        }
    }

    fn root_moved() -> io::Error {
        io::Error::new(
            io::ErrorKind::PermissionDenied,
            "the approved directory no longer has its original filesystem name",
        )
    }

    fn final_name(file: &File) -> Result<Vec<u16>, io::Error> {
        let mut buffer = vec![0; 512];
        loop {
            let written = read_final_name(file, &mut buffer)?;
            if written < buffer.len() {
                buffer.truncate(written);
                return Ok(buffer);
            }
            let needed = written.checked_add(1).ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "the directory name is oversized",
                )
            })?;
            if needed > MAX_FINAL_PATH_UNITS {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "the directory name exceeds the Windows path ceiling",
                ));
            }
            buffer.resize(needed, 0);
        }
    }

    #[expect(
        unsafe_code,
        reason = "Windows exposes a handle's current final name only through one buffer-writing kernel call whose handle and capacity are owned beside the call"
    )]
    fn read_final_name(file: &File, buffer: &mut [u16]) -> Result<usize, io::Error> {
        let capacity = u32::try_from(buffer.len())
            .map_err(|_| io::Error::other("the directory name buffer is oversized"))?;
        // SAFETY: `file` owns a live directory handle and `buffer` exposes exactly `capacity` writable
        // UTF-16 units for the duration of the call. The API retains neither.
        let written = unsafe {
            GetFinalPathNameByHandleW(
                file.as_raw_handle(),
                buffer.as_mut_ptr(),
                capacity,
                VOLUME_NAME_DOS,
            )
        };
        if written == 0 {
            return Err(io::Error::last_os_error());
        }
        usize::try_from(written)
            .map_err(|_| io::Error::other("the directory name length is oversized"))
    }

    #[expect(
        unsafe_code,
        reason = "Windows exposes stable directory identity only through one handle query whose live handle and exact output layout are bounded beside the call"
    )]
    fn identity(file: &File) -> Result<ProjectRootIdentity, io::Error> {
        let mut information = FILE_ID_INFO::default();
        let information_size = u32::try_from(size_of::<FILE_ID_INFO>())
            .map_err(|_| io::Error::other("Windows file identity structure is oversized"))?;
        // SAFETY: `file` owns a live directory handle for the duration of the call. `information`
        // points to writable storage of exactly `information_size` bytes and the API writes only
        // that documented FILE_ID_INFO layout.
        let identified = unsafe {
            GetFileInformationByHandleEx(
                file.as_raw_handle(),
                FileIdInfo,
                (&raw mut information).cast(),
                information_size,
            )
        };
        if identified == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut bytes = [0_u8; 24];
        bytes[..8].copy_from_slice(&information.VolumeSerialNumber.to_le_bytes());
        bytes[8..].copy_from_slice(&information.FileId.Identifier);
        Ok(ProjectRootIdentity(bytes))
    }
}

#[cfg(all(test, windows))]
mod tests {
    use std::os::windows::fs::OpenOptionsExt as _;

    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_READ_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };

    use super::*;

    #[test]
    fn a_live_guard_rejects_identity_mismatch_and_detects_a_replaced_path() {
        let base = std::env::temp_dir().join(format!(
            "runtrol-root-guard-{}-{}",
            std::process::id(),
            uuid::Uuid::now_v7()
        ));
        let root_path = base.join("root");
        std::fs::create_dir_all(&root_path).expect("create guarded root");
        let root =
            AbsPath::canonicalize(root_path.to_str().expect("UTF-8 root")).expect("canonical root");
        let identity = ProjectRootIdentity::read(&root).expect("read root identity");
        assert!(
            ProjectRootGuard::acquire(&root, ProjectRootIdentity::from_bytes([0; 24])).is_err(),
            "a different durable identity must not acquire a guard"
        );
        let provider_cwd_handle = std::fs::OpenOptions::new()
            .access_mode(FILE_READ_ATTRIBUTES)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(&root_path)
            .expect("open the root as an existing provider may hold it");
        let mut guard = ProjectRootGuard::acquire(&root, identity).expect("guard approved root");
        drop(provider_cwd_handle);
        guard.validate().expect("the original directory name holds");
        let moved = base.join("moved");
        std::fs::rename(&root_path, &moved)
            .expect("rename remains compatible with provider cwd handles");
        std::fs::create_dir(&root_path).expect("replace approved path");
        assert!(
            guard.validate().is_err(),
            "the handle must expose that its original directory moved"
        );
        drop(guard);
        std::fs::remove_dir_all(&base).expect("clean root guard fixture");
    }
}
