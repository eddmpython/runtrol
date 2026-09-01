//! What is left of the MCP registrations Runtrol once made, read and removed through the CLIs' own commands.
//!
//! Earlier builds registered this executable as an Agent Tools server and registered one CLI inside another
//! as a consultant. Neither surface exists in the product any more. What remains here is the read-only
//! inventory of those names, their removal, and the status and unwire half of the old consult toggle, all of it
//! through each CLI's own official commands. runtrol never writes a provider's configuration file, never holds
//! the registered state anywhere of its own, and registers nothing.
//!
//! # Where the truth lives
//!
//! In the registering CLI's configuration, asked for fresh with its own `get` command every time. The driver
//! reads back the command, arguments, environment, working directory, and enabled state that the CLI exposes,
//! so a matching name never becomes permission to remove somebody else's entry. A copy held here would be a
//! second place for the state to live, and the operator can change the first place without runtrol in the room.
//!
//! # Why every judgement runs against a control name
//!
//! Measured: one CLI answers a subcommand it does not have by printing its parent help and exiting zero. An
//! exit code alone therefore proves nothing. Every `get` is judged beside the same command run with an
//! invented name that must not exist. Only "the real name succeeds, the invented one fails, and the real
//! command reads back exactly" counts as ours. A CLI that answers both names the same way or changes its
//! readback shape is reported as unreadable rather than guessed about.

use core::time::Duration;

use runtrol_childproc::{Output, Program, capture};
use runtrol_drivers::{ConsultTool, McpConsultServer, McpRegistrar, McpRegistrationState};
use runtrol_ipc::wire::{
    ConsultLine, ConsultState, LegacyMcpKind, LegacyMcpLine, LegacyMcpState, Request, Response,
};
use runtrol_provider::ProviderId;

use crate::compose::Composed;
use crate::dispatch::refuse;

/// How long one wiring question may take.
///
/// The commands here are configuration reads and writes, not sessions, and the slowest measured participant
/// is a CLI cold start at under a second. The bound exists for the one that hangs.
const CONSULT_DEADLINE: Duration = Duration::from_secs(15);

/// An invented registration name that must not exist, for judging what a refusal looks like.
///
/// Same doctrine as the probe's control flags: a CLI's answer about the real name means nothing until the
/// same question about a name nobody registered is answered differently.
const CONTROL_NAME: &str = "runtrolConsultAbsentControl";

/// The stable entry coding agents see in their provider's MCP catalogue.
const AGENT_TOOLS_NAME: &str = "runtrolTools";

/// The name one CLI is registered under inside another.
///
/// Derived from the provider being served, so the operator and their agent both read what it gives them:
/// `codexConsult` is the entry that consults codex. The one constant of runtrol's own in this whole feature.
pub(crate) fn consult_name(to: ProviderId) -> String {
    format!("{to}Consult")
}

/// Answer one consult request, whichever of the three it is.
///
/// Wire and unwire answer with the same full status a plain status request gets, so a surface renders one
/// shape and never derives state on its own.
pub(crate) async fn answer(composed: &Composed, request: &Request) -> Response {
    match request {
        Request::Consult => Response::Consult(status(composed).await),
        Request::LegacyMcpInventory => {
            Response::LegacyMcpInventory(legacy_mcp_inventory(composed).await)
        }
        Request::LegacyMcpCleanup => Response::LegacyMcpCleanup(legacy_mcp_cleanup(composed).await),
        Request::ConsultUnwire { from, to } => unwire_direction(composed, from, to).await,
        Request::AgentToolsUnwire => match unwire_agent_tools(composed).await {
            Ok(()) => Response::Done,
            Err(why) => refuse(&why),
        },
        _ => refuse("consult preparation does not belong to this request"),
    }
}

/// Whether a request belongs to this module.
pub(crate) const fn is_consult(request: &Request) -> bool {
    matches!(
        request,
        Request::Consult
            | Request::LegacyMcpInventory
            | Request::LegacyMcpCleanup
            | Request::ConsultUnwire { .. }
            | Request::AgentToolsUnwire
    )
}

