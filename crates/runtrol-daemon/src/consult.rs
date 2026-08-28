//! Wiring one CLI into another as a consultant, through the CLIs' own commands and nothing else.
//!
//! One toggle stands between an operator and "my agent can ask the other vendor's agent mid-turn". What the
//! toggle actually does is small on purpose: it runs the registering CLI's own official add or remove
//! command, and it asks the CLIs' own answers for every judgement it makes. runtrol never writes a provider's
//! configuration file, never holds the wired state anywhere of its own, and never sees a word of the
//! consultation, which travels on a stdio pipe between the two CLIs.
//!
//! # Where the truth lives
//!
//! In the registering CLI's configuration, asked for fresh with its own `get` command every time. The driver
//! reads back the command, arguments, environment, working directory, and enabled state that the CLI exposes,
//! so a matching name never becomes permission to overwrite or remove somebody else's entry. A copy held here
//! would be a second place for the state to live, and the operator can change the first place without runtrol in
//! the room.
//!
//! # Why every judgement runs against a control name
//!
//! Measured: one CLI answers a subcommand it does not have by printing its parent help and exiting zero. An
//! exit code alone therefore proves nothing. Every `get` is judged beside the same command run with an
//! invented name that must not exist. Only "the real name succeeds, the invented one fails, and the real
//! command reads back exactly" counts as wired. A CLI that answers both names the same way or changes its
//! readback shape is reported as unreadable rather than guessed about.
//!
//! # Why the server is asked for its tool list before anything is registered
//!
//! The direction's whole point is a named tool on the counterpart's own MCP server. The name is declared by
//! the driver and verified against a fresh `tools/list` handshake at wire time, so a vendor rename becomes a
//! refusal at the toggle instead of a turn that fails somewhere an operator cannot see the cause.

use core::time::Duration;

use runtrol_childproc::{Output, Program, capture, capture_with_input};
use runtrol_drivers::{ConsultTool, McpConsultServer, McpRegistrar, McpRegistrationState};
use runtrol_ipc::wire::{ConsultLine, ConsultState, Request, Response};
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

/// The `tools/list` question, written whole and closed, the shape [`capture_with_input`] exists for.
///
/// Measured: both handshake answers arrive and the server exits cleanly on end of input.
const TOOLS_LIST_HANDSHAKE: &[u8] = br#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"runtrol","version":"0"}}}
{"jsonrpc":"2.0","method":"notifications/initialized"}
{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}
"#;

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
        Request::ConsultWire { from, to } => change(composed, from, to, Change::Wire).await,
        Request::ConsultUnwire { from, to } => change(composed, from, to, Change::Unwire).await,
        Request::AgentToolsWire => match wire_agent_tools(composed).await {
            Ok(()) => Response::Done,
            Err(why) => refuse(&why),
        },
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
            | Request::ConsultWire { .. }
            | Request::ConsultUnwire { .. }
            | Request::AgentToolsWire
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

fn this_executable() -> Result<Program, String> {
    let executable = std::env::current_exe()
        .map_err(|error| format!("cannot locate this runtrol executable: {error}"))?;
    let executable = executable
        .to_str()
        .ok_or_else(|| "this runtrol executable path is not UTF-8".to_owned())?;
    runtrol_childproc::resolve(executable).map_err(|error| error.to_string())
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
        if let Err(why) = remove_registration(composed, target, AGENT_TOOLS_NAME).await {
            failures.push(why);
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("; "))
    }
}

