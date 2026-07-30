//! What a provider declares about itself. The schema, not the loader.
//!
//! Registering a CLI with runtrol is one TOML file. A CLI that already speaks a protocol runtrol has a
//! driver for needs no Rust at all, and adding one never edits the kernel. That is the whole point of this
//! file existing separately from anything that reads it.
//!
//! # The hard rule: if it can be discovered, it may not be declared
//!
//! A manifest says **how to reach** a CLI. It never says what that CLI can do. No capability lists, no
//! model identifiers, no flag tables, no event mappings. Those are probed at runtime or compiled into a
//! driver, and a manifest that restated one would be a hardcoded fact wearing a data file's clothes: it
//! would be stale the first time the vendor shipped a release, and nothing would notice.
//!
//! The key set below is therefore closed, and every structure refuses keys it does not know. An operator
//! who mistypes a key gets a message naming the line rather than a setting that silently did nothing.
//!
//! # Why declaration and code both exist
//!
//! A pure TOML dialect cannot express stream framing, session identifier extraction, interrupt semantics,
//! or an approval round trip without growing into a programming language. A pure trait would demand a Rust
//! crate and a recompile to add a CLI that already speaks a protocol runtrol supports. So the manifest
//! declares the reachable facts and names a `kind`, and the kind selects code that already exists.
//!
//! # What is deliberately not a type here
//!
//! [`Kind`] is validated text and not an enumeration. The set of kinds belongs to whatever ships drivers,
//! and an enumeration here would put every provider's name in the contract crate, which is precisely the
//! coupling the kind indirection exists to prevent.

use core::fmt;

use serde::{Deserialize, Deserializer, Serialize};

use crate::id::{IdError, ProviderId};

/// The manifest format this build understands.
///
/// Declared in every file so that a manifest written for a later runtrol is refused by name instead of
/// being read with today's meaning for keys that have changed.
pub const MANIFEST_SCHEMA: u32 = 1;

/// A manifest did not describe something runtrol can use.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ManifestError {
    /// The file declares a format version this build does not read.
    #[error("manifest format {found} is not the {understood} this build reads")]
    Schema {
        /// What the file said.
        found: u32,
        /// What this build reads.
        understood: u32,
    },

    /// A field runtrol needs was present and empty.
    ///
    /// Told apart from a missing field on purpose: an empty list is something somebody wrote, and the
    /// message has to say that rather than claim the key is absent.
    #[error("{field} is present and empty, and runtrol needs at least one entry")]
    Empty {
        /// Which field.
        field: &'static str,
    },

    /// A binary name is not a bare file name.
    ///
    /// A manifest is data, and data that reaches a process launch is the one place a hostile file could
    /// try to name a program of its own choosing. A name with a path separator in it is refused here so
    /// that resolution can only ever consult the operator's own search path.
    #[error("binary name {name:?} {why}")]
    BinaryName {
        /// The name as written.
        name: String,
        /// Why it was refused.
        why: &'static str,
    },

    /// A model alias is not a bare token.
    ///
    /// Aliases are tokens like `opus`, never model identifiers. A model identifier in a manifest is a fact
    /// that goes stale, and the whole point of the alias list is to carry what cannot be discovered
    /// without carrying what can.
    #[error("model alias {token:?} {why}")]
    Alias {
        /// The alias as written.
        token: String,
        /// Why it was refused.
        why: &'static str,
    },

    /// A declared secret directory is not a plain path under the home.
    ///
    /// The wall refuses any workspace overlapping these, so an entry that reached outside the home would be a
    /// manifest deciding what the wall protects rather than saying where its own login lives.
    #[error("secret path {path:?} {why}")]
    SecretPath {
        /// The path as written.
        path: String,
        /// Why it was refused.
        why: &'static str,
    },

    /// An identifier in the file is not one runtrol accepts.
    #[error(transparent)]
    Id(#[from] IdError),
}

