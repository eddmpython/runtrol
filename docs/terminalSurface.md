# Terminal surface

The conversation surface is the coding service's own terminal interface. Runtime either starts that TUI on a pseudo
terminal or reaches an already-live owner through a measured provider or operating-system attachment. It keeps one
bounded screen snapshot and lets authorized viewers attach. Runtrol transports terminal bytes and never turns them
into a Runtrol-owned chat transcript.

## Why

Provider CLIs already own current model selection, permissions, approvals, history, and interactive presentation.
Rebuilding those controls in every client creates a second product that drifts from the provider. A terminal viewer
inherits new provider features without prompt injection, semantic parsing, or a model connection owned by Runtrol.

The release provider set is data, not a Core branch. The tracked
[Claude](../crates/runtrol-drivers/manifests/claude.toml) and
[Codex](../crates/runtrol-drivers/manifests/codex.toml) manifests are the providers packaged and exercised for this
release. A future provider still enters through the same manifest and driver contracts without changing terminal
transport or Studio navigation.

## Host

- `runtrol-childproc::pty` owns ConPTY on Microsoft Windows and `openpty` on Unix.
- One live conversation has one provider-owned conversation owner. Runtime exposes at most one central terminal
  renderer for it: either the owner TUI, one official attachment client, or one Windows console mirror. That renderer
  has one reader, one bounded output ring, and one `vt100` screen snapshot without scrollback. Adding VS Code windows,
  phone views, or SDK viewers never duplicates those central objects.
- [`runtrol-core::terminal`](../crates/runtrol-core/src/terminal/mod.rs) owns the executable ring, geometry, screen,
  and shared-state limits. [`terminal_surface.rs`](../crates/runtrol-daemon/src/terminal_surface.rs) binds hosted
  terminal admission to the Core hot-process ceiling and proves the complete-set memory bound. Viewers reuse that
  fan-out and add no payload ring of their own; control records are bounded separately.
- Runtime answers terminal capability and cursor-position queries once at the host. Viewers do not race to answer.
- The host reads a burst whole: after a partial read it waits at most one millisecond for the rest, so a fast
  provider arrives as a few full chunks rather than hundreds of scraps and a healthy viewer never falls behind the
  bounded ring on a burst. Bytes and order are the provider's; only the read boundary is the host's.
- A viewer that crosses the ring's lag boundary receives one replacement checkpoint and then live output at the
  announced sequence; one that stops taking output for ten seconds is closed explicitly. Neither delays a healthy
  viewer, which drains its own receiver from the shared ring.
- A write has one receipt: written, or failed. When its outcome is unknown (a short write, a broken pipe, a write
  the terminal never acknowledged within two seconds) the host ends that terminal generation at once, so nothing is
  ever written on top of a partial input; the Runtime answers such a write with `outcomeUnknown`, keeps its pending
  record so a retry of the same request identity is refused rather than written again, and a repeat of a completed
  request identity is answered from the record without a second write.
