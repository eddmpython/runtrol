//! What each installed CLI was found to be, remembered between runs.
//!
//! Asking costs a process start, measured at roughly 300 ms on this machine before the program does anything
//! at all. Asking twice for the same binary is 300 ms the operator pays for nothing, so the answer is kept.
//!
//! # What makes an entry stale, and why a version is not enough
//!
//! Validity is decided by a `stat`: the resolved path, the size, and the modification time. A version string
//! alone would not do it. A package manager reinstalling the same version happens, and a self-updater
//! swapping a binary in place happens, and in both cases the version is unchanged while the program is not.
//! Comparing the file's own identity costs a sub-millisecond system call and catches both.
//!
//! # An unreadable cache is an absent cache
//!
//! Every failure to read this file, in every form (missing, truncated, written by a later runtrol, corrupted
//! by a power cut mid-write) produces the same answer: no cached entry. That is safe here and nowhere else,
//! because nothing is stored here that cannot be asked again. The cost of being wrong is one process start;
//! the cost of reporting it would be an error message about a file the operator did not create and cannot
//! fix. What is *not* silent is a failure to **write** it, which means the next start pays again.
//!
//! # Why the file is replaced and never edited
//!
//! Written to a temporary name, flushed, then renamed over the old one. A rename is atomic on every
//! supported platform, so a reader never sees half a file, and a power cut leaves either the old answer or
//! the new one. Editing in place would leave a third possibility.

use std::collections::BTreeMap;
use std::path::Path;

use runtrol_childproc::Program;
use runtrol_provider::{AbsPath, ProviderId, WallMs};
use serde::{Deserialize, Serialize};

use crate::probe::{Flags, ProbeError};

/// The cache format this build writes.
///
/// A file declaring anything else is not read. There is no migration, because there is nothing here worth
/// migrating: the answer can always be asked for again.
pub const CACHE_SCHEMA: u32 = 3;

/// One leading argument that helps decide what code a resolved program actually runs.
#[derive(Clone, PartialEq, Eq, Debug, Deserialize, Serialize)]
pub struct LeadingArgFacts {
    /// The argument exactly as it will be passed.
    pub value: String,
    /// The identity of the file it names, when it is an absolute path to a regular file.
    pub file: Option<LeadingFileFacts>,
}

/// The filesystem identity of a file named by a leading program argument.
#[derive(Clone, PartialEq, Eq, Debug, Deserialize, Serialize)]
pub struct LeadingFileFacts {
    /// The canonical file path.
    pub path: AbsPath,
    /// Its size in bytes.
    pub size: u64,
    /// Its modification time in milliseconds, when the platform reports one.
    pub modified_ms: Option<u64>,
}

/// The identity of a program file, as the filesystem reports it.
///
/// Compared, never interpreted. Two of these being equal is what makes a cached answer still true.
#[derive(Clone, PartialEq, Eq, Debug, Deserialize, Serialize)]
pub struct BinFacts {
    /// Where the program is, after launchers were unwrapped.
    pub path: AbsPath,
    /// Its size in bytes.
    pub size: u64,
    /// Its modification time in milliseconds, when the platform reports one.
    ///
    /// `None` on a filesystem that does not keep one. An entry without it is still usable: the size alone
    /// catches most changes, and the alternative is refusing to cache anything on that filesystem.
    pub modified_ms: Option<u64>,
    /// Arguments inserted by launcher resolution, including the identity of any file they name.
    ///
    /// An interpreted CLI runs the interpreter at [`Self::path`] but gets its actual implementation from a script in
    /// this list. Both the literal arguments and those files therefore belong to the cache identity.
    pub leading: Vec<LeadingArgFacts>,
}

impl BinFacts {
    /// Ask the filesystem about a program.
    ///
    /// # Errors
    ///
    /// [`ProbeError::Stat`] when the file cannot be examined, which means it was removed or replaced between
    /// resolving it and asking about it.
    pub fn of(path: &AbsPath) -> Result<Self, ProbeError> {
        let data = std::fs::metadata(path.as_std_path()).map_err(|error| ProbeError::Stat {
            path: path.clone(),
            detail: error.to_string(),
        })?;

        Ok(Self {
            path: path.clone(),
            size: data.len(),
            modified_ms: millis_since_epoch(&data),
            leading: Vec::new(),
        })
    }

