//! How to read back, and remove, what an earlier Runtrol build registered in a CLI's own MCP catalogue.
//!
//! Earlier builds registered this executable as an Agent Tools server and registered one CLI inside another
//! as a consultant, through the CLIs' own official commands. Neither surface exists any more. What each driver
//! still declares is the exact argv of those commands and the exact command shape an earlier build wrote, so
//! the legacy cleanup can prove which entry is ours before it removes one and leave every other entry alone.
//!
//! # Why this is a declared surface rather than a discovery
//!
//! The *existence* of the commands is confirmed at runtime with the CLI's own answers (exit codes against a
//! control name). The *shape* of the commands (which words, in which order, which scope flag) is semantics
//! that no probe can read mechanically, so each driver declares the exact argv it binds, the same way it
//! declares its flags.

/// How one CLI registers and unregisters an external MCP server, using its own commands.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct McpRegistrar {
    /// Words before `<name> -- <server command...>` when registering.
    pub add: &'static [&'static str],
    /// Words before `<name>` when unregistering.
    pub remove: &'static [&'static str],
    /// Words before `<name>` when asking whether a registration exists.
    ///
    /// The exit code of this command is the ground truth for wired state. It is compared against the same
    /// command run with an invented control name, because one measured CLI answers an absent subcommand
    /// with its parent help and exit zero, so an exit code alone cannot be trusted without a control.
    pub get: &'static [&'static str],
    /// Words after `<name>` when reading one registration.
    ///
    /// Some CLIs can return a machine-readable shape only when an explicit flag follows the name. Keeping
    /// that word in the driver means the daemon does not know which CLI has that surface.
    pub get_suffix: &'static [&'static str],
    /// How this CLI's official `get` answer proves the command it will actually start.
    pub readback: McpReadback,
}

/// A provider CLI's official MCP registration readback shape.
///
/// These are deliberately output contracts rather than configuration file contracts. The provider CLI owns
/// its file and runtrol reads only the answer from the same command surface that performed the mutation.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum McpReadback {
    /// Human-readable labeled fields: `Type`, `Command`, `Args`, and a blank `Environment` section.
    LabeledText,
    /// A JSON object with a closed stdio transport containing command, arguments, environment, and cwd.
    Json,
}

/// Whether a present MCP registration is the exact authority-free stdio entry runtrol expected.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum McpRegistrationState {
    /// Exact command and arguments, no environment or cwd, and enabled when the format exposes that state.
    ExactEnabled,
    /// Exact command and arguments with no ambient authority, but explicitly disabled by the provider.
    ExactDisabled,
    /// A registration with this name exists, but it is not runtrol's exact entry.
    Different,
    /// runtrol's own entry from an earlier build: the same name, arguments and shape, naming a runtrol
    /// executable that stood beside this one and is gone.
    ///
    /// The extension installs the Core under a name taken from its contents and removes the image it
    /// replaces, so every update leaves this entry naming a file that no longer exists. Every conversation in
    /// that project then opens with "MCP client for runtrolTools failed to start: the system cannot find the
    /// file specified" (operator, 2026-08-28, with a picture). It is ours to correct, and only ours: the
    /// judgement is that the vanished command sat in the very directory this executable runs from.
    Superseded,
}

impl McpRegistrar {
    /// Classify one successful official `get` answer against the exact stdio entry runtrol expects.
    ///
    /// # Errors
    ///
    /// The answer is not valid UTF-8 or no longer has the declared output shape. Shape drift is an error,
    /// never permission to overwrite or remove an entry whose ownership cannot be proved.
    pub fn registration_state(
        &self,
        stdout: &[u8],
        name: &str,
        command: &str,
        args: &[&str],
    ) -> Result<McpRegistrationState, String> {
        match self.readback {
            McpReadback::LabeledText => labeled_registration(stdout, name, command, args),
            McpReadback::Json => json_registration(stdout, name, command, args),
        }
    }
}

fn labeled_registration(
    stdout: &[u8],
    name: &str,
    command: &str,
    args: &[&str],
) -> Result<McpRegistrationState, String> {
    let text = core::str::from_utf8(stdout)
        .map_err(|_| "the MCP registration readback was not UTF-8".to_owned())?;
    let lines: Vec<&str> = text.lines().collect();
    let kind = one_labeled_value(&lines, "Type")?;
    let read_command = one_labeled_value(&lines, "Command")?;
    let read_args = one_labeled_value(&lines, "Args")?;
    let environment_at = lines
        .iter()
        .position(|line| *line == "  Environment:")
        .ok_or_else(|| "the MCP registration readback has no Environment field".to_owned())?;
    let has_environment = lines
        .iter()
        .skip(environment_at.saturating_add(1))
        .take_while(|line| !line.trim().is_empty())
        .any(|line| !line.trim().is_empty());
    let expected_args = args.join(" ");
    let expected_heading = format!("{name}:");

    if lines.iter().any(|line| *line == expected_heading)
        && kind == "stdio"
        && read_args == expected_args
        && !has_environment
    {
        if read_command == command {
            return Ok(McpRegistrationState::ExactEnabled);
        }
        if superseded_runtrol(read_command, command) {
            return Ok(McpRegistrationState::Superseded);
        }
    }
    Ok(McpRegistrationState::Different)
}

