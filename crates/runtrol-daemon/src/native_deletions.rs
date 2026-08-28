//! The record of every conversation this Runtime removes from a coding service's store.
//!
//! # Why the record exists
//!
//! Deleting a native conversation moves it into `runtrol-deleted` and says nothing else: not when, not which
//! integration asked, not which folder they named. On 2026-08-28 eight conversations were found there, and
//! reconstructing who had done it from file times and a stale catalogue took a session and still did not reach
//! an answer. When six more went the same way the next day, this record named the integration and the minute
//! for each one, and the cause was found in the time it takes to read six lines.
//!
//! # Why it is its own module
//!
//! Because it is the one place in the daemon that writes to a file outside the provider's own commands, and the
//! disk-mutation gate names owners by module. Leaving it inside the public request handler would have meant
//! allowlisting a file that serves the whole protocol, which would make every future write in that file
//! invisible to the gate. One small owner keeps the gate sharp.

use std::io::Write;

use runtrol_provider::{AbsPath, NativeSessionId, ProviderId};

use crate::Composed;
use crate::runtime_auth::AuthorizedIntegration;

/// Who asked for a native mutation and the folder they named.
pub(crate) struct MutationOrigin<'a> {
    pub(crate) integration: &'a AuthorizedIntegration,
    pub(crate) workspace: &'a AbsPath,
}

/// Append one line naming the conversation removed and who asked for it.
///
/// A failed write is reported to the daemon's own error stream and no further. The conversation has already
/// been moved by the provider, and turning a bookkeeping failure into a refusal would tell the person the
/// opposite of what happened to their store.
#[expect(
    clippy::print_stderr,
    reason = "the daemon's own error stream is where a failed record has to land: the conversation is already gone and there is no other surface left to say so"
)]
pub(crate) fn record(
    composed: &Composed,
    origin: &MutationOrigin<'_>,
    provider: ProviderId,
    native: &NativeSessionId,
) {
    let at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_millis());
    let path = composed.home.paths().native_deletions();
    let opened = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path.as_std_path());
    match opened {
        Ok(mut file) => {
            let written = writeln!(
                file,
                "{at} {integration} {provider} {native} {workspace}",
                integration = origin.integration.grant.integration_id,
                provider = provider.as_str(),
                native = native.as_str(),
                workspace = origin.workspace.as_str(),
            );
            if let Err(error) = written {
                eprintln!("runtrol: could not record a conversation deletion: {error}");
            }
        }
        Err(error) => eprintln!("runtrol: could not open the deletion record: {error}"),
    }
}