    /// Ask the filesystem about every part of a resolved program that affects what it runs.
    ///
    /// The executable is always included. Every leading argument is retained literally, and an absolute argument
    /// naming a regular file also carries that file's identity. This covers interpreter-plus-script launchers without
    /// treating ordinary option values as paths.
    ///
    /// # Errors
    ///
    /// [`ProbeError::Stat`] when the executable itself cannot be examined.
    pub fn of_program(program: &Program) -> Result<Self, ProbeError> {
        Self::of_invocation(program.path(), program.leading())
    }

    /// Build the identity of an executable and the arguments launcher resolution put in front of probe arguments.
    fn of_invocation(path: &AbsPath, leading: &[String]) -> Result<Self, ProbeError> {
        let mut facts = Self::of(path)?;
        facts.leading = leading
            .iter()
            .map(|value| LeadingArgFacts {
                value: value.clone(),
                file: leading_file(value),
            })
            .collect();
        Ok(facts)
    }

    /// Whether this is the same file, unchanged.
    #[must_use]
    pub fn same_as(&self, other: &Self) -> bool {
        self == other
    }
}

/// Stat one absolute regular-file argument, or leave an ordinary argument as a literal only.
fn leading_file(value: &str) -> Option<LeadingFileFacts> {
    let path = Path::new(value);
    if !path.is_absolute() {
        return None;
    }
    let Ok(canonical) = AbsPath::canonicalize(value) else {
        return None;
    };
    let Ok(data) = std::fs::metadata(canonical.as_std_path()) else {
        return None;
    };
    if !data.is_file() {
        return None;
    }
    Some(LeadingFileFacts {
        path: canonical,
        size: data.len(),
        modified_ms: millis_since_epoch(&data),
    })
}

/// What one provider was found to be.
#[derive(Clone, PartialEq, Eq, Debug, Deserialize, Serialize)]
pub struct Entry {
    /// When the answer was taken.
    pub probed_at: WallMs,
    /// The file the answer is about.
    pub bin: BinFacts,
    /// The version it reported, as it reported it.
    pub version: String,
    /// The flags its own argument parser accepts.
    pub flags: Flags,
    /// The exact driver-owned flags that were offered to the parser.
    ///
    /// An empty list means the generic help surface was inspected. Keeping the question beside the answer prevents
    /// a changed driver contract from reusing an observation about a different set of flags.
    pub asked_flags: Vec<String>,
}

/// The cache file's contents.
#[derive(Clone, PartialEq, Eq, Debug, Deserialize, Serialize)]
struct File {
    /// The format this file was written in.
    schema: u32,
    /// One entry per provider.
    ///
    /// One, not a history. Only one binary is current for a provider at a time, and keeping the answer for a
    /// binary that is no longer installed would save a process start in the one case where somebody
    /// downgrades and upgrades again.
    entries: BTreeMap<String, Entry>,
}

/// Answers about installed CLIs, kept between runs.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ProbeCache {
    /// Where the file is.
    path: AbsPath,
    /// What was read, or nothing.
    entries: BTreeMap<String, Entry>,
    /// Which providers this instance answered fresh since it was read.
    ///
    /// Keys rather than a flag, because saving merges exactly these into whatever the file holds by
    /// then: several holders probing different providers concurrently each own only their keys, so
    /// none can erase what another learned.
    dirty: std::collections::BTreeSet<String>,
}

impl ProbeCache {
    /// Read the cache at `path`, or start empty when it cannot be read for any reason.
    #[must_use]
    pub fn open(path: &AbsPath) -> Self {
        let entries = read(path).unwrap_or_default();
        Self {
            path: path.clone(),
            entries,
            dirty: std::collections::BTreeSet::new(),
        }
    }

    /// The answer for a provider, if it is still about the file that is installed now.
    ///
    /// `now` is the current identity of the program. An entry about a different file is not returned, and is
    /// not deleted either: the write that replaces it is the one that happens anyway.
    #[must_use]
    pub fn get(&self, id: ProviderId, now: &BinFacts) -> Option<&Entry> {
        let entry = self.entries.get(id.as_str())?;
        if entry.bin.same_as(now) {
            Some(entry)
        } else {
            None
        }
    }

    /// Remember an answer, replacing whatever was there for that provider.
    pub fn put(&mut self, id: ProviderId, entry: Entry) {
        self.entries.insert(id.as_str().to_owned(), entry);
        self.dirty.insert(id.as_str().to_owned());
    }

