//! Platform-standard Runtime locator discovery and bounded validation.

use std::path::{Path, PathBuf};

use runtrol_runtime_protocol::{RUNTIME_LOCATOR_SCHEMA, RuntimeEndpointKind, RuntimeLocatorRecord};

const LOCATOR_FILE: &str = "runtime.locator.json";
const RUNTIME_FOLDER: &str = "runtrol";
const MAX_LOCATOR_BYTES: u64 = 8 * 1024;
const MAX_ENDPOINT_BYTES: usize = 1024;

/// A validated Runtime locator ready for connection and instance proof.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeLocator {
    path: PathBuf,
    #[cfg(test)]
    fixture: Option<ValidatedLocator>,
}

impl RuntimeLocator {
    /// Derive the one platform-standard per-user locator path.
    ///
    /// This never searches `PATH`, scans the filesystem, or accepts a configured executable or endpoint.
    ///
    /// # Errors
    ///
    /// [`LocatorError::Environment`] when the operating system's required per-user directory is unavailable or not
    /// absolute.
    pub fn system() -> Result<Self, LocatorError> {
        Ok(Self {
            path: system_state_root()?.join(RUNTIME_FOLDER).join(LOCATOR_FILE),
            #[cfg(test)]
            fixture: None,
        })
    }

    /// Read and validate the current locator without starting or downloading Runtime.
    ///
    /// # Errors
    ///
    /// A typed missing, unsafe, malformed, or I/O state.
    pub fn inspect(&self) -> Result<LocatorState, LocatorError> {
        #[cfg(test)]
        if let Some(locator) = &self.fixture {
            return Ok(LocatorState::Running(locator.clone()));
        }
        let metadata = match std::fs::symlink_metadata(&self.path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(LocatorState::NotInstalled);
            }
            Err(error) => {
                return Err(LocatorError::Io(io_detail(
                    "reading locator metadata",
                    &error,
                )));
            }
        };
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(LocatorError::Unsafe(
                "the Runtime locator is not a regular file".to_owned(),
            ));
        }
        if metadata.len() > MAX_LOCATOR_BYTES {
            return Err(LocatorError::Unsafe(format!(
                "the Runtime locator is {} bytes and the limit is {MAX_LOCATOR_BYTES}",
                metadata.len()
            )));
        }
        #[cfg(unix)]
        validate_unix_mode(&metadata)?;
        #[cfg(windows)]
        validate_windows_security(&self.path)?;

        let bytes = std::fs::read(&self.path)
            .map_err(|error| LocatorError::Io(io_detail("reading the Runtime locator", &error)))?;
        let record: RuntimeLocatorRecord = serde_json::from_slice(&bytes)
            .map_err(|error| LocatorError::Malformed(error.to_string()))?;
        validate_record(&record, &self.path)?;
        Ok(LocatorState::Running(ValidatedLocator {
            instance_id: record.instance_id,
            endpoint: record.endpoint,
            runtime_version: record.runtime_version,
        }))
    }

    /// Construct a locator at an isolated path for contract and packed-artifact tests.
    #[cfg(any(test, feature = "testing"))]
    #[must_use]
    pub fn for_testing(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            #[cfg(test)]
            fixture: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn for_testing_endpoint(
        instance_id: impl Into<String>,
        endpoint: impl Into<String>,
        runtime_version: impl Into<String>,
    ) -> Self {
        Self {
            path: PathBuf::new(),
            fixture: Some(ValidatedLocator {
                instance_id: instance_id.into(),
                endpoint: endpoint.into(),
                runtime_version: runtime_version.into(),
            }),
        }
    }
}

/// Observable locator state. Missing Runtime is a product state, not an I/O failure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LocatorState {
    /// No locator exists at the platform-standard location.
    NotInstalled,
    /// A bounded locator names a running candidate. Initialization still proves the instance.
    Running(ValidatedLocator),
}

