//! Model choices the CLI has already recorded in its own user configuration.
//!
//! This CLI has no command that enumerates the account catalogue. Its manifest therefore carries stable aliases,
//! while `~/.claude.json` may carry exact model options the provider itself learned. The file remains provider
//! owned: every discovery call opens it read-only, keeps aliases when it is absent or unreadable, and reports why
//! the result is partial.

use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::time::Duration;

use runtrol_childproc::{Containment, Program, capture};
use runtrol_provider::{
    MAX_MODEL_CHOICES, MAX_REASONING_CHOICES, ModelAliases, ModelChoice, ReasoningChoice,
};
use serde::de::{SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};

const CONFIG_FILE: &str = ".claude.json";
const HELP_DEADLINE: Duration = Duration::from_secs(10);
const MAX_REASONING_ID_BYTES: usize = 128;

#[cfg(windows)]
const OPERATOR_HOME_ENV: &str = "USERPROFILE";
#[cfg(not(windows))]
const OPERATOR_HOME_ENV: &str = "HOME";

/// The stable aliases and provider-owned options found for one request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ClaudeModelDiscovery {
    /// Stable manifest aliases, in manifest order.
    pub aliases: Vec<Box<str>>,
    /// Exact options found in the provider-owned cache, after the aliases.
    pub models: Vec<ModelChoice>,
    /// Why this is a partial answer rather than an enumerable account catalogue.
    pub why: Box<str>,
}

/// Read-only discovery for the stream-json provider's model choices.
#[derive(Clone, Debug)]
pub(super) struct ClaudeModels {
    aliases: Vec<Box<str>>,
    config: Result<PathBuf, HomeProblem>,
}

impl ClaudeModels {
    /// Resolve the provider's user configuration from the environment inherited by the CLI.
    #[must_use]
    pub(super) fn from_environment(models: ModelAliases) -> Self {
        Self {
            aliases: models.aliases,
            config: config_path_from(|name| std::env::var_os(name)),
        }
    }

    /// Read the provider-owned cache now and merge it after the stable aliases.
    #[must_use]
    pub(super) fn discover(&self) -> ClaudeModelDiscovery {
        let aliases = unique_aliases(&self.aliases);
        let remaining = MAX_MODEL_CHOICES.saturating_sub(aliases.len());
        let mut seen = aliases
            .iter()
            .map(ToString::to_string)
            .collect::<BTreeSet<_>>();

        let cache = match &self.config {
            Ok(path) => read_cache(path, remaining, &mut seen),
            Err(problem) => CacheRead::unavailable(problem.to_string()),
        };

        let why = cache.explanation().into();
        ClaudeModelDiscovery {
            aliases,
            models: cache.models,
            why,
        }
    }

    #[cfg(test)]
    fn from_home(models: ModelAliases, home: &Path) -> Self {
        Self {
            aliases: models.aliases,
            config: Ok(home.join(CONFIG_FILE)),
        }
    }
}

/// Discover the installed CLI's current reasoning choices from the same official help surface that declares the
/// bound `--effort` flag. Values are kept opaque and an unfamiliar or changed help shape degrades to no choices.
pub(super) async fn discover_reasoning_efforts(
    program: &Program,
    contained_by: &Containment,
) -> Vec<ReasoningChoice> {
    let Ok(output) = capture(program, &["--help".to_owned()], HELP_DEADLINE, contained_by).await
    else {
        return Vec::new();
    };
    if output.truncated {
        return Vec::new();
    }
    reasoning_efforts(&output.text())
}

fn reasoning_efforts(help: &str) -> Vec<ReasoningChoice> {
    let Some(flag_at) = help.find("--effort") else {
        return Vec::new();
    };
    let after_flag = &help[flag_at + "--effort".len()..];
    let Some(next_flag) = after_flag.find("\n  --") else {
        return Vec::new();
    };
    let section = &after_flag[..next_flag];
    let choices = if let Some(choices_at) = section.find("choices:") {
        &section[choices_at + "choices:".len()..]
    } else {
        let Some(open) = section.find('(') else {
            return Vec::new();
        };
        let choices = &section[open + 1..];
        if !choices.contains(',') {
            return Vec::new();
        }
        choices
    };
    let Some(close) = choices.find(')') else {
        return Vec::new();
    };
    let mut seen = BTreeSet::new();
    choices[..close]
        .split(',')
        .map(|choice| choice.trim().trim_matches('"'))
        .filter(|choice| {
            !choice.is_empty()
                && choice.len() <= MAX_REASONING_ID_BYTES
                && !choice.chars().any(char::is_control)
                && !choice.chars().any(char::is_whitespace)
                && seen.insert((*choice).to_owned())
        })
        .take(MAX_REASONING_CHOICES)
        .map(|choice| ReasoningChoice {
            id: choice.into(),
            description: Box::default(),
        })
        .collect()
}