    /// How many answers are held.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether nothing is held.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Whether there is anything worth writing.
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        !self.dirty.is_empty()
    }

    /// Write this instance's fresh answers, merged into whatever the file holds now, atomically.
    ///
    /// Does nothing when nothing changed, so a start that answered every question from the cache does not
    /// touch the disk.
    ///
    /// The merge is the concurrency story: a probe takes hundreds of milliseconds, so two holders preparing
    /// different providers overlap routinely. Writing this instance's whole snapshot would resurrect the
    /// state of the file as of its own `open` and erase what the other holder just learned. Re-reading and
    /// changing only this instance's own keys means the last writer can only be stale about its own
    /// providers, which it is not, because it just probed them. Saves themselves are expected to be brief
    /// and serialized by the caller when overlap is possible.
    ///
    /// # Errors
    ///
    /// [`ProbeError::CacheWrite`] when the file cannot be written or replaced. Reported rather than ignored:
    /// it means the next start pays for every probe again, and the operator's answer to "why is this slow
    /// every time" is in here.
    pub fn save(&mut self) -> Result<(), ProbeError> {
        if self.dirty.is_empty() {
            return Ok(());
        }

        let mut entries = read(&self.path).unwrap_or_default();
        for key in &self.dirty {
            if let Some(entry) = self.entries.get(key) {
                entries.insert(key.clone(), entry.clone());
            }
        }
        let file = File {
            schema: CACHE_SCHEMA,
            entries,
        };
        let encoded = serde_json::to_vec(&file).map_err(|error| ProbeError::CacheWrite {
            path: self.path.clone(),
            detail: error.to_string(),
        })?;

        let temporary = self.temporary_path()?;
        write_then_rename(&temporary, &self.path, &encoded)?;
        self.dirty.clear();
        Ok(())
    }

    /// Where the half-written file goes before it becomes the real one.
    ///
    /// Beside the real file, so the rename cannot cross a filesystem boundary. A rename across mount points
    /// is a copy, and a copy is not atomic.
    fn temporary_path(&self) -> Result<AbsPath, ProbeError> {
        let name = self.path.file_name().unwrap_or("probe.json");
        let parent = self.path.parent().ok_or_else(|| ProbeError::CacheWrite {
            path: self.path.clone(),
            detail: "the cache path has no parent directory".to_owned(),
        })?;
        parent
            .join(&format!("{name}.writing"))
            .map_err(|error| ProbeError::CacheWrite {
                path: self.path.clone(),
                detail: error.to_string(),
            })
    }
}

/// A file's modification time in milliseconds, or `None` when there is no usable one.
///
/// Three ways to have no answer, and the same thing to do about each: the platform keeps no modification
/// time, the time predates the epoch, or it is further from the epoch than milliseconds can count. All three
/// leave the file's size guarding the entry, and refusing to cache on any of them would cost a process start
/// on every list in exchange for nothing.
fn millis_since_epoch(data: &std::fs::Metadata) -> Option<u64> {
    let Ok(modified) = data.modified() else {
        return None;
    };
    let Ok(since) = modified.duration_since(std::time::UNIX_EPOCH) else {
        return None;
    };
    let Ok(millis) = u64::try_from(since.as_millis()) else {
        return None;
    };
    Some(millis)
}

/// Read and decode the file, or `None` for every reason a cache can be unusable.
fn read(path: &AbsPath) -> Option<BTreeMap<String, Entry>> {
    // Missing, unreadable, not valid JSON, or written in a format this build does not know. Each answers the
    // same question the same way, and every one of them is safe: the next probe asks again, and the only cost
    // is the process start the cache exists to save.
    let Ok(text) = std::fs::read_to_string(path.as_std_path()) else {
        return None;
    };
    let Ok(file) = serde_json::from_str::<File>(&text) else {
        return None;
    };
    if file.schema == CACHE_SCHEMA {
        Some(file.entries)
    } else {
        None
    }
}

