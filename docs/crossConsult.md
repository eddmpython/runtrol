# Cross consult

One toggle registers one CLI as a consultable MCP server inside another CLI's own configuration, so an
agent asks the other vendor's agent for an opinion mid-turn and gets it back. The user never types a
registration command and never learns what MCP is. This document is the operating truth for how the wiring
works, what was measured, and where its boundaries are.

## What runtrol does, and refuses to do

| does | never does |
|---|---|
| runs the registering CLI's own official add / remove / get commands | writes a provider's configuration file (`configReadOnly` is the floor) |
| shows whether a direction is wired, asked fresh from the CLIs | holds a copy of the wired state anywhere of its own |
| verifies the counterpart's server offers the declared consult tool | reads, stores, or relays a word of the consultation itself |
| undoes exactly what it registered | any automatic agent loop (A output wired into B input stays forbidden) |

The consultation travels on a stdio pipe between the two CLIs. runtrol knows the pipe exists and nothing
about its contents, which is what keeps `noTranscriptCopy` and `egressContract` untouched by this feature.

## The measured asymmetry (claude 2.1.220, codex 0.146.0, 2026-08-03)

- `codex mcp-server` answers `tools/list` with a `codex` tool that runs a session. That is a consultation,
  so **claude consulting codex is the direction that ships**. A real mid-turn reception was measured: a
  `claude -p` turn called the wired `codex` tool and relayed its answer verbatim.
- `claude mcp serve` answers `tools/list` with that CLI's own toolset (Bash, Read, and the rest). The one
  delegating tool in it answers `Agent type 'general-purpose' not found` over an empty available list in
  serve context, in 0.6 seconds, with or without a subagent type. There is no official way to ask claude
  for an opinion over MCP today, so **codex consulting claude shows as unsupported with that measurement**
  instead of being wired and failing mid-turn. The day the vendor ships a consult tool, the declaration in
  `crates/runtrol-drivers/src/claude/bound.rs` is one line to update and the same machinery lights up.

## How a direction is judged, and why exit codes alone are not trusted

Measured: `claude mcp <absent-subcommand> --help` prints the parent help and exits **zero**, while codex
exits 2. So no judgement here reads an exit code by itself, and none reads help prose at all:

- **Wired state** is `mcp get <name>` judged beside the same command run with an invented control name
  (`runtrolConsultAbsentControl`). Only "the real name succeeds and the control fails" reads as wired. A
  CLI that answers both the same way is reported as unreadable rather than guessed about. This is the
  probe's control doctrine applied to subcommands.
- **A wire** runs the official add command, then judges success with that get-plus-control, never with the
  add's own exit code.
- **The consult tool** is verified before anything is registered: the counterpart's server is started once,
  asked `initialize` -> `initialized` -> `tools/list` in one closed stdin write, and must name the declared
  tool. Measured: the server answers and exits cleanly on end of input, so this costs one short process and
  zero tokens. A vendor rename becomes a refusal at the toggle rather than a mid-turn failure.

What is registered is the counterpart's **candidate name** (`codex`) plus its serve words, not a resolved
path: a path goes stale on the counterpart's next update, while the name keeps meaning "whatever is
installed" for as long as the operator's search path does. The registration name is `<provider>Consult`
(`codexConsult`), which is also what the consulting agent reads in its own tool list.

## Where each piece lives

- `crates/runtrol-drivers/src/consult.rs`: the declared surface types. Each driver's `bound.rs` declares
  its registrar argv, its serve argv, and its consult tool or the measured reason it has none. The shape of
  a vendor's commands is semantics no probe can read, so it is declared and then exercised, like bound flags.
- `crates/runtrol-daemon/src/consult.rs`: the executor. Enumerates ordered provider pairs, judges state,
  wires and unwires, and answers one `Consult` response shape for status and for both flips.
- Requests `consult` / `consult wire <from> <to>` / `consult unwire <from> <to>` on the command surface,
  and the `AI 자문` dialog in the desktop window, both over the same daemon requests.
- Consult work runs in the connection task behind the provider preparation gate, so a toggle never stops a
  running session's events.

## Security

Wiring expands what an agent can reach mid-turn and edits the CLIs' own configuration, so it is
`LocalScope::ConsultWire`: answered at the keyboard, never grantable to a device, enforced by the missing
`LocalScope -> DeviceScope` conversion and refused at the scope wall with "go to the machine" rather than
"ask for a permission". Reading consult status is `ConfigRead`. Approvals for anything the consulted agent
wants to do stay in each CLI's own permission flow; runtrol invents no new approval path.

## Evidence

`crossConsultSmoke` (local-only, operator's machine, token-free) drives the real product binary against the
real CLIs in isolated homes: honest status both ways, wire, the registering CLI's own `get` confirming, the
written entry being nothing but a command, idempotent re-flips, and exact restoration on unwire. Its
selftest injects nine defects first. The real mid-turn reception costs a real turn on both CLIs, so it is
hand-measured evidence (2026-08-03) rather than a gate, and the gate's output says so. Scores and tiers are
`tests/audit/northStar/board.toml`'s to state.
