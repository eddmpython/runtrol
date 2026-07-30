//! Finding out what an installed CLI actually is, instead of assuming.
//!
//! The failure this exists to prevent has a name and a corpse. A project in this space shut down and said
//! why: "we built a wrapper around a CLI, and the CLI kept changing until it became unmaintainable." Every
//! fact a wrapper hardcodes is a fact that expires without telling anybody. So runtrol asks.
//!
//! # The ladder
//!
//! 1. **Resolve.** Try the manifest's candidate names in order, on the operator's own search path, and unwrap
//!    the launchers a package manager puts in front of the real program.
//! 2. **Identify the file.** A `stat`, which is what makes a remembered answer verifiable later.
//! 3. **Ask its version.** The one question every CLI answers, and the key everything else hangs off.
//! 4. **Ask which flags it has.** Its own argument parser is asked, and only flag *names* are taken.
//!
//! Rung four never reads help text for meaning. Flag names are stable across releases and help prose is not,
//! so a scan for names degrades gracefully where a scan for sentences breaks on a wording change. What a flag
//! *does* is never inferred from anything.
//!
//! # This is the fallback rung, not the good one
//!
//! Both supported CLIs declare their capabilities on their own protocol, at the start of a session, without
//! being asked. That is strictly better: no parsing, no guessing, and the answer comes from the program
//! rather than from its documentation. It needs a live session, so it arrives with the drivers. Until then a
//! flag set is what there is, and the type says which one produced it.
//!
//! # Never at startup
//!
//! Probing spawns a process. Measured on this machine, a cold start of one of these costs roughly 300 ms
//! before it does anything, so probing every provider at boot would put a second of nothing in front of the
//! operator's first list. Probing is lazy, its answer is cached against the binary's own identity, and the
//! list is built from the provider's own files rather than by asking a CLI anything.
//!
//! # What is deliberately not here
//!
//! A file watch on the vendors' own state directories. The design calls for one, and it would let runtrol
//! notice a new model the moment the vendor's own cache learned about it. It is absent because it needs a
//! dependency that runs a background thread, the idle footprint is a fixed contract, and correctness does not
//! depend on it: the cheapest rung of invalidation, a `stat` on every use, is complete on its own. What the
//! watch would buy is freshness between one probe and the next, and it arrives when there is a number saying
//! the thread fits.

pub mod cache;

use core::time::Duration;

use runtrol_childproc::{Containment, Program, SpawnError, capture, resolve};
use runtrol_provider::{AbsPath, Manifest, VersionParse, WallMs};
use serde::{Deserialize, Serialize};

pub use cache::{BinFacts, CACHE_SCHEMA, Entry, ProbeCache};

/// How long one question may take.
///
/// Generous on purpose. Measured on this machine: a cold start of one of these CLIs costs 300 to 900 ms
/// before it prints anything, and one unrelated query took 39.9 seconds. A tight bound here would report a
/// working CLI as broken on a cold cache or a busy machine, and the cost of being generous is only paid when
/// something really is wrong.
pub const QUESTION_DEADLINE: Duration = Duration::from_secs(15);

/// A CLI could not be asked what it is.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ProbeError {
    /// None of the manifest's candidate names resolved to anything.
    ///
    /// Names every candidate, because the operator's next move is to install the CLI or to correct the
    /// manifest, and which one depends on what was looked for.
    #[error("none of {candidates} is installed")]
    NotInstalled {
        /// The names that were tried, in order.
        candidates: String,
    },

    /// The program file could not be examined.
    #[error("cannot examine {path}: {detail}")]
    Stat {
        /// The program.
        path: AbsPath,
        /// What the OS said.
        detail: String,
    },

    /// Running the program failed.
    #[error(transparent)]
    Run(#[from] SpawnError),

    /// The program ran and said nothing that looks like a version.
    ///
    /// Refused rather than filled in. A version is the key a remembered answer hangs off, and inventing one
    /// would mean caching an answer that can never be invalidated correctly.
    #[error("{path} did not report a version. it said: {said:?}")]
    NoVersion {
        /// The program that was asked.
        path: AbsPath,
        /// What it said instead, cut short for a message.
        said: String,
    },

    /// The cache file could not be written.
    ///
    /// Not fatal and not ignored: it means the next start pays for every probe again.
    #[error("cannot write the probe cache at {path}: {detail}")]
    CacheWrite {
        /// Where the cache lives.
        path: AbsPath,
        /// What went wrong.
        detail: String,
    },
}

