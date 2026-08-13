//! Stable filesystem identity for an approved project root.

use std::io;

use runtrol_provider::AbsPath;

/// Kernel-issued identity of the directory that occupied an approved path.
///
/// Paths are operator-facing labels. This value is the authority binding that makes deleting or
/// replacing a directory at the same path invalidate an existing grant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProjectRootIdentity([u8; 24]);

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
    use std::fs::OpenOptions;
    use std::mem::size_of;
    use std::os::windows::fs::OpenOptionsExt as _;
    use std::os::windows::io::AsRawHandle as _;

    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_ID_INFO, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE,
        FILE_SHARE_READ, FILE_SHARE_WRITE, FileIdInfo, GetFileInformationByHandleEx,
    };

    use super::{AbsPath, ProjectRootIdentity, io};

    #[expect(
        unsafe_code,
        reason = "Windows exposes stable directory identity only through one handle query whose live handle and exact output layout are bounded beside the call"
    )]
    pub(super) fn read(path: &AbsPath) -> Result<ProjectRootIdentity, io::Error> {
        let file = OpenOptions::new()
            .access_mode(FILE_READ_ATTRIBUTES)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(path.as_std_path())?;
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