/// One installed provider whose official CLI can register the Agent Tools server.
struct AgentRegistrar {
    provider: Box<str>,
    program: Program,
    registrar: McpRegistrar,
}

fn agent_registrars(composed: &Composed) -> Vec<AgentRegistrar> {
    let mut targets = Vec::new();
    for provider in composed.registry.all() {
        let Some(kind) = composed.driver_for(provider.manifest.kind.as_str()) else {
            continue;
        };
        let Some(registrar) = kind.consult.registrar else {
            continue;
        };
        let Ok(program) = runtrol_core::locate(&provider.manifest) else {
            continue;
        };
        targets.push(AgentRegistrar {
            provider: provider.id().as_str().into(),
            program,
            registrar,
        });
    }
    targets
}

/// One legacy name as read, beside the CLI that answered for it.
///
/// The program and registrar are kept so that a later removal acts on exactly the catalogue that was read, not
/// on a second lookup that could land on a different installed CLI.
struct LegacyName {
    line: LegacyMcpLine,
    program: Option<Program>,
    registrar: Option<McpRegistrar>,
}

/// Read every legacy MCP name this build can inspect without changing provider or Runtrol state.
async fn legacy_mcp_inventory(composed: &Composed) -> Vec<LegacyMcpLine> {
    legacy_mcp_names(composed)
        .await
        .into_iter()
        .map(|named| named.line)
        .collect()
}

async fn legacy_mcp_names(composed: &Composed) -> Vec<LegacyName> {
    let mut names = agent_tools_inventory(composed).await;
    names.extend(cross_consult_inventory(composed).await);
    names.sort_by(|left, right| {
        let (left, right) = (&left.line, &right.line);
        legacy_kind_order(left.kind)
            .cmp(&legacy_kind_order(right.kind))
            .then_with(|| left.provider.cmp(&right.provider))
            .then_with(|| left.name.cmp(&right.name))
            .then_with(|| left.target.cmp(&right.target))
    });
    names
}

const fn legacy_kind_order(kind: LegacyMcpKind) -> u8 {
    match kind {
        LegacyMcpKind::AgentTools => 0,
        LegacyMcpKind::CrossConsult => 1,
    }
}

async fn agent_tools_inventory(composed: &Composed) -> Vec<LegacyName> {
    let server_program = this_executable();
    let expected_args = ["mcp"];
    let mut names = Vec::new();
    for target in agent_registrars(composed) {
        let state = match &server_program {
            Ok(program) => {
                exact_registration(
                    composed,
                    &target.program,
                    &target.registrar,
                    AGENT_TOOLS_NAME,
                    program.path().as_str(),
                    &expected_args,
                )
                .await
            }
            Err(why) => Err(why.clone()),
        };
        names.push(LegacyName {
            line: legacy_line(
                LegacyMcpKind::AgentTools,
                target.provider,
                AGENT_TOOLS_NAME.into(),
                None,
                state,
            ),
            program: Some(target.program),
            registrar: Some(target.registrar),
        });
    }
    names
}

async fn cross_consult_inventory(composed: &Composed) -> Vec<LegacyName> {
    let mut names = Vec::new();
    for direction in directions(composed) {
        let Some(from_kind) = composed.driver_for(direction.from.manifest.kind.as_str()) else {
            continue;
        };
        let Some(registrar) = from_kind.consult.registrar else {
            continue;
        };
        let name = consult_name(direction.to.id());
        let from_program = runtrol_core::locate(&direction.from.manifest)
            .map_err(|error| format!("cannot inspect {name} in {}: {error}", direction.from.id()));
        let (state, program) = match from_program {
            Ok(program) => (
                cross_consult_registration(composed, &direction, &program, &registrar, &name).await,
                Some(program),
            ),
            Err(why) => (Err(why), None),
        };
        names.push(LegacyName {
            line: legacy_line(
                LegacyMcpKind::CrossConsult,
                direction.from.id().as_str().into(),
                name.into(),
                Some(direction.to.id().as_str().into()),
                state,
            ),
            program,
            registrar: Some(registrar),
        });
    }
    names
}