impl ProbeError {
    /// Whether the operator has to do something at their own machine.
    #[must_use]
    pub const fn needs_the_operator(&self) -> bool {
        matches!(self, Self::NotInstalled { .. })
    }
}

/// The flags a program's own argument parser accepts.
///
/// An enumeration rather than a set, so that "this build does not have that flag" cannot be confused with
/// "nobody managed to ask". The two lead to opposite decisions: the first degrades a feature deliberately and
/// says so, the second is a question still open.
#[derive(Clone, PartialEq, Eq, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Flags {
    /// The program was asked and these are the flag names it printed.
    Observed(std::collections::BTreeSet<String>),
    /// The program could not be asked.
    Unknown {
        /// Why not, for an operator wondering why a feature is unavailable.
        why: String,
    },
}

impl Flags {
    /// Whether the program accepts this flag.
    ///
    /// `false` when nothing was observed, which is the safe direction: a feature that depends on a flag runtrol
    /// is not sure about degrades instead of failing at the moment it is used. Callers that need to tell the
    /// two apart ask [`Flags::is_observed`].
    #[must_use]
    pub fn has(&self, flag: &str) -> bool {
        match self {
            Self::Observed(flags) => flags.contains(flag),
            Self::Unknown { .. } => false,
        }
    }

    /// Whether the program was actually asked.
    #[must_use]
    pub const fn is_observed(&self) -> bool {
        matches!(self, Self::Observed(_))
    }

    /// How many flags were seen. Zero when nothing was observed.
    #[must_use]
    pub fn len(&self) -> usize {
        match self {
            Self::Observed(flags) => flags.len(),
            Self::Unknown { .. } => 0,
        }
    }

