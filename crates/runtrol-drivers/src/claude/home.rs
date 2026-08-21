//! Where this CLI keeps what it owns, resolved from the environment the CLI itself inherits.
//!
//! Two provider-owned places are read by this driver, both read-only: the user configuration beside the home
//! directory (`~/.claude.json`, for model options the CLI learned) and the configuration directory (`~/.claude`,
//! for the conversations it stored). The CLI moves the second with `CLAUDE_CONFIG_DIR`; measured on 2.1.238, the
//! variable is honoured when set and the home default applies otherwise. Resolving both in one place keeps the
//! two reads from disagreeing about whose home this is.

use std::ffi::{OsStr, OsString};
use std::path::PathBuf;

#[cfg(windows)]
pub(super) const OPERATOR_HOME_ENV: &str = "USERPROFILE";
#[cfg(not(windows))]
pub(super) const OPERATOR_HOME_ENV: &str = "HOME";

/// The CLI's own override for where its configuration directory lives.
const CONFIG_DIR_ENV: &str = "CLAUDE_CONFIG_DIR";

/// The configuration directory's default name under the operator's home.
const CONFIG_DIR_DEFAULT: &str = ".claude";

/// Why the operator's home could not be named.
#[derive(Clone, Debug)]
pub(super) enum HomeProblem {
    Missing,
    Empty,
    Relative(PathBuf),
}

impl core::fmt::Display for HomeProblem {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Missing => write!(formatter, "{OPERATOR_HOME_ENV} is not set"),
            Self::Empty => write!(formatter, "{OPERATOR_HOME_ENV} is empty"),
            Self::Relative(path) => write!(
                formatter,
                "{OPERATOR_HOME_ENV} is not an absolute path: {path}",
                path = path.display()
            ),
        }
    }
}

/// The operator's home directory, from the variable the CLI itself reads.
pub(super) fn operator_home(
    look: &mut impl FnMut(&str) -> Option<OsString>,
) -> Result<PathBuf, HomeProblem> {
    let value = look(OPERATOR_HOME_ENV).ok_or(HomeProblem::Missing)?;
    if value == OsStr::new("") {
        return Err(HomeProblem::Empty);
    }
    let home = PathBuf::from(value);
    if !home.is_absolute() {
        return Err(HomeProblem::Relative(home));
    }
    Ok(home)
}

/// The CLI's configuration directory: its own override when set to an absolute path, else the home default.
///
/// A relative override is ignored rather than resolved, because resolving it would need a working directory
/// this driver does not have, and the CLI in that state is what the operator sees anyway.
pub(super) fn config_directory(
    look: &mut impl FnMut(&str) -> Option<OsString>,
) -> Result<PathBuf, HomeProblem> {
    if let Some(overridden) = look(CONFIG_DIR_ENV).filter(|value| value != OsStr::new("")) {
        let path = PathBuf::from(overridden);
        if path.is_absolute() {
            return Ok(path);
        }
    }
    Ok(operator_home(look)?.join(CONFIG_DIR_DEFAULT))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_home_variable_is_read_as_the_cli_reads_it() {
        let missing = operator_home(&mut |_| None).expect_err("a missing home must stay unknown");
        assert!(matches!(missing, HomeProblem::Missing));
        let empty = operator_home(&mut |_| Some(OsString::new()))
            .expect_err("an empty home must stay unknown");
        assert!(matches!(empty, HomeProblem::Empty));
        let relative = operator_home(&mut |_| Some(OsString::from("relative/home")))
            .expect_err("a relative home must stay unknown");
        assert!(matches!(relative, HomeProblem::Relative(_)));
    }

    #[test]
    fn the_configuration_directory_follows_the_override_only_when_it_is_absolute() {
        let absolute = std::env::temp_dir().join("claude-config-override");
        let mut overridden =
            |name: &str| (name == CONFIG_DIR_ENV).then(|| absolute.clone().into_os_string());
        assert_eq!(
            config_directory(&mut overridden).expect("an absolute override names the directory"),
            absolute
        );

        let home = std::env::temp_dir().join("claude-home");
        let mut relative_override = |name: &str| match name {
            CONFIG_DIR_ENV => Some(OsString::from("relative/config")),
            OPERATOR_HOME_ENV => Some(home.clone().into_os_string()),
            _ => None,
        };
        assert_eq!(
            config_directory(&mut relative_override)
                .expect("a relative override falls back to the home default"),
            home.join(".claude")
        );

        let mut empty_override = |name: &str| match name {
            CONFIG_DIR_ENV => Some(OsString::new()),
            OPERATOR_HOME_ENV => Some(home.clone().into_os_string()),
            _ => None,
        };
        assert_eq!(
            config_directory(&mut empty_override).expect("an empty override is no override"),
            home.join(".claude")
        );
    }
}