async fn cross_consult_registration(
    composed: &Composed,
    direction: &Direction<'_>,
    from_program: &Program,
    registrar: &McpRegistrar,
    name: &str,
) -> Result<Option<McpRegistrationState>, String> {
    let Some(to_kind) = composed.driver_for(direction.to.manifest.kind.as_str()) else {
        return registration_without_expected_shape(composed, from_program, registrar, name).await;
    };
    let Some(server) = to_kind.consult.server else {
        return registration_without_expected_shape(composed, from_program, registrar, name).await;
    };
    let Ok((to_name, _)) = runtrol_core::locate_named(&direction.to.manifest) else {
        return registration_without_expected_shape(composed, from_program, registrar, name).await;
    };
    exact_registration(
        composed,
        from_program,
        registrar,
        name,
        to_name,
        server.serve,
    )
    .await
}

async fn registration_without_expected_shape(
    composed: &Composed,
    program: &Program,
    registrar: &McpRegistrar,
    name: &str,
) -> Result<Option<McpRegistrationState>, String> {
    match registration(composed, program, registrar, name).await? {
        None => Ok(None),
        Some(_) => Err(format!(
            "{name} exists, but this build cannot derive its exact expected command shape"
        )),
    }
}

fn legacy_line(
    kind: LegacyMcpKind,
    provider: Box<str>,
    name: Box<str>,
    target: Option<Box<str>>,
    result: Result<Option<McpRegistrationState>, String>,
) -> LegacyMcpLine {
    let (state, why) = match result {
        Ok(None) => (LegacyMcpState::Absent, None),
        Ok(Some(McpRegistrationState::ExactEnabled)) => (LegacyMcpState::ExactEnabled, None),
        Ok(Some(McpRegistrationState::ExactDisabled)) => (LegacyMcpState::ExactDisabled, None),
        Ok(Some(McpRegistrationState::Superseded)) => (LegacyMcpState::Superseded, None),
        Ok(Some(McpRegistrationState::Different)) => (
            LegacyMcpState::Foreign,
            Some(
                "the reserved name exists with a command shape Runtrol cannot prove it owns".into(),
            ),
        ),
        Err(why) => (LegacyMcpState::Unreadable, Some(why.into())),
    };
    LegacyMcpLine {
        kind,
        provider,
        name,
        target,
        state,
        why,
    }
}

fn this_executable() -> Result<Program, String> {
    let executable = std::env::current_exe()
        .map_err(|error| format!("cannot locate this runtrol executable: {error}"))?;
    let executable = executable
        .to_str()
        .ok_or_else(|| "this runtrol executable path is not UTF-8".to_owned())?;
    runtrol_childproc::resolve(executable).map_err(|error| error.to_string())
}

/// Whether an inventory state is one this build owns outright and may therefore remove.
///
/// Exact entries name this executable. Superseded entries name the image this executable replaced in the
/// directory it runs from. Everything else is somebody's, or cannot be told, and stays.
const fn owned_outright(state: LegacyMcpState) -> bool {
    matches!(
        state,
        LegacyMcpState::ExactEnabled | LegacyMcpState::ExactDisabled | LegacyMcpState::Superseded
    )
}

/// Remove every legacy name this build proves it owns and read each one back, leaving the rest as found.
///
/// A removal that the provider does not confirm absent is reported as unreadable with the reason, never as
/// removed. Running this twice is the same as running it once: the second pass finds every owned name absent.
async fn legacy_mcp_cleanup(composed: &Composed) -> Vec<LegacyMcpLine> {
    let mut lines = Vec::new();
    for named in legacy_mcp_names(composed).await {
        let LegacyName {
            mut line,
            program,
            registrar,
        } = named;
        if owned_outright(line.state)
            && let (Some(program), Some(registrar)) = (program, registrar)
        {
            match remove_registration(composed, &program, &registrar, &line.provider, &line.name)
                .await
            {
                Ok(()) => {
                    line.state = LegacyMcpState::Removed;
                    line.why = None;
                }
                Err(why) => {
                    line.state = LegacyMcpState::Unreadable;
                    line.why =
                        Some(format!("removal not confirmed, entry preserved: {why}").into());
                }
            }
        }
        lines.push(line);
    }
    lines
}