/// Which driver serves a provider.
///
/// Validated text rather than an enumeration, because the set of values belongs to whatever ships drivers
/// and not to this crate. The character set is narrow for the same reason a provider id's is: this value
/// selects code and appears in error messages, and an identifier that can hold whitespace or a path
/// separator is a hostile manifest's first foothold.
///
/// `:` is permitted for the one shape that needs it, a driver implemented outside this repository and
/// named `native:<something>`.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct Kind(Box<str>);

impl Kind {
    /// Longest kind runtrol accepts.
    ///
    /// Room for the longest shape that exists: `native:` plus a provider id at its own maximum.
    pub const MAX_LEN: usize = 47;

    /// Name of this value, as it appears in [`IdError`] messages.
    pub const WHAT: &'static str = "kind";

    /// Description of the permitted character set, for error messages.
    const ALLOWED: &'static str = "lowercase ascii letters, digits, '-', and ':'";

    /// Validate text as a kind.
    ///
    /// # Errors
    ///
    /// [`IdError::Empty`] for empty text, [`IdError::TooLong`] past [`Self::MAX_LEN`],
    /// [`IdError::Charset`] outside `[a-z0-9-:]`, [`IdError::Shape`] for a leading or trailing separator.
    pub fn parse(text: &str) -> Result<Self, IdError> {
        if text.is_empty() {
            return Err(IdError::Empty { what: Self::WHAT });
        }
        if text.len() > Self::MAX_LEN {
            return Err(IdError::TooLong {
                what: Self::WHAT,
                len: text.len(),
                max: Self::MAX_LEN,
            });
        }
        for (at, ch) in text.char_indices() {
            if !matches!(ch, 'a'..='z' | '0'..='9' | '-' | ':') {
                return Err(IdError::Charset {
                    what: Self::WHAT,
                    allowed: Self::ALLOWED,
                    found: ch,
                    at,
                });
            }
        }
        if text.starts_with(['-', ':']) || text.ends_with(['-', ':']) {
            return Err(IdError::Shape {
                what: Self::WHAT,
                why: "must not start or end with '-' or ':'",
            });
        }
        Ok(Self(text.into()))
    }

    /// The kind as text, for a table lookup.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Kind({})", self.0)
    }
}

impl core::str::FromStr for Kind {
    type Err = IdError;

    fn from_str(text: &str) -> Result<Self, IdError> {
        Self::parse(text)
    }
}

impl<'de> Deserialize<'de> for Kind {
    /// Decoding runs the same validation as [`Kind::parse`], so a file cannot introduce a kind the
    /// constructor would have refused. The format's own error carries the line number, which is what makes
    /// this worth doing at the field rather than afterwards.
    fn deserialize<D: Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        let text = String::deserialize(de)?;
        Self::parse(&text).map_err(serde::de::Error::custom)
    }
}

/// Everything a provider declares about itself.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// The manifest format version.
    pub schema: u32,
    /// The stable identifier this provider is known by.
    pub id: ProviderId,
    /// What to call it in front of a person.
    pub display_name: Box<str>,
    /// Which driver serves it.
    pub kind: Kind,
    /// How to find its executable.
    pub bin: BinSpec,
    /// How to ask it its version.
    #[serde(default)]
    pub probe: ProbeSpec,
    /// How to reach its structured surface.
    #[serde(default)]
    pub transport: TransportSpec,
    /// Model alias tokens that cannot be discovered.
    #[serde(default)]
    pub models: ModelAliases,
    /// How this provider is likely to update itself.
    #[serde(default)]
    pub update: UpdateSpec,
    /// Where to degrade to when the primary surface is unusable.
    #[serde(default)]
    pub fallback: Option<FallbackSpec>,
    /// Where this CLI keeps its own login.
    #[serde(default)]
    pub secrets: SecretPaths,
}