    /// Whether no flag is known to be present.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Ask a CLI what it is, or read the remembered answer.
///
/// The cache is consulted against the installed file's own identity, so a remembered answer is only used when
/// it is about the program that is there now. A fresh answer is stored, and storing it is the caller's cue to
/// save the cache once rather than after every provider.
///
/// # Errors
///
/// Any [`ProbeError`]: the CLI is not installed, its file cannot be examined, it could not be run, or it did
/// not report a version.
pub async fn probe(
    manifest: &Manifest,
    cache: &mut ProbeCache,
    contained_by: &Containment,
) -> Result<Entry, ProbeError> {
    let program = locate(manifest)?;
    let bin = BinFacts::of(program.path())?;

    if let Some(known) = cache.get(manifest.id, &bin) {
        return Ok(known.clone());
    }

    let version = ask_version(manifest, &program, contained_by).await?;
    let flags = ask_flags(&program, contained_by).await;
    let entry = Entry {
        probed_at: WallMs::now(),
        bin,
        version,
        flags,
    };
    cache.put(manifest.id, entry.clone());
    Ok(entry)
}

/// Try the manifest's candidate names on the operator's search path.
fn locate(manifest: &Manifest) -> Result<Program, ProbeError> {
    locate_with(manifest, resolve)
}

/// The first candidate name that resolves, in the manifest's own order.
///
/// The order is the manifest author's and it decides which program runs: a native executable and a launcher
/// script for the same CLI are both commonly on the path, they behave the same, and one of them costs an extra
/// process per session. Trying them in a different order would silently pick the slower one.
///
/// Generic over what resolving produces, so the ordering rule can be exercised without two real programs
/// installed. The shipping call and the tested call are the same code.
fn locate_with<P>(
    manifest: &Manifest,
    try_one: impl Fn(&str) -> Result<P, SpawnError>,
) -> Result<P, ProbeError> {
    for name in &manifest.bin.names {
        if let Ok(program) = try_one(name) {
            return Ok(program);
        }
    }
    // Nothing resolved. Every individual failure is "not on the path", and reporting five of those would bury
    // the one thing the operator needs: the list of names that were tried.
    Err(ProbeError::NotInstalled {
        candidates: manifest
            .bin
            .names
            .iter()
            .map(|name| format!("{name:?}"))
            .collect::<Vec<_>>()
            .join(", "),
    })
}

/// Run the manifest's version query and read the answer.
async fn ask_version(
    manifest: &Manifest,
    program: &Program,
    contained_by: &Containment,
) -> Result<String, ProbeError> {
    let args: Vec<String> = manifest
        .probe
        .version
        .args
        .iter()
        .map(ToString::to_string)
        .collect();

    let output = capture(program, &args, QUESTION_DEADLINE, contained_by).await?;
    let said = output.text();

    // The exit code is not consulted. Measured: a CLI can print its version and exit non-zero, and one prints
    // it to standard error. What matters is whether a version is in what it said.
    match manifest.probe.version.parse {
        VersionParse::SemverAnywhere => {
            find_version(&said)
                .map(str::to_owned)
                .ok_or_else(|| ProbeError::NoVersion {
                    path: program.path().clone(),
                    said: shorten(&said),
                })
        }
    }
}

/// Run the program's own help and take the flag names out of it.
///
/// Never an error. A CLI without a help flag, or one that fails when asked, is a CLI whose flag set is
/// unknown, and that is a different thing from a CLI with no flags. The reason travels with the answer.
async fn ask_flags(program: &Program, contained_by: &Containment) -> Flags {
    /// The flag every CLI in this space has, and the only one runtrol needs in order to ask about the rest.
    const HELP: &str = "--help";

    match capture(program, &[HELP.to_owned()], QUESTION_DEADLINE, contained_by).await {
        Ok(output) => classify_flags(&output.text()),
        Err(error) => Flags::Unknown {
            why: error.to_string(),
        },
    }
}

/// Turn help output into a flag set, or into a reason there is none.
///
/// Separated from the run so the classification can be exercised directly. The judgement it makes is the whole
/// point: output naming no flags is **not** an observation that the program has none. Every CLI in this space
/// has flags, so what actually happened is that the output was not a flag list, and recording that as an empty
/// set would let a capability check conclude a feature is absent when nobody managed to ask.
fn classify_flags(text: &str) -> Flags {
    let flags = flag_names(text);
    if flags.is_empty() {
        Flags::Unknown {
            why: "the program's help printed nothing that looks like a flag".to_owned(),
        }
    } else {
        Flags::Observed(flags)
    }
}

/// The first thing in `text` that looks like a version.
///
/// Hand written rather than a pattern library. The shape is two or three numbers separated by dots, with an
/// optional trailing label, and a scanner for that is shorter than the dependency and has no configuration to
/// get wrong.
///
/// Anywhere in the text, because these programs put their version behind a name, a banner, a build hash, or
/// nothing at all, and the position changes between releases.
#[must_use]
pub fn find_version(text: &str) -> Option<&str> {
    let bytes = text.as_bytes();
    let mut start = 0;

    while start < bytes.len() {
        let is_digit = bytes.get(start).is_some_and(u8::is_ascii_digit);
        if is_digit
            && starts_a_version(bytes, start)
            && let Some(end) = version_end(bytes, start)
        {
            return text.get(start..end);
        }
        start += 1;
    }
    None
}

/// Whether the digit at `at` begins a version rather than continuing a word.
///
/// A lone `v` in front is transparent, because `v1.2.3` is how a great many programs write it. The `v` itself
/// has to be at a boundary, so `rev3.2.1` is a word with a number in it and not a version.
fn starts_a_version(bytes: &[u8], at: usize) -> bool {
    /// Whether this byte would make a digit part of a longer word.
    fn continues_a_word(byte: u8) -> bool {
        byte.is_ascii_alphanumeric() || byte == b'.'
    }

    let Some(index) = at.checked_sub(1) else {
        return true;
    };
    let Some(before) = bytes.get(index).copied() else {
        return true;
    };
    if !continues_a_word(before) {
        return true;
    }
    if !matches!(before, b'v' | b'V') {
        return false;
    }
    // The `v` is transparent only when it is itself at a boundary.
    match index.checked_sub(1).and_then(|earlier| bytes.get(earlier)) {
        None => true,
        Some(byte) => !continues_a_word(*byte),
    }
}

/// Where a version starting at `start` ends, or `None` when what is there is not one.
fn version_end(bytes: &[u8], start: usize) -> Option<usize> {
    let mut cursor = start;
    let mut dots = 0_usize;
    let mut digits_in_part = 0_usize;

    while cursor < bytes.len() {
        match bytes.get(cursor) {
            Some(byte) if byte.is_ascii_digit() => digits_in_part += 1,
            Some(b'.') if digits_in_part > 0 => {
                dots += 1;
                digits_in_part = 0;
                if dots > 2 {
                    break;
                }
            }
            _ => break,
        }
        cursor += 1;
    }

    // Two numbers separated by a dot is the shortest thing worth calling a version. One number is a count of
    // something, and taking it would key the cache off a word count in a banner.
    if dots < 1 || digits_in_part == 0 {
        return None;
    }

    // A trailing label, as in a pre-release or a build tag. Taken along because it distinguishes two builds
    // that share their numbers, and dropping it would let the cache confuse them.
    while cursor < bytes.len() {
        match bytes.get(cursor) {
            Some(byte)
                if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'+' | b'_') =>
            {
                cursor += 1;
            }
            _ => break,
        }
    }
    // A label cannot end on a separator: that would take the punctuation of the sentence around it.
    while cursor > start
        && bytes
            .get(cursor - 1)
            .is_some_and(|byte| matches!(byte, b'.' | b'-' | b'+' | b'_'))
    {
        cursor -= 1;
    }
    Some(cursor)
}