/// Remove the Agent Tools registration through each provider's official CLI and verify it is gone.
async fn unwire_agent_tools(composed: &Composed) -> Result<(), String> {
    let server_program = this_executable()?;
    let expected_args = ["mcp"];
    let targets = agent_registrars(composed);

    // Ownership is proved everywhere before the first mutation. A collision in one provider must not leave
    // the other provider half-unwired, and a name alone is never authority to remove somebody else's entry.
    let mut owned = Vec::new();
    for target in &targets {
        match exact_registration(
            composed,
            &target.program,
            &target.registrar,
            AGENT_TOOLS_NAME,
            server_program.path().as_str(),
            &expected_args,
        )
        .await?
        {
            None => {}
            Some(
                McpRegistrationState::ExactEnabled
                | McpRegistrationState::ExactDisabled
                | McpRegistrationState::Superseded,
            ) => {
                owned.push(target);
            }
            Some(McpRegistrationState::Different) => {
                return Err(format!(
                    "{} has an MCP entry named {AGENT_TOOLS_NAME}, but it is not this exact runtrol executable. runtrol left it untouched",
                    target.provider
                ));
            }
        }
    }

    let mut failures = Vec::new();
    for target in owned {
        if let Err(why) = remove_registration(
            composed,
            &target.program,
            &target.registrar,
            &target.provider,
            AGENT_TOOLS_NAME,
        )
        .await
        {
            failures.push(why);
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("; "))
    }
}

/// Remove one registration through the CLI's own command and read it back until the CLI says it is gone.
async fn remove_registration(
    composed: &Composed,
    program: &Program,
    registrar: &McpRegistrar,
    provider: &str,
    name: &str,
) -> Result<(), String> {
    let removed = ask(composed, program, registrar.remove, &[name]).await?;
    if registration(composed, program, registrar, name)
        .await?
        .is_some()
    {
        Err(format!(
            "{provider} still has {name} registered after removal. it said: {}",
            said(&removed)
        ))
    } else {
        Ok(())
    }
}

/// One direction that could in principle exist: an ordered pair of two declared providers.
struct Direction<'registry> {
    from: &'registry runtrol_core::Provider,
    to: &'registry runtrol_core::Provider,
}

/// Every ordered pair of distinct declared providers.
fn directions(composed: &Composed) -> Vec<Direction<'_>> {
    let mut pairs = Vec::new();
    for from in composed.registry.all() {
        for to in composed.registry.all() {
            if from.id() != to.id() {
                pairs.push(Direction { from, to });
            }
        }
    }
    pairs
}

/// What one direction needs before its registration can be read or removed, or why it never could be wired.
struct Wireable {
    registrar: McpRegistrar,
    server: McpConsultServer,
}

/// Classify one direction against what its two drivers declare.
fn wireable(composed: &Composed, direction: &Direction<'_>) -> Result<Wireable, String> {
    let from_kind = composed
        .driver_for(direction.from.manifest.kind.as_str())
        .ok_or_else(|| format!("this build has no driver for {}", direction.from.id()))?;
    let to_kind = composed
        .driver_for(direction.to.manifest.kind.as_str())
        .ok_or_else(|| format!("this build has no driver for {}", direction.to.id()))?;

    let registrar = from_kind.consult.registrar.ok_or_else(|| {
        format!(
            "the {} CLI has no official registration command this build binds",
            direction.from.id()
        )
    })?;
    let server = to_kind.consult.server.ok_or_else(|| {
        format!(
            "the {} CLI has no MCP server surface this build binds",
            direction.to.id()
        )
    })?;
    if let ConsultTool::Absent { why } = server.tool {
        return Err(why.to_owned());
    }
    Ok(Wireable { registrar, server })
}