impl Manifest {
    /// Check what the field types could not.
    ///
    /// Field validation happens while decoding, so that the format's own error can name the line. What is
    /// left is the whole-file questions: the format version, and lists that are present and empty.
    ///
    /// # Errors
    ///
    /// [`ManifestError::Schema`] for a version this build does not read, [`ManifestError::Empty`] for an
    /// empty list runtrol needs, [`ManifestError::BinaryName`] for a name that is not a bare file name,
    /// [`ManifestError::Alias`] for an alias that is not a bare token.
    pub fn validate(&self) -> Result<(), ManifestError> {
        if self.schema != MANIFEST_SCHEMA {
            return Err(ManifestError::Schema {
                found: self.schema,
                understood: MANIFEST_SCHEMA,
            });
        }
        if self.display_name.trim().is_empty() {
            return Err(ManifestError::Empty {
                field: "display_name",
            });
        }
        self.bin.validate()?;
        self.models.validate()?;
        self.secrets.validate()?;
        Ok(())
    }
}

/// How to find a provider's executable.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BinSpec {
    /// Candidate file names, in the order to try them.
    ///
    /// Bare names only. Resolution consults the operator's own search path, so a manifest cannot point at
    /// a program of its own choosing.
    pub names: Vec<Box<str>>,
}

impl BinSpec {
    /// Refuse an empty list and any name that is not a bare file name.
    fn validate(&self) -> Result<(), ManifestError> {
        if self.names.is_empty() {
            return Err(ManifestError::Empty { field: "bin.names" });
        }
        for name in &self.names {
            let refuse = |why: &'static str| ManifestError::BinaryName {
                name: name.to_string(),
                why,
            };
            if name.is_empty() {
                return Err(refuse("is empty"));
            }
            if name.contains(['/', '\\']) {
                return Err(refuse(
                    "must be a bare file name, so that only the operator's search path decides what runs",
                ));
            }
            if name.starts_with('-') {
                return Err(refuse(
                    "must not start with '-', which a shell reads as an option",
                ));
            }
            if name.contains(|ch: char| ch.is_whitespace() || ch.is_control()) {
                return Err(refuse("must not contain whitespace or a control character"));
            }
        }
        Ok(())
    }
}

/// How to ask a provider its version.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProbeSpec {
    /// The version query.
    #[serde(default)]
    pub version: VersionProbe,
    /// How to ask this CLI whether it knows a flag, without letting it start work.
    #[serde(default)]
    pub flags: Option<FlagProbe>,
}

/// How to ask a CLI about its own flags safely.
///
/// # Why this cannot be discovered
///
/// Everything else in a probe is discoverable by asking. This one is not, because asking is the thing being made
/// safe: to find out whether a CLI knows a flag, the flag has to be offered to it, and offering a flag to a CLI
/// that is willing to start work means starting work. A turn costs the operator money and, worse, appears in
/// their session history as something they did not ask for.
///
/// So a manifest names the arguments that make this particular CLI refuse to do anything. Measured on one of
/// them: with its print flag and no input, every argument combination fails at parsing, which is exactly the
/// state a flag question needs.
///
/// # Why asking is worth the trouble at all
///
/// Measured on this machine, version 2.1.220 of one CLI: `--permission-prompt-tool` **exists and is absent from
/// its own help output**. Confirmed with a control group, asking the parser rather than reading the text: the real
/// flag answers "argument missing" and an invented one answers "unknown option".
///
/// A capability check that read help would conclude the flag is absent and quietly disable approval prompts, and
/// the operator would never find out why their phone stopped asking them things. Reading help is the fallback
/// rung; this is the accurate one.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FlagProbe {
    /// Arguments that make this CLI refuse to do any work.
    ///
    /// Every flag question is asked with these in front of it, so the answer is always a parse failure and never
    /// a turn.
    pub safe_with: Vec<Box<str>>,
}

