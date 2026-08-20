//! The running executable's own identity, measured once and announced in the hello.

use std::sync::OnceLock;

use sha2::{Digest as _, Sha256};

static BUILD_DIGEST: OnceLock<Option<String>> = OnceLock::new();

/// SHA-256 of the executable this process is running, as lowercase hex.
///
/// A manager that installed this binary compares the announced digest with the file it installed
/// and supersedes the daemon on any difference, so this is how a replaced-on-disk core gets
/// noticed by the process still running the old image. `None` when the executable cannot be read
/// back, and absence makes the manager supersede too: an unidentifiable daemon is replaced by one
/// that can say who it is, which is why not carrying the error forward is safe here.
pub(crate) fn build_digest() -> Option<&'static str> {
    BUILD_DIGEST
        .get_or_init(|| {
            #[expect(
                clippy::disallowed_methods,
                reason = "an unreadable own image yields no digest on purpose: absence in the \
                          hello makes the manager supersede this daemon, which is the handling"
            )]
            measure().ok()
        })
        .as_deref()
}

fn measure() -> std::io::Result<String> {
    let executable = std::env::current_exe()?;
    let mut file = std::fs::File::open(executable)?;
    let mut hasher = Sha256::new();
    // Streamed so the whole image never sits in memory: the idle footprint is a contract.
    std::io::copy(&mut file, &mut hasher)?;
    let digest: [u8; 32] = hasher.finalize().into();
    Ok(crate::runtime_auth::hex(&digest))
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
    }
}