/// Register this exact executable's bounded Agent Tools server in every usable CLI registrar.
/// Point a registration that is ours, and stale, back at the image now running.
///
/// # Why this runs on its own
///
/// The extension installs the Core under a name made from its digest, so every update puts this program at a
/// new path and deletes the one it replaced. The registration written at wiring time still names the old path,
/// and from then on every conversation in that project opens with `MCP client for runtrolTools failed to
/// start` and the file-not-found the operating system gives for a program that is gone (operator's window,
/// 2026-08-28: measured, the registered image was absent from the folder the current one runs from). Nothing
/// asked for that, and nothing was going to fix it: wiring only happens when a person presses the toggle, and
/// they already pressed it.
///
/// # What it will not do
///
/// It repairs and never creates. A provider with no entry keeps none, because an entry is a person's decision.
/// Anything but [`McpRegistrationState::Superseded`] is left exactly as it stands, which is the same ownership
/// rule the toggle holds: the file has to be gone, and it has to have stood where this executable stands.
pub(crate) async fn repair_agent_tools(composed: &Composed) -> Vec<String> {
    let server_program = match this_executable() {
        Ok(program) => program,
        Err(why) => return vec![format!("the running Core could not name itself: {why}")],
    };
    let expected_args = ["mcp"];
    let mut trouble = Vec::new();
    for target in agent_registrars(composed) {
        let state = exact_registration(
            composed,
            &target.program,
            &target.registrar,
            AGENT_TOOLS_NAME,
            server_program.path().as_str(),
            &expected_args,
        )
        .await;
        match state {
            Ok(Some(McpRegistrationState::Superseded)) => {}
            Ok(_) => continue,
            Err(why) => {
                trouble.push(format!("{}: {why}", target.provider));
                continue;
            }
        }
        if let Err(why) = remove_registration(composed, &target, AGENT_TOOLS_NAME).await {
            trouble.push(format!("{}: {why}", target.provider));
            continue;
        }
        let mut add: Vec<&str> = target.registrar.add.to_vec();
        add.extend([
            AGENT_TOOLS_NAME,
            "--",
            server_program.path().as_str(),
            "mcp",
        ]);
        if let Err(why) = ask(composed, &target.program, &add, &[]).await {
            trouble.push(format!(
                "{}: the stale entry was removed and the new one could not be added: {why}",
                target.provider
            ));
        }
    }
    trouble
}

async fn wire_agent_tools(composed: &Composed) -> Result<(), String> {
    let server_program = this_executable()?;
    verify_agent_tools(composed, &server_program).await?;
    let targets = agent_registrars(composed);
    if targets.is_empty() {
        return Err(
            "no installed provider CLI exposes an official MCP registration command in this build"
                .to_owned(),
        );
    }

    let expected_args = ["mcp"];
    let mut missing = Vec::new();
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
            None => missing.push(target),
            Some(McpRegistrationState::ExactEnabled) => {}
            Some(McpRegistrationState::Superseded) => {
                // Ours, from the image this one replaced. Take it out and let the add below put this
                // executable in its place, so the project stops opening every conversation with a failure.
                remove_registration(composed, target, AGENT_TOOLS_NAME).await?;
                missing.push(target);
            }
            Some(McpRegistrationState::ExactDisabled) => {
                return Err(format!(
                    "{} has this exact {AGENT_TOOLS_NAME} entry disabled. enable or remove it in that CLI before retrying",
                    target.provider
                ));
            }
            Some(McpRegistrationState::Different) => {
                return Err(format!(
                    "{} already has an MCP entry named {AGENT_TOOLS_NAME} that points somewhere else. runtrol will not overwrite it",
                    target.provider
                ));
            }
        }
    }

    let mut added: Vec<&AgentRegistrar> = Vec::new();
    for target in missing {
        let mut add: Vec<&str> = target.registrar.add.to_vec();
        add.extend([
            AGENT_TOOLS_NAME,
            "--",
            server_program.path().as_str(),
            "mcp",
        ]);
        let add_output = match ask(composed, &target.program, &add, &[]).await {
            Ok(output) => output,
            Err(why) => {
                return Err(with_agent_tools_rollback(composed, &added, why).await);
            }
        };
        match exact_registration(
            composed,
            &target.program,
            &target.registrar,
            AGENT_TOOLS_NAME,
            server_program.path().as_str(),
            &expected_args,
        )
        .await
        {
            Ok(Some(McpRegistrationState::ExactEnabled)) => added.push(target),
            Ok(state) => {
                let why = format!(
                    "{} did not register the exact enabled Agent Tools entry ({state:?}). it said: {}",
                    target.provider,
                    said(&add_output)
                );
                return Err(with_agent_tools_rollback(composed, &added, why).await);
            }
            Err(why) => {
                return Err(with_agent_tools_rollback(composed, &added, why).await);
            }
        }
    }
    Ok(())
}

async fn with_agent_tools_rollback(
    composed: &Composed,
    added: &[&AgentRegistrar],
    why: String,
) -> String {
    let mut rollback_failures = Vec::new();
    for target in added.iter().rev() {
        if let Err(error) = remove_registration(composed, target, AGENT_TOOLS_NAME).await {
            rollback_failures.push(error);
        }
    }
    if rollback_failures.is_empty() {
        why
    } else {
        format!(
            "{why}; rollback also failed: {}",
            rollback_failures.join("; ")
        )
    }
}