#[derive(Clone, Debug)]
struct CacheRead {
    models: Vec<ModelChoice>,
    state: CacheState,
}

impl CacheRead {
    fn unavailable(why: impl Into<String>) -> Self {
        Self {
            models: Vec::new(),
            state: CacheState::Unavailable(why.into()),
        }
    }

    fn explanation(&self) -> String {
        let limitation = "this CLI does not enumerate its account model catalogue";
        match &self.state {
            CacheState::Found { truncated: false } => format!(
                "{limitation}; its provider-owned additionalModelOptionsCache supplied {} exact option(s), and stable manifest aliases remain available",
                self.models.len()
            ),
            CacheState::Found { truncated: true } => format!(
                "{limitation}; its provider-owned additionalModelOptionsCache exceeded the {MAX_MODEL_CHOICES}-choice bound, so only the first {} non-duplicate option(s) fit after stable manifest aliases",
                self.models.len()
            ),
            CacheState::Absent => format!(
                "{limitation}; its provider-owned additionalModelOptionsCache is absent, so only stable manifest aliases are available"
            ),
            CacheState::Unavailable(why) => format!(
                "{limitation}; its provider-owned additionalModelOptionsCache could not be read ({why}), so stable manifest aliases remain available"
            ),
        }
    }
}

#[derive(Clone, Debug)]
enum CacheState {
    Found { truncated: bool },
    Absent,
    Unavailable(String),
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClaudeConfiguration {
    #[serde(default)]
    additional_model_options_cache: Option<CachedOptions>,
}

#[derive(Debug)]
struct CachedOptions {
    options: Vec<CachedOption>,
    truncated: bool,
}

impl<'de> Deserialize<'de> for CachedOptions {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_seq(CachedOptionsVisitor)
    }
}

struct CachedOptionsVisitor;

impl<'de> Visitor<'de> for CachedOptionsVisitor {
    type Value = CachedOptions;

    fn expecting(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("an array of provider model options")
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut options = Vec::with_capacity(
            sequence
                .size_hint()
                .unwrap_or_default()
                .min(MAX_MODEL_CHOICES),
        );
        let mut truncated = false;
        while let Some(option) = sequence.next_element()? {
            if options.len() < MAX_MODEL_CHOICES {
                options.push(option);
            } else {
                truncated = true;
            }
        }
        Ok(CachedOptions { options, truncated })
    }
}

#[derive(Debug, Deserialize)]
struct CachedOption {
    value: String,
    label: String,
    description: String,
}

fn read_cache(path: &Path, capacity: usize, seen: &mut BTreeSet<String>) -> CacheRead {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return CacheRead {
                models: Vec::new(),
                state: CacheState::Absent,
            };
        }
        Err(error) => return CacheRead::unavailable(error.kind().to_string()),
    };

    let configuration: ClaudeConfiguration = match serde_json::from_reader(BufReader::new(file)) {
        Ok(configuration) => configuration,
        Err(error) => {
            return CacheRead::unavailable(format!(
                "unreadable cache data at line {} column {}",
                error.line(),
                error.column()
            ));
        }
    };
    let Some(cached) = configuration.additional_model_options_cache else {
        return CacheRead {
            models: Vec::new(),
            state: CacheState::Absent,
        };
    };

    let mut models = Vec::with_capacity(cached.options.len().min(capacity));
    let mut truncated = cached.truncated;
    for option in cached.options {
        if option.value.trim().is_empty() || option.label.trim().is_empty() {
            return CacheRead::unavailable("an option has an empty value or label");
        }
        if !seen.insert(option.value.clone()) {
            continue;
        }
        if models.len() == capacity {
            truncated = true;
            continue;
        }
        models.push(ModelChoice {
            id: option.value.into(),
            display_name: option.label.into(),
            description: option.description.into(),
            is_default: false,
            reasoning_efforts: Vec::new(),
        });
    }

    CacheRead {
        models,
        state: CacheState::Found { truncated },
    }
}

