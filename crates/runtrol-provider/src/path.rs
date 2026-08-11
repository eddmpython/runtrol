//! The one path type that crosses the provider seam.
//!
//! Nothing else in runtrol joins, compares, or stores a path as a plain string. A path arriving
//! from a manifest, a command line, or a provider's output becomes an [`AbsPath`] or it is
//! rejected, and everything downstream then works with a value whose invariants are already true.
//!
//! The invariants exist for one job: deciding whether a directory lies under a permitted root. That
//! decision gates where a coding agent may write, so `.` and `..` are refused at the door rather
//! than resolved later, and comparison is by path component rather than by text prefix.

use core::fmt;
use std::path::Path;

use camino::{Utf8Component, Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Whether a new session must own its workspace alone.
///
/// This is an operator decision carried across surfaces and the Core. A provider never interprets it. The Core uses
/// it while admitting the process, before the provider is allowed to open one.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum WorkspaceAccess {
    /// Refuse when another opening, live, or closing process owns the same working tree.
    Exclusive,
    /// The operator explicitly accepted concurrent writers for this start.
    Shared,
}

/// A path offered to runtrol did not satisfy [`AbsPath`]'s invariants.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PathError {
    /// Empty text.
    #[error("path is empty")]
    Empty,

    /// Not an absolute path.
    ///
    /// runtrol refuses to resolve a relative path itself: the answer would depend on the daemon's
    /// working directory, which is not something the operator chose or can see.
    #[error("path must be absolute, got {given:?}")]
    Relative {
        /// The path as offered.
        given: String,
    },

    /// Contains a `.` or `..` component.
    ///
    /// Refused rather than collapsed. Collapsing text is only correct when no component is a
    /// symbolic link, and being wrong here means an agent writing outside its permitted root.
    /// Callers that need `..` resolved call [`AbsPath::canonicalize`], which asks the filesystem.
    #[error("path must not contain '.' or '..', got {given:?}")]
    Relatives {
        /// The path as offered.
        given: String,
    },

    /// Contains a NUL byte.
    ///
    /// Every OS path API terminates at NUL, so such a path names one thing to runtrol and a
    /// different, shorter thing to the kernel.
    #[error("path must not contain a NUL byte, got {given:?}")]
    Nul {
        /// The path as offered.
        given: String,
    },

    /// Not valid UTF-8.
    ///
    /// runtrol's own surfaces (a database key, a JSON frame, a log line) are UTF-8 throughout, and
    /// carrying a lossy copy alongside the real bytes would mean two spellings of one path.
    #[error("path is not valid UTF-8: {given:?}")]
    NotUtf8 {
        /// The path as offered, with invalid sequences replaced.
        given: String,
    },

    /// The filesystem refused to resolve the path.
    ///
    /// The OS error is flattened into a kind and its own message rather than carried as a nested
    /// `std::io::Error`. That keeps this type comparable and cloneable, which callers need in order
    /// to cache a resolution failure, and `kind` stays matchable so "this path does not exist" can
    /// be told apart from "permission denied" without reading the message text.
    #[error("cannot resolve {given:?}: {detail}")]
    Resolve {
        /// The path as offered.
        given: String,
        /// What class of failure the OS reported.
        kind: std::io::ErrorKind,
        /// What the OS said, verbatim.
        detail: String,
    },
}

/// An absolute, UTF-8 path with no relative components.
///
/// Cheap to compare and safe to join against. Construction is the only place these rules are
/// checked, so a function taking an `AbsPath` needs no defensive checks of its own.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AbsPath(Utf8PathBuf);

impl AbsPath {
    /// Validate text as an absolute path, without touching the filesystem.
    ///
    /// On Windows, forward slashes are rewritten to backslashes and a `\\?\` prefix in front of a
    /// drive path is removed, so that one location has exactly one spelling. Both are the same
    /// location to the OS but different text, and text that differs is text that fails to match a
    /// permitted root.
    ///
    /// # Errors
    ///
    /// [`PathError::Empty`], [`PathError::Nul`], [`PathError::Relative`] when the path is not
    /// absolute, or [`PathError::Relatives`] when it contains `.` or `..`.
    pub fn new(text: &str) -> Result<Self, PathError> {
        if text.is_empty() {
            return Err(PathError::Empty);
        }
        if text.contains('\0') {
            return Err(PathError::Nul {
                given: text.to_owned(),
            });
        }

        let normalized = normalize(text);
        let path = Utf8Path::new(&normalized);
        if !path.is_absolute() {
            return Err(PathError::Relative {
                given: text.to_owned(),
            });
        }
        for component in path.components() {
            if matches!(component, Utf8Component::CurDir | Utf8Component::ParentDir) {
                return Err(PathError::Relatives {
                    given: text.to_owned(),
                });
            }
        }
        Ok(Self(Utf8PathBuf::from(normalized)))
    }