/// The current state of every direction, asked from the CLIs' own configuration.
async fn status(composed: &Composed) -> Vec<ConsultLine> {
    let mut lines = Vec::new();
    for direction in directions(composed) {
        lines.push(line_of(composed, &direction).await);
    }
    lines
}

/// One direction's line, with the wired state asked fresh when the direction is wireable at all.
async fn line_of(composed: &Composed, direction: &Direction<'_>) -> ConsultLine {
    let line = |state: ConsultState, why: Option<String>| ConsultLine {
        from: direction.from.id().as_str().into(),
        to: direction.to.id().as_str().into(),
        state,
        why: why.map(Into::into),
    };

    let wireable = match wireable(composed, direction) {
        Ok(wireable) => wireable,
        Err(why) => return line(ConsultState::Unsupported, Some(why)),
    };
    let from_program = match runtrol_core::locate(&direction.from.manifest) {
        Ok(program) => program,
        Err(error) => return line(ConsultState::Unsupported, Some(error.to_string())),
    };
    let (to_name, _) = match runtrol_core::locate_named(&direction.to.manifest) {
        Ok(located) => located,
        Err(error) => return line(ConsultState::Unsupported, Some(error.to_string())),
    };
    let name = consult_name(direction.to.id());
    match exact_registration(
        composed,
        &from_program,
        &wireable.registrar,
        &name,
        to_name,
        wireable.server.serve,
    )
    .await
    {
        Ok(Some(McpRegistrationState::ExactEnabled)) => line(ConsultState::Wired, None),
        // Nothing of ours is wired: either there is no registration, or the one there names the image this
        // build replaced. Both are repaired the same way, by wiring again.
        Ok(Some(McpRegistrationState::Superseded) | None) => line(ConsultState::Unwired, None),
        Ok(Some(McpRegistrationState::ExactDisabled)) => line(
            ConsultState::Unsupported,
            Some(format!(
                "the {name} registration is exact but disabled in {}",
                direction.from.id()
            )),
        ),
        Ok(Some(McpRegistrationState::Different)) => line(
            ConsultState::Unsupported,
            Some(format!(
                "{} already has a different MCP entry named {name}; runtrol will not overwrite or remove it",
                direction.from.id()
            )),
        ),
        Err(why) => line(ConsultState::Unsupported, Some(why)),
    }
}

/// Read one present registration from the CLI behind `program`, judged beside the control name.
///
/// # Errors
///
/// A sentence, when the CLI's answers cannot be told apart or it could not be run at all.
async fn registration(
    composed: &Composed,
    program: &Program,
    registrar: &McpRegistrar,
    name: &str,
) -> Result<Option<Output>, String> {
    let target = get_registration(composed, program, registrar, name).await?;
    let control = get_registration(composed, program, registrar, CONTROL_NAME).await?;
    if control.succeeded() {
        // The CLI treated a name nobody registered as existing, which is what an absent subcommand looks
        // like on the CLI that answers those with its parent help. Nothing can be concluded from the target.
        return Err(format!(
            "{} answered an invented registration name as if it existed, so its configuration cannot be read",
            program.path()
        ));
    }
    Ok(target.succeeded().then_some(target))
}

async fn get_registration(
    composed: &Composed,
    program: &Program,
    registrar: &McpRegistrar,
    name: &str,
) -> Result<Output, String> {
    let words: Vec<&str> = registrar
        .get
        .iter()
        .copied()
        .chain(core::iter::once(name))
        .chain(registrar.get_suffix.iter().copied())
        .collect();
    ask(composed, program, &words, &[]).await
}

