//! The running executable's own identity, measured once and announced in the hello.
//!
//! A daemon generation is one build of this program, and the build is identified by its bytes. Every endpoint
//! a generation binds and every locator entry it publishes carries this digest, so two builds never share a
//! name and a client can always tell which build answered.

use std::path::Path;
use std::sync::OnceLock;

use sha2::{Digest as _, Sha256};

static BUILD_DIGEST: OnceLock<Option<String>> = OnceLock::new();

/// SHA-256 of the executable this process is running, as lowercase hex.
///
/// `None` when the executable cannot be read back, which the daemon treats as a refusal to start: a
/// generation without an identity cannot bind a generation endpoint. Commands that only announce the
/// digest carry the absence forward and let the reader decide.
pub(crate) fn build_digest() -> Option<&'static str> {
    BUILD_DIGEST
        .get_or_init(|| {
            #[expect(
                clippy::disallowed_methods,
                reason = "an unreadable own image yields no digest on purpose: the daemon refuses to \
                          serve without one, and a command announces the absence, which is the handling"
            )]
            measure_own_image().ok()
        })
        .as_deref()
}

/// SHA-256 of one executable file, as lowercase hex.
///
/// Streamed so the whole image never sits in memory: the idle footprint is a contract.
///
/// # Errors
///
/// The file could not be opened or read.
pub(crate) fn digest_of(executable: &Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(executable)?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher)?;
    let digest: [u8; 32] = hasher.finalize().into();
    Ok(crate::runtime_auth::hex(&digest))
}

fn measure_own_image() -> std::io::Result<String> {
    let digest = digest_of(&std::env::current_exe()?)?;
    // Reading the whole executable leaves its pages in this process's working set, and an idle
    // daemon's resident size is a checked contract, not a mood. Measured 2026-08-20: without this,
    // the idle budget went from five green runs to five red ones, over by 65~270 KiB. Windows has no
    // allocator-only equivalent: EmptyWorkingSet would evict the live code this identity protects and
    // make the first real request fault it all back in, so session cleanup remains its release boundary.
    #[cfg(not(windows))]
    runtrol_childproc::footprint::release_unused_memory();
    Ok(digest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_digest_is_stable_hex_of_this_executable() {
        let first = build_digest().expect("the test runner executable is readable");
        assert_eq!(first.len(), 64);
        assert!(first.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(build_digest(), Some(first));
        let again = digest_of(&std::env::current_exe().expect("own path")).expect("readable");
        assert_eq!(again, first);
    }
}
