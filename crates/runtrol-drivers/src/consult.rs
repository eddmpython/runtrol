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
}
