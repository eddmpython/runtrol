//! The detached daemon's last words.
//!
//! A daemon started by a command runs with every stream pointed at nothing, so a panic in it used to
//! vanish: the process disappeared and `runtrol list` said only "the daemon stopped without
//! answering" (measured three times across two machines before this file existed). The panic hook
//! installed here appends what happened to one bounded file inside the daemon's own home, which is
//! the one place the operator and a gate can already look.
//!
//! This is not a log surface. Ordinary diagnostics still have no decided destination; the only thing
//! recorded here is a panic, because a crash whose reason evaporates is the exact silence the error
//! rules forbid.

use std::fmt::Write as _;
use std::io::{Seek as _, SeekFrom, Write as _};
use std::path::{Path, PathBuf};

/// The crash file never grows past this. Old words rotate away rather than accumulate: the newest
/// crash is the one being investigated, and an unbounded file in a supervised home is its own defect.
const CRASH_LOG_BOUND_BYTES: usize = 128 * 1024;
const TRUNCATED: &str = "\n[crash report truncated]\n";

/// Record every later panic of this process into `path`, then keep unwinding as before.
///
/// The previous hook still runs first, so a foreground daemon keeps printing to stderr exactly as it
/// did. Installed once at daemon start; installing it again would only chain another writer.
pub fn record_panics_at(path: &Path) {
    let target = PathBuf::from(path);
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        previous(info);
        record(&target, info);
    }));
}

/// Append one panic to the bounded crash file.
#[expect(
    clippy::print_stderr,
    reason = "inside a panic hook there is nobody left to return an error to, and stderr is the only remaining honest channel when even the crash file cannot be written"
)]
fn record(path: &Path, info: &std::panic::PanicHookInfo<'_>) {
    let moment = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis());
    let mut entry = Entry(String::with_capacity(CRASH_LOG_BOUND_BYTES));
    if write!(&mut entry, "at_epoch_ms={moment}\n{info}\nbacktrace:\n").is_err()
        || writeln!(
            &mut entry,
            "{}\n---",
            std::backtrace::Backtrace::force_capture()
        )
        .is_err()
    {
        // The formatter stops at the byte ceiling. Do not build an unbounded intermediate string, and
        // do not capture or format a backtrace after the panic information already exhausted the budget.
        entry.0.push_str(TRUNCATED);
    }
    // A panic hook must not panic and has nobody left to tell: stderr is the remaining channel when
    // the owned crash file cannot be opened, locked, rotated or written.
    if let Err(error) = append(path, entry.0.as_bytes()) {
        eprintln!(
            "runtrol could not record its own crash at {}: {error}",
            path.display()
        );
    }
}

/// One bounded UTF-8 report. The reserved suffix makes truncation explicit without exceeding the ceiling.
struct Entry(String);

impl std::fmt::Write for Entry {
    fn write_str(&mut self, value: &str) -> std::fmt::Result {
        let remaining = CRASH_LOG_BOUND_BYTES.saturating_sub(TRUNCATED.len() + self.0.len());
        if value.len() <= remaining {
            self.0.push_str(value);
            return Ok(());
        }
        if let Some(prefix) = value.get(..value.floor_char_boundary(remaining)) {
            self.0.push_str(prefix);
        }
        Err(std::fmt::Error)
    }
}

/// The same open file owns rotation and append, serialized across simultaneous Runtime generations.
fn append(path: &Path, entry: &[u8]) -> std::io::Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(path)?;
    file.lock()?;
    let bound = u64::try_from(CRASH_LOG_BOUND_BYTES).map_err(std::io::Error::other)?;
    let added = u64::try_from(entry.len()).map_err(std::io::Error::other)?;
    if file.metadata()?.len().saturating_add(added) > bound {
        file.set_len(0)?;
    }
    file.seek(SeekFrom::End(0))?;
    file.write_all(entry)
}

#[cfg(test)]
#[path = "crash/tests/courier.rs"]
mod courier_tests;