/// Classify a present registration against one exact authority-free stdio command.
async fn exact_registration(
    composed: &Composed,
    program: &Program,
    registrar: &McpRegistrar,
    name: &str,
    command: &str,
    args: &[&str],
) -> Result<Option<McpRegistrationState>, String> {
    let Some(output) = registration(composed, program, registrar, name).await? else {
        return Ok(None);
    };
    if output.truncated {
        return Err(format!(
            "{} returned a truncated registration readback for {name}, so runtrol cannot prove who owns it",
            program.path()
        ));
    }
    registrar
        .registration_state(&output.stdout, name, command, args)
        .map(Some)
        .map_err(|why| {
            format!(
                "{} returned an unreadable registration for {name}: {why}. it said: {}",
                program.path(),
                said(&output)
            )
        })
}

/// Remove one direction's registration, then answer with the same full status a status request gets.
async fn unwire_direction(composed: &Composed, from: &str, to: &str) -> Response {
    let (Ok(from_id), Ok(to_id)) = (ProviderId::parse(from), ProviderId::parse(to)) else {
        return refuse(&format!(
            "{from:?} -> {to:?} does not name two providers runtrol accepts"
        ));
    };
    let all = directions(composed);
    let Some(direction) = all
        .iter()
        .find(|direction| direction.from.id() == from_id && direction.to.id() == to_id)
    else {
        return refuse(&format!("there is no consult direction {from} -> {to}"));
    };

    let wireable = match wireable(composed, direction) {
        Ok(wireable) => wireable,
        Err(why) => return refuse(&why),
    };
    let from_program = match runtrol_core::locate(&direction.from.manifest) {
        Ok(program) => program,
        Err(error) => return refuse(&error.to_string()),
    };
    let (to_name, _) = match runtrol_core::locate_named(&direction.to.manifest) {
        Ok(located) => located,
        Err(error) => return refuse(&error.to_string()),
    };
    let name = consult_name(to_id);
    let state = match exact_registration(
        composed,
        &from_program,
        &wireable.registrar,
        &name,
        to_name,
        wireable.server.serve,
    )
    .await
    {
        Ok(state) => state,
        Err(why) => return refuse(&why),
    };
    let outcome = match state {
        // Nothing of ours is registered. The operator asked for a state, not for a transition, and that state
        // already holds.
        None => Ok(()),
        // Exact, disabled, or naming the image this build replaced: all three are this product's own entry.
        Some(
            McpRegistrationState::ExactEnabled
            | McpRegistrationState::ExactDisabled
            | McpRegistrationState::Superseded,
        ) => unwire(composed, &wireable, &from_program, &name).await,
        Some(McpRegistrationState::Different) => {
            return refuse(&format!(
                "{from} already has a different MCP entry named {name}; runtrol will not overwrite or remove it"
            ));
        }
    };
    match outcome {
        Ok(()) => Response::Consult(status(composed).await),
        Err(why) => refuse(&why),
    }
}

/// Remove the registration and confirm with the CLI's own answer that it is gone.
async fn unwire(
    composed: &Composed,
    wireable: &Wireable,
    from_program: &Program,
    name: &str,
) -> Result<(), String> {
    let removed = ask(composed, from_program, wireable.registrar.remove, &[name]).await?;
    if registration(composed, from_program, &wireable.registrar, name)
        .await?
        .is_some()
    {
        Err(format!(
            "the registration {name} is still there after removal. the CLI said: {}",
            said(&removed)
        ))
    } else {
        Ok(())
    }
}

/// Run one of the registering CLI's own commands under the consult deadline.
async fn ask(
    composed: &Composed,
    program: &Program,
    words: &[&str],
    trailing: &[&str],
) -> Result<Output, String> {
    let args: Vec<String> = words
        .iter()
        .chain(trailing)
        .map(ToString::to_string)
        .collect();
    capture(program, &args, CONSULT_DEADLINE, &composed.containment)
        .await
        .map_err(|error| error.to_string())
}