/// Operational bootstrap fields after closed-schema and platform validation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidatedLocator {
    pub(crate) instance_id: String,
    pub(crate) endpoint: String,
    pub(crate) runtime_version: String,
}

impl ValidatedLocator {
    /// Durable Runtime instance identity proved during connection setup.
    #[must_use]
    pub fn instance_id(&self) -> &str {
        &self.instance_id
    }

    /// Owner-local endpoint admitted by the validated locator.
    #[must_use]
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// Runtime package version published with this endpoint.
    #[must_use]
    pub fn runtime_version(&self) -> &str {
        &self.runtime_version
    }
}

fn validate_record(record: &RuntimeLocatorRecord, locator_path: &Path) -> Result<(), LocatorError> {
    if record.schema != RUNTIME_LOCATOR_SCHEMA {
        return Err(LocatorError::Malformed(format!(
            "unsupported Runtime locator schema {}",
            record.schema
        )));
    }
    if record.instance_id.is_empty() || record.instance_id.len() > 128 {
        return Err(LocatorError::Malformed(
            "Runtime instance identity is empty or oversized".to_owned(),
        ));
    }
    if record.runtime_version.is_empty() || record.runtime_version.len() > 128 {
        return Err(LocatorError::Malformed(
            "Runtime version is empty or oversized".to_owned(),
        ));
    }
    if record.process_id == 0
        || record.endpoint.is_empty()
        || record.endpoint.len() > MAX_ENDPOINT_BYTES
    {
        return Err(LocatorError::Malformed(
            "Runtime process or endpoint is invalid".to_owned(),
        ));
    }
    validate_endpoint(record.endpoint_kind, &record.endpoint, locator_path)
}