async fn remove_registration(
    composed: &Composed,
    target: &AgentRegistrar,
    name: &str,
) -> Result<(), String> {
    let removed = ask(composed, &target.program, target.registrar.remove, &[name]).await?;
    if registration(composed, &target.program, &target.registrar, name)
        .await?
        .is_some()
    {
        Err(format!(
            "{} still has {name} registered after removal. it said: {}",
            target.provider,
            said(&removed)
        ))
    } else {
        Ok(())
    }
}

/// Ask the exact local MCP server for its catalogue before any provider configuration changes.
async fn verify_agent_tools(composed: &Composed, program: &Program) -> Result<(), String> {
    let output = capture_with_input(
        program,
        &["mcp".to_owned()],
        TOOLS_LIST_HANDSHAKE,
        CONSULT_DEADLINE,
        &composed.containment,
    )
    .await
    .map_err(|error| error.to_string())?;
    let names = tools_named(&output.stdout);
    if names.iter().any(|name| name == "runtrol_start")
        && names.iter().any(|name| name == "runtrol_send")
    {
        return Ok(());
    }
    Err(format!(
        "this runtrol executable did not expose the required Agent Tools catalogue. it said: {}",
        said(&output)
    ))
}

/// Which way a toggle is being flipped.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Change {
    Wire,
    Unwire,
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