/// Whether a registered command is this runtrol's own earlier image rather than somebody else's program.
///
/// Three things have to hold together, and no two of them are enough.
///
/// The file has to be gone, because a program that is still there is still somebody's. It has to have stood in
/// the directory this executable runs from, which is the folder the extension keeps its Core images in. And it
/// has to be named the way the extension names those images: `runtrol-<digest>` plus this platform's
/// extension, differing from ours only in the digest. Without the name, an absent program that merely happened
/// to sit beside us would be taken over, which is somebody else's entry (caught by
/// `labeled_readback_proves_only_the_exact_authority_free_entry`, 2026-08-28).
fn superseded_runtrol(registered: &str, ours: &str) -> bool {
    let registered = std::path::Path::new(registered);
    let ours = std::path::Path::new(ours);
    if registered.exists() {
        return false;
    }
    let (Some(theirs), Some(mine)) = (registered.parent(), ours.parent()) else {
        return false;
    };
    if mine.as_os_str().is_empty() || theirs != mine {
        return false;
    }
    match (registered.file_name(), ours.file_name()) {
        (Some(theirs), Some(mine)) => {
            managed_image_name(&theirs.to_string_lossy())
                && managed_image_name(&mine.to_string_lossy())
        }
        _ => false,
    }
}

/// Whether a file name is one the extension gives a Core image: `runtrol-<hex digest>`, and on Windows `.exe`.
///
/// The digest is what changes between one image and the next, so the name is the only part of the path that
/// says "this was an image of ours" once the file itself is gone.
fn managed_image_name(name: &str) -> bool {
    let stem = name.strip_suffix(".exe").unwrap_or(name);
    let Some(digest) = stem.strip_prefix("runtrol-") else {
        return false;
    };
    !digest.is_empty()
        && digest
            .chars()
            .all(|character| character.is_ascii_hexdigit())
}

fn one_labeled_value<'line>(lines: &[&'line str], label: &str) -> Result<&'line str, String> {
    let prefix = format!("  {label}:");
    let mut values = lines.iter().filter_map(|line| {
        line.strip_prefix(&prefix)
            .map(|value| value.strip_prefix(' ').unwrap_or(value))
    });
    let value = values
        .next()
        .ok_or_else(|| format!("the MCP registration readback has no {label} field"))?;
    if values.next().is_some() {
        return Err(format!(
            "the MCP registration readback has more than one {label} field"
        ));
    }
    Ok(value)
}

fn json_registration(
    stdout: &[u8],
    name: &str,
    command: &str,
    args: &[&str],
) -> Result<McpRegistrationState, String> {
    let read: JsonRegistration = serde_json::from_slice(stdout)
        .map_err(|error| format!("the MCP registration JSON could not be read: {error}"))?;
    let owned_shape = read.name == name
        && read.transport.kind == "stdio"
        && read
            .transport
            .args
            .iter()
            .map(String::as_str)
            .eq(args.iter().copied())
        && read.transport.env.is_null()
        && read.transport.env_vars.is_empty()
        && read.transport.cwd.is_null();
    if !owned_shape {
        return Ok(McpRegistrationState::Different);
    }
    if read.transport.command == command && read.enabled {
        Ok(McpRegistrationState::ExactEnabled)
    } else if read.transport.command == command {
        Ok(McpRegistrationState::ExactDisabled)
    } else if read.enabled && superseded_runtrol(&read.transport.command, command) {
        Ok(McpRegistrationState::Superseded)
    } else {
        Ok(McpRegistrationState::Different)
    }
}

#[derive(serde::Deserialize)]
struct JsonRegistration {
    name: String,
    enabled: bool,
    transport: JsonTransport,
}

#[derive(serde::Deserialize)]
struct JsonTransport {
    #[serde(rename = "type")]
    kind: String,
    command: String,
    args: Vec<String>,
    env: serde_json::Value,
    env_vars: Vec<String>,
    cwd: serde_json::Value,
}

