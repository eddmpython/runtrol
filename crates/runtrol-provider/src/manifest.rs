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

use crate::account::MAX_ACCOUNT_TOKEN_BYTES;
use crate::id::{IdError, ProviderId};

/// The manifest format this build understands.
///
/// Declared in every file so that a manifest written for a later runtrol is refused by name instead of
/// being read with today's meaning for keys that have changed.
pub const MANIFEST_SCHEMA: u32 = 1;

/// Longest glyph name a manifest may declare.
///
/// Measured against the editor's own icon registry: the longest name there is well inside this, so a name past
/// it is a mistake rather than a glyph.
const MAX_ICON_NAME: usize = 64;

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

    /// The declared glyph name is not a bare editor icon name.
    #[error("the icon name `{found}` is not a bare editor glyph name")]
    Icon {
        /// What was declared.
        found: String,
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

    /// A mode token is not a bounded plain identifier.
    ///
    /// The switchable list travels into a control request verbatim, so a token is refused unless it is the
    /// kind of short plain identifier every measured CLI actually uses.
    #[error("mode token {token:?} {why}")]
    Mode {
        /// The token as written.
        token: String,
        /// Why it was refused.
        why: &'static str,
    },

    /// An offered help command could mean more than one command.
    ///
    /// These strings are the only manifest text that reaches a person's own shell, and a manifest can come
    /// from the operator's `providers/` directory rather than from this build. So the refusal is a
    /// whitelist: a character a shell reads as a separator would turn one offered command into the visible
    /// one plus whatever followed it.
    #[error("help command {text:?} {why}")]
    HelpCommand {
        /// The text as written.
        text: String,
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

    /// A declared token (a store format, an event source, an environment name) is not a bare token.
    #[error("{what} {token:?} {why}")]
    Token {
        /// Which field.
        what: &'static str,
        /// The token as written.
        token: String,
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
    /// The editor glyph that stands for this service, by the name the editor knows it under.
    ///
    /// Declared rather than discovered, and declared here rather than chosen by a surface. A surface choosing it
    /// would mean a table of service names inside that surface, and adding a provider would stop being a matter
    /// of shipping a manifest.
    ///
    /// Names a glyph the editor already has. Measured in VS Code 1.132: its own icon font carries the official
    /// marks for several of these services (`claude`, `openai`, `xai`, `google-gemini`), and the chat extension
    /// bundled with it refers to them by name rather than shipping the artwork. Naming one is therefore how a
    /// service comes to be shown with its own mark without this repository distributing anybody's trademark.
    ///
    /// Absent when the editor has no mark for this service, and a surface then shows whatever it shows for a
    /// service it does not recognise.
    #[serde(default)]
    pub icon: Option<Box<str>>,
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
    /// The permission or approval modes this CLI accepts a mid-conversation switch to, when its own
    /// protocol cannot announce them.
    #[serde(default)]
    pub modes: ModeTokens,
    /// How this provider is likely to update itself.
    ///
    /// Optional rather than defaulted so the registry can distinguish an absent declaration from an on-disk
    /// manifest trying to claim update authority. Only declarations compiled into the product may carry this
    /// section.
    #[serde(default)]
    pub update: Option<UpdateSpec>,
    /// Where to degrade to when the primary surface is unusable.
    #[serde(default)]
    pub fallback: Option<FallbackSpec>,
    /// Where this CLI keeps its own login.
    #[serde(default)]
    pub secrets: SecretPaths,
    /// This CLI's own commands for getting itself working.
    #[serde(default)]
    pub help: HelpCommands,
    /// Where this CLI keeps the conversations it owns, and its own commands over them.
    #[serde(default)]
    pub store: StoreSpec,
    /// How to run this CLI's own terminal interface for one conversation.
    ///
    /// The conversation surface streams the CLI's own TUI, so this is what a surface needs and nothing
    /// more: the arguments for a new conversation, the arguments that reopen one by identity, and the
    /// environment the TUI expects. Absent means this provider offers no terminal surface (a bare protocol
    /// adapter), and the surface says so rather than guessing a command line.
    #[serde(default)]
    pub tui: Option<TuiSpec>,
    /// How to ask this CLI where the operator's account stands, outside any turn.
    #[serde(default)]
    pub account: Option<AccountSpec>,
    /// Where this provider's turn boundaries and activity come from.
    #[serde(default)]
    pub events: Option<EventsSpec>,
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
        if let Some(icon) = &self.icon {
            // A glyph name reaches a surface and is put straight into a stylesheet by the editor, so it is
            // bounded here the same way every other declared value is: a bare token, nothing that could be read
            // as something else.
            if icon.is_empty()
                || icon.len() > MAX_ICON_NAME
                || !icon.chars().all(|character| {
                    character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
                })
            {
                return Err(ManifestError::Icon {
                    found: icon.to_string(),
                });
            }
        }
        self.bin.validate()?;
        self.models.validate()?;
        self.modes.validate()?;
        self.secrets.validate()?;
        self.help.validate()?;
        self.store.validate()?;
        if let Some(tui) = &self.tui {
            tui.validate()?;
        }
        if let Some(account) = &self.account {
            account.validate()?;
        }
        if let Some(events) = &self.events {
            events.validate()?;
        }
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

/// The longest help command runtrol will offer a person.
///
/// Generous for a real command line and far short of anything that could hide a second one inside a text box.
const MAX_HELP_COMMAND_BYTES: usize = 200;

/// A CLI's own commands for getting itself working, offered to a person who is stuck.
///
/// # Why this is declared and not discovered
///
/// The install line is needed exactly when the executable is absent, so there is nothing to ask. The sign-in and
/// diagnose commands do exist inside an installed CLI, but the only way to learn their names is to read help text,
/// and approximating a capability from help text is what the driver contract refuses: the shape differs per vendor
/// and changes per release, and a wrong guess prints a command that silently does nothing.
///
/// It stays a declaration of **how to reach** the CLI rather than a claim about what it can do. Nothing here is
/// consulted to decide whether an operation is possible.
///
/// # What runtrol does with them
///
/// It types one into the operator's own terminal, and stops. runtrol never runs them. Fetching and executing on a
/// person's behalf is the one capability this product refused from the start, and an installer button is exactly
/// that capability wearing a helpful label. The operator reads the line and presses Enter, or does not.
///
/// This is also why the validation below is a whitelist rather than a blacklist. A manifest may come from the
/// operator's own `providers/` directory, so this is untrusted text on its way to a shell.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HelpCommands {
    /// Arguments to this CLI's own sign-in command, run against its own executable.
    ///
    /// Empty means this CLI declares none, and the surface offers nothing rather than guessing.
    #[serde(default)]
    pub sign_in: Vec<Box<str>>,
    /// Arguments to this CLI's own sign-out command, run against its own executable.
    ///
    /// Empty means this CLI declares none, and the surface offers nothing rather than guessing.
    #[serde(default)]
    pub sign_out: Vec<Box<str>>,
    /// Arguments to this CLI's own self-diagnosis command.
    ///
    /// Worth more than any check runtrol could write: the CLI knows its own installation, configuration, login and
    /// runtime health, and it is the party that stays correct when the vendor changes any of them.
    #[serde(default)]
    pub diagnose: Vec<Box<str>>,
    /// The whole command line that installs this CLI, for when no executable exists to ask.
    #[serde(default)]
    pub install: Option<Box<str>>,
}

impl HelpCommands {
    /// Refuse any text that a shell could read as more than one command.
    fn validate(&self) -> Result<(), ManifestError> {
        for argument in self
            .sign_in
            .iter()
            .chain(&self.sign_out)
            .chain(&self.diagnose)
        {
            Self::refuse_unless_one_word(argument)?;
        }
        if let Some(install) = &self.install {
            Self::refuse_unless_one_command(install)?;
        }
        Ok(())
    }

    /// One argument of a command, which never needs a space.
    fn refuse_unless_one_word(text: &str) -> Result<(), ManifestError> {
        Self::refuse_unless_safe(text, false)
    }

    /// A whole command line, which needs spaces between its words and nothing else.
    fn refuse_unless_one_command(text: &str) -> Result<(), ManifestError> {
        Self::refuse_unless_safe(text, true)
    }

    fn refuse_unless_safe(text: &str, spaces_allowed: bool) -> Result<(), ManifestError> {
        let refuse = |why: &'static str| ManifestError::HelpCommand {
            text: text.to_string(),
            why,
        };
        if text.trim().is_empty() {
            return Err(refuse("is empty, so there is nothing to offer"));
        }
        if text.len() > MAX_HELP_COMMAND_BYTES {
            return Err(refuse("is longer than any real command line"));
        }
        for character in text.chars() {
            // A whitelist, because the set of characters a shell treats specially differs between shells and
            // grows. Everything a real install or sign-in line needs is here, and a separator cannot be.
            let allowed = character.is_ascii_alphanumeric()
                || matches!(character, '.' | '_' | '-' | '/' | ':' | '=' | '@' | '+')
                || (spaces_allowed && character == ' ');
            if !allowed {
                return Err(refuse(
                    "contains a character a shell could read as the start of a second command",
                ));
            }
        }
        Ok(())
    }
}

/// This CLI's own command for listing the conversations it already owns.
///
/// # Why this may be declared
///
/// The same reason the model listing command may be. A driver's protocol may have no session enumeration method
/// while the CLI behind it has a command that does, and naming that command is declaring how to reach the CLI
/// rather than restating what it holds. Every conversation still comes from the CLI at the moment of asking.
///
/// Measured on cline 3.0.52: `cline history --json --limit N --page N` prints the sessions it owns as structured
/// JSON with pagination. Before finding it, this repository had concluded that discovering those conversations was
/// impossible without scanning provider storage, which the thin principle forbids outright. The conclusion was
/// wrong, and the way it was wrong is worth remembering: the protocol's silence was mistaken for the CLI's.
///
/// # What a driver may read from the answer
///
/// The four fields a session entry needs, and nothing else. A history record from a coding CLI contains the
/// conversation: cline's carries the operator's own prompt in full and a path to the stored messages. Reading
/// either would make Runtrol a second holder of a transcript, which is the one thing this product refuses. The
/// driver reads identity, working directory, title and timestamp, and a test asserts that nothing else survives.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StoreSpec {
    /// Directories under the operator's home where this CLI keeps its conversations, `/` separated.
    ///
    /// Declared for the same reason the login directory is: a CLI does not report where it stores what it
    /// resumes from. Named so a surface can say where a conversation lives. Empty means the CLI publishes
    /// no file store this build knows of.
    #[serde(default)]
    pub location: Vec<Box<str>>,
    /// How that store is laid out, as a bare token a driver recognises (`jsonl-per-session`,
    /// `rollout-jsonl`, `cli-history`, `protocol`). Absent means the layout is the driver's business.
    #[serde(default)]
    pub format: Option<Box<str>>,
    /// Arguments to the CLI's own session listing command, when it has one.
    ///
    /// Empty means it has none, and the driver reports the absence rather than approximating a list.
    #[serde(default)]
    pub list: Vec<Box<str>>,
    /// The flag that bounds how many conversations the listing prints, when the CLI paginates.
    ///
    /// Only the spelling lives here. How many to ask for is the driver's business and stays in code, so the
    /// number cannot drift between a manifest and the bound it is supposed to describe.
    ///
    /// # Why a driver needs it
    ///
    /// A CLI that paginates prints its own default page and says nothing about what is past it. Without a way to
    /// ask for one more than the driver intends to keep, a short answer and a truncated answer look identical,
    /// and the catalogue has no way to tell completeness from a full first page. Measured on cline 3.0.55, the
    /// default is fifty: a history longer than that reported an empty page for a root as complete.
    ///
    /// Absent means the driver cannot prove completeness and must not claim it.
    #[serde(default)]
    pub limit_flag: Option<Box<str>>,
    /// Arguments to the CLI's own command for deleting one stored conversation, when it has one.
    ///
    /// The conversation's own identifier is appended as the last argument. Empty means the CLI publishes no
    /// such command and the driver says so; it never reaches into the provider's store instead. Measured on
    /// cline 3.0.55: `cline history delete --session-id <id>`.
    #[serde(default)]
    pub delete: Vec<Box<str>>,
}

