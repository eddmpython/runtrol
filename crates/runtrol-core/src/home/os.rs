//! The directory each operating system designates for a program's own per-user state.
//!
//! Three platforms, three rules, one output type. This is the only place in runtrol that reads an
//! environment variable in order to find a directory, and it reads only the variables the platform
//! itself defines. What runtrol then puts inside that directory belongs to [`super::layout`], and
//! nothing here knows any of those names.
//!
//! # Why the rules take a lookup instead of reading the environment
//!
//! Setting an environment variable is `unsafe` and process-global, and this crate forbids `unsafe`.
//! So a platform rule is a function of a lookup rather than of the real environment, and the tests
//! drive it with a fake one. The rule that ships and the rule under test are then the same code
//! rather than two readings of the same paragraph.
//!
//! # Why an ignored variable is a value and not a shrug
//!
//! The XDG specification requires a relative `XDG_STATE_HOME` to be ignored. Ignoring it silently
//! would leave an operator looking in the directory they set while runtrol writes to a different
//! one, with nothing anywhere saying so. The fallback happens as specified, and the fact that it
//! happened travels out in [`Base::ignored`] for the caller to put in front of a person.

use std::env;

use runtrol_provider::AbsPath;

use crate::home::HomeError;

/// The directory runtrol's own folder goes under, and any operator setting that was not usable.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct Base {
    /// The platform's base directory for per-user state.
    pub(crate) path: AbsPath,
    /// A variable that named a base and could not be used.
    pub(crate) ignored: Option<Ignored>,
}

/// An environment variable that was set, was not usable as a directory, and was therefore not used.
///
/// Carried out of resolution rather than dropped, because the operator who set it is the only person
/// who can correct it and the only one who will not otherwise find out.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Ignored {
    /// The variable's name.
    pub name: &'static str,
    /// What it was set to.
    pub value: String,
    /// Why it could not be used, in the words of the check that refused it.
    pub why: String,
}

/// Why a variable does not name a directory.
///
/// The three are told apart only where the difference changes what happens: a variable the platform
/// requires becomes an error naming the reason, and an optional one falls back to a default.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Unusable {
    /// The variable is not in the environment.
    Unset,
    /// The variable is set to the empty string, which names nothing.
    Empty,
    /// The variable's bytes are not UTF-8, and runtrol's paths are UTF-8 throughout.
    NotUtf8,
}

impl Unusable {
    /// How to say this to the operator, as the tail of "`NAME` ...".
    pub(crate) const fn detail(self) -> &'static str {
        match self {
            Self::Unset => "is not set",
            Self::Empty => "is set to the empty string",
            Self::NotUtf8 => "is not valid UTF-8",
        }
    }
}

/// Look a variable up in this process's environment.
///
/// The one function here that touches real state. Everything that decides anything takes this as an
/// argument, which is what lets the decisions be tested.
pub(crate) fn looked_up(name: &str) -> Result<String, Unusable> {
    match env::var_os(name) {
        None => Err(Unusable::Unset),
        Some(value) => match value.into_string() {
            Ok(text) if text.is_empty() => Err(Unusable::Empty),
            Ok(text) => Ok(text),
            Err(_) => Err(Unusable::NotUtf8),
        },
    }
}

/// A variable this platform cannot do without, or an error that names it.
fn required(
    look: &impl Fn(&str) -> Result<String, Unusable>,
    name: &'static str,
) -> Result<String, HomeError> {
    look(name).map_err(|unusable| HomeError::EnvUnusable {
        name,
        why: unusable.detail(),
    })
}

/// A variable's value as an absolute path, or an error that names both.
fn absolute(name: &'static str, value: String) -> Result<AbsPath, HomeError> {
    match AbsPath::new(&value) {
        Ok(path) => Ok(path),
        Err(source) => Err(HomeError::BadEnv {
            name,
            value,
            source,
        }),
    }
}

