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
- One conversation has one provider process, one PTY, one reader, one bounded output ring, and one `vt100` screen
  snapshot without scrollback. Adding windows or terminal viewers never duplicates those central objects.
- The hard per-terminal bounds are a 512 KiB output ring, no scrollback, and at most 25,000 screen cells. The
  primary and alternate screens, reader queue, fan-out queue, and slot metadata have a 3 MiB shared-state ceiling.
  Runtime admits at most eight hosted provider terminals, for a 24 MiB complete-set ceiling. Viewers reuse that
  fan-out and add no payload ring of their own; control records are bounded separately.
- Runtime answers terminal capability and cursor-position queries once at the host. Viewers do not race to answer.
- Mouse reports are normalized into provider-visible key input. Provider-specific launch behavior remains declarative
  in the manifest `[tui]` section through `new`, `resume`, `env`, and `env_unset`.
- No Runtime, Studio, SDK, or phone code selects behavior by a hardcoded provider name.

The screen model exists only for geometry, mouse translation, and late-view snapshots. It is dropped with the hosted
terminal and is never persisted as a conversation copy.

## Process-birth broker

Runtime installation materializes provider-neutral shell command shims from discovered manifests. In a new terminal,
the shim forwards the provider identity, exact working directory, geometry, and exact argument vector to the local
broker. The daemon creates the provider process on its PTY, registers it in the terminal index, and keeps the original
terminal as the first viewer. Studio windows and other authorized clients attach to that same terminal generation.

The broker does not interpret arguments for meaning. It recognizes a native resume identity only when the argument
vector structurally matches the manifest's discovered resume prefix followed by one bounded opaque identity. Provider
resolution removes Runtrol-owned shim directories from its search path and refuses an owned shim as the real provider
program, preventing recursive launch.

A provider may mint its native conversation identity after process start. A cheap provider-owned process roster can
publish a validated process-to-native binding. Runtime atomically binds that identity to the exact PTY process, and a
targeted catalogue refresh replaces the project-name placeholder with the provider title in both the sidebar and open
terminal tabs. Terminal output is never parsed to find the identity or title.

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

Each view has its own renewable lease generation. One window renewing, expiring, releasing, or closing its lease does
not invalidate another window. Authorized writes from all live views are serialized through the one PTY writer, so
every viewer observes one input order and one resulting output stream.

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

Opening a Studio window is observation, not permission to start work. Activation restores selection and subscribes to
the live indexes, but never runs `continue` or `resume`. A cold native conversation starts a process only after an
explicit open action. A live descriptor always attaches to its exact terminal and generation.

## External process boundary

A process that began outside the transparent broker remains owned by its original terminal. A provider observer may
detect its exact live native identity and Studio marks it as externally running within the bounded compatibility
clock. Multiple windows share a 200 ms daemon cache, so provider roster scans do not multiply with viewer count.
While the original process is live, Runtime blocks duplicate resume and permanent deletion.

Windows console attachment APIs do not grant arbitrary access to another terminal host's existing ConPTY pipes, and
the same ownership boundary exists on Unix PTYs. Runtime therefore never kills, restarts, migrates, or claims to stream
an arbitrary pre-existing PTY. It promotes that process into the shared byte stream only when the provider or original
terminal host exposes an official attach channel. Until then the row is honestly observable but unattached.

## Deliberately absent

There is no transcript storage, screen interpretation, prompt rewrite, semantic routing, hidden model call, or API
key relay. Runtime carries bytes, authority, geometry, bounded replay, and process lifetime only.
