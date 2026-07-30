//! What may be passed to a child on its command line.
//!
//! # This file does not escape anything, and that is the point
//!
//! Windows command-line quoting is the surface of CVE-2024-24576 (BatBadBut, CVSS 10.0), and it matters
//! here specifically: both supervised CLIs install as npm-generated `.cmd` launchers on Windows, which is
//! exactly the case that vulnerability was about.
//!
//! So the first question was whether runtrol needs its own escaper. **Measured on this toolchain, against
//! a real `.cmd` that echoed what it received, with the marker file that would prove injection:**
//!
//! | argument | reached the batch file as | injected |
//! |---|---|---|
//! | `a&echo INJECTED>file` | `"a&echo INJECTED>""file"""` | no |
//! | `a\|whoami` | `"a\|whoami"` | no |
//! | `a^b` | `"a^b"` | no |
//! | `%PATH%` | `"%PATH%"` (not expanded) | no |
//! | `a"b` | `"a""b"` | no |
//! | `a\nb`, `a\rb` | refused: `batch file arguments are invalid` | no |
//! | `a\0b` | refused: `nul byte found in provided data` | no |
//!
//! The standard library's mitigation is correct and complete. Writing a second escaper would mean
//! replacing audited, platform-specific security code with a hand-rolled version, which is the wrong
//! direction on the one surface where being wrong is a remote code execution.
//!
//! # So what is left to do
//!
//! Two things the standard library does not do.
//!
//! It refuses a newline **at the spawn**, with a message that names neither the argument nor the
//! character. An operator reading `batch file arguments are invalid` has nowhere to start. This module
//! refuses earlier and says which argument, which character, and where.
//!
//! And it allows control characters through to an ordinary executable, where they are legal. runtrol
//! refuses them anyway, on every platform: nothing runtrol puts on a command line (flags, session
//! identifiers, model names, paths) needs one, they forge log lines, and provider output is untrusted by
//! standing rule. Deliberately stricter than the operating system, in the direction that cannot break
//! anything runtrol actually does.

use crate::error::SpawnError;

/// Longest argument runtrol will pass.
///
/// Windows caps a whole command line at 32,767 characters, and one oversized argument would fail the
/// spawn with a message about the total rather than about the argument that caused it. Half the platform
/// limit leaves room for everything else on the line.
pub const MAX_ARGUMENT_LEN: usize = 16_384;

/// Check every argument before any of them reaches a spawn.
///
/// # Errors
///
/// [`SpawnError::ArgvUnsafe`] naming the first argument that cannot be passed, which character made it
/// unpassable, and where in the argument it sits.
pub fn check_all<S: AsRef<str>>(arguments: &[S]) -> Result<(), SpawnError> {
    for (index, argument) in arguments.iter().enumerate() {
        check_one(index, argument.as_ref())?;
    }
    Ok(())
}