/// Windows keeps per-user, machine-local state under `%LOCALAPPDATA%`.
///
/// Not `%APPDATA%`: that directory roams between machines, and what runtrol stores is process
/// identifiers and workspace paths from one particular machine. Carrying those to another machine
/// would describe sessions that never existed there.
#[cfg(windows)]
pub(crate) fn base_from(
    look: &impl Fn(&str) -> Result<String, Unusable>,
) -> Result<Base, HomeError> {
    /// The variable Windows sets to the per-user, non-roaming application data directory.
    const LOCAL_APP_DATA: &str = "LOCALAPPDATA";

    let value = required(look, LOCAL_APP_DATA)?;
    Ok(Base {
        path: absolute(LOCAL_APP_DATA, value)?,
        ignored: None,
    })
}

/// macOS puts a program's own support files under `~/Library/Application Support`.
#[cfg(target_os = "macos")]
pub(crate) fn base_from(
    look: &impl Fn(&str) -> Result<String, Unusable>,
) -> Result<Base, HomeError> {
    /// Apple's designated location for a program's own support files.
    const SUPPORT: &str = "Library/Application Support";

    Ok(Base {
        path: under_home(look, SUPPORT)?,
        ignored: None,
    })
}

/// Everything else Unix follows the XDG base directory specification.
///
/// State rather than data or cache: what runtrol keeps is meant to survive a reboot, is not worth
/// backing up, and must not be shared between machines. That is what `XDG_STATE_HOME` is for.
#[cfg(all(unix, not(target_os = "macos")))]
pub(crate) fn base_from(
    look: &impl Fn(&str) -> Result<String, Unusable>,
) -> Result<Base, HomeError> {
    /// The variable the specification lets an operator point elsewhere.
    const XDG_STATE_HOME: &str = "XDG_STATE_HOME";
    /// Where the specification puts state when that variable says nothing.
    const XDG_STATE_DEFAULT: &str = ".local/state";

    let Ok(value) = look(XDG_STATE_HOME) else {
        // Unset, empty, or not UTF-8. All three name no directory, so the specified default applies
        // unchanged. Telling them apart matters only for a variable the platform requires, and this
        // one is optional by design.
        return Ok(Base {
            path: under_home(look, XDG_STATE_DEFAULT)?,
            ignored: None,
        });
    };

    match AbsPath::new(&value) {
        Ok(path) => Ok(Base {
            path,
            ignored: None,
        }),
        // The specification requires a relative value to be ignored. The fallback is taken as
        // specified, and the reason goes with it so the operator can see why their setting had no
        // effect.
        Err(source) => Ok(Base {
            path: under_home(look, XDG_STATE_DEFAULT)?,
            ignored: Some(Ignored {
                name: XDG_STATE_HOME,
                value,
                why: source.to_string(),
            }),
        }),
    }
}

