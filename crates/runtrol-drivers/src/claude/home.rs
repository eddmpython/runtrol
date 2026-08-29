//! Where this CLI keeps what it owns, resolved from the environment the CLI itself inherits.
//!
//! Two provider-owned places are read by this driver, both read-only: the user configuration beside the home
//! directory (`~/.claude.json`, for model options the CLI learned) and the configuration directory (`~/.claude`,
//! for the conversations it stored). The CLI moves the second with `CLAUDE_CONFIG_DIR`; measured on 2.1.238, the
//! variable is honoured when set and the home default applies otherwise. Resolving both in one place keeps the
//! two reads from disagreeing about whose home this is.
//!
//! Which variable names the person's home, and what counts as an answer, is `crate::operator`: every driver
//! asks the same question there and only the override and the folder name are this CLI's own.

use std::ffi::OsString;
use std::path::PathBuf;

pub(super) use crate::operator::{HomeProblem, operator_home};

/// The CLI's own override for where its configuration directory lives.
const CONFIG_DIR_ENV: &str = "CLAUDE_CONFIG_DIR";

/// The configuration directory's default name under the operator's home.
const CONFIG_DIR_DEFAULT: &str = ".claude";

/// The CLI's configuration directory: its own override when set to an absolute path, else the home default.
pub(super) fn config_directory(
    look: &mut impl FnMut(&str) -> Option<OsString>,
) -> Result<PathBuf, HomeProblem> {
    crate::operator::provider_home(look, CONFIG_DIR_ENV, CONFIG_DIR_DEFAULT)
}

#[cfg(test)]
mod tests {
    use super::*;

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
            crate::operator::OPERATOR_HOME_ENV => Some(home.clone().into_os_string()),
            _ => None,
        };
        assert_eq!(
            config_directory(&mut relative_override)
                .expect("a relative override falls back to the home default"),
            home.join(".claude")
        );

        let mut empty_override = |name: &str| match name {
            CONFIG_DIR_ENV => Some(OsString::new()),
            crate::operator::OPERATOR_HOME_ENV => Some(home.clone().into_os_string()),
            _ => None,
        };
        assert_eq!(
            config_directory(&mut empty_override).expect("an empty override is no override"),
            home.join(".claude")
        );
    }
}