/// Cut a CLI's output down to something that fits in a refusal.
fn said(output: &Output) -> String {
    const KEEP: usize = 300;
    let text = output.text();
    let trimmed = text.trim();
    match trimmed.char_indices().nth(KEEP) {
        None => trimmed.to_owned(),
        Some((at, _)) => trimmed.get(..at).map_or_else(
            // A boundary from char_indices is always valid; keeping the whole thing rather than panicking
            // keeps a refusal a refusal.
            || trimmed.to_owned(),
            |head| format!("{head}..."),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_registration_name_says_what_it_gives_the_agent_that_reads_it() {
        // The agent sees this name in its own tool list. `codexConsult` reads as "consult codex", which is
        // the entire user-facing vocabulary of this feature: no operator ever types it.
        let codex = ProviderId::parse("codex").expect("a valid provider id");
        assert_eq!(consult_name(codex), "codexConsult");
    }

    #[test]
    fn the_control_name_is_not_a_name_this_module_would_ever_register() {
        // The control exists to be absent. A control that collided with a real registration name would read
        // every unwired direction as unreadable.
        for id in ["claude", "codex"] {
            let id = ProviderId::parse(id).expect("valid");
            assert_ne!(consult_name(id), CONTROL_NAME);
        }
    }

    #[test]
    fn the_inventory_tells_exact_foreign_absent_and_unreadable_apart() {
        let line = |result| {
            legacy_line(
                LegacyMcpKind::AgentTools,
                "fixture".into(),
                AGENT_TOOLS_NAME.into(),
                None,
                result,
            )
        };
        assert_eq!(
            line(Ok(Some(McpRegistrationState::ExactEnabled))).state,
            LegacyMcpState::ExactEnabled
        );
        assert_eq!(
            line(Ok(Some(McpRegistrationState::Different))).state,
            LegacyMcpState::Foreign
        );
        assert_eq!(line(Ok(None)).state, LegacyMcpState::Absent);
        let unreadable = line(Err("provider readback changed".to_owned()));
        assert_eq!(unreadable.state, LegacyMcpState::Unreadable);
        assert_eq!(unreadable.why.as_deref(), Some("provider readback changed"));
    }

    #[test]
    fn cleanup_removes_only_what_this_build_owns_outright() {
        for owned in [
            LegacyMcpState::ExactEnabled,
            LegacyMcpState::ExactDisabled,
            LegacyMcpState::Superseded,
        ] {
            assert!(owned_outright(owned), "{owned:?} is this build's own entry");
        }
        for preserved in [
            LegacyMcpState::Absent,
            LegacyMcpState::Foreign,
            LegacyMcpState::Unreadable,
            LegacyMcpState::Removed,
        ] {
            assert!(
                !owned_outright(preserved),
                "{preserved:?} is not proof of ownership and must be left alone"
            );
        }
    }

    #[test]
    fn inventory_and_daemon_startup_have_no_legacy_mcp_mutation_path() {
        let source = include_str!("consult.rs");
        let inventory = source
            .split("async fn legacy_mcp_inventory")
            .nth(1)
            .expect("inventory exists")
            .split("fn this_executable")
            .next()
            .expect("inventory helpers end before executable resolution");
        for forbidden in [
            "remove_registration",
            "wire_agent_tools",
            "unwire_agent_tools",
            "registrar.add",
            "registrar.remove",
        ] {
            assert!(
                !inventory.contains(forbidden),
                "read-only inventory contains mutating helper {forbidden}"
            );
        }
        let startup = include_str!("serve.rs");
        assert!(
            !startup.contains("repair_agent_tools"),
            "daemon startup must not repair a legacy MCP registration"
        );
    }

    #[test]
    fn what_a_cli_said_is_cut_down_before_it_reaches_a_refusal() {
        let output = Output {
            code: Some(1),
            stdout: vec![b'x'; 2_000],
            stderr: Vec::new(),
            truncated: false,
        };
        let cut = said(&output);
        assert!(cut.len() < 400);
        assert!(cut.ends_with("..."));
    }
}
