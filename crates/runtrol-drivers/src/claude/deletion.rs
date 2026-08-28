//! Permanent removal of one operator-selected conversation from this CLI's own store.
//!
//! This is the driver's only write surface into that store. The CLI publishes no delete command, so the driver
//! removes the complete measured artifact set it already reads: the conversation JSONL, its sidecar directory,
//! and prompt-history rows carrying the same native identity. A same-identity remnant made by Runtrol's retired
//! reversible-trash implementation is removed too. No transcript is copied elsewhere.

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, BufWriter, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::Deserialize;

const HISTORY_FILE: &str = "history.jsonl";
const LEGACY_TRASH_DIRECTORY: &str = "runtrol-deleted";
const CONVERSATION_EXTENSION: &str = "jsonl";
const MAX_HISTORY_RECORD_BYTES: usize = 64 * 1024;
static NEXT_REWRITE: AtomicU64 = AtomicU64::new(0);

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct HistoryIdentity {
    #[serde(default)]
    session_id: Option<String>,
}

/// Permanently remove one conversation and verify that its complete known artifact set is absent.
///
/// # Errors
///
/// An artifact could not be inspected or removed, a prompt-history record exceeded the safe rewrite bound, or
/// verification still found the selected native identity.
pub(super) fn remove(projects: &Path, native: &str) -> io::Result<()> {
    if native.is_empty() || native.contains(['/', '\\', '.']) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "the native conversation identity is not a safe store name",
        ));
    }
    let config = projects.parent().unwrap_or(projects);
    let history = config.join(HISTORY_FILE);
    remove_history_rows(&history, native)?;

    let artifacts = conversation_artifacts(projects, native)?;
    for artifact in &artifacts {
        remove_path(artifact)?;
    }
    remove_legacy_remnant(config, native)?;

    for artifact in conversation_artifacts(projects, native)? {
        if artifact.exists() {
            return Err(io::Error::other(format!(
                "the deleted conversation artifact remains at {}",
                artifact.display()
            )));
        }
    }
    if history_contains(&history, native)? {
        return Err(io::Error::other(
            "the deleted conversation identity remains in prompt history",
        ));
    }
    Ok(())
}

fn conversation_artifacts(projects: &Path, native: &str) -> io::Result<Vec<PathBuf>> {
    let entries = match fs::read_dir(projects) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let transcript = format!("{native}.{CONVERSATION_EXTENSION}");
    let mut artifacts = Vec::new();
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let directory = entry.path();
        let file = directory.join(&transcript);
        if file.exists() {
            artifacts.push(file);
        }
        let sidecar = directory.join(native);
        if sidecar.exists() {
            artifacts.push(sidecar);
        }
    }
    Ok(artifacts)
}

fn remove_legacy_remnant(config: &Path, native: &str) -> io::Result<()> {
    let legacy = config.join(LEGACY_TRASH_DIRECTORY);
    remove_path(&legacy.join(format!("{native}.{CONVERSATION_EXTENSION}")))?;
    remove_path(&legacy.join(native))
}

fn remove_path(path: &Path) -> io::Result<()> {
    if path.is_dir() {
        match fs::remove_dir_all(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    } else {
        match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }
}

fn remove_history_rows(path: &Path, native: &str) -> io::Result<()> {
    let input = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    let temporary = temporary_history_path(path)?;
    let mut output = BufWriter::new(
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?,
    );
    let rewrite = (|| {
        let mut input = BufReader::new(input);
        let mut record = Vec::new();
        loop {
            let read = read_history_record(&mut input, &mut record)?;
            if read == 0 {
                break;
            }
            let body = record.strip_suffix(b"\n").unwrap_or(&record);
            let body = body.strip_suffix(b"\r").unwrap_or(body);
            let selected = history_record_names(body, native);
            if !selected {
                output.write_all(&record)?;
            }
        }
        output.flush()?;
        output.get_ref().sync_all()
    })();
    if let Err(error) = rewrite {
        drop(output);
        drop(fs::remove_file(&temporary));
        return Err(error);
    }
    drop(output);
    if let Err(error) = fs::rename(&temporary, path) {
        drop(fs::remove_file(&temporary));
        return Err(error);
    }
    Ok(())
}

fn temporary_history_path(path: &Path) -> io::Result<PathBuf> {
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "the prompt-history path has no parent directory",
        )
    })?;
    let serial = NEXT_REWRITE.fetch_add(1, Ordering::Relaxed);
    Ok(parent.join(format!(
        ".runtrol-history-delete-{}-{serial}.writing",
        std::process::id()
    )))
}