impl StoreSpec {
    /// Refuse anything a shell or an argument parser could read as something else.
    fn validate(&self) -> Result<(), ManifestError> {
        SecretPaths::validate_under_home(&self.location)?;
        if let Some(format) = &self.format {
            refuse_unless_token("store.format", format)?;
        }
        for argument in &self.list {
            HelpCommands::refuse_unless_one_word(argument)?;
        }
        if let Some(flag) = &self.limit_flag {
            HelpCommands::refuse_unless_one_word(flag)?;
        }
        for argument in &self.delete {
            HelpCommands::refuse_unless_one_word(argument)?;
        }
        Ok(())
    }
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
        Self::validate_under_home(&self.under_home)
    }

    /// The same rule for every list of home-relative directories a manifest may declare.
    fn validate_under_home(entries: &[Box<str>]) -> Result<(), ManifestError> {
        for entry in entries {
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

/// A bare token: lowercase letters, digits and `-`, bounded. What every declared classification is.
fn refuse_unless_token(what: &'static str, token: &str) -> Result<(), ManifestError> {
    let refuse = |why: &'static str| ManifestError::Token {
        what,
        token: token.to_string(),
        why,
    };
    if token.is_empty() {
        return Err(refuse("is empty"));
    }
    if token.len() > MAX_ICON_NAME {
        return Err(refuse("is longer than any token this build knows"));
    }
    if !token.chars().all(|character| {
        character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
    }) {
        return Err(refuse("must be lowercase letters, digits and '-'"));
    }
    Ok(())
}

/// How to run this CLI's own terminal interface for one conversation.
///
/// # Why this is declared and not discovered
///
/// A CLI's help text names its resume flag, but reading help to learn a flag is the approximation the driver
/// contract refuses, and getting it wrong here opens the wrong conversation in front of a person. Measured on
/// each shipped CLI and written down: that is what a manifest is for.
///
/// # What a surface does with it
///
/// It runs the program with `new` for a fresh conversation, or with `resume` followed by the conversation's own
/// identifier to reopen one, under `env`, inside a pseudo-terminal it owns, and streams that terminal to the
/// person. Nothing here interprets what the TUI draws.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TuiSpec {
    /// Arguments that open a fresh conversation in the CLI's own terminal interface. Empty means bare.
    #[serde(default)]
    pub new: Vec<Box<str>>,
    /// Arguments that reopen one conversation; its own identifier is appended last. Empty means the CLI
    /// publishes no way to reopen by identity from the command line.
    #[serde(default)]
    pub resume: Vec<Box<str>>,
    /// Environment the TUI expects, name to value, applied on top of the operator's own.
    #[serde(default)]
    pub env: std::collections::BTreeMap<Box<str>, Box<str>>,
    /// Environment removed before `env` is applied. A name, or a prefix ending in `*`.
    ///
    /// For markers the CLI leaves in its own children: a daemon started from inside one of this CLI's
    /// sessions inherits them, and the hosted TUI would read them as "you are a child of a session" and
    /// behave as one (measured 2026-08-25: Claude Code switched transcript saving off).
    #[serde(default)]
    pub env_unset: Vec<Box<str>>,
    /// Whether this TUI enables terminal mouse reporting on its own (measured). Informational: the surface's
    /// own mouse translation covers every CLI the same way regardless.
    #[serde(default)]
    pub mouse: bool,
}

