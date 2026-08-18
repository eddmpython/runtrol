//! Rules every driver applies to a listing the provider printed itself.
//!
//! A CLI that lists what it owns answers in its own spelling and at its own length. Two questions come up for
//! every one of them and must be answered the same way each time: whether an entry belongs to the folder that was
//! asked about, and how long a title may be. Answering them twice would let two drivers disagree about which
//! sessions an operator has.
//!
//! Neither of these is the security boundary. Runtime canonicalises every entry and re-checks it against the
//! caller's approved roots before anything is shown. What lives here only keeps a page from being filled with
//! rows that would be discarded, and keeps a title from being unbounded.

use runtrol_provider::MAX_NATIVE_TITLE_BYTES;

/// Whether a working directory the provider reported sits inside the requested root.
///
/// Compared as text, because a provider's listing names folders that may no longer exist and asking the
/// filesystem would drop exactly the rows an operator most wants to see. Separators are folded and the comparison
/// ignores ASCII case, both measured as necessary: one CLI prints `C:\work` and `c:\work` for one folder in a
/// single answer, and another prints backslashes in one field and forward slashes in another.
///
/// A sibling whose name merely starts with the root is not inside it, which is why the boundary character is
/// checked rather than the prefix alone.
pub(crate) fn under(cwd: &str, root: &str) -> bool {
    let normalise = |path: &str| path.replace('\\', "/").trim_end_matches('/').to_owned();
    let cwd = normalise(cwd);
    let root = normalise(root);
    if root.is_empty() {
        return true;
    }
    cwd.eq_ignore_ascii_case(&root)
        || cwd.len() > root.len()
            && cwd[..root.len()].eq_ignore_ascii_case(&root)
            && cwd.as_bytes().get(root.len()) == Some(&b'/')
}

/// A provider's title, cut to the bound a catalogue entry accepts.
///
/// Cut on a character boundary so a multi-byte title loses its last character rather than becoming invalid text.
pub(crate) fn bounded(text: &str) -> String {
    if text.len() <= MAX_NATIVE_TITLE_BYTES {
        return text.to_owned();
    }
    let mut end = MAX_NATIVE_TITLE_BYTES;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_conversation_outside_the_requested_root_is_not_offered() {
        assert!(under("/work/alpha/nested", "/work/alpha"));
        assert!(under("/work/alpha", "/work/alpha"));
        assert!(under("C:\\work\\alpha\\deep", "C:/work/alpha"));
    }

    #[test]
    fn a_sibling_whose_name_starts_with_the_root_is_not_inside_it() {
        // The reason the boundary character is checked and not the prefix alone. Two projects beside each other,
        // one named for the other with a suffix, would otherwise pour into a single folder's list.
        assert!(!under("/work/alpha-other", "/work/alpha"));
        assert!(!under("/work/alphabet", "/work/alpha"));
    }

    #[test]
    fn one_folder_spelled_two_ways_is_one_folder() {
        // Measured on a real answer: a CLI prints both drive-letter cases for one directory, and another mixes
        // separators between two fields of the same record. Comparing the text as written would show an operator
        // half of their sessions.
        assert!(under("c:\\work\\alpha", "C:/work/alpha"));
        assert!(under("C:/work/alpha", "c:\\work\\alpha"));
        assert!(under("/work/alpha/", "/work/alpha"));
    }

    #[test]
    fn a_title_longer_than_the_bound_keeps_its_characters_intact() {
        let long = "가".repeat(MAX_NATIVE_TITLE_BYTES);
        let cut = bounded(&long);
        assert!(cut.len() <= MAX_NATIVE_TITLE_BYTES);
        assert!(
            long.starts_with(&cut),
            "the cut is a prefix, not a re-encoding"
        );
        assert_eq!(bounded("short"), "short");
    }
}