/// Check one argument.
///
/// # Errors
///
/// [`SpawnError::ArgvUnsafe`] when the argument contains a NUL, any other control character, or is longer
/// than [`MAX_ARGUMENT_LEN`].
pub fn check_one(index: usize, argument: &str) -> Result<(), SpawnError> {
    let refuse = |what: &'static str, at: usize| SpawnError::ArgvUnsafe {
        index,
        argument: argument.to_owned(),
        what,
        at,
    };

    if argument.len() > MAX_ARGUMENT_LEN {
        return Err(refuse(
            "more text than a command line can carry",
            MAX_ARGUMENT_LEN,
        ));
    }

    for (at, character) in argument.char_indices() {
        // Checked first and named separately: a NUL truncates the argument inside the operating system, so
        // the child receives something shorter than what was passed. That is a different failure from a
        // rejected newline and deserves a different sentence.
        if character == '\0' {
            return Err(refuse("a NUL byte, which truncates the argument", at));
        }
        if character == '\n' || character == '\r' {
            return Err(refuse("a line break, which forges log lines", at));
        }
        if character.is_control() {
            return Err(refuse("a control character", at));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinary_arguments_pass() {
        let fine = [
            "--version",
            "--session-id",
            "0199c0de-1234-7000-8000-abcdef012345",
            "--model=claude-opus-5[1m]",
            r"C:\Users\me\projects\app",
            "/home/me/projects/app",
            "a message with spaces",
            "한국어 인자",
        ];
        assert!(check_all(&fine).is_ok());
    }

    #[test]
    fn shell_metacharacters_pass_because_the_standard_library_handles_them() {
        // Measured: none of these injected through a real `.cmd`. Refusing them here would remove
        // capability for no gain, and it would suggest runtrol was doing escaping it is not doing.
        let metacharacters = [
            "a&echo INJECTED",
            "a|whoami",
            "a^b",
            "a\"b",
            "%PATH%",
            "a>out.txt",
            "a<in.txt",
            "a;b",
            "$(whoami)",
            "`whoami`",
            "a\\",
        ];
        for (index, argument) in metacharacters.iter().enumerate() {
            assert!(
                check_one(index, argument).is_ok(),
                "{argument:?} should pass: the standard library neutralizes it"
            );
        }
    }

    #[test]
    fn a_nul_is_refused_and_named_as_a_truncation() {
        let error = check_one(2, "safe\0hidden").expect_err("a NUL must be refused");
        match error {
            SpawnError::ArgvUnsafe {
                index, what, at, ..
            } => {
                assert_eq!(index, 2, "the operator has to know which argument");
                assert_eq!(at, 4, "and where in it");
                assert!(what.contains("truncates"), "and why it matters: {what}");
            }
            other => panic!("expected an argv refusal, got {other:?}"),
        }
    }

    #[test]
    fn line_breaks_are_refused_with_a_reason_the_os_does_not_give() {
        // The operating system's own message, measured, is "batch file arguments are invalid". It names
        // neither the argument nor the character, and this is the whole reason to check earlier.
        for (argument, offset) in [("a\nb", 1), ("a\r\nb", 1), ("trailing\n", 8)] {
            let error = check_one(0, argument).expect_err("a line break must be refused");
            match error {
                SpawnError::ArgvUnsafe { what, at, .. } => {
                    assert_eq!(at, offset);
                    assert!(what.contains("line break"), "{what}");
                }
                other => panic!("expected an argv refusal, got {other:?}"),
            }
        }
    }

    #[test]
    fn other_control_characters_are_refused_too() {
        // Legal to an ordinary executable, refused anyway. An escape sequence in an argument rewrites the
        // terminal of whoever reads the log, and nothing runtrol passes needs one.
        for argument in ["a\u{1b}[31mb", "a\tb", "a\u{7}b", "a\u{0c}b"] {
            assert!(
                check_one(0, argument).is_err(),
                "{argument:?} should be refused"
            );
        }
    }

    #[test]
    fn the_message_names_the_argument_so_it_can_be_found() {
        let error = check_one(3, "bad\nvalue").expect_err("refused");
        let message = error.to_string();
        assert!(message.contains("argument 3"), "{message}");
        assert!(message.contains("line break"), "{message}");
        assert!(message.contains("byte 3"), "{message}");
    }

    #[test]
    fn an_oversized_argument_is_refused_before_the_command_line_is_built() {
        // Otherwise the spawn fails with a message about the total command line length, which points at
        // the wrong thing entirely.
        let huge = "a".repeat(MAX_ARGUMENT_LEN + 1);
        assert!(check_one(0, &huge).is_err());
        assert!(check_one(0, &"a".repeat(MAX_ARGUMENT_LEN)).is_ok());
    }

    #[test]
    fn checking_stops_at_the_first_bad_argument() {
        // Reporting one problem at a time is right here: the operator fixes it and runs again, and a list
        // of three refusals from one mistake reads as three mistakes.
        let arguments = ["fine", "also fine", "bad\nhere", "worse\0still"];
        let error = check_all(&arguments).expect_err("refused");
        match error {
            SpawnError::ArgvUnsafe { index, .. } => assert_eq!(index, 2),
            other => panic!("expected an argv refusal, got {other:?}"),
        }
    }

    #[test]
    fn an_empty_argument_list_and_an_empty_argument_are_both_fine() {
        // An empty string is a legitimate argument, and some CLIs take one.
        let none: [&str; 0] = [];
        assert!(check_all(&none).is_ok());
        assert!(check_one(0, "").is_ok());
    }
}
