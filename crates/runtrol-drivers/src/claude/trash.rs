//! The one place this driver writes to the CLI's own store, and nothing else lives here.
//!
//! # Why a module of its own
//!
//! Reading a provider's store is ordinary; writing to it is not. Every other read in [`super::store`] is a
//! listing, so keeping the four filesystem calls that move a conversation in a file of their own means the
//! reviewed write surface is these eighty lines rather than a fifteen-hundred-line reader that happens to
//! contain them. The disk-mutation gate registers this exact path, so a future write anywhere else in the
//! driver is a new architecture decision that has to be made deliberately.
//!
//! # Why writing here is allowed at all
//!
//! Claude Code publishes no delete command (measured on 2.1.241: its own surface offers resume and list, and
//! nothing that removes a stored conversation), yet a person who can see their conversations in a list expects
//! to be able to remove one from it. runtrol already reads this store to build that list, so the deletion is
//! served by the same contract rather than by a second mechanism.
//!
//! Three things bound it. Nothing is erased: the conversation is moved into `runtrol-deleted`, a sibling of
//! `projects` that no listing walks, so the act is reversible by hand. Only the exact conversation the operator
//! chose is touched. And the surface is reachable only under the delete scope the Runtime grants at the machine,
//! never from a paired phone.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// The directory a deleted conversation is kept in, beside the store rather than inside it.
const TRASH_DIRECTORY: &str = "runtrol-deleted";

/// Move one conversation, and the side transcripts stored beside it, out of the CLI's listing.
///
/// `projects` is the store's own conversation root, `file` the conversation's file within it, and `native` the
/// identifier the CLI knows it by, which is also the name of its sidecar directory.
///
/// # Errors
///
/// The underlying filesystem error when the move cannot be made: a file the CLI still holds open, or a trash
/// directory that could not be created.
pub(super) fn discard(projects: &Path, file: &Path, native: &str) -> io::Result<()> {
    let trash = trash_directory(projects);
    fs::create_dir_all(&trash)?;
    let folder = file.parent().map(Path::to_path_buf);
    move_out(file, &trash)?;
    // The CLI keeps a subagent's side transcripts in a directory named for the conversation, beside its file;
    // it leaves the store with the conversation it belongs to.
    if let Some(folder) = folder {
        let sidecar = folder.join(native);
        if sidecar.is_dir() {
            move_out(&sidecar, &trash)?;
        }
    }
    Ok(())
}

/// Where a deleted conversation is kept: `runtrol-deleted`, a sibling of `projects`.
///
/// A sibling, not a child, so a moved conversation leaves every listing at once (the listing only ever walks
/// `projects`). Named plainly so an operator can find and restore it by hand.
fn trash_directory(projects: &Path) -> PathBuf {
    projects.parent().unwrap_or(projects).join(TRASH_DIRECTORY)
}

/// Move one path into the trash, replacing any earlier deletion of the same name.
///
/// The trash is a bin, not an archive: the newest deletion of a given identifier is the one kept, so a
/// same-named remnant from a previous deletion is cleared first. The trash sits on the same volume as the
/// store, so the move is a rename rather than a copy.
fn move_out(source: &Path, trash: &Path) -> io::Result<()> {
    let Some(name) = source.file_name() else {
        return Ok(());
    };
    let destination = trash.join(name);
    if destination.is_dir() {
        fs::remove_dir_all(&destination)?;
    } else if destination.exists() {
        fs::remove_file(&destination)?;
    }
    fs::rename(source, &destination)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_trash_sits_beside_the_store_rather_than_inside_it() {
        // A stand-in root, not the CLI's own path: where the store lives is discovered from the environment,
        // and writing that path here would be the driver claiming to know it.
        let projects = Path::new("/somewhere/config/projects");
        let trash = trash_directory(projects);
        assert_eq!(trash, Path::new("/somewhere/config/runtrol-deleted"));
        assert!(
            !trash.starts_with(projects),
            "a trash inside projects would still be walked by the listing"
        );
    }

    #[test]
    fn a_root_with_no_parent_keeps_its_trash_under_itself() {
        // Not a store this driver can meet, but the fallback must still name a directory rather than panic.
        let trash = trash_directory(Path::new("/"));
        assert!(trash.ends_with(TRASH_DIRECTORY));
    }
}
