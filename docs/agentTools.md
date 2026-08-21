# Agent Tools

Agent Tools lets an installed coding-agent CLI delegate bounded work through the same public Runtrol Runtime that
Studio uses. A person enables one project from its VS Code heading. Runtrol then registers its exact Core executable
through every usable provider CLI's official MCP command. The provider-owned agent loop decides whether to use the
tools. Runtrol does not add a planner, prompt, transcript store, or autonomous loop of its own.

## User contract

1. Open the Runtrol Conversations view.
2. Select **Enable Agent Tools for This Project** on a project heading. The inline sparkle is the one-click entry.
3. The heading says `Agent Tools` when the project is ready. The state is restored from Core after an Extension Host
   restart, including projects outside the current window.
4. Select **Disable Agent Tools for This Project** to remove the provider registrations when the last enabled project
   is disabled, revoke that project's Runtime integration, delete its public grant, and delete its protected local
   identity.

The command palette offers the same enable and disable actions. A one-folder window selects that folder without a
question. A multi-root window asks for exactly one root. Provider approvals always remain with a person in Runtrol.

## Fixed tool catalogue

| Tool | Capability | Boundary |
|---|---|---|
| `runtrol_providers` | List Runtime-discovered providers and structural availability | Starts no provider session |
| `runtrol_models` | Ask one discovered provider for its current provider-owned model catalogue | Accepts only an exact discovered provider ID |
| `runtrol_sessions` | List structural Runtime session metadata visible under the approved root | Returns no stored transcript |
| `runtrol_start` | Start an exclusive session and submit caller-owned input unchanged | Workspace must resolve at or below the approved root |
| `runtrol_send` | Acquire exact control, submit input unchanged, and release control | Session ID and canonical workspace must match |
| `runtrol_next_event` | Wait up to 30 seconds for one public Runtime event and the next cursor | Keeps no transcript copy |
| `runtrol_stop` | Interrupt one exact visible session under a short-lived control lease | Does not delete provider conversation state |

There is no approval-answering tool, native-session deletion tool, shared-workspace start, provider-specific semantic
inference, transcript aggregation, scheduler, or hidden retry loop. The fixed grant contains only these scopes:

```text
provider.read
model.read
session.list
session.output.read
session.start
session.input.write
session.stop
```

## Authority model

Each enabled canonical project root receives one independent Runtime integration identity. The private 32-byte
identity is protected by DPAPI on Windows, Keychain on macOS, or Secret Service on Linux. The adjacent JSON file is a
public Runtime grant with a closed schema, exactly one root, and exactly the seven scopes above. Slot names are
domain-separated SHA-256 digests and reveal no project path.

Provider configuration receives only:

```text
runtrolTools -> <exact runtrol executable> mcp
```

No root, secret, environment variable, prompt, API key, or provider credential is copied into provider
configuration. The MCP process selects authority from its canonical startup working directory. A directory outside
every enabled root fails closed. When roots are nested, the longest matching approved root wins. Every mutating call
then rechecks the supplied canonical workspace, and existing-session calls also recheck the Runtime session's exact
workspace before acquiring control.

Enablement and removal are local `ConfigWrite` actions. They cannot be granted to a phone or another Runtime
integration. Provider registration is performed and verified only through the registrar commands declared by each
driver. Runtrol never edits a provider configuration file directly. All provider configuration work is serialized by
the daemon's ordered provider lanes, including concurrent VS Code windows.

The stable name is not treated as proof of ownership. Before the first provider mutation, each official `get`
answer must either say the name is absent or read back the exact executable, the single `mcp` argument, no
environment, no working directory, and an enabled entry. A pre-existing entry that points anywhere else is reported
and left untouched. Removal performs the same ownership preflight across every provider before deleting anything,
so an entry replaced outside Runtrol is never removed as collateral.

## Protocol contract

The same `runtrol mcp` process supports the finalized stateless MCP discovery flow and the legacy initialization
flow:

- `server/discover` advertises revision `2026-07-28`, a complete result, tools capability, and
  `_meta["io.modelcontextprotocol/serverInfo"]` as required by the finalized
  [server discovery contract](https://modelcontextprotocol.io/specification/2026-07-28/server/discover).
- `initialize` preserves supported legacy revisions through `2025-11-25` and returns the legacy `serverInfo` shape.
- `tools/list` returns the seven fixed definitions. `tools/call` returns both MCP text content and structured content;
  tool failures are successful JSON-RPC responses with `isError: true`, matching the
  [tools contract](https://modelcontextprotocol.io/specification/2026-07-28/server/tools).
- Standard input is newline-delimited UTF-8 JSON-RPC. One line is bounded at 1 MiB before JSON parsing. Protocol
  output uses standard output exclusively, while diagnostics use standard error.

## CLI and recovery

```text
runtrol tools enable [project]
runtrol tools disable [project]
runtrol tools status
runtrol tools list
```

`enable` is idempotent. It verifies Runtime authentication and the local MCP catalogue before provider configuration
changes. It preflights every provider before the first write, verifies each write through that provider's official
`get` command, and removes entries it added earlier in the same failed attempt. If a first enable cannot finish, it
also revokes the just-created Runtime integration and deletes the new credential slot.

`disable` is also idempotent. With another approved root present it revokes only the requested root and keeps the
global provider registration. With the last root it removes the global registration first, verifies it is gone, then
revokes Runtime authority and deletes local credentials. If provider removal fails, local Runtime authority and
credentials are still removed and the command reports the stale provider registration as a warning. A registration
that no longer matches the exact Runtrol command is treated as somebody else's entry and explicitly left untouched.
An exact leftover entry has no root or secret and therefore starts a server that fails closed until it is removed or
a project is enabled again.

`status` selects the approved root from the current directory and proves that the protected identity still
authenticates to Runtime. `list` is a local structural read used by Studio to restore project badges; it starts no
daemon and exposes no credentials.

## Verification

| Gate or journey | Assertion |
|---|---|
| `agentToolsSmoke --selftest` | Ten mutations prove that missing tools, approval authority, extra roots, leaked environment, wrong executables, and false default-deny results are rejected |
| `agentToolsSmoke` | Real installed Claude and Codex CLIs in isolated homes prove collision refusal, failed-enable rollback, exact registration readback, modern and legacy MCP discovery, two real Runtime reads, outside-root denial, full disable, replacement preservation, and post-revocation denial with zero model turns |
| `RUNTROL_EYE_ENTRY=agentToolsEye node tooling/real-window-eye.mjs` | A real isolated VS Code 1.132.1 Extension Host photographs the project badge after enable and after complete revocation, while `tools list` independently confirms both states |
| `scopeWall`, `configReadOnly`, `dependencyDirection`, `gateCoverage` | Local-only administration, no direct provider configuration writes, the top-level dependency boundary, and active gate ownership remain machine-enforced |

The real CLI smoke is local-only because hosted runners do not own an operator's installed subscription CLIs. It
uses isolated Runtrol and provider homes, sends no prompt, starts no session, and terminates only its exact isolated
daemon.
