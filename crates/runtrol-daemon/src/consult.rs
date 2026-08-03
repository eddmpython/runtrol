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
//! In the registering CLI's configuration, asked for fresh with its own `get` command every time. A copy held
//! here would be a second place for the state to live, and the operator can change the first place without
//! runtrol in the room.
//!
//! # Why every judgement runs against a control name
//!
//! Measured: one CLI answers a subcommand it does not have by printing its parent help and exiting zero. An
//! exit code alone therefore proves nothing. Every `get` is judged beside the same command run with an
//! invented name that must not exist: only "the real name succeeds and the invented one fails" reads as
//! wired, and a CLI that answers both the same way is reported as unreadable rather than guessed about.
//!
//! # Why the server is asked for its tool list before anything is registered
//!
//! The direction's whole point is a named tool on the counterpart's own MCP server. The name is declared by
//! the driver and verified against a fresh `tools/list` handshake at wire time, so a vendor rename becomes a
//! refusal at the toggle instead of a turn that fails somewhere an operator cannot see the cause.

use core::time::Duration;

use runtrol_childproc::{Output, Program, capture, capture_with_input};
use runtrol_drivers::{ConsultTool, McpConsultServer, McpRegistrar};
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
        _ => refuse("consult preparation does not belong to this request"),
    }
}

/// Whether a request belongs to this module.
pub(crate) const fn is_consult(request: &Request) -> bool {
    matches!(
        request,
        Request::Consult | Request::ConsultWire { .. } | Request::ConsultUnwire { .. }
    )
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
    match registered(
        composed,
        &from_program,
        &wireable.registrar,
        &consult_name(direction.to.id()),
    )
    .await
    {
        Ok(true) => line(ConsultState::Wired, None),
        Ok(false) => line(ConsultState::Unwired, None),
        Err(why) => line(ConsultState::Unsupported, Some(why)),
    }
}

/// Whether `name` is registered in the CLI behind `program`, judged beside the control name.
///
/// # Errors
///
/// A sentence, when the CLI's answers cannot be told apart or it could not be run at all.
async fn registered(
    composed: &Composed,
    program: &Program,
    registrar: &McpRegistrar,
    name: &str,
) -> Result<bool, String> {
    let target = ask(composed, program, registrar.get, &[name]).await?;
    let control = ask(composed, program, registrar.get, &[CONTROL_NAME]).await?;
    if control.succeeded() {
        // The CLI treated a name nobody registered as existing, which is what an absent subcommand looks
        // like on the CLI that answers those with its parent help. Nothing can be concluded from the target.
        return Err(format!(
            "{} answered an invented registration name as if it existed, so its configuration cannot be read",
            program.path()
        ));
    }
    Ok(target.succeeded())
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
    let name = consult_name(to_id);
    let already = match registered(composed, &from_program, &wireable.registrar, &name).await {
        Ok(wired) => wired,
        Err(why) => return refuse(&why),
    };

    let outcome = match how {
        // Flipping to where it already stands is success, not an error: the operator asked for a state, not
        // for a transition, and refusing would make the toggle order-sensitive for no one's benefit.
        Change::Wire if already => Ok(()),
        Change::Unwire if !already => Ok(()),
        Change::Wire => wire(composed, direction, &wireable, &from_program, &name).await,
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
) -> Result<(), String> {
    // The counterpart's server command, as it will be written into the registering CLI's configuration: the
    // candidate name rather than a resolved path, because a path goes stale on the counterpart's next update
    // while the name keeps meaning "whatever is installed".
    let (to_name, to_program) =
        runtrol_core::locate_named(&direction.to.manifest).map_err(|error| error.to_string())?;

    verify_consult_tool(composed, &to_program, &wireable.server, wireable.tool).await?;

    let mut add: Vec<&str> = wireable.registrar.add.to_vec();
    add.push(name);
    add.push("--");
    add.push(to_name);
    add.extend_from_slice(wireable.server.serve);
    let added = ask(composed, from_program, &add, &[]).await?;

    // The add's own exit code is not the judgement, because the CLI that answers absent subcommands with
    // help also exits zero for them. The get that follows is.
    if registered(composed, from_program, &wireable.registrar, name).await? {
        Ok(())
    } else {
        Err(format!(
            "{} did not register {name}. it said: {}",
            direction.from.id(),
            said(&added)
        ))
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
    if registered(composed, from_program, &wireable.registrar, name).await? {
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