    /// Validate an OS path, requiring it to be UTF-8.
    ///
    /// # Errors
    ///
    /// [`PathError::NotUtf8`] when the bytes are not UTF-8, plus everything [`AbsPath::new`]
    /// returns.
    pub fn from_os(path: &Path) -> Result<Self, PathError> {
        match path.to_str() {
            Some(text) => Self::new(text),
            None => Err(PathError::NotUtf8 {
                given: path.to_string_lossy().into_owned(),
            }),
        }
    }

    /// Ask the filesystem to resolve the path, then validate the answer.
    ///
    /// This is the only constructor that resolves symbolic links and `..`, and therefore the only
    /// one whose result is safe to compare against a permitted root. It requires the path to exist.
    ///
    /// # Errors
    ///
    /// [`PathError::Resolve`] when the OS cannot resolve the path (it does not exist, or a
    /// component is not a directory, or permission was denied), plus everything
    /// [`AbsPath::from_os`] returns.
    pub fn canonicalize(text: &str) -> Result<Self, PathError> {
        let resolved = std::fs::canonicalize(text).map_err(|source| PathError::Resolve {
            given: text.to_owned(),
            kind: source.kind(),
            detail: source.to_string(),
        })?;
        Self::from_os(&resolved)
    }

    /// The path as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    /// The path for handing to the OS.
    #[must_use]
    pub fn as_std_path(&self) -> &Path {
        self.0.as_std_path()
    }

    /// Borrow as a UTF-8 path.
    #[must_use]
    pub fn as_utf8_path(&self) -> &Utf8Path {
        &self.0
    }

    /// Join a relative segment, keeping the invariants.
    ///
    /// # Errors
    ///
    /// [`PathError::Relatives`] when `segment` contains `.` or `..`, [`PathError::Nul`] for a NUL
    /// byte, [`PathError::Relative`] when `segment` is itself absolute (which would silently
    /// discard `self`, the classic path-join surprise).
    pub fn join(&self, segment: &str) -> Result<Self, PathError> {
        if segment.is_empty() {
            return Err(PathError::Empty);
        }
        if segment.contains('\0') {
            return Err(PathError::Nul {
                given: segment.to_owned(),
            });
        }
        let normalized = normalize(segment);
        let relative = Utf8Path::new(&normalized);
        if relative.is_absolute() {
            return Err(PathError::Relative {
                given: segment.to_owned(),
            });
        }
        for component in relative.components() {
            if matches!(component, Utf8Component::CurDir | Utf8Component::ParentDir) {
                return Err(PathError::Relatives {
                    given: segment.to_owned(),
                });
            }
        }
        Ok(Self(self.0.join(relative)))
    }

    /// The containing directory, or `None` at a filesystem root.
    #[must_use]
    pub fn parent(&self) -> Option<Self> {
        self.0.parent().map(|parent| Self(parent.to_owned()))
    }

    /// The final component.
    #[must_use]
    pub fn file_name(&self) -> Option<&str> {
        self.0.file_name()
    }

    /// Whether this path is `root` or lies beneath it.
    ///
    /// Compared component by component, never as a text prefix. A text prefix would report
    /// `C:\work-secrets` as being under `C:\work`, which is exactly the mistake this type exists to
    /// prevent.
    ///
    /// On Windows the comparison ignores ASCII case, because the OS does. Non-ASCII components must
    /// match exactly: full Windows case folding needs the OS's own table, and getting it wrong in
    /// the permissive direction would grant access. Being strict here can only refuse a path that
    /// should have been allowed, which the operator sees and can correct.
    ///
    /// Both sides should already be canonical. This function compares what it is given; resolving
    /// symbolic links is [`AbsPath::canonicalize`]'s job, and comparing unresolved paths answers a
    /// question about text rather than about the filesystem.
    #[must_use]
    pub fn is_under(&self, root: &Self) -> bool {
        let mut mine = self.0.components();
        for expected in root.0.components() {
            match mine.next() {
                Some(actual) if components_match(actual, expected) => {}
                _ => return false,
            }
        }
        true
    }
}

/// Compare two path components under the platform's own equality rules.
fn components_match(left: Utf8Component<'_>, right: Utf8Component<'_>) -> bool {
    if cfg!(windows) {
        left.as_str().eq_ignore_ascii_case(right.as_str())
    } else {
        left == right
    }
}

/// Give one location one spelling, on the platforms where it has several.
#[cfg(windows)]
fn normalize(text: &str) -> String {
    // A verbatim prefix tells the OS to skip its own normalization. Keeping it would mean
    // `\\?\C:\work` and `C:\work` compare as different roots. UNC verbatim paths (`\\?\UNC\...`)
    // are left alone: rewriting them changes which share is named.
    let stripped = match text.strip_prefix(r"\\?\") {
        Some(rest) if !rest.starts_with("UNC\\") && !rest.starts_with("unc\\") => rest,
        _ => text,
    };
    stripped.replace('/', "\\")
}

/// Give one location one spelling, on the platforms where it has several.
#[cfg(not(windows))]
fn normalize(text: &str) -> String {
    // On Unix a backslash is an ordinary filename character and a path has exactly one spelling,
    // so there is nothing to normalize. Returning an owned string keeps one signature for both
    // platforms.
    text.to_owned()
}

