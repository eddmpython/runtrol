//! The one place the database file is opened.
//!
//! One call site, so the cache size and the version check cannot be applied in one place and forgotten in
//! another. This is also why the command surface asks the daemon rather than reading the file: the engine
//! takes an exclusive lock, measured on all three platforms, so a second opener is not something to work
//! around and [`StoreError::AlreadyOpen`] is a first-class outcome rather than a surprise.

use redb::{Database, DatabaseError, Durability, ReadableDatabase as _};
use runtrol_provider::AbsPath;

use crate::error::StoreError;
use crate::schema::{META, META_SCHEMA_VERSION, META_WRITTEN_BY, SCHEMA_VERSION};

/// How much of the database the engine may keep in memory.
///
/// The engine's own default is one gibibyte. That is a reasonable default for a database server and an absurd
/// one for a supervisor with a strict tens-of-mebibytes process budget, so it is set here, at the only
/// open call site, and pinned by a gate.
pub const CACHE_BYTES: usize = 1024 * 1024;

/// The runtrol version recorded in a fresh file.
///
/// Written so that a future schema refusal can name which build to look for, rather than leaving the operator
/// to guess which runtrol they were running when the file was made.
const WRITTEN_BY: &str = env!("CARGO_PKG_VERSION");

/// An open runtrol database.
///
/// Holding it is holding the exclusive lock, so exactly one process has one at a time.
#[derive(Debug)]
pub struct Store {
    /// The engine's handle.
    db: Database,
    /// Where the file is, kept for error messages that have to name it.
    path: AbsPath,
}

impl Store {
    /// Open or create the database at `path`, and check its version.
    ///
    /// # Errors
    ///
    /// [`StoreError::AlreadyOpen`] when another runtrol has it, [`StoreError::Open`] when the file cannot be
    /// opened, [`StoreError::SchemaTooNew`] or [`StoreError::SchemaTooOld`] when the file's format does not
    /// match this build.
    pub fn open(path: &AbsPath) -> Result<Self, StoreError> {
        let mut builder = Database::builder();
        builder.set_cache_size(CACHE_BYTES);

        let db = builder
            .create(path.as_std_path())
            .map_err(|error| match error {
                // Not an unexpected error: it is how a second daemon discovers the first one.
                DatabaseError::DatabaseAlreadyOpen => {
                    StoreError::AlreadyOpen { path: path.clone() }
                }
                other => StoreError::Open {
                    path: path.clone(),
                    source: Box::new(other),
                },
            })?;

        let store = Self {
            db,
            path: path.clone(),
        };
        store.check_version()?;
        Ok(store)
    }

    /// The engine handle, for the tables in this crate.
    pub(crate) const fn db(&self) -> &Database {
        &self.db
    }

    /// Where the file is.
    #[must_use]
    pub const fn path(&self) -> &AbsPath {
        &self.path
    }

    /// Read the file's schema version, writing it on a fresh file and refusing anything else.
    ///
    /// All four outcomes are live and tested. There is no migration list, because there is no earlier version
    /// to migrate from, and an empty one would be exactly the deferred wiring this repository refuses to carry:
    /// the mechanism is written down in the module documentation of [`crate::schema`] instead, so that it is
    /// not invented under pressure the day it is needed.
    fn check_version(&self) -> Result<(), StoreError> {
        let found = self.read_version()?;
        match found {
            None => self.write_version(),
            Some(version) if version == SCHEMA_VERSION => Ok(()),
            Some(version) if version > SCHEMA_VERSION => Err(StoreError::SchemaTooNew {
                path: self.path.clone(),
                found: version,
                understood: SCHEMA_VERSION,
            }),
            Some(version) => Err(StoreError::SchemaTooOld {
                path: self.path.clone(),
                found: version,
                understood: SCHEMA_VERSION,
                backup: format!("{}.pre-{version}.bak", self.path),
            }),
        }
    }