impl TuiSpec {
    fn validate(&self) -> Result<(), ManifestError> {
        for argument in self.new.iter().chain(&self.resume) {
            HelpCommands::refuse_unless_one_word(argument)?;
        }
        for (name, value) in &self.env {
            let refuse = |why: &'static str| ManifestError::Token {
                what: "tui.env",
                token: name.to_string(),
                why,
            };
            if !is_environment_name(name) {
                return Err(refuse("must be an uppercase environment variable name"));
            }
            if value.chars().any(char::is_control) {
                return Err(refuse("has a value with a control character"));
            }
        }
        for pattern in &self.env_unset {
            let name = pattern.strip_suffix('*').unwrap_or(pattern);
            if !is_environment_name(name) {
                return Err(ManifestError::Token {
                    what: "tui.env_unset",
                    token: pattern.to_string(),
                    why: "must be an uppercase environment variable name, or such a prefix ending in *",
                });
            }
        }
        Ok(())
    }
}

/// An uppercase environment variable name: letters, digits and underscores, at least one character.
fn is_environment_name(name: &str) -> bool {
    !name.is_empty()
        && name.chars().all(|character| {
            character.is_ascii_uppercase() || character.is_ascii_digit() || character == '_'
        })
}

/// How to ask this CLI where the operator's account stands, outside any turn.
///
/// Either a command on the CLI's own executable that prints its status, or the name of the protocol surface
/// the driver already speaks (`app-server`, `acp`) that publishes account methods. One of the two, so a
/// manifest cannot claim both and a driver cannot guess which to trust.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AccountSpec {
    /// Arguments to the CLI's own status command, whose output the driver reads.
    #[serde(default)]
    pub status: Vec<Box<str>>,
    /// Arguments that open the CLI's own headless control channel, where it answers a usage question.
    ///
    /// Declared rather than discovered because no CLI publishes "the flags that open a machine channel";
    /// they are read out of its help text once and measured. A driver that has these asks the account's
    /// limits without a turn, which is the difference between a bar and "usage arrives with the first turn".
    #[serde(default)]
    pub usage: Vec<Box<str>>,
    /// The protocol surface that publishes the account, when the driver reads it there instead.
    #[serde(default)]
    pub protocol: Option<Box<str>>,
    /// The protocol method that answers who is signed in, and where that answer keeps each fact.
    ///
    /// For an agent whose account surface is a vendor extension of a standard protocol. The generic driver
    /// speaks the standard and knows nothing about any vendor, so the vendor's method name and the places
    /// its answer keeps things are data here rather than a branch in the driver. A new agent joins by
    /// writing this block.
    #[serde(default)]
    pub identity: Option<AccountIdentitySpec>,
    /// Each limit window this agent publishes, and where its numbers sit in the answer.
    ///
    /// Repeated, because one method's answer can describe several windows and an agent can publish them
    /// across several methods. Read only for an account the identity block reports as signed in.
    #[serde(default, rename = "window")]
    pub windows: Vec<AccountWindowSpec>,
    /// How this agent says an account has no numbers of its own to show.
    ///
    /// Some accounts are metered somewhere the operator cannot see: measured, an agent whose account
    /// belongs to a team answers about the plan and the period and states no percentage at all, and its
    /// own screen says so in words rather than standing empty. Without this the row shows a reset with no
    /// bar and no reason, which reads as a service that failed rather than one that answered.
    #[serde(default)]
    pub unmetered: Option<AccountUnmeteredSpec>,
}

