//! Finding manifests, in an order where the operator wins.
//!
//! Three sources, read in this order, and a later one shadows an earlier one by provider id:
//!
//! 1. **Built in.** Compiled into the binary, so a fresh install has providers with no files anywhere.
//! 2. **Beside the executable.** How a packaged build ships an extra provider without touching the operator's
//!    directory.
//! 3. **The operator's own directory.** Last, so that whatever an operator writes wins.
//!
//! # Why the operator wins
//!
//! Shadowing is the whole reason the order is fixed. An operator whose CLI moved, or who needs a different
//! argument, writes a file with the same id and it replaces the built-in entirely. If the built-in won, the
//! only way out would be to edit runtrol.
//!
//! Shadowing is a replacement and never a merge. Merging would mean a manifest whose meaning depends on a
//! file the operator cannot see, and "why is this key not taking effect" is the failure mode that produces.
//!
//! # Why one bad file does not stop a start
//!
//! A directory of manifests is a directory of separate declarations. One that will not parse is reported and
//! set aside, and the providers that do parse still work. Refusing to start would mean a single mistyped key
//! in one file takes away every session the operator has, which is the opposite of what a supervisor is for.
//! Nothing is silent: every rejection is returned with the path and the reason.

use runtrol_provider::{AbsPath, Manifest, ProviderId};

use crate::registry::RegistryError;

/// The extension a manifest file has.
const EXTENSION: &str = "toml";

/// Where a manifest came from.
///
/// Kept with every loaded provider, because the first question about a provider behaving unexpectedly is
/// which file runtrol actually read.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Origin {
    /// Compiled into this binary.
    BuiltIn,
    /// A file on disk.
    File(AbsPath),
}

impl core::fmt::Display for Origin {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::BuiltIn => f.write_str("built in"),
            Self::File(path) => write!(f, "{path}"),
        }
    }
}

/// A manifest that was read, and where it came from.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Loaded {
    /// What it declares.
    pub manifest: Manifest,
    /// Where it was read from.
    pub origin: Origin,
}

/// A manifest that was found and could not be used.
///
/// Returned rather than logged. A caller that ignores these is visibly ignoring a value, and the operator's
/// answer to "why is my provider missing" is in here.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Rejected {
    /// Where the file was.
    pub origin: Origin,
    /// Why it could not be used.
    pub why: RegistryError,
}

/// Everything a scan found.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Scan {
    /// Manifests that parsed and validated, in the order they were read.
    pub loaded: Vec<Loaded>,
    /// Files that were found and refused.
    pub rejected: Vec<Rejected>,
}

impl Scan {
    /// Read a manifest that is compiled into the binary.
    ///
    /// A built-in that will not parse is a bug in this repository rather than a mistake by an operator, and
    /// it still travels as a rejection rather than a panic: one broken built-in must not take away the
    /// providers that work.
    pub(crate) fn take_built_in(&mut self, text: &str) {
        self.take(text, &Origin::BuiltIn);
    }

    /// Read every manifest in a directory, if the directory is there at all.
    ///
    /// A missing directory is not a failure. Neither of the two file locations is required to exist, and a
    /// fresh install has neither.
    pub(crate) fn take_directory(&mut self, directory: &AbsPath) {
        let entries = match std::fs::read_dir(directory.as_std_path()) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
            Err(error) => {
                // The directory is there and unreadable. That is worth saying: an operator who put a file
                // in it will otherwise wonder why nothing happened.
                self.rejected.push(Rejected {
                    origin: Origin::File(directory.clone()),
                    why: RegistryError::Read {
                        kind: error.kind(),
                        detail: error.to_string(),
                    },
                });
                return;
            }
        };

