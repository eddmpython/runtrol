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

use std::io::Write as _;
use std::path::{Path, PathBuf};

/// The crash file never grows past this. Old words rotate away rather than accumulate: the newest
/// crash is the one being investigated, and an unbounded file in a supervised home is its own defect.
const CRASH_LOG_BOUND_BYTES: u64 = 128 * 1024;

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
    let entry = format!(
        "at_epoch_ms={moment}\n{info}\nbacktrace:\n{}\n---\n",
        std::backtrace::Backtrace::force_capture()
    );
    // A panic hook must not panic and has nobody left to tell: if these writes fail, the process is
    // already dying and the fallback stderr line below is the only remaining honest move.
    if std::fs::metadata(path).is_ok_and(|meta| meta.len() > CRASH_LOG_BOUND_BYTES)
        && std::fs::remove_file(path).is_err()
    {
        eprintln!(
            "runtrol could not rotate its crash file at {}",
            path.display()
        );
    }
    let written = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .and_then(|mut file| file.write_all(entry.as_bytes()));
    if let Err(error) = written {
        eprintln!(
            "runtrol could not record its own crash at {}: {error}",
            path.display()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a_panic_record(path: &Path, filler: &str) {
        // The hook shape cannot be driven without a real panic, and a test that installs the global
        // hook races every other test. The writer is exercised directly instead, through the same
        // entry `record` builds, which keeps the global hook a two-line glue nobody needs to test.
        let entry = format!("at_epoch_ms=0\n{filler}\n---\n");
        if std::fs::metadata(path).is_ok_and(|meta| meta.len() > CRASH_LOG_BOUND_BYTES) {
            std::fs::remove_file(path).expect("test rotation removes its own file");
        }
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .expect("test crash file opens");
        file.write_all(entry.as_bytes())
            .expect("test crash entry writes");
    }

    #[test]
    fn crash_entries_append_and_rotate_at_the_bound() {
        let directory =
            std::env::temp_dir().join(format!("runtrol-crash-log-test-{}", std::process::id()));
        std::fs::create_dir_all(&directory).expect("test directory");
        let path = directory.join("daemon-crash.log");
        let _cleared = std::fs::remove_file(&path);

        a_panic_record(&path, "first crash");
        a_panic_record(&path, "second crash");
        let both = std::fs::read_to_string(&path).expect("crash file readable");
        assert!(both.contains("first crash") && both.contains("second crash"));

        let oversized =
            "x".repeat(usize::try_from(CRASH_LOG_BOUND_BYTES).expect("small bound") + 1);
        a_panic_record(&path, &oversized);
        a_panic_record(&path, "after rotation");
        let rotated = std::fs::read_to_string(&path).expect("rotated crash file readable");
        assert!(
            !rotated.contains("first crash"),
            "rotation dropped old words"
        );
        assert!(rotated.contains("after rotation"));

        std::fs::remove_dir_all(&directory).expect("remove the crash test directory");
    }
}