/// Where an agent says the numbers are not this account's, and what to say when it does.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AccountUnmeteredSpec {
    /// The protocol method whose answer carries the fact.
    pub method: Box<str>,
    /// Where that answer keeps it. A value that is present and not false means yes.
    pub when: Box<str>,
    /// The reason, in as few words as a row has room for.
    ///
    /// Declared rather than composed, because the reason is a fact about that service's accounts and a
    /// sentence runtrol wrote would be runtrol explaining somebody else's billing.
    pub say: Box<str>,
}

/// Where one agent's own extension keeps the account's identity.
///
/// Every field but the method is a JSON Pointer (RFC 6901) into that method's result. A pointer that finds
/// nothing costs the fact it names and nothing else, so an agent that stops publishing its plan keeps its
/// sign-in state.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AccountIdentitySpec {
    /// The protocol method to call, exactly as the agent answers it.
    pub method: Box<str>,
    /// Where the answer keeps whether anybody is signed in.
    pub signed_in: Box<str>,
    /// Where it keeps the plan token, when it publishes one.
    #[serde(default)]
    pub plan: Option<Box<str>>,
    /// Where it keeps how the operator signed in, when it says.
    #[serde(default)]
    pub via: Option<Box<str>>,
}

/// One limit window an agent publishes, and where to read its numbers.
///
/// The window's length is never declared here. It is read from the agent's own two instants when it states
/// both, because an account billed monthly and one billed weekly answer the same method and a declared
/// length would be wrong for one of them.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AccountWindowSpec {
    /// The protocol method whose answer carries this window.
    pub method: Box<str>,
    /// The identity this window keeps across reports.
    pub id: Box<str>,
    /// Where the answer keeps how full the window is, as a percentage, when it publishes one.
    #[serde(default)]
    pub used_percent: Option<Box<str>>,
    /// Where it keeps when the window opened, which is half of how long the window is.
    #[serde(default)]
    pub starts_at: Option<Box<str>>,
    /// Where it keeps when the window resets.
    #[serde(default)]
    pub resets_at: Option<Box<str>>,
}