fn unique_aliases(aliases: &[Box<str>]) -> Vec<Box<str>> {
    let mut seen = BTreeSet::new();
    aliases
        .iter()
        .filter(|alias| seen.insert(alias.to_string()))
        .cloned()
        .collect()
}

#[derive(Clone, Debug)]
enum HomeProblem {
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

fn config_path_from(
    mut look: impl FnMut(&str) -> Option<OsString>,
) -> Result<PathBuf, HomeProblem> {
    let value = look(OPERATOR_HOME_ENV).ok_or(HomeProblem::Missing)?;
    if value == OsStr::new("") {
        return Err(HomeProblem::Empty);
    }
    let home = PathBuf::from(value);
    if !home.is_absolute() {
        return Err(HomeProblem::Relative(home));
    }
    Ok(home.join(CONFIG_FILE))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    static NEXT_SCRATCH: AtomicUsize = AtomicUsize::new(0);

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let serial = NEXT_SCRATCH.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "runtrol-claude-models-{}-{name}-{serial}",
                std::process::id()
            ));
            std::fs::create_dir_all(&path).expect("the test scratch directory must be creatable");
            Self(path)
        }

        fn write(&self, text: &str) {
            std::fs::write(self.0.join(CONFIG_FILE), text)
                .expect("the provider configuration fixture must be writable");
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            if let Err(error) = std::fs::remove_dir_all(&self.0) {
                assert_eq!(
                    error.kind(),
                    std::io::ErrorKind::NotFound,
                    "the test scratch directory must be removable: {error}"
                );
            }
        }
    }

    fn aliases(values: &[&str]) -> ModelAliases {
        ModelAliases {
            aliases: values
                .iter()
                .map(|value| Box::<str>::from(*value))
                .collect(),
        }
    }

    #[test]
    fn stable_aliases_precede_cached_options_and_the_first_value_wins() {
        let scratch = Scratch::new("merge");
        scratch.write(
            r#"{
                "additionalModelOptionsCache": [
                    {"value":"sonnet","label":"Duplicate alias","description":"ignored"},
                    {"value":"claude-fable-5[1m]","label":"Fable","description":"fast"},
                    {"value":"claude-fable-5[1m]","label":"Duplicate option","description":"ignored"},
                    {"value":"claude-opus-5","label":"Opus 5","description":"deep"}
                ]
            }"#,
        );

        let found =
            ClaudeModels::from_home(aliases(&["sonnet", "opus", "sonnet"]), &scratch.0).discover();

        assert_eq!(
            found.aliases,
            vec![Box::<str>::from("sonnet"), "opus".into()]
        );
        assert_eq!(
            found
                .models
                .iter()
                .map(|model| model.id.as_ref())
                .collect::<Vec<_>>(),
            ["claude-fable-5[1m]", "claude-opus-5"]
        );
        assert_eq!(
            found
                .models
                .first()
                .expect("one discovered option must exist")
                .display_name
                .as_ref(),
            "Fable"
        );
        assert!(found.why.contains("supplied 2 exact option(s)"));
    }

    #[test]
    fn an_absent_provider_file_keeps_every_alias_and_says_why() {
        let scratch = Scratch::new("absent");
        let found = ClaudeModels::from_home(aliases(&["opus", "sonnet"]), &scratch.0).discover();

        assert_eq!(
            found.aliases,
            vec![Box::<str>::from("opus"), "sonnet".into()]
        );
        assert!(found.models.is_empty());
        assert!(found.why.contains("is absent"));
    }

    #[test]
    fn broken_json_keeps_every_alias_and_exposes_the_failure() {
        let scratch = Scratch::new("broken");
        scratch.write(r#"{"additionalModelOptionsCache":["#);
        let found = ClaudeModels::from_home(aliases(&["opus", "sonnet"]), &scratch.0).discover();

        assert_eq!(
            found.aliases,
            vec![Box::<str>::from("opus"), "sonnet".into()]
        );
        assert!(found.models.is_empty());
        assert!(found.why.contains("unreadable cache data"));
    }

    #[test]
    fn a_damaged_cache_entry_does_not_replace_the_stable_fallback() {
        let scratch = Scratch::new("damaged-entry");
        scratch.write(
            r#"{"additionalModelOptionsCache":[{"value":"claude-opus-5","description":"missing label"}]}"#,
        );
        let found = ClaudeModels::from_home(aliases(&["opus"]), &scratch.0).discover();

        assert_eq!(found.aliases, vec![Box::<str>::from("opus")]);
        assert!(found.models.is_empty());
        assert!(found.why.contains("unreadable cache data"));
    }

    #[test]
    fn a_configuration_without_the_provider_cache_is_an_honest_alias_only_answer() {
        let scratch = Scratch::new("missing-field");
        scratch.write(r#"{"theme":"dark"}"#);
        let found = ClaudeModels::from_home(aliases(&["haiku"]), &scratch.0).discover();

        assert_eq!(found.aliases, vec![Box::<str>::from("haiku")]);
        assert!(found.models.is_empty());
        assert!(found.why.contains("is absent"));
    }

    #[test]
    fn every_discovery_reopens_the_provider_owned_file_without_writing_it() {
        let scratch = Scratch::new("refresh");
        let first = r#"{"additionalModelOptionsCache":[{"value":"claude-first","label":"First","description":"one"}]}"#;
        scratch.write(first);
        let discovery = ClaudeModels::from_home(aliases(&["sonnet"]), &scratch.0);

        let before = std::fs::read(scratch.0.join(CONFIG_FILE))
            .expect("the provider configuration fixture must be readable");
        let first_found = discovery.discover();
        let after = std::fs::read(scratch.0.join(CONFIG_FILE))
            .expect("the provider configuration fixture must remain readable");
        assert_eq!(
            before, after,
            "discovery must not alter provider-owned data"
        );
        assert_eq!(
            first_found
                .models
                .first()
                .expect("the first cached option must be found")
                .id
                .as_ref(),
            "claude-first"
        );

        scratch.write(
            r#"{"additionalModelOptionsCache":[{"value":"claude-second","label":"Second","description":"two"}]}"#,
        );
        let second_found = discovery.discover();
        assert_eq!(
            second_found
                .models
                .first()
                .expect("the replacement cached option must be found")
                .id
                .as_ref(),
            "claude-second"
        );
    }

    #[test]
    fn the_provider_home_must_come_from_an_absolute_runtime_environment_value() {
        let missing = config_path_from(|_| None).expect_err("a missing home must stay unknown");
        assert!(missing.to_string().contains("is not set"));

        let empty = config_path_from(|_| Some(OsString::new()))
            .expect_err("an empty home must stay unknown");
        assert!(empty.to_string().contains("is empty"));

        let relative = config_path_from(|_| Some(OsString::from("relative/home")))
            .expect_err("a relative home must not be guessed");
        assert!(relative.to_string().contains("is not an absolute path"));

        let scratch = Scratch::new("home");
        let path = config_path_from(|name| {
            assert_eq!(name, OPERATOR_HOME_ENV);
            Some(scratch.0.clone().into_os_string())
        })
        .expect("an absolute runtime home must resolve");
        assert_eq!(path, scratch.0.join(CONFIG_FILE));
    }

    #[test]
    fn reasoning_choices_come_from_the_installed_help_and_stay_opaque() {
        for help in [
            "  --effort <level>  Effort for this session\n                      (low, medium, provider-new)\n  --model <model>  Model",
            "  --effort <level>  Effort for this session\n                      (choices: low, medium, provider-new)\n  --model <model>  Model",
        ] {
            let choices = reasoning_efforts(help);
            assert_eq!(
                choices
                    .iter()
                    .map(|choice| choice.id.as_ref())
                    .collect::<Vec<_>>(),
                ["low", "medium", "provider-new"]
            );
        }
    }

    #[test]
    fn changed_or_unbounded_effort_help_degrades_to_no_choices() {
        assert!(reasoning_efforts("--effort accepts provider values").is_empty());
        assert!(
            reasoning_efforts("  --effort <level> (recommended)\n  --model <model>").is_empty()
        );
        let oversized = format!(
            "  --effort <level> (choices: {})\n  --model <model>",
            "x".repeat(MAX_REASONING_ID_BYTES + 1)
        );
        assert!(reasoning_efforts(&oversized).is_empty());
    }
}