        // Read in name order. A directory listing has no guaranteed order, and two files declaring one id
        // must not shadow each other differently between runs.
        let mut paths: Vec<AbsPath> = Vec::new();
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    self.rejected.push(Rejected {
                        origin: Origin::File(directory.clone()),
                        why: RegistryError::Read {
                            kind: error.kind(),
                            detail: error.to_string(),
                        },
                    });
                    continue;
                }
            };
            let Some(name) = manifest_name(&entry) else {
                continue;
            };
            match directory.join(&name) {
                Ok(path) => paths.push(path),
                // A name the operator's filesystem holds and runtrol cannot form a path from. Reported
                // rather than skipped: a file that is silently not read is a file the operator believes is
                // in effect.
                Err(source) => self.rejected.push(Rejected {
                    origin: Origin::File(directory.clone()),
                    why: RegistryError::Name { name, source },
                }),
            }
        }
        paths.sort();

        for path in paths {
            match std::fs::read_to_string(path.as_std_path()) {
                Ok(text) => self.take(&text, &Origin::File(path)),
                Err(error) => self.rejected.push(Rejected {
                    origin: Origin::File(path),
                    why: RegistryError::Read {
                        kind: error.kind(),
                        detail: error.to_string(),
                    },
                }),
            }
        }
    }

    /// Parse and validate one manifest's text.
    fn take(&mut self, text: &str, origin: &Origin) {
        let manifest: Manifest = match toml::from_str(text) {
            Ok(manifest) => manifest,
            Err(error) => {
                self.rejected.push(Rejected {
                    origin: origin.clone(),
                    // The format's own message carries the line, which is the only part an operator
                    // editing a file actually needs.
                    why: RegistryError::Syntax {
                        detail: error.to_string(),
                    },
                });
                return;
            }
        };

        if !matches!(origin, Origin::BuiltIn) && manifest.update.is_some() {
            self.rejected.push(Rejected {
                origin: origin.clone(),
                why: RegistryError::UpdateAuthority,
            });
            return;
        }

        if let Err(error) = manifest.validate() {
            self.rejected.push(Rejected {
                origin: origin.clone(),
                why: RegistryError::Invalid { source: error },
            });
            return;
        }

        self.loaded.push(Loaded {
            manifest,
            origin: origin.clone(),
        });
    }

    /// Reduce to one manifest per id, with the last one read winning.
    ///
    /// Returns the survivors in the order they were read, and every shadowing that happened.
    pub(crate) fn resolve(self) -> (Vec<Loaded>, Vec<Shadowed>, Vec<Rejected>) {
        let mut kept: Vec<Loaded> = Vec::with_capacity(self.loaded.len());
        let mut shadowed: Vec<Shadowed> = Vec::new();

        for candidate in self.loaded {
            match kept
                .iter()
                .position(|held| held.manifest.id == candidate.manifest.id)
            {
                Some(index) => match kept.get_mut(index) {
                    Some(slot) => {
                        let replaced = core::mem::replace(slot, candidate);
                        // Report what lost, and to what. An operator who shadowed a provider by accident,
                        // by reusing an id, finds out from this.
                        let winner = slot.origin.clone();
                        shadowed.push(Shadowed {
                            id: replaced.manifest.id,
                            hidden: replaced.origin,
                            winner,
                        });
                    }
                    // `position` returned this index, so the slot is there. Keeping the candidate rather
                    // than dropping it means an impossible branch still loses no provider.
                    None => kept.push(candidate),
                },
                None => kept.push(candidate),
            }
        }

        (kept, shadowed, self.rejected)
    }
}

/// One manifest replaced another with the same id.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Shadowed {
    /// The id both declared.
    pub id: ProviderId,
    /// Where the manifest that lost came from.
    pub hidden: Origin,
    /// Where the manifest that won came from.
    pub winner: Origin,
}