impl AccountSpec {
    /// One argument the driver hands the process spawner, never a line typed into a shell.
    ///
    /// `[help]` commands are placed in the operator's own terminal, so their characters are checked against
    /// what a shell does with them. These never meet a shell: they become argv elements directly. One of
    /// them has to be a JSON document, because a CLI that takes its configuration inline takes it as
    /// `--settings {"...":true}`, and the shell whitelist has no braces in it by design.
    ///
    /// What still matters is that one argument stays one argument, so whitespace and control characters are
    /// refused and the length is bounded exactly as before.
    fn refuse_unless_argv_token(text: &str) -> Result<(), ManifestError> {
        let refuse = |why: &'static str| ManifestError::HelpCommand {
            text: text.to_string(),
            why,
        };
        if text.trim().is_empty() {
            return Err(refuse("is empty, so there is nothing to pass"));
        }
        if text.len() > MAX_HELP_COMMAND_BYTES {
            return Err(refuse("is longer than any real argument"));
        }
        if text.chars().any(char::is_whitespace) {
            return Err(refuse(
                "carries whitespace, so it is more than one argument",
            ));
        }
        if text.chars().any(char::is_control) {
            return Err(refuse("carries a control character"));
        }
        Ok(())
    }

    /// One name that travels on a protocol line, or a refusal naming which declaration is wrong.
    ///
    /// Wider than a bare token, because a protocol extension's method name is the vendor's to choose and the
    /// measured ones carry dots, slashes and a leading underscore. Narrower than free text: bounded, and
    /// free of whitespace and control characters, so a declaration cannot smuggle a second line onto a
    /// stream that is framed by lines.
    fn refuse_unless_wire_name(what: &'static str, name: &str) -> Result<(), ManifestError> {
        let refuse = |why: &'static str| ManifestError::Token {
            what,
            token: name.to_owned(),
            why,
        };
        if name.is_empty() {
            return Err(refuse("is empty"));
        }
        if name.len() > MAX_HELP_COMMAND_BYTES {
            return Err(refuse("is longer than any protocol name this build knows"));
        }
        if name
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
        {
            return Err(refuse("carries whitespace or a control character"));
        }
        Ok(())
    }

    /// One JSON Pointer into a protocol answer, or a refusal naming which declaration is wrong.
    ///
    /// Bounded and rooted, which is the whole of RFC 6901's shape that matters here: a pointer that does
    /// not start at the root is a relative path this build has no root to resolve it against.
    fn refuse_unless_pointer(what: &'static str, pointer: &str) -> Result<(), ManifestError> {
        if !pointer.starts_with('/') || pointer.len() > MAX_HELP_COMMAND_BYTES {
            return Err(ManifestError::Token {
                what,
                token: pointer.to_owned(),
                why: "is not a rooted JSON Pointer into the answer",
            });
        }
        Ok(())
    }

    fn validate(&self) -> Result<(), ManifestError> {
        for argument in &self.status {
            HelpCommands::refuse_unless_one_word(argument)?;
        }
        for argument in &self.usage {
            Self::refuse_unless_argv_token(argument)?;
        }
        if let Some(protocol) = &self.protocol {
            refuse_unless_token("account.protocol", protocol)?;
        }
        if let Some(identity) = &self.identity {
            Self::refuse_unless_wire_name("account.identity.method", &identity.method)?;
            Self::refuse_unless_pointer("account.identity.signed_in", &identity.signed_in)?;
            for (what, pointer) in [
                ("account.identity.plan", identity.plan.as_deref()),
                ("account.identity.via", identity.via.as_deref()),
            ] {
                if let Some(pointer) = pointer {
                    Self::refuse_unless_pointer(what, pointer)?;
                }
            }
        }
        if let Some(unmetered) = &self.unmetered {
            Self::refuse_unless_wire_name("account.unmetered.method", &unmetered.method)?;
            Self::refuse_unless_pointer("account.unmetered.when", &unmetered.when)?;
            if unmetered.say.trim().is_empty() || unmetered.say.len() > MAX_ACCOUNT_TOKEN_BYTES {
                return Err(ManifestError::Token {
                    what: "account.unmetered.say",
                    token: unmetered.say.to_string(),
                    why: "is empty or longer than a row has room for",
                });
            }
        }
        if self.unmetered.is_some() && self.identity.is_none() {
            return Err(ManifestError::Token {
                what: "account",
                token: "unmetered".to_owned(),
                why: "explains an absence with no identity block to ask about, so nothing would read it",
            });
        }
        if !self.windows.is_empty() && self.identity.is_none() {
            return Err(ManifestError::Token {
                what: "account",
                token: "window".to_owned(),
                why: "declares limit windows with no identity block, so nothing knows whether to ask",
            });
        }
        for (position, window) in self.windows.iter().enumerate() {
            Self::refuse_unless_wire_name("account.window.method", &window.method)?;
            Self::refuse_unless_wire_name("account.window.id", &window.id)?;
            // An identity is what a surface keeps a row on and what two readings of one account are merged
            // by, so two windows cannot share one. Refused here rather than quietly deduplicated, because
            // the file says two things and only one of them can be what was meant.
            if self
                .windows
                .iter()
                .take(position)
                .any(|earlier| earlier.id == window.id)
            {
                return Err(ManifestError::Token {
                    what: "account.window.id",
                    token: window.id.to_string(),
                    why: "is declared by more than one window, and an identity names one limit",
                });
            }
            if window.used_percent.is_none()
                && window.starts_at.is_none()
                && window.resets_at.is_none()
            {
                return Err(ManifestError::Token {
                    what: "account.window",
                    token: window.id.to_string(),
                    why: "declares no field to read, so it could only ever draw an empty window",
                });
            }
            for (what, pointer) in [
                (
                    "account.window.used_percent",
                    window.used_percent.as_deref(),
                ),
                ("account.window.starts_at", window.starts_at.as_deref()),
                ("account.window.resets_at", window.resets_at.as_deref()),
            ] {
                if let Some(pointer) = pointer {
                    Self::refuse_unless_pointer(what, pointer)?;
                }
            }
        }
        if !self.status.is_empty() && self.protocol.is_some() {
            return Err(ManifestError::Token {
                what: "account",
                token: "status and protocol".to_owned(),
                why: "declares both a status command and a protocol surface; one is the truth",
            });
        }
        Ok(())
    }
}

