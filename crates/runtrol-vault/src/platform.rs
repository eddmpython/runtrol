//! Operating-system protection for the fixed-size machine identity payload.

use crate::VaultError;
use runtrol_provider::AbsPath;

#[cfg(windows)]
mod current {
    use core::ptr;

    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
    };

    use super::{AbsPath, VaultError};

    const ENTROPY: &[u8] = b"runtrol/machine-identity/1";

    pub(super) fn protect(_: &AbsPath, plaintext: &[u8]) -> Result<Vec<u8>, VaultError> {
        apply(
            "protecting the machine identity",
            plaintext,
            CryptProtectData,
        )
    }

    #[expect(
        unsafe_code,
        reason = "DPAPI unprotection and its LocalAlloc output have no safe Windows API. pointer lifetimes and ownership are stated beside each block"
    )]
    pub(super) fn unprotect(_: &AbsPath, ciphertext: &[u8]) -> Result<Vec<u8>, VaultError> {
        let input_len = u32::try_from(ciphertext.len()).map_err(|_| VaultError::Platform {
            doing: "unprotecting the machine identity",
            detail: "the protected blob is longer than DPAPI can represent".to_owned(),
        })?;
        let input = CRYPT_INTEGER_BLOB {
            cbData: input_len,
            pbData: ciphertext.as_ptr().cast_mut(),
        };
        let entropy = entropy_blob()?;
        let mut output = CRYPT_INTEGER_BLOB::default();
        // SAFETY: `input` and `entropy` point at live immutable slices for the duration of the call. Output starts
        // empty and DPAPI initializes it on success. Every optional pointer is null, UI is forbidden, and the
        // returned allocation is immediately owned by `LocalBlob`, which zeroes plaintext and calls `LocalFree`.
        let succeeded = unsafe {
            CryptUnprotectData(
                &raw const input,
                ptr::null_mut(),
                &raw const entropy,
                ptr::null(),
                ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &raw mut output,
            )
        };
        if succeeded == 0 {
            return Err(VaultError::platform("unprotecting the machine identity"));
        }
        LocalBlob::new(output, true).copy()
    }

    pub(super) fn delete(_: &AbsPath, _: &[u8]) {}

    type Protect = unsafe extern "system" fn(
        *const CRYPT_INTEGER_BLOB,
        windows_sys::core::PCWSTR,
        *const CRYPT_INTEGER_BLOB,
        *const core::ffi::c_void,
        *const windows_sys::Win32::Security::Cryptography::CRYPTPROTECT_PROMPTSTRUCT,
        u32,
        *mut CRYPT_INTEGER_BLOB,
    ) -> windows_sys::core::BOOL;

    #[expect(
        unsafe_code,
        reason = "DPAPI protection and its LocalAlloc output have no safe Windows API. pointer lifetimes and ownership are stated beside each block"
    )]
    fn apply(
        doing: &'static str,
        plaintext: &[u8],
        protect: Protect,
    ) -> Result<Vec<u8>, VaultError> {
        let input_len = u32::try_from(plaintext.len()).map_err(|_| VaultError::Platform {
            doing,
            detail: "the plaintext is longer than DPAPI can represent".to_owned(),
        })?;
        let input = CRYPT_INTEGER_BLOB {
            cbData: input_len,
            pbData: plaintext.as_ptr().cast_mut(),
        };
        let entropy = entropy_blob()?;
        let mut output = CRYPT_INTEGER_BLOB::default();
        // SAFETY: `input` and `entropy` point at live immutable slices for the duration of the call. DPAPI documents
        // both as input-only despite the legacy mutable `pbData` member. Output is initialized on success, optional
        // pointers are null, UI is forbidden, and `LocalBlob` owns the returned `LocalAlloc` allocation immediately.
        let succeeded = unsafe {
            protect(
                &raw const input,
                ptr::null(),
                &raw const entropy,
                ptr::null(),
                ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &raw mut output,
            )
        };
        if succeeded == 0 {
            return Err(VaultError::platform(doing));
        }
        LocalBlob::new(output, false).copy()
    }

    fn entropy_blob() -> Result<CRYPT_INTEGER_BLOB, VaultError> {
        let length = u32::try_from(ENTROPY.len()).map_err(|_| VaultError::Platform {
            doing: "binding DPAPI optional entropy",
            detail: "the fixed entropy length does not fit in u32".to_owned(),
        })?;
        Ok(CRYPT_INTEGER_BLOB {
            cbData: length,
            pbData: ENTROPY.as_ptr().cast_mut(),
        })
    }

    struct LocalBlob {
        blob: CRYPT_INTEGER_BLOB,
        sensitive: bool,
    }

    impl LocalBlob {
        const fn new(blob: CRYPT_INTEGER_BLOB, sensitive: bool) -> Self {
            Self { blob, sensitive }
        }

        #[expect(
            unsafe_code,
            reason = "DPAPI returns a raw LocalAlloc range. the checked length and owner lifetime are stated beside the slice construction"
        )]
        fn copy(&self) -> Result<Vec<u8>, VaultError> {
            let len = usize::try_from(self.blob.cbData).map_err(|_| VaultError::Platform {
                doing: "reading DPAPI output",
                detail: "the DPAPI output length does not fit this process".to_owned(),
            })?;
            if len == 0 {
                return Err(VaultError::Platform {
                    doing: "reading DPAPI output",
                    detail: "DPAPI returned an empty output allocation".to_owned(),
                });
            }
            if self.blob.pbData.is_null() {
                return Err(VaultError::Platform {
                    doing: "reading DPAPI output",
                    detail: "DPAPI returned a null output pointer with a nonzero length".to_owned(),
                });
            }
            // SAFETY: DPAPI returned `pbData` with `cbData` initialized on success, `LocalBlob` owns the allocation,
            // and it remains live until this method returns and `Drop` releases it.
            let bytes = unsafe { core::slice::from_raw_parts(self.blob.pbData, len) };
            Ok(bytes.to_vec())
        }
    }

    impl Drop for LocalBlob {
        #[expect(
            unsafe_code,
            reason = "DPAPI plaintext must be cleared and its LocalAlloc allocation released exactly once. the owner and byte bounds are stated beside both calls"
        )]
        fn drop(&mut self) {
            let len = if let Ok(len) = usize::try_from(self.blob.cbData) {
                len
            } else {
                eprintln!(
                    "DPAPI output length could not be represented while clearing sensitive memory"
                );
                0
            };
            if self.sensitive && len > 0 && !self.blob.pbData.is_null() {
                // SAFETY: `LocalBlob` exclusively owns the DPAPI allocation until `LocalFree` below. Writing exactly
                // `cbData` bytes clears plaintext before the allocation returns to the process heap.
                unsafe { core::ptr::write_bytes(self.blob.pbData, 0, len) };
            }
            // SAFETY: DPAPI documents its output as a `LocalAlloc` allocation. This object owns it exactly once and
            // no read occurs after this call.
            let not_freed = unsafe { LocalFree(self.blob.pbData.cast()) };
            if !not_freed.is_null() {
                eprintln!("DPAPI output memory could not be released");
            }
        }
    }
}