#[cfg(windows)]
fn validate_endpoint(
    kind: RuntimeEndpointKind,
    endpoint: &str,
    _locator_path: &Path,
) -> Result<(), LocatorError> {
    if !matches!(kind, RuntimeEndpointKind::NamedPipe)
        || !endpoint.starts_with(r"\\.\pipe\runtrol-runtime-")
    {
        return Err(LocatorError::Unsafe(
            "the Runtime locator does not name its dedicated local pipe".to_owned(),
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn validate_endpoint(
    kind: RuntimeEndpointKind,
    endpoint: &str,
    locator_path: &Path,
) -> Result<(), LocatorError> {
    if !matches!(kind, RuntimeEndpointKind::UnixSocket) {
        return Err(LocatorError::Unsafe(
            "the Runtime locator does not name a Unix socket".to_owned(),
        ));
    }
    let endpoint = Path::new(endpoint);
    let expected_parent = locator_path.parent().ok_or_else(|| {
        LocatorError::Unsafe("the Runtime locator has no owning state directory".to_owned())
    })?;
    if !endpoint.is_absolute()
        || endpoint.parent() != Some(expected_parent)
        || endpoint.file_name().and_then(std::ffi::OsStr::to_str) != Some("runtrol-runtime.sock")
    {
        return Err(LocatorError::Unsafe(
            "the Runtime socket escaped its owner-only state directory".to_owned(),
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn validate_unix_mode(metadata: &std::fs::Metadata) -> Result<(), LocatorError> {
    use std::os::unix::fs::MetadataExt as _;

    if metadata.mode() & 0o077 != 0 {
        return Err(LocatorError::Unsafe(
            "the Runtime locator is readable outside its owner".to_owned(),
        ));
    }
    Ok(())
}

#[cfg(windows)]
fn validate_windows_security(path: &Path) -> Result<(), LocatorError> {
    windows_security::validate(path).map_err(|error| {
        LocatorError::Unsafe(format!(
            "the Runtime locator owner or DACL is not current-user-only: {error}"
        ))
    })
}

#[cfg(windows)]
mod windows_security {
    use std::os::windows::ffi::OsStrExt as _;

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, LocalFree};
    use windows_sys::Win32::Security::Authorization::{GetNamedSecurityInfoW, SE_FILE_OBJECT};
    use windows_sys::Win32::Security::{
        ACCESS_ALLOWED_ACE, ACL, ACL_SIZE_INFORMATION, AclSizeInformation,
        DACL_SECURITY_INFORMATION, EqualSid, GetAce, GetAclInformation,
        GetSecurityDescriptorControl, GetTokenInformation, OWNER_SECURITY_INFORMATION,
        PSECURITY_DESCRIPTOR, PSID, SE_DACL_PROTECTED, TOKEN_QUERY, TOKEN_USER, TokenUser,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    pub(super) fn validate(path: &std::path::Path) -> std::io::Result<()> {
        validate_inner(path)
    }

    #[expect(
        unsafe_code,
        reason = "Windows returns one self-relative file security descriptor whose owner and DACL pointers remain bounded by an RAII allocation during validation"
    )]
    fn validate_inner(path: &std::path::Path) -> std::io::Result<()> {
        let wide: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(core::iter::once(0))
            .collect();
        let mut owner: PSID = core::ptr::null_mut();
        let mut dacl: *mut ACL = core::ptr::null_mut();
        let mut descriptor: PSECURITY_DESCRIPTOR = core::ptr::null_mut();
        // SAFETY: `wide` is NUL terminated and every output pointer is writable. On success descriptor owns the
        // owner and DACL memory until LocalFree.
        let status = unsafe {
            GetNamedSecurityInfoW(
                wide.as_ptr(),
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                &raw mut owner,
                core::ptr::null_mut(),
                &raw mut dacl,
                core::ptr::null_mut(),
                &raw mut descriptor,
            )
        };
        if status != 0 || descriptor.is_null() || owner.is_null() || dacl.is_null() {
            let error = if status == 0 {
                std::io::Error::other("security descriptor omitted its owner or DACL")
            } else {
                std::io::Error::from_raw_os_error(i32::try_from(status).unwrap_or(i32::MAX))
            };
            if !descriptor.is_null() {
                // SAFETY: a non-null descriptor returned by GetNamedSecurityInfoW is LocalFree-owned.
                let leftover = unsafe { LocalFree(descriptor) };
                debug_assert!(
                    leftover.is_null(),
                    "LocalFree did not release a rejected descriptor"
                );
            }
            return Err(error);
        }
        let descriptor = OwnedDescriptor(descriptor);
        let current = current_user()?;
        // SAFETY: both SIDs belong to live security buffers during this call.
        if unsafe { EqualSid(owner, current.sid()) } == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "the locator owner SID is not the current user",
            ));
        }

        let mut control = 0_u16;
        let mut revision = 0_u32;
        // SAFETY: the descriptor is live and both output values are writable.
        if unsafe {
            GetSecurityDescriptorControl(descriptor.0, &raw mut control, &raw mut revision)
        } == 0
            || control & SE_DACL_PROTECTED == 0
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "the locator DACL is not protected from inherited access",
            ));
        }

        let mut information = ACL_SIZE_INFORMATION::default();
        // SAFETY: dacl belongs to the live descriptor and information is correctly sized writable storage.
        if unsafe {
            GetAclInformation(
                dacl,
                (&raw mut information).cast(),
                u32::try_from(core::mem::size_of::<ACL_SIZE_INFORMATION>())
                    .map_err(|_| std::io::Error::other("ACL information size does not fit u32"))?,
                AclSizeInformation,
            )
        } == 0
            || information.AceCount != 1
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "the locator DACL is not one exact owner rule",
            ));
        }
        let mut raw_ace = core::ptr::null_mut();
        // SAFETY: the ACL reports exactly one ACE, and raw_ace is a writable output pointer.
        if unsafe { GetAce(dacl, 0, &raw mut raw_ace) } == 0 || raw_ace.is_null() {
            return Err(std::io::Error::last_os_error());
        }
        let ace = raw_ace.cast::<ACCESS_ALLOWED_ACE>();
        // SDDL `A` is ACCESS_ALLOWED_ACE_TYPE, whose stable Windows value is zero.
        // SAFETY: GetAce returned a pointer to the first complete ACE inside the live DACL.
        if unsafe { (*ace).Header.AceType } != 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "the locator DACL rule is not an allow rule",
            ));
        }
        // SAFETY: SidStart is the variable-width SID beginning inside ACCESS_ALLOWED_ACE, and both buffers are live.
        let ace_sid = unsafe { core::ptr::addr_of!((*ace).SidStart).cast_mut().cast() };
        // SAFETY: both inputs are valid live SID pointers.
        if unsafe { EqualSid(ace_sid, current.sid()) } == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "the locator DACL grants a principal other than its owner",
            ));
        }
        Ok(())
    }

    struct OwnedDescriptor(PSECURITY_DESCRIPTOR);

    impl Drop for OwnedDescriptor {
        #[expect(
            unsafe_code,
            reason = "the descriptor came from GetNamedSecurityInfoW and LocalFree is its release API"
        )]
        fn drop(&mut self) {
            // SAFETY: this value is the only owner and no validation call retains the descriptor.
            let leftover = unsafe { LocalFree(self.0) };
            debug_assert!(leftover.is_null(), "LocalFree did not release a descriptor");
        }
    }

    struct OwnedHandle(HANDLE);

    impl Drop for OwnedHandle {
        #[expect(
            unsafe_code,
            reason = "the token handle is owned by this value and CloseHandle is its release API"
        )]
        fn drop(&mut self) {
            // SAFETY: a successful OpenProcessToken returned this one owned non-null handle.
            let closed = unsafe { CloseHandle(self.0) };
            debug_assert_ne!(closed, 0, "CloseHandle did not release a token");
        }
    }

    struct TokenUserBuffer(Vec<usize>);

    impl TokenUserBuffer {
        #[expect(
            unsafe_code,
            reason = "GetTokenInformation writes one bounded TOKEN_USER into aligned owned storage"
        )]
        fn read(token: HANDLE) -> std::io::Result<Self> {
            let mut needed = 0_u32;
            // SAFETY: the first call has no output buffer and asks only for the required length.
            unsafe {
                GetTokenInformation(token, TokenUser, core::ptr::null_mut(), 0, &raw mut needed)
            };
            if needed == 0 {
                return Err(std::io::Error::last_os_error());
            }
            let bytes = usize::try_from(needed)
                .map_err(|_| std::io::Error::other("token user size does not fit usize"))?;
            let mut storage = vec![0_usize; bytes.div_ceil(core::mem::size_of::<usize>())];
            // SAFETY: the aligned storage has at least needed writable bytes and remains live.
            if unsafe {
                GetTokenInformation(
                    token,
                    TokenUser,
                    storage.as_mut_ptr().cast(),
                    needed,
                    &raw mut needed,
                )
            } == 0
            {
                return Err(std::io::Error::last_os_error());
            }
            Ok(Self(storage))
        }

        #[expect(
            unsafe_code,
            reason = "the aligned buffer was populated as TOKEN_USER and owns the SID it points into"
        )]
        fn sid(&self) -> PSID {
            // SAFETY: `read` populated this aligned live allocation as TOKEN_USER.
            unsafe { (*self.0.as_ptr().cast::<TOKEN_USER>()).User.Sid }
        }
    }

    #[expect(
        unsafe_code,
        reason = "the current process pseudo-handle is borrowed only to open one owned query token"
    )]
    fn current_user() -> std::io::Result<TokenUserBuffer> {
        let mut token: HANDLE = core::ptr::null_mut();
        // SAFETY: GetCurrentProcess returns a valid pseudo-handle and token is a writable output pointer.
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &raw mut token) } == 0
            || token.is_null()
        {
            return Err(std::io::Error::last_os_error());
        }
        TokenUserBuffer::read(OwnedHandle(token).0)
    }
}

