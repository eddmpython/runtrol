//! One atomic ownership-format transition that cached legacy writers cannot overwrite after a crash.

use runtrol_provider::AbsPath;
use std::fs::{self, OpenOptions};
use std::io::Write as _;

use super::{DATA_FILE, data_path, encode, plain_directory, read_file};

/// The caller holds the existing registry lock through this complete synchronous publication.
pub(super) fn publish(path: &AbsPath) -> Result<(), String> {
    if data_path(path)? != path.as_std_path() {
        return Ok(());
    }
    #[cfg(not(windows))]
    if path.as_std_path().exists() {
        return Err(
            "migration of a legacy worktree registry requires its platform writer exclusion"
                .to_owned(),
        );
    }
    // Schema one always commits through this exact sibling name (introduced in 13ef50a).
    // Holding its source without write/delete sharing excludes both a current writer and a delayed rename.
    let legacy = path
        .as_std_path()
        .with_file_name("isolated-workspaces.json.writing");
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt as _;
        options.share_mode(1); // FILE_SHARE_READ only.
    }
    let held = options
        .open(&legacy)
        .map_err(|error| format!("a legacy worktree commit is busy: {error}"))?;
    // The old writer either committed before exclusion or must fail after it. Read only after acquiring it.
    let records = read_file(path)?.map_or_else(Vec::new, |file| file.records);
    let staged = path.as_std_path().with_extension("migrating");
    clear_staging(&staged)?;
    fs::create_dir(&staged).map_err(|error| error.to_string())?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(staged.join(DATA_FILE))
        .map_err(|error| error.to_string())?;
    file.write_all(&encode(&records)?)
        .map_err(|error| error.to_string())?;
    file.sync_all().map_err(|error| error.to_string())?;
    drop(file);
    // Windows atomically replaces the old file with this nonempty directory. A legacy file can no
    // longer replace it, even after every migration handle and Runtime has exited.
    fs::rename(&staged, path.as_std_path())
        .map_err(|error| format!("publishing worktree ownership: {error}"))?;
    drop(held);
    Ok(())
}

fn clear_staging(path: &std::path::Path) -> Result<(), String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.to_string()),
    };
    if !plain_directory(&metadata) {
        return Err("the worktree migration staging path changed type".to_owned());
    }
    let mut entries = fs::read_dir(path).map_err(|error| error.to_string())?;
    let entry = entries
        .next()
        .transpose()
        .map_err(|error| error.to_string())?;
    if entries
        .next()
        .transpose()
        .map_err(|error| error.to_string())?
        .is_some()
    {
        return Err("the worktree migration staging directory has extra entries".to_owned());
    }
    if let Some(entry) = entry {
        if entry.file_name() != DATA_FILE
            || !entry
                .file_type()
                .map_err(|error| error.to_string())?
                .is_file()
        {
            return Err("the worktree migration staging directory has an unknown entry".to_owned());
        }
        fs::remove_file(entry.path()).map_err(|error| error.to_string())?;
    }
    fs::remove_dir(path).map_err(|error| error.to_string())
}