/// Where this provider's turn boundaries and activity signals come from.
///
/// A token naming the surface the driver already reads (`stream-json`, `app-server`, `acp`), declared so a
/// surface that streams the TUI still knows where the sidebar's working and needs-you states are read from.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EventsSpec {
    /// The structured surface turn boundaries are read from.
    pub source: Box<str>,
}

impl EventsSpec {
    fn validate(&self) -> Result<(), ManifestError> {
        refuse_unless_token("events.source", &self.source)
    }
}

/// How to learn this providers models, when its protocol cannot say.
///
/// Two ways, and neither one is a model list written into the manifest.
///
/// `list` names the CLI's own enumeration command, for a CLI that has one outside the protocol its driver
/// speaks. Measured: the Agent Client Protocol carries no model enumeration method, and one CLI that speaks
/// it answers `models` on its own command line with the current catalogue, one identifier per line. Naming
/// that command is declaring how to reach the CLI; the answer still comes from the CLI, so it cannot go
/// stale the way a written list would.
///
/// `aliases` carries tokens for the provider whose list cannot be enumerated at all: measured, one CLI
/// answers a wrong model name with a bare error and no list. Tokens only, never model identifiers. The
/// honest response is the four alias tokens it accepts with the limitation shown as unknown, rather than an
/// invented catalogue.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ModelAliases {
    /// Arguments to this CLI's own model enumeration command, when it has one.
    ///
    /// Empty means it has none, and the driver reports the absence rather than guessing.
    #[serde(default)]
    pub list: Vec<Box<str>>,
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
        // The enumeration command reaches a process launch, so it takes the same refusal the help commands
        // take: one argument, no character a shell or an argument parser could read as something else.
        for argument in &self.list {
            HelpCommands::refuse_unless_one_word(argument)?;
        }
        Ok(())
    }
}