#[cfg(windows)]
fn system_state_root() -> Result<PathBuf, LocatorError> {
    absolute_environment("LOCALAPPDATA")
}

#[cfg(target_os = "macos")]
fn system_state_root() -> Result<PathBuf, LocatorError> {
    Ok(absolute_environment("HOME")?.join("Library/Application Support"))
}

#[cfg(all(unix, not(target_os = "macos")))]
fn system_state_root() -> Result<PathBuf, LocatorError> {
    match std::env::var_os("XDG_STATE_HOME") {
        Some(value) if !value.is_empty() && Path::new(&value).is_absolute() => {
            Ok(PathBuf::from(value))
        }
        _ => Ok(absolute_environment("HOME")?.join(".local/state")),
    }
}

fn absolute_environment(name: &'static str) -> Result<PathBuf, LocatorError> {
    let value = std::env::var_os(name).ok_or(LocatorError::Environment {
        name,
        why: "is not set",
    })?;
    if value.is_empty() {
        return Err(LocatorError::Environment {
            name,
            why: "is empty",
        });
    }
    let path = PathBuf::from(value);
    if !path.is_absolute() {
        return Err(LocatorError::Environment {
            name,
            why: "is not absolute",
        });
    }
    Ok(path)
}

fn io_detail(doing: &'static str, error: &std::io::Error) -> String {
    format!("{doing}: {error}")
}