fn history_contains(path: &Path, native: &str) -> io::Result<bool> {
    let input = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    let mut input = BufReader::new(input);
    let mut record = Vec::new();
    loop {
        let read = read_history_record(&mut input, &mut record)?;
        if read == 0 {
            return Ok(false);
        }
        let body = record.strip_suffix(b"\n").unwrap_or(&record);
        let body = body.strip_suffix(b"\r").unwrap_or(body);
        if history_record_names(body, native) {
            return Ok(true);
        }
    }
}

/// Read at most one bounded history row, refusing before an untrusted missing newline can grow memory further.
fn read_history_record(input: &mut impl BufRead, record: &mut Vec<u8>) -> io::Result<usize> {
    record.clear();
    let limit = u64::try_from(MAX_HISTORY_RECORD_BYTES)
        .unwrap_or(u64::MAX)
        .saturating_add(1);
    let read = (&mut *input).take(limit).read_until(b'\n', record)?;
    if record.len() > MAX_HISTORY_RECORD_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "a prompt-history record exceeds the permanent-delete bound",
        ));
    }
    Ok(read)
}

/// Whether one complete history row explicitly names the selected provider identity.
///
/// A malformed unrelated row is preserved. Deleting it would expand a targeted permanent deletion into provider
/// state the driver cannot identify, while every selected row that can be resumed has the structured identity this
/// parser checks.
fn history_record_names(body: &[u8], native: &str) -> bool {
    match serde_json::from_slice::<HistoryIdentity>(body) {
        Ok(row) => row.session_id.is_some_and(|session| session == native),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selected_history_rows_and_legacy_remnants_are_permanently_removed() {
        let root = std::env::temp_dir().join(format!(
            "runtrol-claude-permanent-delete-{}",
            std::process::id()
        ));
        drop(fs::remove_dir_all(&root));
        let projects = root.join("projects");
        let folder = projects.join("folder");
        fs::create_dir_all(folder.join("selected")).expect("sidecar is created");
        fs::write(folder.join("selected.jsonl"), b"transcript\n").expect("transcript is written");
        fs::write(folder.join("selected").join("agent.jsonl"), b"side\n")
            .expect("side transcript is written");
        fs::write(
            root.join(HISTORY_FILE),
            b"{\"sessionId\":\"kept\",\"display\":\"keep\"}\n{\"sessionId\":\"selected\",\"display\":\"remove\"}\n",
        )
        .expect("history is written");
        let legacy = root.join(LEGACY_TRASH_DIRECTORY);
        fs::create_dir_all(legacy.join("selected")).expect("legacy sidecar is created");
        fs::write(legacy.join("selected.jsonl"), b"old copy\n").expect("legacy copy is written");

        remove(&projects, "selected").expect("permanent deletion succeeds");

        assert!(!folder.join("selected.jsonl").exists());
        assert!(!folder.join("selected").exists());
        assert!(!legacy.join("selected.jsonl").exists());
        assert!(!legacy.join("selected").exists());
        let history =
            fs::read_to_string(root.join(HISTORY_FILE)).expect("history remains readable");
        assert!(history.contains("kept"));
        assert!(!history.contains("selected"));
        drop(fs::remove_dir_all(root));
    }

    #[test]
    fn an_oversized_history_row_refuses_before_any_artifact_is_removed() {
        let root = std::env::temp_dir().join(format!(
            "runtrol-claude-bounded-delete-{}",
            std::process::id()
        ));
        drop(fs::remove_dir_all(&root));
        let projects = root.join("projects");
        let folder = projects.join("folder");
        fs::create_dir_all(&folder).expect("project is created");
        let transcript = folder.join("selected.jsonl");
        fs::write(&transcript, b"transcript\n").expect("transcript is written");
        fs::write(
            root.join(HISTORY_FILE),
            vec![b'x'; MAX_HISTORY_RECORD_BYTES + 1],
        )
        .expect("oversized history is written");

        let error = remove(&projects, "selected").expect_err("oversized history is refused");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(transcript.exists(), "the transcript remains after refusal");
        assert_eq!(
            fs::metadata(root.join(HISTORY_FILE))
                .expect("history remains")
                .len(),
            u64::try_from(MAX_HISTORY_RECORD_BYTES + 1).expect("test size fits")
        );
        drop(fs::remove_dir_all(root));
    }
}