/// The one question runtrol asks before anything else.
///
/// A version is the key the whole probe cache is stored under, so it is asked separately from everything
/// else and must never start a turn or cost money.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct VersionProbe {
    /// Arguments that make the provider print its version.
    pub args: Vec<Box<str>>,
    /// How to read the answer.
    #[serde(default)]
    pub parse: VersionParse,
}

impl Default for VersionProbe {
    /// The near-universal spelling, so most manifests can leave the block out entirely.
    fn default() -> Self {
        Self {
            args: vec!["--version".into()],
            parse: VersionParse::default(),
        }
    }
}

/// How to read a version out of whatever a provider prints.
///
/// Named in the manifest rather than assumed, so that adding a second strategy does not silently change
/// how every manifest already on disk is read.
///
/// Exhaustive, unlike [`Listen`]. The rule is who matches on it: a driver written outside this repository
/// legitimately serves some transports and not others, so [`Listen`] must be able to grow without breaking
/// one. Nothing outside runtrol reads a version, so a new strategy here has to be a compile error in the
/// kernel rather than a wildcard arm nobody notices.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum VersionParse {
    /// Take the first thing anywhere in the output that looks like a version.
    ///
    /// Providers wrap their version in a banner, a name, a build hash, or nothing at all, and the position
    /// changes between releases. Finding it anywhere is the only reading that survives that.
    #[default]
    SemverAnywhere,
}

/// How to reach a provider's structured surface.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TransportSpec {
    /// Extra arguments that put the provider into its structured mode.
    #[serde(default)]
    pub argv: Vec<Box<str>>,
    /// Where the structured conversation happens.
    #[serde(default)]
    pub listen: Listen,
}

/// Where a provider's structured conversation happens.
///
/// One value, because one value is what any driver can serve. The design notes also list a local socket
/// and a websocket; they are absent until something can honour them, because a key runtrol accepts and
/// cannot act on is worse than one it refuses by name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum Listen {
    /// The child's own standard input and output.
    ///
    /// Uniform across platforms and needs no socket to clean up, which is why it is the only one so far.
    #[default]
    Stdio,
}

/// Where a provider keeps its own login.
///
/// # Why this is declared and not discovered
///
/// Because there is nothing to ask. A CLI does not report where it stores its credentials, and guessing from its
/// name is how a wall comes to protect a directory nobody uses while leaving the real one open. This is the case
/// the manifest exists for: not discoverable, and the cost of being wrong is a credential.
///
/// # What it is for
///
/// runtrol never holds a provider's credential. That promise is worth nothing if runtrol will happily approve a
/// workspace that lets an agent read one, so the scope wall refuses any root overlapping these. A provider added
/// by shipping a manifest is therefore covered the moment it exists, with nothing in the security crate to edit,
/// which is the whole reason this is a manifest key rather than a list somewhere in that crate.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SecretPaths {
    /// Directories relative to the operator's home, written with `/` as the separator.
    #[serde(default)]
    pub under_home: Vec<Box<str>>,
}

impl SecretPaths {
    /// Refuse anything that is not a plain relative directory under the home.
    ///
    /// An absolute path, a parent step, or an empty entry would each let a manifest name a directory outside the
    /// operator's home, which is a manifest deciding what the wall protects rather than declaring where its own
    /// login lives.
    fn validate(&self) -> Result<(), ManifestError> {
        for entry in &self.under_home {
            let refuse = |why: &'static str| ManifestError::SecretPath {
                path: entry.to_string(),
                why,
            };
            if entry.is_empty() {
                return Err(refuse("an empty path names the home directory itself"));
            }
            if entry.starts_with(['/', '\\']) || entry.contains(':') {
                return Err(refuse("this has to be relative to the home directory"));
            }
            if entry.split(['/', '\\']).any(|part| part == "..") {
                return Err(refuse(
                    "a step upwards would name something outside the home directory",
                ));
            }
        }
        Ok(())
    }
}