/// Why a Runtime locator could not be trusted.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum LocatorError {
    /// A platform state location is missing or unsafe.
    #[error("{name} {why}")]
    Environment {
        /// Operating-system variable name.
        name: &'static str,
        /// Structural reason.
        why: &'static str,
    },
    /// The locator had a closed-schema or value failure.
    #[error("malformed Runtime locator: {0}")]
    Malformed(String),
    /// The locator or endpoint shape could cross the local owner boundary.
    #[error("unsafe Runtime locator: {0}")]
    Unsafe(String),
    /// Reading bounded locator state failed.
    #[error("Runtime locator I/O failed: {0}")]
    Io(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Scratch(PathBuf);

    impl Scratch {
        fn make(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "runtrol-runtime-client-{name}-{}",
                std::process::id()
            ));
            drop(std::fs::remove_dir_all(&path));
            std::fs::create_dir_all(&path).expect("create scratch");
            Self(path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            drop(std::fs::remove_dir_all(&self.0));
        }
    }

    #[test]
    fn missing_locator_is_a_product_state() {
        let scratch = Scratch::make("missing");
        let locator = RuntimeLocator::for_testing(scratch.0.join(LOCATOR_FILE));
        assert_eq!(locator.inspect(), Ok(LocatorState::NotInstalled));
    }

    #[test]
    fn oversize_and_unknown_fields_fail_closed() {
        let scratch = Scratch::make("hostile");
        let path = scratch.0.join(LOCATOR_FILE);
        let oversize =
            usize::try_from(MAX_LOCATOR_BYTES).expect("the locator limit fits usize") + 1;
        std::fs::write(&path, vec![b'x'; oversize]).expect("write oversize locator");
        let locator = RuntimeLocator::for_testing(&path);
        assert!(matches!(locator.inspect(), Err(LocatorError::Unsafe(_))));

        std::fs::write(
            &path,
            r#"{"schema":1,"instanceId":"rtm_x","endpointKind":"namedPipe","endpoint":"x","runtimeVersion":"1","processId":1,"authority":"self"}"#,
        )
        .expect("write unknown field");
        assert!(
            serde_json::from_slice::<RuntimeLocatorRecord>(
                &std::fs::read(&path).expect("read hostile record")
            )
            .is_err()
        );
        assert!(locator.inspect().is_err());
    }
}