impl fmt::Display for AbsPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0.as_str())
    }
}

impl fmt::Debug for AbsPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "AbsPath({})", self.0)
    }
}

impl AsRef<Path> for AbsPath {
    fn as_ref(&self) -> &Path {
        self.0.as_std_path()
    }
}

impl core::str::FromStr for AbsPath {
    type Err = PathError;

    fn from_str(text: &str) -> Result<Self, PathError> {
        Self::new(text)
    }
}

impl Serialize for AbsPath {
    fn serialize<S: Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        ser.serialize_str(self.0.as_str())
    }
}

impl<'de> Deserialize<'de> for AbsPath {
    /// Decoding runs the same validation as [`AbsPath::new`], so a manifest or a wire frame cannot
    /// introduce a path the constructor would have refused.
    fn deserialize<D: Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        let text = String::deserialize(de)?;
        Self::new(&text).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An absolute path for the platform the test is running on.
    fn abs(tail: &str) -> String {
        if cfg!(windows) {
            format!("C:\\{tail}")
        } else {
            format!("/{tail}")
        }
    }

    #[test]
    fn rejects_relative_paths() {
        for bad in ["work", "./work", "../work", ""] {
            assert!(AbsPath::new(bad).is_err(), "accepted {bad:?}");
        }
    }

    #[test]
    fn rejects_relative_components_instead_of_collapsing_them() {
        // Collapsing text is only sound when no component is a symbolic link, and being wrong
        // means an agent writing outside its root.
        let bad = abs("work/../secrets");
        assert!(matches!(
            AbsPath::new(&bad),
            Err(PathError::Relatives { .. })
        ));
    }

    #[test]
    fn rejects_nul() {
        let bad = abs("work\0extra");
        assert!(matches!(AbsPath::new(&bad), Err(PathError::Nul { .. })));
    }

    #[test]
    fn is_under_compares_components_not_text() {
        let root = AbsPath::new(&abs("work")).expect("valid root");
        let inside = AbsPath::new(&abs("work/project/src")).expect("valid child");
        let sibling = AbsPath::new(&abs("work-secrets/project")).expect("valid sibling");

        assert!(inside.is_under(&root));
        assert!(root.is_under(&root), "a root is under itself");
        assert!(
            !sibling.is_under(&root),
            "a text prefix check would wrongly allow this"
        );
        assert!(!root.is_under(&inside), "a parent is not under its child");
    }

    #[test]
    fn join_refuses_to_escape() {
        let root = AbsPath::new(&abs("work")).expect("valid root");
        assert_eq!(
            root.join("project").expect("valid segment").as_str(),
            AbsPath::new(&abs("work/project")).expect("valid").as_str()
        );
        for bad in ["../escape", "./here", "a/../../escape", "", "with\0nul"] {
            assert!(root.join(bad).is_err(), "accepted segment {bad:?}");
        }
        // An absolute segment would discard the base entirely. That surprise is a refusal here.
        assert!(root.join(&abs("elsewhere")).is_err());
    }

    #[test]
    fn parent_walks_up_and_stops_at_the_root() {
        let path = AbsPath::new(&abs("work/project")).expect("valid");
        let parent = path.parent().expect("has a parent");
        assert_eq!(
            parent.as_str(),
            AbsPath::new(&abs("work")).expect("valid").as_str()
        );
        let mut cursor = parent;
        for _ in 0..8 {
            match cursor.parent() {
                Some(next) => cursor = next,
                None => return,
            }
        }
        panic!("parent() never reached a root");
    }

    #[test]
    fn display_round_trips() {
        let path = AbsPath::new(&abs("work/project")).expect("valid");
        let parsed: AbsPath = path.to_string().parse().expect("display must be parseable");
        assert_eq!(path, parsed);
    }

    #[cfg(windows)]
    #[test]
    fn windows_gives_one_location_one_spelling() {
        let with_slashes = AbsPath::new("C:/work/project").expect("valid");
        let with_backslashes = AbsPath::new(r"C:\work\project").expect("valid");
        let verbatim = AbsPath::new(r"\\?\C:\work\project").expect("valid");
        assert_eq!(with_slashes, with_backslashes);
        assert_eq!(with_backslashes, verbatim);
    }

    #[cfg(windows)]
    #[test]
    fn windows_ignores_ascii_case_because_the_os_does() {
        let root = AbsPath::new(r"C:\Work").expect("valid");
        let inside = AbsPath::new(r"c:\work\project").expect("valid");
        assert!(inside.is_under(&root));
    }

    #[cfg(windows)]
    #[test]
    fn windows_keeps_unc_verbatim_paths_intact() {
        // Rewriting these would name a different share.
        let unc = AbsPath::new(r"\\?\UNC\server\share\project").expect("valid");
        assert!(unc.as_str().starts_with(r"\\?\UNC\"));
    }

    #[cfg(not(windows))]
    #[test]
    fn unix_treats_case_as_significant() {
        let root = AbsPath::new("/Work").expect("valid");
        let other = AbsPath::new("/work/project").expect("valid");
        assert!(!other.is_under(&root));
    }
}