#[cfg(not(windows))]
mod current {
    use keyring::Entry;
    use runtrol_provider::AbsPath;
    use sha2::{Digest as _, Sha256};

    use super::VaultError;

    const SERVICE: &str = "runtrol.machine-identity";
    const ACCOUNT_DOMAIN: &[u8] = b"runtrol/native-vault-account/1";

    pub(super) fn protect(path: &AbsPath, plaintext: &[u8]) -> Result<Vec<u8>, VaultError> {
        let account = account_for(path);
        let entry = entry(&account, "opening the native machine identity entry")?;
        entry
            .set_secret(plaintext)
            .map_err(|error| platform("storing the native machine identity", error))?;
        Ok(account.into_bytes())
    }

    pub(super) fn unprotect(path: &AbsPath, protected: &[u8]) -> Result<Vec<u8>, VaultError> {
        let expected = account_for(path);
        if protected != expected.as_bytes() {
            return Err(VaultError::platform_detail(
                "binding the native machine identity entry",
                "the vault lookup identifier does not match its canonical path",
            ));
        }
        entry(&expected, "opening the native machine identity entry")?
            .get_secret()
            .map_err(|error| platform("reading the native machine identity", error))
    }

    pub(super) fn delete(path: &AbsPath, protected: &[u8]) -> Result<(), VaultError> {
        let expected = account_for(path);
        if protected != expected.as_bytes() {
            return Err(VaultError::platform_detail(
                "binding the native protected-secret entry for deletion",
                "the vault lookup identifier does not match its canonical path",
            ));
        }
        match entry(
            &expected,
            "opening the native protected-secret entry for deletion",
        )?
        .delete_credential()
        {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(platform(
                "deleting the native protected-secret entry",
                error,
            )),
        }
    }

    fn account_for(path: &AbsPath) -> String {
        let mut hasher = Sha256::new();
        hasher.update(ACCOUNT_DOMAIN);
        hasher.update(path.as_str().as_bytes());
        format!("{:x}", hasher.finalize())
    }

    fn entry(account: &str, doing: &'static str) -> Result<Entry, VaultError> {
        Entry::new(SERVICE, account).map_err(|error| platform(doing, error))
    }

    fn platform(doing: &'static str, _: keyring::Error) -> VaultError {
        VaultError::platform_detail(doing, "the native credential store refused the operation")
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn lookup_identifiers_are_canonical_path_bound_and_non_secret() {
            let root = std::env::temp_dir().join("runtrol-native-vault-account-test");
            std::fs::create_dir_all(&root).expect("create native vault account test directory");
            let root = AbsPath::canonicalize(
                root.to_str()
                    .expect("native vault account test path is UTF-8"),
            )
            .expect("canonical native vault account test directory");
            let first = root.join("first.vault").expect("valid first vault path");
            let second = root.join("second.vault").expect("valid second vault path");

            let first_account = account_for(&first);
            assert_eq!(first_account.len(), 64);
            assert!(
                first_account
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            );
            assert_ne!(first_account, account_for(&second));
            assert!(!first_account.contains(first.as_str()));
        }
    }
}

pub(crate) fn protect(path: &AbsPath, plaintext: &[u8]) -> Result<Vec<u8>, VaultError> {
    current::protect(path, plaintext)
}

pub(crate) fn unprotect(path: &AbsPath, ciphertext: &[u8]) -> Result<Vec<u8>, VaultError> {
    current::unprotect(path, ciphertext)
}

#[cfg(windows)]
pub(crate) fn delete(path: &AbsPath, ciphertext: &[u8]) {
    current::delete(path, ciphertext);
}

#[cfg(not(windows))]
pub(crate) fn delete(path: &AbsPath, ciphertext: &[u8]) -> Result<(), VaultError> {
    current::delete(path, ciphertext)
}