/// Every long flag name in `text`.
///
/// Names only. What a flag does is never inferred from the words around it: help prose changes wording between
/// releases while flag names do not, so a reader of names degrades gracefully and a reader of sentences does
/// not degrade at all, it simply becomes wrong.
#[must_use]
pub fn flag_names(text: &str) -> std::collections::BTreeSet<String> {
    let mut found = std::collections::BTreeSet::new();
    let bytes = text.as_bytes();
    let mut cursor = 0;

    while cursor + 2 < bytes.len() {
        let starts_here = bytes.get(cursor) == Some(&b'-')
            && bytes.get(cursor + 1) == Some(&b'-')
            && bytes
                .get(cursor + 2)
                .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit());
        // Not preceded by a word character, so a hyphenated word in prose is not read as a flag.
        let at_boundary = cursor == 0
            || !bytes
                .get(cursor - 1)
                .is_some_and(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'));

        if starts_here && at_boundary {
            let mut end = cursor + 2;
            while end < bytes.len() {
                match bytes.get(end) {
                    Some(byte) if byte.is_ascii_lowercase() || byte.is_ascii_digit() => end += 1,
                    // An internal hyphen only counts when a name character follows it, so a flag at the end of
                    // a sentence does not absorb the dash after it.
                    Some(b'-')
                        if bytes.get(end + 1).is_some_and(|next| {
                            next.is_ascii_lowercase() || next.is_ascii_digit()
                        }) =>
                    {
                        end += 1;
                    }
                    _ => break,
                }
            }
            if let Some(name) = text.get(cursor..end) {
                found.insert(name.to_owned());
            }
            cursor = end;
            continue;
        }
        cursor += 1;
    }
    found
}