/// The name of a directory entry that is a manifest, or `None` for anything else.
///
/// A directory may hold notes, backups, and an editor's swap files. Only `*.toml` is a manifest, and a
/// subdirectory is not descended into: nesting would make the shadowing order depend on a tree shape
/// nobody declared.
fn manifest_name(entry: &std::fs::DirEntry) -> Option<String> {
    let name = entry.file_name();
    let name = name.to_str()?;
    let (stem, extension) = name.rsplit_once('.')?;
    if stem.is_empty() || !extension.eq_ignore_ascii_case(EXTENSION) {
        return None;
    }
    match entry.file_type() {
        Ok(kind) if kind.is_dir() => None,
        // A type that cannot be read is treated as a file and will fail at the read, with a message that
        // names the path. Guessing it away here would hide it.
        Ok(_) | Err(_) => Some(name.to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A manifest for a given id, as text.
    fn text(id: &str, display: &str) -> String {
        format!(
            "schema = 1\nid = \"{id}\"\ndisplay_name = \"{display}\"\nkind = \"example\"\n[bin]\nnames = [\"{id}\"]\n"
        )
    }

    /// A directory of this test's own, removed when the test ends.
    struct Scratch {
        root: AbsPath,
    }

    impl Scratch {
        fn make(name: &str) -> Self {
            let base = std::env::temp_dir().join(format!("runtrol-load-{name}"));
            if base.exists() {
                std::fs::remove_dir_all(&base).expect("clear the previous run");
            }
            std::fs::create_dir_all(&base).expect("create the scratch directory");
            Self {
                root: AbsPath::canonicalize(base.to_str().expect("the temporary path is UTF-8"))
                    .expect("canonicalize"),
            }
        }

        fn write(&self, name: &str, body: &str) -> AbsPath {
            let path = self.root.join(name).expect("a valid file name");
            std::fs::write(path.as_std_path(), body).expect("write the fixture");
            path
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            if let Err(error) = std::fs::remove_dir_all(self.root.as_std_path()) {
                eprintln!("could not clean up {}: {error}", self.root);
            }
        }
    }

    #[test]
    fn a_built_in_needs_no_file_anywhere() {
        // A fresh install has providers. If this needed a file on disk, the first run would have none.
        let mut scan = Scan::default();
        scan.take_built_in(&text("codex", "Codex"));
        assert_eq!(scan.loaded.len(), 1);
        assert_eq!(
            scan.loaded.first().map(|one| &one.origin),
            Some(&Origin::BuiltIn)
        );
        assert!(scan.rejected.is_empty());
    }

    #[test]
    fn a_missing_directory_is_not_a_failure() {
        // Neither file location is required to exist, and a fresh install has neither.
        let scratch = Scratch::make("absent");
        let absent = scratch.root.join("not-there").expect("valid");
        let mut scan = Scan::default();
        scan.take_directory(&absent);
        assert_eq!(scan, Scan::default());
    }

    #[test]
    fn the_operators_file_replaces_a_built_in_with_the_same_id() {
        // The whole reason the order is fixed. An operator whose CLI moved must be able to fix it without
        // editing runtrol.
        let scratch = Scratch::make("shadow");
        let path = scratch.write("codex.toml", &text("codex", "My Own Codex"));

        let mut scan = Scan::default();
        scan.take_built_in(&text("codex", "Codex"));
        scan.take_directory(&scratch.root);

        let (kept, shadowed, rejected) = scan.resolve();
        assert!(rejected.is_empty());
        assert_eq!(kept.len(), 1, "one id is one provider");
        let winner = kept.first().expect("one survivor");
        assert_eq!(&*winner.manifest.display_name, "My Own Codex");
        assert_eq!(winner.origin, Origin::File(path.clone()));

        let event = shadowed.first().expect("the shadowing must be reported");
        assert_eq!(event.id.as_str(), "codex");
        assert_eq!(event.hidden, Origin::BuiltIn);
        assert_eq!(event.winner, Origin::File(path));
    }

    #[test]
    fn shadowing_replaces_and_never_merges() {
        // A merge would make a manifest's meaning depend on a file the operator cannot see, and produce the
        // unanswerable "why is this key not taking effect".
        let scratch = Scratch::make("replace");
        scratch.write(
            "thing.toml",
            "schema = 1\nid = \"thing\"\ndisplay_name = \"Mine\"\nkind = \"example\"\n[bin]\nnames = [\"mine\"]\n",
        );

        let mut scan = Scan::default();
        scan.take_built_in(
            "schema = 1\nid = \"thing\"\ndisplay_name = \"Theirs\"\nkind = \"example\"\n[bin]\nnames = [\"theirs\"]\n[models]\naliases = [\"opus\"]\n",
        );
        scan.take_directory(&scratch.root);

        let (kept, _, _) = scan.resolve();
        let winner = kept.first().expect("one survivor");
        assert_eq!(winner.manifest.bin.names, vec!["mine".into()]);
        assert!(
            winner.manifest.models.aliases.is_empty(),
            "nothing from the shadowed manifest may leak through"
        );
    }

    #[test]
    fn one_unreadable_file_does_not_take_away_the_others() {
        // A mistyped key in one file must not cost the operator every session they have.
        let scratch = Scratch::make("partial");
        scratch.write("good.toml", &text("good", "Good"));
        scratch.write(
            "broken.toml",
            "schema = 1\nid = \"broken\"\nthis is not toml",
        );
        scratch.write(
            "unknown.toml",
            &format!("{}\nmystery = true\n", text("unknown", "Unknown")),
        );

        let mut scan = Scan::default();
        scan.take_directory(&scratch.root);
        let (kept, _, rejected) = scan.resolve();

        assert_eq!(kept.len(), 1, "the good file still works");
        assert_eq!(
            kept.first().map(|one| one.manifest.id.as_str()),
            Some("good")
        );
        assert_eq!(rejected.len(), 2, "and both bad ones are reported");
        for refusal in &rejected {
            assert!(
                matches!(refusal.why, RegistryError::Syntax { .. }),
                "{refusal:?}"
            );
        }
    }

    #[test]
    fn a_rejection_names_the_file_and_the_line() {
        // The operator is editing a file. Anything less than a path and a line makes them search.
        let scratch = Scratch::make("named");
        let path = scratch.write("bad.toml", &text("Bad-Id", "Bad"));

        let mut scan = Scan::default();
        scan.take_directory(&scratch.root);

        let refusal = scan.rejected.first().expect("the file must be refused");
        assert_eq!(refusal.origin, Origin::File(path));
        let message = refusal.why.to_string();
        assert!(message.contains("provider id"), "{message}");
    }

    #[test]
    fn a_manifest_that_is_valid_toml_and_invalid_as_a_manifest_is_told_apart() {
        // Two different mistakes with two different fixes: a syntax error is in the file's shape, and an
        // invalid manifest is in what it says.
        let scratch = Scratch::make("invalid");
        scratch.write(
            "future.toml",
            &text("future", "Future").replace("schema = 1", "schema = 9"),
        );

        let mut scan = Scan::default();
        scan.take_directory(&scratch.root);
        let refusal = scan.rejected.first().expect("refused");
        assert!(
            matches!(refusal.why, RegistryError::Invalid { .. }),
            "{refusal:?}"
        );
    }

    #[test]
    fn an_on_disk_manifest_cannot_claim_update_authority() {
        let scratch = Scratch::make("update-authority");
        let body = format!("{}\n[update]\nhint = \"npm\"\n", text("thing", "Thing"));
        let path = scratch.write("thing.toml", &body);

        let mut scan = Scan::default();
        scan.take_directory(&scratch.root);

        assert!(scan.loaded.is_empty());
        let refusal = scan.rejected.first().expect("the file must be refused");
        assert_eq!(refusal.origin, Origin::File(path));
        assert!(matches!(refusal.why, RegistryError::UpdateAuthority));
    }

    #[test]
    fn a_compiled_manifest_may_declare_an_update_hint() {
        let body = format!("{}\n[update]\nhint = \"self\"\n", text("thing", "Thing"));
        let mut scan = Scan::default();
        scan.take_built_in(&body);

        assert_eq!(scan.loaded.len(), 1);
        assert!(scan.rejected.is_empty());
    }

    #[test]
    fn only_toml_files_are_manifests() {
        // A directory holds notes, backups, and an editor's leftovers. Reading those would turn a stray
        // file into a refusal the operator cannot explain.
        let scratch = Scratch::make("extensions");
        scratch.write("real.toml", &text("real", "Real"));
        scratch.write("notes.md", "just notes");
        scratch.write("real.toml.bak", &text("backup", "Backup"));
        scratch.write(".hidden", "not a manifest");

        let mut scan = Scan::default();
        scan.take_directory(&scratch.root);

        assert_eq!(scan.loaded.len(), 1);
        assert!(scan.rejected.is_empty(), "{:?}", scan.rejected);
    }

    #[test]
    fn a_subdirectory_is_not_descended_into() {
        // Nesting would make the shadowing order depend on a tree shape nobody declared.
        let scratch = Scratch::make("nested");
        let nested = scratch.root.join("more.toml").expect("valid");
        std::fs::create_dir_all(nested.as_std_path())
            .expect("create a directory that looks like a file");

        let mut scan = Scan::default();
        scan.take_directory(&scratch.root);
        assert!(scan.loaded.is_empty());
        assert!(scan.rejected.is_empty(), "{:?}", scan.rejected);
    }

    #[test]
    fn two_files_in_one_directory_shadow_in_name_order() {
        // A directory listing has no guaranteed order. Without sorting, which of two files declaring one id
        // wins would change between runs on the same machine.
        let scratch = Scratch::make("order");
        scratch.write("a-first.toml", &text("same", "First"));
        scratch.write("z-last.toml", &text("same", "Last"));

        let mut scan = Scan::default();
        scan.take_directory(&scratch.root);
        let (kept, shadowed, _) = scan.resolve();

        assert_eq!(kept.len(), 1);
        assert_eq!(
            kept.first().map(|one| &*one.manifest.display_name),
            Some("Last"),
            "the later name wins, and it wins the same way every time"
        );
        assert_eq!(shadowed.len(), 1);
    }

    #[test]
    fn an_origin_says_where_to_look() {
        assert_eq!(Origin::BuiltIn.to_string(), "built in");
        let path = AbsPath::new(if cfg!(windows) {
            r"C:\state\runtrol\providers\codex.toml"
        } else {
            "/state/runtrol/providers/codex.toml"
        })
        .expect("valid");
        assert!(
            Origin::File(path.clone())
                .to_string()
                .contains("codex.toml")
        );
    }
}