- The raw lane publishes each chunk the host read, exactly and first, under one sequence. The passive checkpoint
  projector reads that same ring afterwards and can neither delay nor change what a viewer receives; a panic
  inside it, or falling a whole ring behind, resets it and marks the checkpoint unavailable while the provider and
  every viewer stay live (the provider's next full redraw, which a resize makes, brings it back). Attachment is
  atomic at one sequence: the checkpoint is the screen after sequence `n` and live output begins at `n + 1`, never
  both and never neither; a projector that cannot be reached within a bounded wait yields an empty checkpoint
  that says so, with the live stream still exact from the boundary on.
- A viewer keeps its own terminal's selection, focus, and scroll behavior. Runtime forwards the provider's bytes
  exactly as the host read them, mouse-mode toggles included, never switches mouse reporting on toward a viewer,
  and turns no gesture into keys; what a viewer types, a mouse report included, reaches the provider exactly as
  written, and only the terminal answers the viewer's own terminal sends are dropped because the host already
  answered. The Studio tab takes the provider's mouse-mode control family out at its own edge
  (`mouseModeFilter.ts`). Provider-specific launch behavior remains declarative in the manifest `[tui]` section
  through `new`, `resume`, `attach`, `stop`, `env`, and `env_unset`.
- No Runtime, Studio, SDK, or phone code selects behavior by a hardcoded provider name.

The screen model exists only for geometry, host query answers, and late-view snapshots. It is dropped with the hosted
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
publish a validated process-to-native binding. Runtime atomically binds that identity to the exact PTY-owned process
tree, including a provider executable below a package-manager launcher. A targeted catalogue refresh replaces the
project-name placeholder with the provider title in both the sidebar and open terminal tabs. Terminal output is never
parsed to find the identity or title.

One roster observation promotes every unambiguous sibling terminal in a workspace as one ownership transaction. This
matters when two fresh conversations start before either has a native identity: neither still-unnamed sibling can be
mistaken for a duplicate of the other midway through promotion. One process naming several conversations, one terminal
tree containing several differently named processes, or one identity reaching several terminal trees stays unresolved
instead of selecting by iteration order.

The invoking shell receives one view-local terminal title naming the provider identifier and its hosted process ID.
This keeps simultaneous shell-launched conversations distinct instead of exposing the shared content-named Core
executable. The title never enters the hosted PTY, shared screen, output ring, another viewer, or provider storage.

## Live capture ladder

Runtime selects the strongest structurally proven route for each live process, not for each provider name:

1. A process born through the broker stays on its Runtime-owned PTY. This is the exact byte stream and needs no
   attachment process.
2. If the provider roster publishes a complete official target and the manifest declares paired `attach` and `stop`
   commands, Runtime starts one provider TUI attachment client only when the first viewer opens the conversation.
   `attach` is not `resume`: the original owner remains the only conversation owner.
3. On Microsoft Windows, an interactive console process can be joined through `AttachConsole`. Runtime starts one
   bounded helper only when the first viewer opens the conversation. It reads the visible `CONOUT$` buffer and writes
   viewer input to `CONIN$` without replacing the owner.
4. Without one of those proofs, the process is observe-only. Runtime shows that it is live, blocks duplicate resume
   and deletion, and refuses to pretend that it can stream the session.

The provider driver reports this per-process fact as unavailable, console, or official with an opaque target. Core
does not infer an attachment command from a provider name, a session identifier, terminal text, or a path. The opaque
official target may differ from the durable native conversation identity and is never persisted as transcript state.

Every external attachment is lazy for memory and process efficiency. Merely listing a live conversation retains only
its bounded roster record. Before the first open an official route allocates no renderer process or PTY, and a console
route allocates no helper. Neither allocates a screen model or output ring. Once opened, all viewers share the same
renderer and the executable per-terminal shared-state ceiling. Official attachments and console mirrors hold a
content-free terminal-surface admission claim while their renderer is live. The external CLI still owns the
conversation and transcript; the claim only prevents another Runtime generation from allocating a second renderer,
ring, and screen for that owner.

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

Exactly one view holds a terminal's control lease, which is input and resize authority together. Acquiring control
transfers it: the earlier holder's next write or resize is refused with `controlConflict`, and a client that wants to
type asks again, which is a visible, ordered transfer rather than a race. The descriptor carries `controlGeneration`,
a per-terminal count that climbs on every transfer and renewal, and `controlHeld`; the terminal index publishes a
change on every transfer and release, so every window sees who leads in order. Geometry follows the holder: a
follower window renders the canonical geometry and never resizes the shared process, and a window that takes control
by typing sends its own size once. Writes are serialized through the one PTY writer, so every viewer observes one
input order and one resulting output stream, and a refused write is never applied twice.

`vscodeMultiWindowTerminal` is the direct product proof. It runs two simultaneous real VS Code Extension Hosts with
separate profiles, opens the provider TUI through the first editor terminal tab, and attaches the second tab to the
same Runtime generation, terminal ID, terminal generation, and provider PID. Both tabs receive the first tab's input.
After the first window exits, the exact provider PID generation remains alive and the second tab sends and receives
the next input before stopping that provider. The fixture uses a create-new PID marker, so a second owner fails closed
instead of letting a duplicate process satisfy the journey.

Two operator gates add the installed-provider layer without spending a model turn. `providerTerminalParity` measures
Claude and Codex through independent public Runtime clients, requires byte-identical fresh snapshots, closes one
viewer, hands input to a new writer within the catalogue's Runtime-client delivery ceiling, stops the exact terminal,
and proves it can no longer be attached. `vscodeRealProviderMultiWindow` runs the production extension in two
simultaneous isolated VS Code windows for each installed TUI. The first window's input reaches both windows, the first
window exits, and the second writes within the catalogue's first-use delivery ceiling and stops the terminal. Both
gates use reversible navigation when a provider startup modal ignores printable bytes. They never submit a line,
parse provider text, or retain a transcript.

### Multi-window latency evidence

The deterministic Extension Host journey separates cold integration overhead from the warm transport path. The first
sample in each phase enters through VS Code's public `Terminal.sendText` surface. Later samples start at the same
`Pseudoterminal.handleInput` callback to measure Studio, the public TypeScript client, Runtime authorization, PTY echo,
and cross-window fan-out without charging the test-control bounce through the renderer process to the product path.
It records independent raw sample series for sender echo, second-view delivery, and writer handoff after the first
window closes. Summaries are recomputed from those bounded series rather than trusted as standalone numbers.

The 2026-08-31 deterministic two-window run used 21 warm samples per phase. Owner echo and second-window fan-out
each measured 5 ms p95, and writer handoff measured 4 ms p95. Their separate first interactions measured 5 ms,
5 ms, and 10 ms respectively. The preceding recorded run measured 110 ms, 110 ms, and 14 ms p95. These are observed
results, not replacement ceilings; the executable catalogue below remains the release contract.

[`performance-budget.json`](../extensions/runtrol-vscode/performance-budget.json) owns the first-use ceiling, warm p95
ceiling, exact sample count, and installed-provider Runtime-client ceiling. The deterministic and real-provider gates
read that catalogue directly. [`vscodeMultiWindowTerminal.py`](../tests/audit/vscodeMultiWindowTerminal.py) rejects
missing samples, invalid summaries, a duplicate owner, a replaced process generation, or any task-owned survivor.
Documentation does not carry a second copy of those values.

Fresh open needs `session.start`; native resume needs `session.resume`; listing and viewing need
`session.output.read`; write and lifecycle mutations need the corresponding input or stop scope plus an unexpired
control lease. Canonical root checks and provider capabilities are the same boundaries used by structured sessions.

## Live authority without a database hot path

The durable integration store remains authoritative. Before public listeners start, the daemon's integration
authority restores a read-optimized projection of
the committed rows. Approval, grant change, key rotation, and revocation update that projection only after the store
commit succeeds. Reads share immutable rows, so a terminal write does not clone the scope and root collections or
open a synchronous database transaction.

A terminal relay subscribes to authority changes before reading its current row. This closes the admission race:
an update is either already in the row it reads or wakes the subscription. Authority notifications are selected
before terminal output, and a revoked key, reduced grant, changed key generation, or missing row closes that view
before it can keep streaming under old authority.

Filesystem identity is still part of authority. Every terminal mutation validates the current canonical roots before
touching the PTY. Root identity syscalls run outside the async executor on the bounded blocking lane shared by terminal
views and indexes. A quiet output-only view also receives a background root proof. No successful proof is accepted
after one second, and a failure, timeout, or blocking-task failure closes the affected view or index. A successful
result whose key, grant, or terminal generation changed while the check ran authorizes nothing and is discarded. The
exact scheduling, timeout, and concurrency limits are executable constants in
[`runtime_terminal.rs`](../crates/runtrol-daemon/src/runtime_terminal.rs) and the relay ordering lives in
the Runtime serving modules.

During a Runtime upgrade, the old generation freezes its last committed ceiling and accepts only monotonic
intersections delivered by a successor. Missing rows, key changes, revocations, and stale or conflicting snapshots
fail closed. A later successor may continue the same shrinking chain, but no successor can widen what the draining
generation knew before handoff. [`generation_authority.rs`](../crates/runtrol-daemon/src/generation_authority.rs) owns
that transition contract. This authority relay is a periodic fail-closed projection, not a durable replication log.
Missing or stale relay state denies access instead of preserving authority by assumption. The separate control-plane
audit durability boundary is documented in [runtimeSecurity.md](runtimeSecurity.md#public-audit-boundary).

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
windows share the bounded daemon roster cache owned by
[`NATIVE_ACTIVITY_CACHE_MS`](../crates/runtrol-daemon/src/serve.rs), so provider scans do not multiply with viewer
count. While the original process is live, Runtime blocks duplicate resume and permanent deletion.

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