/// Model alias tokens.
///
/// Tokens only, never model identifiers. This block exists for the provider whose model list cannot be
/// enumerated at all: measured, one CLI answers a wrong model name with a bare error and no list. The
/// honest response is to carry the four alias tokens it accepts and show the limitation as unknown, rather
/// than to invent a catalogue that goes stale.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ModelAliases {
    /// The tokens, in the order to offer them.
    #[serde(default)]
    pub aliases: Vec<Box<str>>,
}

impl ModelAliases {
    /// Refuse anything that is a model identifier rather than an alias token.
    fn validate(&self) -> Result<(), ManifestError> {
        for token in &self.aliases {
            let refuse = |why: &'static str| ManifestError::Alias {
                token: token.to_string(),
                why,
            };
            if token.is_empty() {
                return Err(refuse("is empty"));
            }
            // A dated or versioned name is a model identifier, and a model identifier in a manifest is a
            // fact that expires. The alias is the part that does not.
            if token.contains(|ch: char| ch.is_ascii_digit()) {
                return Err(refuse(
                    "looks like a model identifier. aliases are bare tokens, and identifiers are discovered",
                ));
            }
            if token.contains(|ch: char| !ch.is_ascii_lowercase() && ch != '-') {
                return Err(refuse("must be lowercase ascii letters and '-'"));
            }
        }
        Ok(())
    }
}

/// How a provider is likely to keep itself current.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateSpec {
    /// The hint.
    #[serde(default)]
    pub hint: UpdateHint,
}

/// Where a provider's updates come from.
///
/// A hint and not an instruction. Whatever performs an update probes first and believes what it finds; this
/// only says where to look, which is a fact about how the CLI was installed rather than about its version.
///
/// Exhaustive for the same reason as [`VersionParse`]: only runtrol's own updater reads it, so a new channel
/// has to be a compile error rather than a case that silently does nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum UpdateHint {
    /// The provider updates itself.
    ///
    /// Spelled `self` in the file. The variant cannot be, because that is a keyword.
    #[serde(rename = "self")]
    SelfManaged,
    /// A package manager installed it.
    Npm,
    /// Nothing updates it.
    #[default]
    None,
}

/// Where to go when the primary surface is unusable.
///
/// Declared rather than inferred, because the alternative is runtrol guessing a degraded mode, and a guess
/// that lands on a surface with different semantics is worse than a refusal. Reachable only when a
/// capability probe rejects the primary surface, and the degradation is always announced.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FallbackSpec {
    /// Which driver serves the degraded surface.
    pub kind: Kind,
    /// Extra arguments to enter it.
    #[serde(default)]
    pub argv: Vec<Box<str>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shortest manifest that is complete, as a fourth CLI's author would write it.
    const MINIMAL: &str = r#"