/// The mode tokens a CLI accepts a mid-conversation switch to, for a CLI that cannot announce them.
///
/// A manifest fact rather than discovery, with the measurement that justifies each list: one CLI names its
/// mode vocabulary only inside a refusal sentence (measured 2026-08-19: "must be one of acceptEdits, auto,
/// bypassPermissions, default, dontAsk, plan"), and another fixes it in its own generated schema. Prose in an
/// error is not a discovery surface, so the tokens live here, next to the measurement.
///
/// Deliberately NOT in any list: modes that remove safety prompts entirely. A mode that silences every
/// question is switched at the CLI's own surface by someone at the machine, never through runtrol, so for
/// runtrol it does not exist as a destination. Protocols that announce their modes per session (ACP) leave
/// this empty and the driver gates on the announcement instead.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ModeTokens {
    /// The tokens, in the order to offer them.
    #[serde(default)]
    pub switchable: Vec<Box<str>>,
}

impl ModeTokens {
    /// Bounded, plain tokens only: a mode name is a short vendor identifier, not free text.
    fn validate(&self) -> Result<(), ManifestError> {
        if self.switchable.len() > 16 {
            return Err(ManifestError::Mode {
                token: format!("{} entries", self.switchable.len()),
                why: "a mode vocabulary is a handful of tokens, not a catalogue",
            });
        }
        for token in &self.switchable {
            let refuse = |why: &'static str| ManifestError::Mode {
                token: token.to_string(),
                why,
            };
            if token.is_empty() {
                return Err(refuse("is empty"));
            }
            if token.len() > 64 {
                return Err(refuse("is longer than any real mode identifier"));
            }
            if token
                .chars()
                .any(|ch| !ch.is_ascii_alphanumeric() && ch != '-')
            {
                return Err(refuse("must be ascii letters, digits, and '-'"));
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
        assert!(manifest.update.is_none());
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
        assert_eq!(
            manifest.update.as_ref().map(|update| update.hint),
            Some(UpdateHint::Npm)
        );
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
    fn a_declared_glyph_name_is_a_bare_token_or_it_is_refused() {
        // The name reaches a surface and the editor puts it straight into a stylesheet, so it is bounded here the
        // way every other declared value is. Measured against the editor's own registry: names there are lower
        // case letters, digits and hyphens, and nothing else.
        let good = MINIMAL.to_string().replace(
            r#"display_name = "OpenCode""#,
            "display_name = \"opencode\"\nicon = \"openai\"",
        );
        let manifest = parse(&good).expect("parses");
        manifest.validate().expect("a bare glyph name is accepted");
        assert_eq!(manifest.icon.as_deref(), Some("openai"));

        for bad in [
            "Claude",
            "claude code",
            "claude~spin",
            "$(claude)",
            "../claude",
            "",
        ] {
            let text = MINIMAL.to_string().replace(
                r#"display_name = "OpenCode""#,
                &format!("display_name = \"opencode\"\nicon = \"{bad}\""),
            );
            let manifest = parse(&text).expect("the shape is still readable");
            assert!(
                matches!(manifest.validate(), Err(ManifestError::Icon { .. })),
                "accepted a glyph name that is not a bare token: {bad:?}"
            );
        }
    }

    #[test]
    fn a_manifest_that_names_no_glyph_is_still_a_manifest() {
        // Most services have no mark in the editor, and a surface shows them whatever it shows for a service it
        // does not recognise. Requiring one would mean a manifest could not be written until somebody drew an
        // icon for it.
        let manifest = parse(MINIMAL).expect("parses");
        manifest.validate().expect("no glyph is not an error");
        assert_eq!(manifest.icon, None);
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
    fn mode_tokens_are_bounded_plain_identifiers() {
        // The measured vocabularies use plain camelCase and hyphenated tokens (acceptEdits, on-request), so
        // both must pass; free text and shell characters must not travel into a control request.
        let good = format!(
            "{MINIMAL}\n[modes]\nswitchable = [\"default\", \"acceptEdits\", \"on-request\"]\n"
        );
        let manifest = parse(&good).expect("parses");
        manifest.validate().expect("plain mode tokens validate");
        assert_eq!(manifest.modes.switchable.len(), 3);

        let long = "x".repeat(65);
        for bad in ["", "a mode", "mode;rm", long.as_str()] {
            let text = format!("{MINIMAL}\n[modes]\nswitchable = [{bad:?}]\n");
            let refused = parse(&text).expect("parses").validate();
            assert!(refused.is_err(), "{bad:?} must be refused as a mode token");
        }
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
