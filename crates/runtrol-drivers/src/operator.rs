//! The operator's home directory, as every coding CLI reads it.
//!
//! Each driver resolves its own provider's configuration directory, because each provider names its own
//! override variable and its own default folder. What they share is the step underneath: which variable holds
//! the person's home on this platform, and what counts as an answer. One owner for that, so two drivers cannot
//! disagree about whose machine this is.

use std::ffi::{OsStr, OsString};
use std::path::PathBuf;

#[cfg(windows)]
pub(crate) const OPERATOR_HOME_ENV: &str = "USERPROFILE";
#[cfg(not(windows))]
pub(crate) const OPERATOR_HOME_ENV: &str = "HOME";

/// Why the operator's home could not be named.
#[derive(Clone, Debug)]
pub(crate) enum HomeProblem {
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

/// The operator's home directory, from the variable the CLIs themselves read.
pub(crate) fn operator_home(
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

/// A provider's configuration directory: its own override when set to an absolute path, else a folder under
/// the operator's home.
///
/// A relative override is ignored rather than resolved, because resolving it would need a working directory
/// no driver has, and the CLI in that state is what the operator sees anyway.
pub(crate) fn provider_home(
    look: &mut impl FnMut(&str) -> Option<OsString>,
    override_env: &str,
    default_folder: &str,
) -> Result<PathBuf, HomeProblem> {
    if let Some(overridden) = look(override_env).filter(|value| value != OsStr::new("")) {
        let path = PathBuf::from(overridden);
        if path.is_absolute() {
            return Ok(path);
        }
    }
    Ok(operator_home(look)?.join(default_folder))
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
    fn a_provider_folder_follows_its_override_only_when_it_is_absolute() {
        let absolute = std::env::temp_dir().join("provider-home-override");
        let mut overridden =
            |name: &str| (name == "PROVIDER_HOME").then(|| absolute.clone().into_os_string());
        assert_eq!(
            provider_home(&mut overridden, "PROVIDER_HOME", ".provider")
                .expect("an absolute override names the directory"),
            absolute
        );

        let home = std::env::temp_dir().join("operator-home");
        let mut relative_override = |name: &str| match name {
            "PROVIDER_HOME" => Some(OsString::from("relative/config")),
            OPERATOR_HOME_ENV => Some(home.clone().into_os_string()),
            _ => None,
        };
        assert_eq!(
            provider_home(&mut relative_override, "PROVIDER_HOME", ".provider")
                .expect("a relative override falls back to the home default"),
            home.join(".provider")
        );
    }
}