/// Cut a program's output down to something that fits in a message.
fn shorten(text: &str) -> String {
    /// How much of what a program said belongs in an error message.
    const KEEP: usize = 200;

    let trimmed = text.trim();
    match trimmed.char_indices().nth(KEEP) {
        None => trimmed.to_owned(),
        Some((at, _)) => match trimmed.get(..at) {
            Some(head) => format!("{head}..."),
            // A character boundary that came from `char_indices` is always valid, so this cannot be reached;
            // returning the whole thing rather than panicking keeps a probe failure a probe failure.
            None => trimmed.to_owned(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_version_is_found_wherever_the_program_put_it() {
        // These programs put it behind a name, a banner, a build hash, or nothing at all, and the position
        // changes between releases. That is why the search is positional-free rather than a format.
        for (said, expected) in [
            ("1.2.3", "1.2.3"),
            ("codex-cli 0.145.0", "0.145.0"),
            ("2.0.76 (Claude Code)", "2.0.76"),
            ("version: 1.4\n", "1.4"),
            ("  \n\nthing v3.2.1-beta.4\n", "3.2.1-beta.4"),
            ("built from 1.0.0+abc123", "1.0.0+abc123"),
            ("Thing 10.20.30 (build 9)", "10.20.30"),
            ("v1.0.0", "1.0.0"),
        ] {
            assert_eq!(
                find_version(said),
                Some(expected),
                "reading a version out of {said:?}"
            );
        }
    }

    #[test]
    fn something_that_is_not_a_version_is_not_taken_for_one() {
        // Keying a cache off a number in a banner would mean an entry that can never be invalidated
        // correctly, which is worse than having no entry.
        for said in [
            "",
            "no numbers here",
            "error: unknown option",
            "3 files changed",
            "port 8080",
            "1.",
            ".5",
            // A number inside a word is not a version, and a `v` only counts when it is a prefix of its own.
            "rev3.2.1",
            "sha1.2.3",
        ] {
            assert_eq!(find_version(said), None, "took {said:?} for a version");
        }
    }

    #[test]
    fn a_version_label_is_kept_because_it_tells_two_builds_apart() {
        let found = find_version("thing 1.0.0-rc.2, built today").expect("a version is there");
        assert_eq!(found, "1.0.0-rc.2");
    }

    #[test]
    fn flag_names_are_taken_and_help_prose_is_not() {
        // What a flag does is never inferred. Names are stable between releases and wording is not.
        let help = "\
Usage: thing [options]

Options:
  -v, --version            print the version
      --output-format <f>  set the output format
      --input-format <f>   well-known and long-standing behaviour
      --dangerously-skip   do not do this
  -h, --help               show help
";
        let flags = flag_names(help);
        assert!(flags.contains("--version"));
        assert!(flags.contains("--output-format"));
        assert!(flags.contains("--input-format"));
        assert!(flags.contains("--dangerously-skip"));
        assert!(flags.contains("--help"));
        assert_eq!(flags.len(), 5, "prose must not become flags: {flags:?}");
    }

    #[test]
    fn a_hyphenated_word_in_prose_is_not_a_flag() {
        let flags = flag_names("this is a well-known long--standing thing, see also ---");
        assert!(flags.is_empty(), "{flags:?}");
    }

    #[test]
    fn a_flag_at_the_end_of_a_sentence_does_not_absorb_the_punctuation() {
        let flags = flag_names("pass --resume. or pass --continue-here-");
        assert!(flags.contains("--resume"), "{flags:?}");
        assert!(flags.contains("--continue-here"), "{flags:?}");
    }

    #[test]
    fn a_flag_set_and_an_unanswered_question_are_different_things() {
        // The two lead to opposite decisions: one degrades a feature on purpose and says so, the other is a
        // question still open. Collapsing them is how a capability check starts lying.
        let observed = Flags::Observed(["--resume".to_owned()].into_iter().collect());
        let unknown = Flags::Unknown {
            why: "the program could not be run".to_owned(),
        };

        assert!(observed.is_observed());
        assert!(observed.has("--resume"));
        assert!(!observed.has("--nothing"));

        assert!(!unknown.is_observed());
        assert!(
            !unknown.has("--resume"),
            "an unasked question answers no, which is the safe direction"
        );
        assert!(unknown.is_empty());
        assert_ne!(observed, unknown);
    }

    #[test]
    fn help_that_names_no_flag_is_an_unanswered_question_and_not_an_empty_answer() {
        // Every CLI in this space has flags. Output with none in it means the question was not answered, and
        // recording an empty set would let a capability check conclude a feature is absent.
        for said in ["", "error: unknown option", "Usage: thing [FILE]", "-v  -h"] {
            match classify_flags(said) {
                Flags::Unknown { why } => {
                    assert!(!why.is_empty(), "the reason has to say something");
                }
                other @ Flags::Observed(_) => {
                    panic!("{said:?} was classified as an answer: {other:?}")
                }
            }
        }
        assert!(classify_flags("  --resume  ").is_observed());
    }

    #[test]
    fn candidates_are_tried_in_the_order_the_manifest_wrote_them() {
        // The order decides which program runs. A native executable and a launcher for the same CLI are both
        // commonly on the path and behave identically, and one of them costs an extra process per session.
        let manifest: Manifest = toml::from_str(
            "schema = 1\nid = \"thing\"\ndisplay_name = \"Thing\"\nkind = \"example\"\n[bin]\nnames = [\"first\", \"second\", \"third\"]\n",
        )
        .expect("the fixture parses");

        let missing = |name: &str| SpawnError::NotFound {
            program: name.to_owned(),
            searched: "nowhere".to_owned(),
        };

        // Everything installed: the first wins.
        let chosen = locate_with(&manifest, |name| Ok::<String, SpawnError>(name.to_owned()))
            .expect("something resolves");
        assert_eq!(chosen, "first");

        // The preferred one absent: the next in the manifest's order wins, not the last.
        let chosen = locate_with(&manifest, |name| {
            if name == "first" {
                Err(missing(name))
            } else {
                Ok(name.to_owned())
            }
        })
        .expect("something resolves");
        assert_eq!(chosen, "second");

        // Only the last one installed: it is still found, so a failure does not stop the search.
        let chosen = locate_with(&manifest, |name| {
            if name == "third" {
                Ok(name.to_owned())
            } else {
                Err(missing(name))
            }
        })
        .expect("the last candidate resolves");
        assert_eq!(chosen, "third");
    }

    #[test]
    fn a_missing_cli_names_every_candidate_that_was_tried() {
        // The operator's next move is to install something or fix the manifest, and which one depends on what
        // was looked for.
        let manifest: Manifest = toml::from_str(
            "schema = 1\nid = \"absent\"\ndisplay_name = \"Absent\"\nkind = \"example\"\n[bin]\nnames = [\"runtrol-no-such-program-a\", \"runtrol-no-such-program-b\"]\n",
        )
        .expect("the fixture parses");

        match locate_with(&manifest, resolve) {
            Err(ProbeError::NotInstalled { candidates }) => {
                assert!(
                    candidates.contains("runtrol-no-such-program-a"),
                    "{candidates}"
                );
                assert!(
                    candidates.contains("runtrol-no-such-program-b"),
                    "{candidates}"
                );
            }
            other => panic!("expected a named absence, got {other:?}"),
        }
    }

    #[test]
    fn a_missing_cli_is_the_operators_move_and_a_write_failure_is_not() {
        assert!(
            ProbeError::NotInstalled {
                candidates: "\"thing\"".to_owned()
            }
            .needs_the_operator()
        );
        assert!(
            !ProbeError::CacheWrite {
                path: AbsPath::new(if cfg!(windows) { r"C:\x" } else { "/x" }).expect("valid"),
                detail: "disk full".to_owned(),
            }
            .needs_the_operator(),
            "a cache that cannot be written costs time, not correctness"
        );
    }

    #[tokio::test]
    async fn a_real_cli_is_asked_what_it_is_and_the_answer_is_remembered() {
        // The whole ladder against a program that is genuinely installed, because the rule for judging this
        // is measured product behaviour rather than a mock agreeing with itself. The build tool is the one CLI
        // guaranteed to be here: it is what is running this test.
        let manifest: Manifest = toml::from_str(
            "schema = 1\nid = \"cargo\"\ndisplay_name = \"Cargo\"\nkind = \"example\"\n[bin]\nnames = [\"cargo\"]\n",
        )
        .expect("the fixture parses");

        let scratch = std::env::temp_dir().join("runtrol-probe-live");
        if scratch.exists() {
            std::fs::remove_dir_all(&scratch).expect("clear the previous run");
        }
        std::fs::create_dir_all(&scratch).expect("create the scratch directory");
        let root = AbsPath::canonicalize(scratch.to_str().expect("the temporary path is UTF-8"))
            .expect("canonicalize");
        let cache_path = root.join("probe.json").expect("a valid file name");

        let mut cache = ProbeCache::open(&cache_path);
        let contained_by = Containment::without_any();

        let first = probe(&manifest, &mut cache, &contained_by)
            .await
            .expect("an installed CLI must be probeable");

        assert!(
            find_version(&first.version).is_some(),
            "the version has to be a version: {:?}",
            first.version
        );
        assert!(
            first.flags.is_observed(),
            "this CLI has a help flag, so its flag set must be observed: {:?}",
            first.flags
        );
        assert!(
            first.flags.has("--version"),
            "the flag that was just used must be in the set"
        );
        assert!(first.bin.size > 0, "the program has a size");

        cache.save().expect("the cache must be writable");
        let reopened = ProbeCache::open(&cache_path);
        let second = probe(&manifest, &mut cache, &contained_by)
            .await
            .expect("the second ask must be answered");
        assert_eq!(
            second, first,
            "the second ask must be the remembered answer, not a second process"
        );
        assert_eq!(reopened.len(), 1, "and it must have survived the restart");

        std::fs::remove_dir_all(root.as_std_path()).expect("clean up");
    }

    #[test]
    fn what_a_program_said_is_cut_down_before_it_reaches_a_message() {
        let long = "x".repeat(1_000);
        let short = shorten(&long);
        assert!(short.len() < long.len());
        assert!(short.ends_with("..."));
        assert_eq!(shorten("  1.0.0  "), "1.0.0");
    }
}