/// A directory under the operator's home, for the two Unix rules that are expressed that way.
#[cfg(unix)]
fn under_home(
    look: &impl Fn(&str) -> Result<String, Unusable>,
    segment: &'static str,
) -> Result<AbsPath, HomeError> {
    /// The variable every Unix sets to the operator's home directory.
    const HOME: &str = "HOME";

    let value = required(look, HOME)?;
    absolute(HOME, value)?
        .join(segment)
        .map_err(|source| HomeError::Layout { segment, source })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A lookup that answers from a list, so a rule can be exercised without touching the process.
    fn fake(entries: &[(&str, &str)]) -> impl Fn(&str) -> Result<String, Unusable> + use<> {
        let owned: Vec<(String, String)> = entries
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect();
        move |name| {
            let found = owned
                .iter()
                .find(|(candidate, _)| candidate == name)
                .map(|(_, value)| value.clone());
            match found {
                None => Err(Unusable::Unset),
                Some(value) if value.is_empty() => Err(Unusable::Empty),
                Some(value) => Ok(value),
            }
        }
    }

    #[test]
    fn the_real_environment_yields_a_base_on_this_machine() {
        // Every supported platform sets the variable its rule needs. If this fails on somebody's
        // machine, the message has to say which variable, which is what `EnvUnusable` carries.
        let base = base_from(&looked_up).expect("this platform must have a base directory");
        assert!(base.path.as_str().len() > 1, "{:?}", base.path);
    }

    #[test]
    fn a_missing_variable_is_named_rather_than_guessed_around() {
        // The alternative is inventing a directory the operator never chose and cannot find.
        match base_from(&fake(&[])) {
            Err(HomeError::EnvUnusable { name, why }) => {
                assert!(!name.is_empty());
                assert_eq!(why, Unusable::Unset.detail());
            }
            other => panic!("expected a named missing variable, got {other:?}"),
        }
    }

    #[test]
    fn every_reason_a_variable_is_unusable_reads_as_a_sentence() {
        for unusable in [Unusable::Unset, Unusable::Empty, Unusable::NotUtf8] {
            let sentence = format!("LOCALAPPDATA {}", unusable.detail());
            assert!(sentence.starts_with("LOCALAPPDATA is "), "{sentence}");
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_uses_the_non_roaming_directory() {
        let base = base_from(&fake(&[
            ("LOCALAPPDATA", r"C:\Users\someone\AppData\Local"),
            ("APPDATA", r"C:\Users\someone\AppData\Roaming"),
        ]))
        .expect("a set variable must resolve");
        assert_eq!(base.path.as_str(), r"C:\Users\someone\AppData\Local");
        assert_eq!(base.ignored, None);
    }

    #[cfg(windows)]
    #[test]
    fn windows_refuses_a_relative_setting_instead_of_resolving_it() {
        // A relative base would depend on the daemon's working directory, which nobody chose and
        // nobody can see.
        match base_from(&fake(&[("LOCALAPPDATA", r"AppData\Local")])) {
            Err(HomeError::BadEnv { name, value, .. }) => {
                assert_eq!(name, "LOCALAPPDATA");
                assert_eq!(value, r"AppData\Local");
            }
            other => panic!("expected a refusal that names the value, got {other:?}"),
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_uses_the_application_support_directory() {
        let base = base_from(&fake(&[("HOME", "/Users/someone")]))
            .expect("a set home directory must resolve");
        assert_eq!(
            base.path.as_str(),
            "/Users/someone/Library/Application Support"
        );
        assert_eq!(base.ignored, None);
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn an_absolute_xdg_setting_is_honoured() {
        let base = base_from(&fake(&[
            ("XDG_STATE_HOME", "/var/lib/someone"),
            ("HOME", "/home/someone"),
        ]))
        .expect("an absolute setting must resolve");
        assert_eq!(base.path.as_str(), "/var/lib/someone");
        assert_eq!(base.ignored, None);
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn no_xdg_setting_falls_back_to_the_specified_default() {
        let base = base_from(&fake(&[("HOME", "/home/someone")]))
            .expect("a set home directory must resolve");
        assert_eq!(base.path.as_str(), "/home/someone/.local/state");
        assert_eq!(base.ignored, None);
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn a_relative_xdg_setting_is_ignored_as_specified_and_said_out_loud() {
        // The specification says ignore it. Ignoring it without a word is how an operator ends up
        // looking in a directory runtrol never wrote to.
        let base = base_from(&fake(&[
            ("XDG_STATE_HOME", "relative/state"),
            ("HOME", "/home/someone"),
        ]))
        .expect("the fallback must still resolve");
        assert_eq!(base.path.as_str(), "/home/someone/.local/state");
        let ignored = base.ignored.expect("the ignored setting must be reported");
        assert_eq!(ignored.name, "XDG_STATE_HOME");
        assert_eq!(ignored.value, "relative/state");
        assert!(!ignored.why.is_empty(), "the reason has to say something");
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn an_empty_xdg_setting_takes_the_default_without_a_complaint() {
        // An empty variable is how a shell spells "unset" by accident. There is nothing for the
        // operator to fix, so there is nothing to report.
        let base = base_from(&fake(&[("XDG_STATE_HOME", ""), ("HOME", "/home/someone")]))
            .expect("the fallback must resolve");
        assert_eq!(base.path.as_str(), "/home/someone/.local/state");
        assert_eq!(base.ignored, None);
    }
}