schema = 1
id = "opencode"
display_name = "OpenCode"
kind = "acp"
[bin]
names = ["opencode"]
[transport]
argv = ["acp"]
"#;

    fn parse(text: &str) -> Result<Manifest, toml::de::Error> {
        toml::from_str(text)
    }

    #[test]
    fn a_cli_that_already_speaks_a_supported_protocol_needs_nine_lines() {
        // The claim the whole design rests on: no Rust, no kernel change, one small file. If this ever
        // stops being true, the manifest has grown into a program.
        let manifest = parse(MINIMAL).expect("the minimal manifest must parse");
        manifest.validate().expect("and must validate");

        assert_eq!(manifest.id.as_str(), "opencode");
        assert_eq!(manifest.kind.as_str(), "acp");
        assert_eq!(manifest.bin.names.len(), 1);
        assert_eq!(
            manifest.transport.argv,
            vec!["acp".into()],
            "the one thing this CLI needs is an argument"
        );
        assert_eq!(
            manifest.probe.version.args,
            vec!["--version".into()],
            "the version query defaults, because it is the same nearly everywhere"
        );
        assert_eq!(manifest.transport.listen, Listen::Stdio);
        assert_eq!(manifest.update.hint, UpdateHint::None);
        assert!(manifest.fallback.is_none());
    }

    #[test]
    fn every_declared_key_is_readable() {
        // The complete key set in one file, so that a key silently dropped from the schema is caught here
        // rather than by an operator whose setting stopped having an effect.
        let full = r#"
schema = 1
id = "codex"
display_name = "OpenAI Codex CLI"
kind = "codex-app-server"

[bin]
names = ["codex", "codex.cmd", "codex.exe"]

[probe]
version = { args = ["--version"], parse = "semver-anywhere" }

[transport]
argv = ["app-server"]
listen = "stdio"

[models]
aliases = ["opus", "sonnet"]

[update]
hint = "npm"

[fallback]
kind = "exec-oneshot"
argv = ["exec", "--json"]
"#;
        let manifest = parse(full).expect("the full manifest must parse");
        manifest.validate().expect("and must validate");

        assert_eq!(manifest.bin.names.len(), 3);
        assert_eq!(manifest.probe.version.parse, VersionParse::SemverAnywhere);
        assert_eq!(manifest.models.aliases.len(), 2);
        assert_eq!(manifest.update.hint, UpdateHint::Npm);
        let fallback = manifest.fallback.expect("declared");
        assert_eq!(fallback.kind.as_str(), "exec-oneshot");
    }

    #[test]
    fn a_key_runtrol_does_not_know_is_refused_rather_than_ignored() {
        // The failure this prevents: an operator writes a setting, runtrol ignores it, and nothing anywhere
        // says so. Every table in the schema refuses unknown keys for the same reason, so the top level is
        // checked separately from the tables. A key appended after `MINIMAL` would land inside its last
        // table rather than at the top level, which would leave the top level untested.
        let top_level = format!("capabilities = [\"streaming\"]\n{MINIMAL}");
        assert!(
            parse(&top_level).is_err(),
            "the top level accepted a key it does not know"
        );

        for bad in [
            "[models]\nids = [\"some-model\"]",
            "[bin]\nnames = [\"x\"]\npath = \"/usr/bin/thing\"",
            "[transport]\nframing = \"ndjson\"",
            "[probe]\nlatency = 3",
            "[update]\nchannel = \"beta\"",
            "[fallback]\nkind = \"other\"\nreason = \"why\"",
        ] {
            let text = format!("{MINIMAL}\n{bad}\n");
            assert!(
                parse(&text).is_err(),
                "accepted a key it does not know: {bad}"
            );
        }
    }

    #[test]
    fn a_manifest_written_for_a_later_runtrol_is_refused_by_name() {
        let text = MINIMAL.replace("schema = 1", "schema = 2");
        let manifest = parse(&text).expect("the shape is still readable");
        assert_eq!(
            manifest.validate(),
            Err(ManifestError::Schema {
                found: 2,
                understood: MANIFEST_SCHEMA,
            })
        );
    }

    #[test]
    fn a_binary_name_cannot_name_a_program_of_its_own_choosing() {
        // A manifest is data, and this is the one field where data reaches a process launch. Only the
        // operator's own search path may decide what runs.
        for bad in [
            r#"names = ["/usr/local/bin/evil"]"#,
            r#"names = ["..\\evil.exe"]"#,
            r#"names = ["-rf"]"#,
            r#"names = ["two words"]"#,
            r#"names = [""]"#,
            "names = []",
        ] {
            let text = MINIMAL.replace(r#"names = ["opencode"]"#, bad);
            let manifest = parse(&text).expect("the shape parses");
            assert!(
                manifest.validate().is_err(),
                "accepted a binary name it should refuse: {bad}"
            );
        }
    }

    #[test]
    fn a_model_identifier_cannot_hide_in_the_alias_list() {
        // An identifier in a manifest is a fact that expires, and the alias list is exactly the place
        // somebody would put one because it looks like it belongs.
        //
        // The message matters as much as the refusal, and is asserted for that reason. A digit is what makes
        // a token an identifier rather than an alias, so it gets the sentence that names the mistake; the
        // character set check behind it would refuse the same token with a message about lowercase letters,
        // which tells an operator nothing about why their model name does not belong here.
        for bad in ["claude-opus-5", "gpt-5", "opus-20260730"] {
            let text = format!("{MINIMAL}\n[models]\naliases = [\"{bad}\"]\n");
            let manifest = parse(&text).expect("the shape parses");
            match manifest.validate() {
                Err(ManifestError::Alias { token, why }) => {
                    assert_eq!(token, bad);
                    assert!(
                        why.contains("model identifier"),
                        "{bad:?} was refused with an unhelpful reason: {why}"
                    );
                }
                other => panic!("accepted {bad:?} as an alias: {other:?}"),
            }
        }

        for bad in ["Opus", "opus latest", "opus_fast", ""] {
            let text = format!("{MINIMAL}\n[models]\naliases = [\"{bad}\"]\n");
            let manifest = parse(&text).expect("the shape parses");
            assert!(
                matches!(manifest.validate(), Err(ManifestError::Alias { .. })),
                "accepted {bad:?} as an alias"
            );
        }
    }

    #[test]
    fn an_alias_token_is_accepted() {
        let text = format!("{MINIMAL}\n[models]\naliases = [\"opus\", \"sonnet\", \"haiku\"]\n");
        let manifest = parse(&text).expect("parses");
        manifest
            .validate()
            .expect("bare tokens are what this is for");
    }

    #[test]
    fn a_kind_is_validated_text_and_not_an_enumeration() {
        // The kind selects code, so its character set is narrow. It is not an enumeration, because the set
        // of values belongs to whatever ships drivers rather than to this crate.
        assert!(Kind::parse("codex-app-server").is_ok());
        assert!(
            Kind::parse("native:my-thing").is_ok(),
            "the one shape with a colon"
        );

        for bad in [
            "",
            "Codex",
            "codex app server",
            "codex/../evil",
            "-codex",
            "codex:",
        ] {
            assert!(Kind::parse(bad).is_err(), "accepted kind {bad:?}");
        }
        assert!(Kind::parse(&"k".repeat(Kind::MAX_LEN + 1)).is_err());
    }

    #[test]
    fn a_kind_reports_itself_for_a_lookup_and_for_a_message() {
        let kind = Kind::parse("claude-stream-json").expect("valid");
        assert_eq!(kind.as_str(), "claude-stream-json");
        assert_eq!(kind.to_string(), "claude-stream-json");
        assert_eq!(format!("{kind:?}"), "Kind(claude-stream-json)");
    }

    #[test]
    fn a_bad_field_is_reported_with_the_line_it_is_on() {
        // The operator is editing a file. A message that names the field but not the line makes them
        // search, and the whole reason the field types validate while decoding is to avoid that.
        let text = MINIMAL.replace(r#"id = "opencode""#, r#"id = "OpenCode""#);
        let error = parse(&text).expect_err("an uppercase id must be refused");
        let message = error.to_string();
        assert!(message.contains("provider id"), "{message}");
        assert!(
            message.contains("line") || message.contains('|'),
            "the message must place the error in the file: {message}"
        );
    }

    #[test]
    fn a_manifest_round_trips_through_its_own_schema() {
        // The built-in manifests are compiled in as text and the loader reads them with this schema. If
        // encoding and decoding disagreed, a built-in could parse into something it does not say.
        let manifest = parse(MINIMAL).expect("parses");
        let encoded = toml::to_string(&manifest).expect("serializable");
        let again: Manifest = toml::from_str(&encoded).expect("re-readable");
        assert_eq!(manifest, again);
    }
}