    /// The version byte in the file, or `None` when the file is new.
    fn read_version(&self) -> Result<Option<u8>, StoreError> {
        let read = self.db.begin_read().map_err(|error| StoreError::Engine {
            doing: "starting a read to check the schema version",
            source: Box::new(error.into()),
        })?;

        let table = match read.open_table(META) {
            Ok(table) => table,
            // A fresh file has no tables at all. That is the one shape of this error that means "new", and it
            // is matched narrowly so a genuinely broken file is still reported.
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(None),
            Err(error) => {
                return Err(StoreError::Engine {
                    doing: "opening the metadata table",
                    source: Box::new(error.into()),
                });
            }
        };

        let stored = table
            .get(META_SCHEMA_VERSION)
            .map_err(|error| StoreError::Engine {
                doing: "reading the schema version",
                source: Box::new(error.into()),
            })?;

        match stored {
            None => Ok(None),
            Some(value) => value
                .value()
                .first()
                .copied()
                .map(Some)
                .ok_or(StoreError::Codec {
                    field: "schema version",
                    why: "the stored version is empty",
                }),
        }
    }

    /// Write this build's version and name into a fresh file.
    fn write_version(&self) -> Result<(), StoreError> {
        let mut write = self.db.begin_write().map_err(|error| StoreError::Engine {
            doing: "starting a write to record the schema version",
            source: Box::new(error.into()),
        })?;
        // The version byte is the one value that must survive a power cut. Everything downstream reads it to
        // decide whether the file can be trusted at all.
        write
            .set_durability(Durability::Immediate)
            .map_err(|error| StoreError::Engine {
                doing: "requesting durability for the schema version",
                source: Box::new(error.into()),
            })?;

        {
            let mut table = write.open_table(META).map_err(|error| StoreError::Engine {
                doing: "creating the metadata table",
                source: Box::new(error.into()),
            })?;
            table
                .insert(META_SCHEMA_VERSION, [SCHEMA_VERSION].as_slice())
                .map_err(|error| StoreError::Engine {
                    doing: "writing the schema version",
                    source: Box::new(error.into()),
                })?;
            table
                .insert(META_WRITTEN_BY, WRITTEN_BY.as_bytes())
                .map_err(|error| StoreError::Engine {
                    doing: "recording which runtrol wrote the file",
                    source: Box::new(error.into()),
                })?;
        }

        write.commit().map_err(|error| StoreError::Engine {
            doing: "committing the schema version",
            source: Box::new(error.into()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch directory that cleans up after itself.
    struct Scratch {
        root: AbsPath,
    }

    impl Scratch {
        fn make(name: &str) -> Self {
            let base = std::env::temp_dir().join(format!("runtrol-store-{name}"));
            if base.exists() {
                std::fs::remove_dir_all(&base).expect("clear the previous run");
            }
            std::fs::create_dir_all(&base).expect("create the scratch directory");
            Self {
                root: AbsPath::canonicalize(base.to_str().expect("temp dir is UTF-8"))
                    .expect("canonicalize"),
            }
        }

        fn db_path(&self) -> AbsPath {
            self.root.join("runtrol.redb").expect("valid file name")
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            if let Err(error) = std::fs::remove_dir_all(self.root.as_std_path()) {
                eprintln!("could not clean up {}: {error}", self.root);
            }
        }
    }

    /// Overwrite the stored schema version, to stand in for a file another build wrote.
    fn force_version(path: &AbsPath, version: u8) {
        let db = Database::create(path.as_std_path()).expect("reopen");
        let mut write = db.begin_write().expect("write");
        write
            .set_durability(Durability::Immediate)
            .expect("durability");
        {
            let mut table = write.open_table(META).expect("meta table");
            table
                .insert(META_SCHEMA_VERSION, [version].as_slice())
                .expect("insert");
        }
        write.commit().expect("commit");
    }

    #[test]
    fn a_fresh_file_gets_this_builds_version() {
        let scratch = Scratch::make("fresh");
        let path = scratch.db_path();

        let store = Store::open(&path).expect("a fresh database must open");
        assert_eq!(
            store.read_version().expect("readable"),
            Some(SCHEMA_VERSION)
        );
        assert_eq!(store.path(), &path);
    }

    #[test]
    fn reopening_an_existing_file_is_accepted() {
        let scratch = Scratch::make("reopen");
        let path = scratch.db_path();

        let first = Store::open(&path).expect("first open");
        drop(first);
        let second = Store::open(&path).expect("reopening the same file must work");
        assert_eq!(
            second.read_version().expect("readable"),
            Some(SCHEMA_VERSION)
        );
    }

    #[test]
    fn a_second_opener_is_told_the_first_one_exists() {
        // The exclusive lock is the measured reason the command surface asks the daemon instead of reading the
        // file, so this outcome has to be a named one rather than a generic failure.
        let scratch = Scratch::make("locked");
        let path = scratch.db_path();

        let held = Store::open(&path).expect("first open");
        match Store::open(&path) {
            Err(StoreError::AlreadyOpen { path: reported }) => {
                assert_eq!(reported, path, "the message has to name the file");
            }
            Err(other) => panic!("expected an already-open refusal, got {other:?}"),
            Ok(_) => panic!("two processes must not hold the database at once"),
        }
        drop(held);
    }

    #[test]
    fn a_file_from_a_newer_build_is_refused_and_never_migrated_down() {
        // A newer build may have written fields this one does not know about. Reading it anyway would drop
        // them silently, which corrupts the operator's session list without any error.
        let scratch = Scratch::make("toonew");
        let path = scratch.db_path();
        drop(Store::open(&path).expect("create"));
        force_version(&path, SCHEMA_VERSION + 1);

        match Store::open(&path) {
            Err(StoreError::SchemaTooNew {
                found, understood, ..
            }) => {
                assert_eq!(found, SCHEMA_VERSION + 1);
                assert_eq!(understood, SCHEMA_VERSION);
            }
            other => panic!("expected a too-new refusal, got {other:?}"),
        }
    }

    #[test]
    fn a_file_from_an_older_build_is_refused_and_names_the_backup() {
        // The operator's next question is where their sessions went, so the answer is in the error rather than
        // in a document they would have to find.
        let scratch = Scratch::make("tooold");
        let path = scratch.db_path();
        drop(Store::open(&path).expect("create"));
        force_version(&path, SCHEMA_VERSION - 1);

        match Store::open(&path) {
            Err(StoreError::SchemaTooOld { backup, found, .. }) => {
                assert_eq!(found, SCHEMA_VERSION - 1);
                assert!(backup.contains("pre-0"), "{backup}");
                assert!(
                    std::path::Path::new(&backup)
                        .extension()
                        .is_some_and(|extension| extension == "bak"),
                    "{backup}"
                );
            }
            other => panic!("expected a too-old refusal, got {other:?}"),
        }
    }

    #[test]
    fn every_schema_refusal_needs_the_operator_and_a_bad_row_does_not() {
        let scratch = Scratch::make("classify");
        let path = scratch.db_path();

        assert!(StoreError::AlreadyOpen { path: path.clone() }.needs_the_operator());
        assert!(
            StoreError::SchemaTooNew {
                path: path.clone(),
                found: 9,
                understood: SCHEMA_VERSION,
            }
            .needs_the_operator()
        );
        assert!(
            !StoreError::Codec {
                field: "label",
                why: "not valid UTF-8",
            }
            .needs_the_operator(),
            "one unreadable row leaves the rest of the list readable"
        );
        assert!(
            StoreError::DeviceCodec {
                field: "scope",
                why: "not valid UTF-8",
            }
            .needs_the_operator(),
            "damaged authority must stop remote startup"
        );
    }

    #[test]
    fn the_cache_is_a_supervisor_sized_budget_and_not_the_engine_default() {
        // The engine defaults to a gibibyte. A bounded supervisor cannot inherit that, and this is the only place
        // the smaller cache is set.
        assert_eq!(CACHE_BYTES, 1024 * 1024);
    }
}
