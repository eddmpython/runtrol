//! How a CLI takes part in cross-consult wiring, declared per driver and confirmed at runtime.
//!
//! Cross-consult registers one CLI as an MCP server inside another CLI's own configuration, using only the
//! CLIs' own official commands. runtrol never writes a provider's configuration file and never carries a word
//! of the consultation itself: the two CLIs talk to each other directly over their own stdio pipe.
//!
//! # Why this is a declared surface rather than a discovery
//!
//! The *existence* of the commands is confirmed at runtime with the CLI's own answers (exit codes against a
//! control name, and the server's own `tools/list`). The *shape* of the commands (which words, in which
//! order, which scope flag) is semantics that no probe can read mechanically, so each driver declares the
//! exact argv it binds, the same way it declares its flags. Drift is then caught where it can be caught: the
//! declared surface is exercised against the installed CLI before anything is registered.
//!
//! # Measured on this machine, 2026-08-03
//!
//! claude 2.1.220 and codex 0.146.0:
//!
//! - `codex mcp-server` answers `tools/list` with a `codex` tool that runs a session. That is a consult
//!   surface, so the codex driver names it.
//! - `claude mcp serve` answers `tools/list` with that CLI's own toolset, and the one delegating tool in it
//!   answers "Agent type not found" with an empty available list in serve context. There is no official way
//!   to ask claude for an opinion over MCP today, so the claude driver declares that absence with the
//!   measurement, and the direction shows as unsupported instead of being wired and failing mid-turn.

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
/// Two things have to hold together, and neither alone is enough. The file has to be gone, because a program
/// that is still there is still somebody's. And it has to have stood in the directory this executable runs
/// from, which is the folder the extension keeps its Core images in and nobody else writes to.
fn superseded_runtrol(registered: &str, ours: &str) -> bool {
    let registered = std::path::Path::new(registered);
    let ours = std::path::Path::new(ours);
    if registered.exists() {
        return false;
    }
    match (registered.parent(), ours.parent()) {
        (Some(theirs), Some(mine)) => !mine.as_os_str().is_empty() && theirs == mine,
        _ => false,
    }
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
    let exact = read.name == name
        && read.transport.kind == "stdio"
        && read.transport.command == command
        && read
            .transport
            .args
            .iter()
            .map(String::as_str)
            .eq(args.iter().copied())
        && read.transport.env.is_null()
        && read.transport.env_vars.is_empty()
        && read.transport.cwd.is_null();
    if !exact {
        return Ok(McpRegistrationState::Different);
    }
    if read.enabled {
        Ok(McpRegistrationState::ExactEnabled)
    } else {
        Ok(McpRegistrationState::ExactDisabled)
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

/// How one CLI serves itself as an MCP server, and which of its tools is a consultation.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct McpConsultServer {
    /// Words after the CLI's own name that start its MCP server on stdio.
    pub serve: &'static [&'static str],
    /// The tool a registered counterpart calls to get this CLI's opinion.
    pub tool: ConsultTool,
}

/// Whether a CLI's own MCP server offers a consultation tool.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ConsultTool {
    /// The tool's exact name, verified against the server's own `tools/list` before wiring.
    Named(&'static str),
    /// Measured absent, and the sentence an operator reads instead of a toggle.
    Absent {
        /// Why this CLI cannot be consulted, with the measurement that says so.
        why: &'static str,
    },
}

/// One driver's whole part in cross-consult wiring.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ConsultSurface {
    /// How this CLI registers a counterpart, when it has an official command for that.
    ///
    /// `None` means registration would require writing the CLI's configuration file directly, which runtrol
    /// refuses to do, so directions starting from this CLI show as unsupported.
    pub registrar: Option<McpRegistrar>,
    /// How this CLI serves itself, when it can be served at all.
    pub server: Option<McpConsultServer>,
}

impl ConsultSurface {
    /// A driver that takes no part in consult wiring, for kinds with no known official commands.
    pub const NONE: Self = Self {
        registrar: None,
        server: None,
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_declared_surface_is_official_commands_and_never_a_config_file() {
        // The thin boundary: registration happens through the CLI's own command surface. An argv that names a
        // configuration file would be runtrol editing another program's config, which `configReadOnly` exists
        // to refuse.
        for kind in crate::KINDS {
            let Some(registrar) = kind.consult.registrar else {
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
    fn a_server_that_cannot_be_consulted_says_why_with_a_measurement() {
        // "Unsupported" without a reason sends the operator hunting. The sentence carries the measured fact,
        // so the day the vendor ships a consult tool the declaration is one line to update.
        let mut absences = 0;
        for kind in crate::KINDS {
            if let Some(server) = kind.consult.server
                && let ConsultTool::Absent { why } = server.tool
            {
                absences += 1;
                assert!(
                    why.len() > 20,
                    "{}: an absence needs a sentence, not a code: {why:?}",
                    kind.kind
                );
            }
        }
        assert!(
            absences > 0,
            "the measured claude absence is what this test exists to keep honest"
        );
    }

    #[test]
    fn at_least_one_direction_is_wireable_in_this_build() {
        // The initiative exists because one real direction works today. A build where no driver serves a
        // named consult tool has lost that without anybody noticing.
        let served = crate::KINDS.iter().any(|kind| {
            matches!(
                kind.consult.server,
                Some(McpConsultServer {
                    tool: ConsultTool::Named(_),
                    ..
                })
            )
        });
        assert!(served, "no driver offers a consultable server");
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
}