/// What one direction needs before it can be wired, or the sentence saying why it never can be.
struct Wireable {
    registrar: McpRegistrar,
    server: McpConsultServer,
    tool: &'static str,
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
    let tool = match server.tool {
        ConsultTool::Named(tool) => tool,
        ConsultTool::Absent { why } => return Err(why.to_owned()),
    };
    Ok(Wireable {
        registrar,
        server,
        tool,
    })
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

/// Flip one direction, then answer with the same full status a status request gets.
async fn change(composed: &Composed, from: &str, to: &str, how: Change) -> Response {
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
    let (to_name, to_program) = match runtrol_core::locate_named(&direction.to.manifest) {
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
    let already = match state {
        Some(McpRegistrationState::ExactEnabled) => true,
        // Nothing registered, or one naming the image this build replaced: both are treated as absent so the
        // wiring below writes this executable in its place.
        None | Some(McpRegistrationState::Superseded) => false,
        Some(McpRegistrationState::ExactDisabled) => {
            return refuse(&format!(
                "the {name} registration is exact but disabled in {from}"
            ));
        }
        Some(McpRegistrationState::Different) => {
            return refuse(&format!(
                "{from} already has a different MCP entry named {name}; runtrol will not overwrite or remove it"
            ));
        }
    };

    let outcome = match how {
        // Flipping to where it already stands is success, not an error: the operator asked for a state, not
        // for a transition, and refusing would make the toggle order-sensitive for no one's benefit.
        Change::Wire if already => Ok(()),
        Change::Unwire if !already => Ok(()),
        Change::Wire => {
            wire(
                composed,
                direction,
                &wireable,
                &from_program,
                &name,
                to_name,
                &to_program,
            )
            .await
        }
        Change::Unwire => unwire(composed, &wireable, &from_program, &name).await,
    };
    match outcome {
        Ok(()) => Response::Consult(status(composed).await),
        Err(why) => refuse(&why),
    }
}

/// Register `direction.to` inside `direction.from`, verifying the consult tool first and the result after.
async fn wire(
    composed: &Composed,
    direction: &Direction<'_>,
    wireable: &Wireable,
    from_program: &Program,
    name: &str,
    to_name: &str,
    to_program: &Program,
) -> Result<(), String> {
    // The counterpart's server command, as it will be written into the registering CLI's configuration: the
    // candidate name rather than a resolved path, because a path goes stale on the counterpart's next update
    // while the name keeps meaning "whatever is installed".
    verify_consult_tool(composed, to_program, &wireable.server, wireable.tool).await?;

    let mut add: Vec<&str> = wireable.registrar.add.to_vec();
    add.push(name);
    add.push("--");
    add.push(to_name);
    add.extend_from_slice(wireable.server.serve);
    let added = ask(composed, from_program, &add, &[]).await?;

    // The add's own exit code is not the judgement, because the CLI that answers absent subcommands with
    // help also exits zero for them. The get that follows is.
    match exact_registration(
        composed,
        from_program,
        &wireable.registrar,
        name,
        to_name,
        wireable.server.serve,
    )
    .await?
    {
        Some(McpRegistrationState::ExactEnabled) => Ok(()),
        state => Err(format!(
            "{} did not register the exact enabled {name} entry ({state:?}). it said: {}",
            direction.from.id(),
            said(&added)
        )),
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

/// Ask the counterpart's own server for its tool list and confirm the declared consult tool is in it.
async fn verify_consult_tool(
    composed: &Composed,
    to_program: &Program,
    server: &McpConsultServer,
    tool: &str,
) -> Result<(), String> {
    let args: Vec<String> = server.serve.iter().map(ToString::to_string).collect();
    let output = capture_with_input(
        to_program,
        &args,
        TOOLS_LIST_HANDSHAKE,
        CONSULT_DEADLINE,
        &composed.containment,
    )
    .await
    .map_err(|error| error.to_string())?;

    if tools_named(&output.stdout).iter().any(|name| name == tool) {
        return Ok(());
    }
    Err(format!(
        "{} no longer offers a tool called {tool:?} on its own server, so wiring it would fail mid-turn. \
         it said: {}",
        to_program.path(),
        said(&output)
    ))
}

/// Every tool name in a `tools/list` answer.
///
/// Reads the server's own structured protocol answer and takes names only, the same rule the probe applies
/// to help text: names are stable and descriptions are prose. A line that is not the answer is skipped, so a
/// server that logs on standard output does not break the reading.
#[expect(
    clippy::manual_ok_err,
    reason = "the equivalent Result::ok is forbidden because dropped errors must stay visible: a line that \
              does not parse is a server's log noise, and the caller reports the raw output when no answer \
              is found"
)]
fn tools_named(stdout: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(stdout)
        .lines()
        .filter_map(
            |line| match serde_json::from_str::<serde_json::Value>(line.trim()) {
                Ok(answer) => Some(answer),
                Err(_) => None,
            },
        )
        .filter(|answer| answer.get("id").and_then(serde_json::Value::as_u64) == Some(2))
        .filter_map(|answer| {
            let tools = answer.get("result")?.get("tools")?.as_array()?.clone();
            Some(tools)
        })
        .flatten()
        .filter_map(|tool| {
            tool.get("name")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        })
        .collect()
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
    fn the_repair_only_ever_touches_a_registration_that_is_ours_and_gone() {
        // Read as source, because what has to hold here is which states act and which are left alone: a
        // repair that created an entry would be runtrol deciding something a person decides, and a repair
        // that took over a `Different` entry would be runtrol overwriting somebody else's.
        let source = include_str!("consult.rs");
        let body = source
            .split("pub(crate) async fn repair_agent_tools")
            .nth(1)
            .expect("the repair exists")
            .split(
                "
async fn wire_agent_tools",
            )
            .next()
            .expect("the repair ends before the toggle");
        assert!(
            body.contains("Ok(Some(McpRegistrationState::Superseded)) => {}"),
            "only a superseded entry is repaired"
        );
        assert!(
            body.contains("Ok(_) => continue,"),
            "every other state is left exactly as it stands"
        );
        assert!(
            !body.contains("None =>"),
            "an absent entry stays absent: an entry is a person's decision"
        );
    }

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
    fn tool_names_are_read_from_the_answer_and_log_noise_is_skipped() {
        let stdout = b"warning: something on stdout\n\
            {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"serverInfo\":{\"name\":\"x\"}}}\n\
            {\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{\"tools\":[{\"name\":\"codex\",\"description\":\"prose\"},{\"name\":\"codex-reply\"}]}}\n";
        let names = tools_named(stdout);
        assert_eq!(names, vec!["codex".to_owned(), "codex-reply".to_owned()]);
    }

    #[test]
    fn an_answer_that_is_not_a_tool_list_yields_no_names_rather_than_a_panic() {
        for stdout in [
            &b""[..],
            b"not json at all",
            br#"{"jsonrpc":"2.0","id":2,"error":{"code":-1,"message":"no"}}"#,
            br#"{"jsonrpc":"2.0","id":1,"result":{"tools":[{"name":"wrong-id"}]}}"#,
        ] {
            assert!(tools_named(stdout).is_empty(), "{stdout:?}");
        }
    }

    #[test]
    fn the_handshake_asks_for_the_tool_list_and_nothing_that_starts_work() {
        // The handshake reaches a real CLI's server. Three requests, none of which is `tools/call`: a wiring
        // check that started a turn would cost the operator money for a question about configuration.
        let text = core::str::from_utf8(TOOLS_LIST_HANDSHAKE).expect("the handshake is UTF-8");
        assert!(text.contains("\"initialize\""));
        assert!(text.contains("\"tools/list\""));
        assert!(!text.contains("tools/call"), "{text}");
        assert!(
            text.ends_with('\n'),
            "the last line must be complete, or the server waits for its end"
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
