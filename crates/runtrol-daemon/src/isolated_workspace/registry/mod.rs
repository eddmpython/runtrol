//! Short cross-generation read-modify-write transactions over the existing registry.

use std::fs::{File, OpenOptions};
use std::io::{Read as _, Write as _};
use std::path::Path;

use runtrol_provider::AbsPath;

use super::{FILE_SCHEMA, MAX_FILE_BYTES, MAX_RECORDS, Record, State, validate_records};

mod migration;
const DATA_FILE: &str = "registry.json";

pub(super) fn data_path(path: &AbsPath) -> Result<std::path::PathBuf, String> {
    match std::fs::symlink_metadata(path.as_std_path()) {
        Ok(metadata) if plain_directory(&metadata) => Ok(path.as_std_path().join(DATA_FILE)),
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
            Ok(path.as_std_path().to_owned())
        }
        Ok(_) => Err("the worktree registry has an unexpected filesystem type".to_owned()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(path.as_std_path().to_owned())
        }
        Err(error) => Err(error.to_string()),
    }
}

fn plain_directory(metadata: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt as _;
        // FILE_ATTRIBUTE_REPARSE_POINT: a junction cannot become the ownership container.
        metadata.is_dir() && metadata.file_attributes() & 0x400 == 0
    }
    #[cfg(not(windows))]
    {
        metadata.is_dir() && !metadata.file_type().is_symlink()
    }
}

fn lock(path: &Path) -> Result<File, String> {
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    file.try_lock()
        .map_err(|e| format!("worktree ownership is busy: {e}"))?;
    Ok(file)
}

pub(super) fn operation(
    path: &AbsPath,
    workspace_id: &str,
) -> Result<std::sync::Arc<File>, String> {
    let id =
        uuid::Uuid::parse_str(workspace_id).map_err(|_| "invalid worktree ownership identity")?;
    let stripe = usize::from(id.as_bytes()[15]) % runtrol_core::session::MAX_HOT;
    // Stable bounded stripes avoid historical per-worktree lock-file accumulation. They are never
    // unlinked while another process might still lock the old inode. A collision refuses promptly.
    lock(
        &path
            .as_std_path()
            .with_extension(format!("operation-{stripe}.lock")),
    )
    .map(std::sync::Arc::new)
}

fn read_file(path: &AbsPath) -> Result<Option<super::File>, String> {
    let current = data_path(path)?;
    let file = match File::open(&current) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if current != path.as_std_path() {
                return Err(
                    "the worktree ownership document is missing from its container".to_owned(),
                );
            }
            return Ok(None);
        }
        Err(error) => return Err(error.to_string()),
    };
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err("the isolated workspace registry exceeds its fixed bound".to_owned());
    }
    let mut file: super::File = serde_json::from_slice(&bytes)
        .map_err(|_| "the isolated workspace registry is malformed")?;
    match file.schema {
        1 => {
            // A legacy row retains its original session ownership and path/Git contract. Capturing
            // today's directory object here would falsely claim an identity absent from schema one.
            for record in &mut file.records {
                if record.terminal.is_some() || record.revision != 0 {
                    return Err("invalid legacy worktree ownership".to_owned());
                }
                record.revision = 1;
                record.legacy = true;
            }
        }
        FILE_SCHEMA => {
            if file.records.iter().any(|r| r.revision == 0) {
                return Err("invalid worktree ownership revision".to_owned());
            }
        }
        _ => return Err("the isolated workspace registry schema is unsupported".to_owned()),
    }
    validate_records(&file.records)?;
    Ok(Some(file))
}

pub(super) fn read(path: &AbsPath) -> Result<Vec<Record>, String> {
    read_file(path).map(|file| file.map_or_else(Vec::new, |file| file.records))
}

fn require_writable(file: Option<&super::File>) -> Result<(), String> {
    if file.is_none_or(|file| file.schema != FILE_SCHEMA) {
        return Err("the worktree registry has no writable ownership document".to_owned());
    }
    Ok(())
}

pub(super) fn check_writable(path: &AbsPath) -> Result<(), String> {
    let _held = lock(&path.as_std_path().with_extension("lock"))?;
    migration::publish(path)?;
    require_writable(read_file(path)?.as_ref())
}

pub(super) fn update(path: &AbsPath, mut changed: Record) -> Result<Vec<Record>, String> {
    let _held = lock(&path.as_std_path().with_extension("lock"))?;
    if data_path(path)? == path.as_std_path() {
        return Err("the worktree registry has not crossed its writer boundary".to_owned());
    }
    let file = read_file(path)?;
    require_writable(file.as_ref())?;
    let mut records = file.ok_or("the worktree registry is not admitted")?.records;
    let existing = records
        .iter()
        .position(|row| row.workspace_id == changed.workspace_id);
    match existing {
        Some(index)
            if records
                .get(index)
                .is_some_and(|record| record.revision == changed.revision)
                && changed.revision != 0 => {}
        None if changed.revision == 0 => {}
        _ => return Err("the worktree ownership revision changed".to_owned()),
    }
    changed.revision = changed
        .revision
        .checked_add(1)
        .ok_or("worktree ownership revision exhausted")?;
    if let Some(index) = existing {
        *records
            .get_mut(index)
            .ok_or("the worktree ownership disappeared")? = changed;
    } else {
        if records.len() >= MAX_RECORDS {
            let index = records
                .iter()
                .position(|r| r.state == State::Released)
                .ok_or("the bounded isolated workspace registry is full")?;
            records.remove(index);
        }
        records.push(changed);
    }
    validate_records(&records)?;
    write_file(path, &records)?;
    Ok(records)
}

fn encode(records: &[Record]) -> Result<Vec<u8>, String> {
    let bytes = serde_json::to_vec(&super::File {
        schema: FILE_SCHEMA,
        records: records.to_vec(),
    })
    .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err("the isolated workspace registry exceeds its fixed bound".to_owned());
    }
    Ok(bytes)
}

fn write_file(path: &AbsPath, records: &[Record]) -> Result<(), String> {
    let bytes = encode(records)?;
    let current = data_path(path)?;
    let temporary = current.with_extension("writing");
    let mut file = File::create(&temporary).map_err(|e| e.to_string())?;
    file.write_all(&bytes).map_err(|e| e.to_string())?;
    file.sync_all().map_err(|e| e.to_string())?;
    drop(file);
    std::fs::rename(temporary, current).map_err(|e| e.to_string())?;
    Ok(())
}