/// Write bytes to `temporary`, flush them, then make them the contents of `final_path`.
fn write_then_rename(
    temporary: &AbsPath,
    final_path: &AbsPath,
    bytes: &[u8],
) -> Result<(), ProbeError> {
    use std::io::Write as _;

    let fail = |detail: String| ProbeError::CacheWrite {
        path: final_path.clone(),
        detail,
    };

    let mut file =
        std::fs::File::create(temporary.as_std_path()).map_err(|error| fail(error.to_string()))?;
    file.write_all(bytes)
        .map_err(|error| fail(error.to_string()))?;
    // Flushed before the rename, so the rename cannot publish a name that points at nothing yet.
    file.sync_all().map_err(|error| fail(error.to_string()))?;
    drop(file);

    std::fs::rename(temporary.as_std_path(), final_path.as_std_path())
        .map_err(|error| fail(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A directory of this test's own, removed when the test ends.
    struct Scratch {
        root: AbsPath,
    }

    impl Scratch {
        fn make(name: &str) -> Self {
            let base = std::env::temp_dir().join(format!("runtrol-probe-{name}"));
            if base.exists() {
                std::fs::remove_dir_all(&base).expect("clear the previous run");
            }
            std::fs::create_dir_all(&base).expect("create the scratch directory");
            Self {
                root: AbsPath::canonicalize(base.to_str().expect("the temporary path is UTF-8"))
                    .expect("canonicalize"),
            }
        }

        fn cache_path(&self) -> AbsPath {
            self.root.join("probe.json").expect("a valid file name")
        }

        /// A file with given contents, and its identity.
        fn program(&self, name: &str, contents: &str) -> (AbsPath, BinFacts) {
            let path = self.root.join(name).expect("a valid file name");
            std::fs::write(path.as_std_path(), contents).expect("write the fixture");
            let facts = BinFacts::of(&path).expect("a file that was just written can be examined");
            (path, facts)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            if let Err(error) = std::fs::remove_dir_all(self.root.as_std_path()) {
                eprintln!("could not clean up {}: {error}", self.root);
            }
        }
    }

    fn id(text: &str) -> ProviderId {
        ProviderId::parse(text).expect("the test's own id must be valid")
    }

    fn entry(bin: BinFacts, version: &str) -> Entry {
        Entry {
            probed_at: WallMs::now(),
            bin,
            version: version.to_owned(),
            flags: Flags::Observed(["--version".to_owned()].into_iter().collect()),
            asked_flags: Vec::new(),
        }
    }

    #[test]
    fn two_holders_probing_different_providers_cannot_erase_each_other() {
        // The concurrency shape of a cold start: several preparations each open the cache before any of
        // them saves. Saving a whole snapshot would make the last writer resurrect its stale view; saving
        // a keyed merge keeps both answers.
        let scratch = Scratch::make("merge");
        let (_, first_facts) = scratch.program("first.exe", "the first program");
        let (_, second_facts) = scratch.program("second.exe", "the second program");

        let mut first = ProbeCache::open(&scratch.cache_path());
        let mut second = ProbeCache::open(&scratch.cache_path());
        first.put(id("first"), entry(first_facts.clone(), "1.0.0"));
        second.put(id("second"), entry(second_facts.clone(), "2.0.0"));
        first.save().expect("the first holder saves");
        second.save().expect("the second holder saves");

        let merged = ProbeCache::open(&scratch.cache_path());
        assert!(
            merged.get(id("first"), &first_facts).is_some(),
            "the first holder's fresh answer must survive the second holder's save"
        );
        assert!(
            merged.get(id("second"), &second_facts).is_some(),
            "the second holder's own answer is there too"
        );
    }

    #[test]
    fn an_answer_survives_a_restart() {
        // The whole point: a process start the operator does not pay for twice.
        let scratch = Scratch::make("survives");
        let (_, facts) = scratch.program("thing.exe", "a program");

        let mut cache = ProbeCache::open(&scratch.cache_path());
        cache.put(id("thing"), entry(facts.clone(), "1.2.3"));
        cache.save().expect("the cache must be writable");

        let reopened = ProbeCache::open(&scratch.cache_path());
        let found = reopened
            .get(id("thing"), &facts)
            .expect("the answer must still be there");
        assert_eq!(found.version, "1.2.3");
    }

    #[test]
    fn a_binary_that_changed_without_changing_version_invalidates_the_answer() {
        // A package manager reinstalling the same version, and a self-updater swapping a file in place, both
        // leave the version string identical. Trusting it would serve an answer about a program that is gone.
        let scratch = Scratch::make("replaced");
        let (path, before) = scratch.program("thing.exe", "the old program");

        let mut cache = ProbeCache::open(&scratch.cache_path());
        cache.put(id("thing"), entry(before.clone(), "1.2.3"));

        std::fs::write(
            path.as_std_path(),
            "a different program of a different size",
        )
        .expect("replace the program");
        let after = BinFacts::of(&path).expect("still examinable");

        assert!(
            cache.get(id("thing"), &before).is_some(),
            "the old file's answer is about the old file"
        );
        assert!(
            cache.get(id("thing"), &after).is_none(),
            "and must not be served for the new one"
        );
    }

    #[test]
    fn an_interpreted_program_invalidates_when_its_script_changes() {
        let scratch = Scratch::make("interpreted-change");
        let (interpreter, _) = scratch.program("interpreter.exe", "stable interpreter");
        let (script, _) = scratch.program("entry.js", "first script");
        let leading = vec![script.to_string()];
        let before = BinFacts::of_invocation(&interpreter, &leading).expect("stat invocation");

        let mut cache = ProbeCache::open(&scratch.cache_path());
        cache.put(id("thing"), entry(before, "1.2.3"));
        std::fs::write(script.as_std_path(), "a longer second script").expect("replace script");
        let after =
            BinFacts::of_invocation(&interpreter, &leading).expect("stat changed invocation");

        assert!(
            cache.get(id("thing"), &after).is_none(),
            "an unchanged interpreter must not hide a changed program script"
        );
    }

    #[test]
    fn changed_launcher_arguments_are_part_of_program_identity() {
        let scratch = Scratch::make("leading-change");
        let (program, _) = scratch.program("thing.exe", "stable program");
        let before = BinFacts::of_invocation(&program, &["--mode=first".to_owned()])
            .expect("stat first invocation");
        let after = BinFacts::of_invocation(&program, &["--mode=second".to_owned()])
            .expect("stat second invocation");

        assert!(!before.same_as(&after));
    }

    #[test]
    fn a_missing_cache_is_simply_empty() {
        let scratch = Scratch::make("missing");
        let cache = ProbeCache::open(&scratch.cache_path());
        assert!(cache.is_empty());
        assert!(!cache.is_dirty());
    }

    #[test]
    fn every_way_a_cache_can_be_unusable_produces_an_empty_cache() {
        // All of these mean the same thing and cost the same thing: one process start. Reporting them would
        // be an error message about a file the operator never created.
        let scratch = Scratch::make("unusable");
        let path = scratch.cache_path();

        for contents in [
            "",
            "not json at all",
            r#"{"schema":1,"entries":{"x":{"version":"1"}}}"#,
            r#"{"schema":99,"entries":{}}"#,
            r#"{"entries":{}}"#,
        ] {
            std::fs::write(path.as_std_path(), contents).expect("write the fixture");
            let cache = ProbeCache::open(&path);
            assert!(cache.is_empty(), "accepted an unusable cache: {contents}");
        }
    }

    #[test]
    fn a_cache_written_by_a_later_runtrol_is_not_read() {
        // A later runtrol may mean something different by a field this build knows. Reading it anyway would
        // serve an answer that was never given. The fixture carries a real entry, so a build that stopped
        // checking the format would visibly read one.
        let scratch = Scratch::make("newer");
        let path = scratch.cache_path();
        let (_, facts) = scratch.program("thing.exe", "a program");

        let readable = File {
            schema: CACHE_SCHEMA,
            entries: [("thing".to_owned(), entry(facts, "1.0.0"))]
                .into_iter()
                .collect(),
        };
        let from_the_future = File {
            schema: CACHE_SCHEMA + 1,
            ..readable.clone()
        };

        std::fs::write(
            path.as_std_path(),
            serde_json::to_vec(&readable).expect("encodable"),
        )
        .expect("write the fixture");
        assert_eq!(
            ProbeCache::open(&path).len(),
            1,
            "this build's own format has to be readable, or the test proves nothing"
        );

        std::fs::write(
            path.as_std_path(),
            serde_json::to_vec(&from_the_future).expect("encodable"),
        )
        .expect("write the fixture");
        assert!(
            ProbeCache::open(&path).is_empty(),
            "a format this build does not know must not be read"
        );
    }

    #[test]
    fn a_write_that_fails_leaves_the_previous_answer_intact() {
        // What the temporary name buys, stated as a property rather than as a leftover check: the destination
        // is only ever replaced by a rename, so a write that cannot complete cannot damage what was there.
        // A directory standing where the temporary file goes is a write that cannot complete.
        let scratch = Scratch::make("failed");
        let path = scratch.cache_path();
        let (_, facts) = scratch.program("thing.exe", "a program");

        let mut cache = ProbeCache::open(&path);
        cache.put(id("thing"), entry(facts.clone(), "1.0.0"));
        cache.save().expect("the first write must succeed");
        let before = std::fs::read(path.as_std_path()).expect("readable");

        let blocked = cache
            .temporary_path()
            .expect("the temporary path is derivable");
        assert_ne!(
            blocked, path,
            "the temporary name must not be the destination, or nothing is atomic"
        );
        assert_eq!(
            blocked.parent(),
            path.parent(),
            "it must sit beside the destination, or the rename crosses a filesystem and becomes a copy"
        );
        std::fs::create_dir_all(blocked.as_std_path()).expect("block the temporary path");

        cache.put(id("thing"), entry(facts, "2.0.0"));
        assert!(
            cache.save().is_err(),
            "a write that cannot happen must be reported"
        );
        assert_eq!(
            std::fs::read(path.as_std_path()).expect("readable"),
            before,
            "the previous answer must still be there, byte for byte"
        );
    }

    #[test]
    fn nothing_is_written_when_nothing_changed() {
        // A start that answered every question from the cache must not touch the disk.
        let scratch = Scratch::make("clean");
        let path = scratch.cache_path();
        let mut cache = ProbeCache::open(&path);
        cache.save().expect("saving nothing must succeed");
        assert!(
            !path.as_std_path().exists(),
            "an unchanged cache must not create a file"
        );
    }

    #[test]
    fn the_half_written_file_never_becomes_the_real_one() {
        // A reader must never see part of a file, and a power cut must leave either the old answer or the new
        // one. The temporary name is what makes the third case impossible.
        let scratch = Scratch::make("atomic");
        let path = scratch.cache_path();
        let (_, facts) = scratch.program("thing.exe", "a program");

        let mut cache = ProbeCache::open(&path);
        cache.put(id("thing"), entry(facts, "1.0.0"));
        cache.save().expect("writable");

        let leftovers: Vec<String> = std::fs::read_dir(scratch.root.as_std_path())
            .expect("readable")
            .filter_map(|entry| match entry {
                Ok(entry) => Some(entry.file_name()),
                // A directory entry this test cannot read is not the thing it is asserting about.
                Err(_) => None,
            })
            .filter_map(|name| name.to_str().map(str::to_owned))
            .filter(|name| name.contains("writing"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "the temporary file must be gone: {leftovers:?}"
        );
        assert!(path.as_std_path().is_file());
    }

    #[test]
    fn saving_twice_writes_once() {
        let scratch = Scratch::make("twice");
        let (_, facts) = scratch.program("thing.exe", "a program");
        let mut cache = ProbeCache::open(&scratch.cache_path());
        cache.put(id("thing"), entry(facts, "1.0.0"));

        assert!(cache.is_dirty());
        cache.save().expect("writable");
        assert!(!cache.is_dirty(), "a saved cache has nothing left to write");
        cache
            .save()
            .expect("saving again must succeed and do nothing");
    }

    #[test]
    fn a_changed_answer_atomically_replaces_an_existing_cache_file() {
        let scratch = Scratch::make("replace-existing");
        let path = scratch.cache_path();
        let (_, facts) = scratch.program("thing.exe", "a program");
        let mut cache = ProbeCache::open(&path);
        cache.put(id("thing"), entry(facts.clone(), "1.0.0"));
        cache.save().expect("first answer is writable");

        cache.put(id("thing"), entry(facts.clone(), "2.0.0"));
        cache
            .save()
            .expect("an existing cache is atomically replaceable");

        let reopened = ProbeCache::open(&path);
        assert_eq!(
            reopened
                .get(id("thing"), &facts)
                .map(|found| found.version.as_str()),
            Some("2.0.0")
        );
    }

    #[test]
    fn a_program_that_vanished_between_resolving_and_asking_is_reported() {
        // Not tolerated: a probe about a file that is not there would produce an answer about nothing.
        let scratch = Scratch::make("vanished");
        let absent = scratch.root.join("never-existed.exe").expect("valid");
        match BinFacts::of(&absent) {
            Err(ProbeError::Stat { path, .. }) => assert_eq!(path, absent),
            other => panic!("expected a stat failure, got {other:?}"),
        }
    }

    #[test]
    fn one_provider_holds_one_answer() {
        let scratch = Scratch::make("single");
        let (path, first) = scratch.program("thing.exe", "one");
        let mut cache = ProbeCache::open(&scratch.cache_path());
        cache.put(id("thing"), entry(first, "1.0.0"));

        std::fs::write(path.as_std_path(), "a longer second program").expect("replace");
        let second = BinFacts::of(&path).expect("examinable");
        cache.put(id("thing"), entry(second.clone(), "2.0.0"));

        assert_eq!(cache.len(), 1, "a provider does not accumulate history");
        assert_eq!(
            cache
                .get(id("thing"), &second)
                .map(|one| one.version.as_str()),
            Some("2.0.0")
        );
    }
}