/// What an earlier build could have registered through this CLI, declared so the legacy cleanup can read it back.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct LegacyMcpSurface {
    /// How this CLI reads and removes a registration, when it has official commands for that.
    ///
    /// `None` means the CLI's configuration could only be read by opening its file directly, which runtrol
    /// refuses to do, so nothing of this CLI's catalogue is inspected or touched.
    pub registrar: Option<McpRegistrar>,
    /// The words an earlier build wrote after this CLI's own name to register it as a consultant inside another
    /// CLI. Only an entry with exactly that shape is recognised as ours.
    pub consult_serve: Option<&'static [&'static str]>,
}

impl LegacyMcpSurface {
    /// A driver whose CLI has no official commands this build binds.
    pub const NONE: Self = Self {
        registrar: None,
        consult_serve: None,
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_registration_is_ours_again_only_when_its_image_is_gone_from_our_own_folder() {
        let folder = std::env::temp_dir().join("runtrolSupersededTest");
        std::fs::create_dir_all(&folder).expect("the test folder");
        let ours = folder.join("runtrol-abc123.exe");
        let older = folder.join("runtrol-def456.exe");
        let stranger = folder.join("someoneElse.exe");
        std::fs::write(&ours, b"x").expect("our image");
        std::fs::write(&stranger, b"x").expect("their image");
        if older.exists() {
            std::fs::remove_file(&older).expect("clear the replaced image");
        }

        // The image the extension replaced: gone, and it stood where we stand.
        assert!(superseded_runtrol(
            older.to_str().expect("path"),
            ours.to_str().expect("path")
        ));
        // A program that is still there is still somebody's, even in our folder.
        assert!(!superseded_runtrol(
            stranger.to_str().expect("path"),
            ours.to_str().expect("path")
        ));
        // Gone, but from a folder we never write to: not ours to take over.
        let elsewhere = std::env::temp_dir()
            .join("someOtherPlace")
            .join("runtrol-abc123.exe");
        assert!(!superseded_runtrol(
            elsewhere.to_str().expect("path"),
            ours.to_str().expect("path")
        ));
        // Gone, and beside us, but not named the way the extension names our images. Somebody else's program
        // that happened to sit in the same folder is still somebody else's.
        let stranger_gone = folder.join("other.exe");
        assert!(!superseded_runtrol(
            stranger_gone.to_str().expect("path"),
            ours.to_str().expect("path")
        ));
        // The name has to be the whole shape, digest and all: a bare stem is not one of ours.
        let bare = folder.join("runtrol.exe");
        assert!(!superseded_runtrol(
            bare.to_str().expect("path"),
            ours.to_str().expect("path")
        ));

        std::fs::remove_file(&ours).expect("clean up our image");
        std::fs::remove_file(&stranger).expect("clean up their image");
    }

    #[test]
    fn every_declared_surface_is_official_commands_and_never_a_config_file() {
        // The thin boundary: reading and removing a registration happens through the CLI's own command surface.
        // An argv that names a configuration file would be runtrol editing another program's config, which
        // `configReadOnly` exists to refuse.
        for kind in crate::KINDS {
            let Some(registrar) = kind.legacy_mcp.registrar else {
                continue;
            };
            for word in registrar
                .add
                .iter()
                .chain(registrar.remove)
                .chain(registrar.get)
                .chain(registrar.get_suffix)
            {
                assert!(
                    !word.contains(".json") && !word.contains(".toml"),
                    "{}: {word:?} looks like a config file, not a command",
                    kind.kind
                );
            }
        }
    }

    #[test]
    fn the_consultant_shape_earlier_builds_wrote_is_still_declared_so_cleanup_can_recognise_it() {
        // An earlier build registered `<cli> <consult_serve...>` inside the other CLI. Cleanup proves an entry is
        // ours by that exact shape. Lose the declaration and every such entry becomes foreign and stays forever.
        let mut declared = 0;
        for kind in crate::KINDS {
            let Some(serve) = kind.legacy_mcp.consult_serve else {
                continue;
            };
            declared += 1;
            assert!(
                !serve.is_empty(),
                "{}: an empty consultant shape matches nothing",
                kind.kind
            );
            for word in serve {
                assert!(
                    !word.contains('/') && !word.contains('\\') && !word.contains(".json"),
                    "{}: {word:?} is a path or a file, not a word after the CLI's own name",
                    kind.kind
                );
            }
        }
        assert!(
            declared >= 2,
            "both certified CLIs were registered as consultants by earlier builds; {declared} declared"
        );
    }

    #[test]
    fn labeled_readback_proves_only_the_exact_authority_free_entry() {
        let registrar = McpRegistrar {
            add: &[],
            remove: &[],
            get: &[],
            get_suffix: &[],
            readback: McpReadback::LabeledText,
        };
        let exact = "runtrolTools:\n  Status: connected\n  Type: stdio\n  Command: C:\\runtrol.exe\n  Args: mcp\n  Environment:\n\nTo remove this server, run: remove\n";
        assert_eq!(
            registrar
                .registration_state(
                    exact.as_bytes(),
                    "runtrolTools",
                    "C:\\runtrol.exe",
                    &["mcp"],
                )
                .expect("declared shape"),
            McpRegistrationState::ExactEnabled
        );

        for different in [
            exact.replace("runtrolTools:", "somebodyElse:"),
            exact.replace("C:\\runtrol.exe", "C:\\other.exe"),
            exact.replace("  Args: mcp", "  Args: mcp --root anywhere"),
            exact.replace("  Environment:\n\n", "  Environment:\n    TOKEN=secret\n\n"),
        ] {
            assert_eq!(
                registrar
                    .registration_state(
                        different.as_bytes(),
                        "runtrolTools",
                        "C:\\runtrol.exe",
                        &["mcp"],
                    )
                    .expect("declared shape"),
                McpRegistrationState::Different
            );
        }
    }

    #[test]
    fn json_readback_distinguishes_exact_disabled_and_foreign_entries() {
        let registrar = McpRegistrar {
            add: &[],
            remove: &[],
            get: &[],
            get_suffix: &["--json"],
            readback: McpReadback::Json,
        };
        let readback = |enabled: bool, command: &str, env: serde_json::Value| {
            serde_json::to_vec(&serde_json::json!({
                "name": "runtrolTools",
                "enabled": enabled,
                "transport": {
                    "type": "stdio",
                    "command": command,
                    "args": ["mcp"],
                    "env": env,
                    "env_vars": [],
                    "cwd": null
                }
            }))
            .expect("JSON")
        };
        assert_eq!(
            registrar
                .registration_state(
                    &readback(true, "runtrol", serde_json::Value::Null),
                    "runtrolTools",
                    "runtrol",
                    &["mcp"],
                )
                .expect("declared shape"),
            McpRegistrationState::ExactEnabled
        );
        assert_eq!(
            registrar
                .registration_state(
                    &readback(false, "runtrol", serde_json::Value::Null),
                    "runtrolTools",
                    "runtrol",
                    &["mcp"],
                )
                .expect("declared shape"),
            McpRegistrationState::ExactDisabled
        );
        assert_eq!(
            registrar
                .registration_state(
                    &readback(true, "other", serde_json::json!({"TOKEN": "secret"})),
                    "runtrolTools",
                    "runtrol",
                    &["mcp"],
                )
                .expect("declared shape"),
            McpRegistrationState::Different
        );
    }

    #[test]
    fn json_readback_recognizes_only_a_vanished_managed_image_as_superseded() {
        let registrar = McpRegistrar {
            add: &[],
            remove: &[],
            get: &[],
            get_suffix: &["--json"],
            readback: McpReadback::Json,
        };
        let readback = |enabled: bool, command: &str| {
            serde_json::to_vec(&serde_json::json!({
                "name": "runtrolTools",
                "enabled": enabled,
                "transport": {
                    "type": "stdio",
                    "command": command,
                    "args": ["mcp"],
                    "env": null,
                    "env_vars": [],
                    "cwd": null
                }
            }))
            .expect("JSON")
        };
        let folder =
            std::env::temp_dir().join(format!("runtrolJsonSupersededTest{}", std::process::id()));
        std::fs::create_dir_all(&folder).expect("the test folder");
        let current = folder.join("runtrol-abc123.exe");
        let replaced = folder.join("runtrol-def456.exe");
        std::fs::write(&current, b"current").expect("the current managed image");
        if replaced.exists() {
            std::fs::remove_file(&replaced).expect("clear the replaced image");
        }
        let current = current.to_str().expect("the current path");
        let replaced = replaced.to_str().expect("the replaced path");
        assert_eq!(
            registrar
                .registration_state(&readback(true, replaced), "runtrolTools", current, &["mcp"],)
                .expect("declared shape"),
            McpRegistrationState::Superseded,
            "Codex JSON readback must recognize the vanished Core image an update replaced"
        );
        assert_eq!(
            registrar
                .registration_state(
                    &readback(false, replaced),
                    "runtrolTools",
                    current,
                    &["mcp"],
                )
                .expect("declared shape"),
            McpRegistrationState::Different,
            "a disabled entry remains the provider's explicit choice"
        );

        std::fs::write(replaced, b"still live").expect("the previous image is still live");
        assert_eq!(
            registrar
                .registration_state(&readback(true, replaced), "runtrolTools", current, &["mcp"],)
                .expect("declared shape"),
            McpRegistrationState::Different,
            "a live sibling image is not ours to replace"
        );
        std::fs::remove_file(current).expect("clean up the current image");
        std::fs::remove_file(replaced).expect("clean up the previous image");
        std::fs::remove_dir(folder).expect("clean up the test folder");
    }
}
