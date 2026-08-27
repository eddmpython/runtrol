# Terminal surface

The conversation surface is the coding service's own terminal interface. Runtime starts it on a pseudo terminal,
keeps a bounded screen snapshot, and lets authorized viewers attach. Runtrol transports terminal bytes and never
turns them into a Runtrol-owned chat transcript.

## Why

Provider CLIs already own current model selection, permissions, approvals, history, and interactive presentation.
Rebuilding those controls in every client creates a second product that drifts from the provider. A terminal viewer
inherits new provider features without prompt injection, semantic parsing, or a model connection owned by Runtrol.

## Host

- `runtrol-childproc::pty` owns ConPTY on Windows and `openpty` on Unix.
- `runtrol-core::terminal` owns one reader, a bounded output ring, and a `vt100` screen snapshot without scrollback.
- Runtime answers terminal capability and cursor-position queries once at the host. Viewers do not race to answer.
- Mouse reports are normalized into provider-visible key input. Provider-specific launch behavior remains declarative
  in the manifest `[tui]` section through `new`, `resume`, `env`, and `env_unset`.
- No Runtime, Studio, SDK, or phone code selects behavior by a hardcoded provider name.

The screen model exists only for geometry, mouse translation, and late-view snapshots. It is dropped with the hosted
terminal and is never persisted as a conversation copy.

## Public Runtime contract

Application integrations use the public Runtime methods:

| Method group | Purpose |
|---|---|
| `terminals/list`, `terminals/watchIndex` | Discover live terminal descriptors and their owning Runtime generation |
| `terminals/open`, `terminals/attach` | Open a fresh or native provider terminal, or attach a viewer to an existing one |
| `terminals/acquireControl`, `renewControl`, `releaseControl` | Hold one bounded terminal input lease |
| `terminals/write`, `terminals/resize` | Send base64 bytes or exact geometry under the current lease |
| `terminals/detach`, `terminals/stop` | Detach one viewer or explicitly stop the hosted provider process |
| `terminals/output`, `terminals/lagged`, `terminals/exited` | Stream ordered output, replace a lagged view from a complete snapshot, and report exit |

Open and attach return a terminal descriptor, a view ID, the current base64 screen, and an optional control lease.
Output sequence numbers are per view. A lag notification includes the complete replacement screen and next sequence,
so a client never attempts to reconstruct missing bytes semantically.

Fresh open needs `session.start`; native resume needs `session.resume`; listing and viewing need
`session.output.read`; write and lifecycle mutations need the corresponding input or stop scope plus an unexpired
control lease. Canonical root checks and provider capabilities are the same boundaries used by structured sessions.

## Generation continuity

Every descriptor carries `runtimeGeneration` and `terminalGeneration`. A client that reconnects after transport loss
must re-read the owner-validated locator, select the exact Runtime generation named by the descriptor, attach there,
and replace its screen from the returned snapshot. It must not redirect to the current generation.

`terminalAlreadyLive` identifies the generation and terminal that already own a native provider conversation.
`terminalGenerationUnavailable` means that exact owner no longer exists. `terminalGone`,
`terminalWorkspaceConflict`, `nativeConversationBusy`, and `legacyGenerationBusy` are distinct typed failures. Input,
resize, stop, control acquisition, and approval mutations are never retried after an uncertain outcome.

One atomic native claim registry prevents a native conversation from having both a structured owner and a terminal
owner, including during generation handover. A draining generation may serve terminals it already owns but cannot
open new ones.

## Clients

- Rust exposes the typed terminal client and stream in `runtrol-runtime-client`.
- TypeScript exposes `TerminalClient`, `TerminalView`, exact-generation attach, and typed Runtime failures from
  `@runtrol/runtime-client`.
- Python exposes asynchronous and synchronous terminal clients from `runtrol_runtime`, with the same schema-generated
  params and typed public exceptions.
- Studio uses a dedicated public Runtime terminal connection per editor tab. Its private administration connection
  contains no terminal request or response variants.
- The phone uses its authenticated, device-scoped private transport adapter into the same terminal host. This paired
  device wire is not an SDK or application integration surface.

No published Studio release before the public terminal contract stored a private terminal attachment identity.
Therefore there is no legacy published terminal tab that can be discovered or migrated. Compatibility is enforced by
generation-pinned public attach and the `legacyGenerationBusy` barrier rather than an invented client-side bridge.

## Lifetime

A terminal lives while its provider CLI runs. Closing a Studio tab or SDK view detaches that viewer only. When the
provider exits, Runtime drains the final frame before releasing the terminal. A draining Runtime generation remains
alive until every terminal and structured session it owns has ended.

## Deliberately absent

There is no transcript storage, screen interpretation, prompt rewrite, semantic routing, hidden model call, or API
key relay. Runtime carries bytes, authority, geometry, bounded replay, and process lifetime only.
