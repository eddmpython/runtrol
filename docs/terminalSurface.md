# Terminal surface

The conversation surface is the coding service's own terminal interface. Runtime either starts that TUI on a pseudo
terminal or reaches an already-live owner through a measured provider or operating-system attachment. It keeps one
bounded screen snapshot and lets authorized viewers attach. Runtrol transports terminal bytes and never turns them
into a Runtrol-owned chat transcript.

## Why

Provider CLIs already own current model selection, permissions, approvals, history, and interactive presentation.
Rebuilding those controls in every client creates a second product that drifts from the provider. A terminal viewer
inherits new provider features without prompt injection, semantic parsing, or a model connection owned by Runtrol.

## Host

- `runtrol-childproc::pty` owns ConPTY on Microsoft Windows and `openpty` on Unix.
- One live conversation has one provider-owned conversation owner. Runtime exposes at most one central terminal
  renderer for it: either the owner TUI, one official attachment client, or one Windows console mirror. That renderer
  has one reader, one bounded output ring, and one `vt100` screen snapshot without scrollback. Adding VS Code windows,
  phone views, or SDK viewers never duplicates those central objects.
- The hard per-terminal bounds are a 512 KiB output ring, no scrollback, and at most 25,000 screen cells. The
  primary and alternate screens, reader queue, fan-out queue, and slot metadata have a 3 MiB shared-state ceiling.
  Runtime admits at most eight hosted provider terminals, for a 24 MiB complete-set ceiling. Viewers reuse that
  fan-out and add no payload ring of their own; control records are bounded separately.
- Runtime answers terminal capability and cursor-position queries once at the host. Viewers do not race to answer.
- Snapshot creation and live fan-out share one output-state critical section. A viewer receives a chunk in its
  snapshot or subscribes before that chunk is published, never both and never neither.
- Mouse reports are normalized into provider-visible key input. Provider-specific launch behavior remains declarative
  in the manifest `[tui]` section through `new`, `resume`, `attach`, `stop`, `env`, and `env_unset`.
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

## Live capture ladder

Runtime selects the strongest structurally proven route for each live process, not for each provider name:

1. A process born through the broker stays on its Runtime-owned PTY. This is the exact byte stream and needs no
   attachment process.
2. If the provider roster publishes a complete official target and the manifest declares paired `attach` and `stop`
   commands, Runtime starts one provider TUI attachment client only when the first viewer opens the conversation.
   `attach` is not `resume`: the original owner remains the only conversation owner.
3. On Microsoft Windows, an interactive console process can be joined through `AttachConsole`. One bounded helper
   reads the visible `CONOUT$` buffer and writes viewer input to `CONIN$` without replacing the owner.
4. Without one of those proofs, the process is observe-only. Runtime shows that it is live, blocks duplicate resume
   and deletion, and refuses to pretend that it can stream the session.

The provider driver reports this per-process fact as unavailable, console, or official with an opaque target. Core
does not infer an attachment command from a provider name, a session identifier, terminal text, or a path. The opaque
official target may differ from the durable native conversation identity and is never persisted as transcript state.

Official attachment is lazy for memory and process efficiency. Merely listing a live conversation retains only its
bounded roster record. Before the first open it allocates no renderer process, PTY, screen model, or output ring. Once
opened, all viewers share the same renderer and the existing 3 MiB per-terminal ceiling. Official attachments and
console mirrors hold a content-free terminal-surface admission claim while their renderer is live. The external CLI
still owns the conversation and transcript; the claim only prevents another Runtime generation from allocating a
second renderer, ring, and screen for that owner.

## Public Runtime contract

Application integrations use the public Runtime methods:

| Method group | Purpose |
|---|---|
| `terminals/list`, `terminals/watchIndex` | Discover live terminal descriptors and their owning Runtime generation |
| `terminals/open`, `terminals/attach` | Open a fresh or native provider terminal, or attach a viewer to an existing one |
| `terminals/acquireControl`, `renewControl`, `releaseControl` | Hold one bounded terminal input lease |
| `terminals/write`, `terminals/resize` | Send base64 bytes or exact geometry under the current lease |
| `terminals/detach`, `terminals/stop` | Detach one viewer or explicitly stop the live conversation through its owning route |
| `terminals/output`, `terminals/lagged`, `terminals/exited` | Stream ordered output, replace a lagged view from a complete snapshot, and report exit |

Open and attach return a terminal descriptor, a view ID, the current base64 screen, and an optional control lease.
Output sequence numbers are per view. A lag notification includes the complete replacement screen and next sequence,
so a client never attempts to reconstruct missing bytes semantically.

`terminals/detach` ends only the selected view and returns its dedicated connection to ordinary request mode. An SDK
may open or attach another view on that same authenticated connection. Process exit, lost authority, malformed input,
and transport failure still end the connection.

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

One atomic live-admission registry prevents a native conversation from having both a structured owner and a terminal
surface, including during generation handover. Runtime-owned TUI processes, official attachment renderers, and console
mirrors all export their terminal-surface claim. A terminal reservation is exported before process startup completes,
so a generation handoff cannot lose the launch interval. A draining generation may serve terminals it already owns
but cannot open new ones.

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
provider exits, Runtime drains the final frame before releasing the terminal. An explicit stop ends an owned PTY or
Windows console owner directly; an official attachment invokes the paired provider stop command and then releases only
its attachment renderer. A draining Runtime generation releases a quiet console mirror without stopping its external
owner and ends an official attachment renderer without claiming ownership of the provider transcript. Idle retirement
rechecks viewer count and output age at the same lock boundary where attach subscribes, then marks the renderer
stopping. A reconnect either installs its receiver first and keeps the renderer or receives `terminalGone`. The process
slot and terminal-surface claim remain held until observed exit, so a slow retirement cannot admit a replacement above
the process or memory ceiling.

Opening a Studio window is observation, not permission to start work. Activation restores selection and subscribes to
the live indexes, but never runs `continue` or `resume`. A cold native conversation starts a process only after an
explicit open action. A live descriptor always attaches to its exact terminal and generation.

## External process boundary

A process that began outside the transparent broker remains the conversation owner. A provider observer may detect its
exact live native identity and Studio marks it as externally running within the bounded compatibility clock. Multiple
windows share a 200 ms daemon cache, so provider roster scans do not multiply with viewer count. While the original
process is live, Runtime blocks duplicate resume and permanent deletion.

Microsoft Windows is the operating-system capture layer here. A VS Code window is only a viewer. Windows does not
expose another terminal host's original ConPTY byte pipes, but `AttachConsole` does expose the current console screen
and input queue for a compatible interactive process. Runtime represents that honest screen mirror as terminal bytes.
It does not call the result the original raw byte stream. On Unix, an arbitrary pre-existing PTY remains unattached
unless the provider or original terminal host exposes a supported official channel. Every unsupported row stays
observable rather than being restarted, migrated, or silently resumed.

## Human and machine surfaces

The TUI is the human surface. Future CLI-to-CLI work must use a provider's official structured machine channel, such
as MCP, ACP, or a documented queue, under the same native identity and lifecycle when the provider supports it. It
must not scrape the screen or type prompts blindly into the TUI. A provider without a measured same-session machine
channel cannot be advertised as one, and Runtrol does not create an automatic recursive agent loop.

## Deliberately absent

There is no transcript storage, screen interpretation, prompt rewrite, semantic routing, hidden model call, or API
key relay. Runtime carries bytes, authority, geometry, bounded replay, and process lifetime only.
