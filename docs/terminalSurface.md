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
  renderer for it: either the owner TUI, one official attachment client, or one observed
  mirror fed by the VS Code window that owns the terminal (`fed.rs`; `docs/vscodeSurface.md`, observed mirror). That
  renderer has one reader, one bounded output ring, and one `vt100` screen snapshot without scrollback. Adding VS
  Code windows, phone views, or SDK viewers never duplicates those central objects. The descriptor's `origin` names
  which of the three it is, and its `viewerCount` says how many views are attached right now: a proved count the
  index republishes when a view attaches or ends, and one that never implies model work.
- [`runtrol-core::terminal`](../crates/runtrol-core/src/terminal/mod.rs) owns the executable ring, geometry, screen,
  and shared-state limits. [`terminal_surface/mod.rs`](../crates/runtrol-daemon/src/terminal_surface/mod.rs) binds hosted
  terminal admission to the Core hot-process ceiling and proves the complete-set memory bound. Viewers reuse that
  fan-out and add no payload ring of their own; control records are bounded separately.
- A terminal that fails to initialize reports that failure. If its spawned process has not yet ended, the existing
  terminal table still counts it, its native claim remains held, and the normal exit observer owns cleanup. Its
  unavailable I/O cannot accept input or resize. Cleanup does not wait while holding the shared terminal table.
- Runtime answers terminal capability and cursor-position queries through one ordered host authority. Attaching a
  view or taking a checkpoint never produces a reply. The dependency-free
  [terminal grammar](../crates/runtrol-terminal-protocol/src/lib.rs) owns the mechanical query vocabulary.
- The host coalesces partial reads using the elapsed-time budget in the terminal module. OS queries and scheduling
  consume that same budget; an already delayed read never starts a fresh series of waits. Fast bursts arrive in
  fewer chunks. Bytes and order are the provider's; only the read boundary is the host's. A reader interruption
  retries the read. A real I/O fault follows all bytes already
  accepted and closes the exact terminal generation instead of leaving an unreadable live owner.
- A viewer that crosses the ring's lag boundary receives one replacement checkpoint and then live output at the
  announced sequence; one that stops taking output for ten seconds is closed explicitly. Neither delays a healthy
  viewer, which drains its own receiver from the shared ring.
- A write has one receipt: written, or failed. When its outcome is unknown (a short write, a broken pipe, a write
  the terminal never acknowledged within two seconds) the host ends that terminal generation at once, so nothing is
  ever written on top of a partial input; the Runtime answers such a write with `outcomeUnknown`, keeps its pending
  record so a retry of the same request identity is refused rather than written again, and a repeat of a completed
  request identity is answered from the record without a second write.
- The raw lane publishes each chunk unchanged before projection. The single terminal authority consumes every byte
  in order. Publication waits only before overwriting its oldest unapplied chunk in the existing bounded ring;
  slow viewers do not participate in that bound. A checkpoint serialization failure makes that snapshot unavailable.
  Losing authoritative parser state closes input and the exact terminal generation, because resetting the cursor
  and continuing to answer would invent terminal state. Process exit or control failure releases publication
  backpressure so final raw output can drain. Attachment is atomic at one sequence: a complete checkpoint is the
  screen after sequence `n` and live output begins at `n + 1`. A checkpoint that cannot be obtained within its bounded
  wait is explicitly unavailable, with the live receiver still exact from its announced boundary.
- A viewer keeps its own terminal's selection, focus, and scroll behavior. Runtime forwards the provider's bytes
  exactly as the host read them, mouse-mode toggles included, never switches mouse reporting on toward a viewer,
  and turns no gesture into keys; what a viewer types, a mouse report included, reaches the provider exactly as
  written. Input bytes cannot distinguish a cursor report from Shift-F3, so Runtime never removes reply-shaped
  input or combines fragments from separate viewers. Studio and the CLI bridge instead replace host-owned output
  queries with a nonprinting string terminator before their local emulators see them. This preserves control-sequence
  cancellation and prevents duplicate emulator answers while the public raw stream stays unchanged. The Studio tab
  also takes the provider's mouse-mode control family out at its own edge (`mouseModeFilter.ts`). Provider-specific
  launch behavior remains declarative in the manifest `[tui]` section
  through `new`, `resume`, `attach`, `stop`, `env`, and `env_unset`.
- No Runtime, Studio, SDK, or phone code selects behavior by a hardcoded provider name.

The screen model exists only for geometry, host query answers, and late-view snapshots. It is dropped with the hosted
terminal and is never persisted as a conversation copy.

### Colour output

Provider colour remains terminal output. Studio preserves ANSI SGR, indexed colours and RGB colours through both
presentation filters; it does not choose a replacement provider palette. If a TUI is monochrome while workbench
icons retain their colours, inspect the environment that launched its Runtime before changing terminal rendering.
The PTY inherits that environment except for the manifest's declared additions and removals. In particular,
[`NO_COLOR`](https://no-color.org/) can ask the provider to omit colour even when the manifest advertises a
colour-capable terminal. Changing a launching shell's environment does not change an already-running CLI.

Native GUI verification removes the command tool's inherited `NO_COLOR` through the existing isolated-host
environment builder in `extensions/runtrol-vscode/tooling/isolated-vscode.mjs`. Ordinary product launches retain
explicit user environment preferences. Compare the same Runtime image, provider build and terminal settings before
and after a launch-environment change; a plain screenshot alone cannot distinguish suppressed provider output from
a rendering defect.

### Exact Windows lifetime

Each managed terminal owns a private nested Windows Job, bound atomically at suspended process creation. Its root
cannot execute until final admission validates the current grant, approved filesystem identity, caller and generation.
Slow process creation and durable worktree binding run outside the shared terminal table and courier admission locks.
Reservations still count against the existing capacity while preparation or failed-child cleanup is pending.

One independent process from the same Runtime image retains the generation's Job handles and the exact process
objects across Runtime exit. This keeper receives only bounded structural admissions through private inherited
handles; it receives no terminal bytes, provider credentials or transcript. The terminal and short-command classes
use their existing separate admission limits, so a full terminal set still permits supervised stop and probe work.
Before a suspended child runs, the keeper must acknowledge its retained scope. An uncertain acknowledgement cannot
authorize execution or release that reservation.

The root and every retained Job member must signal completion before the terminal can retire. Windows thread-pool
waits observe those exact handles without a per-terminal timer. Sealing and stopping a Job run outside the async
executor; membership checks reuse the same Job and validate the caller's birth identity through its retained handle.
A failed lifetime proof retains ownership and reports an operational error instead of publishing a fabricated exit.

When Runtime ends, the keeper seals the same Jobs, waits for their retained process objects, and publishes completion
only by an exact compare-and-swap on the existing workspace ownership records. It cannot remove a worktree or replace
a newer occupant. Recovery preserves an older or failed generation whose completion is unproved. A missing PID,
closed pipe or vanished locator alone is not successful cleanup. The owning implementation is
[`contain/keeper`](../crates/runtrol-childproc/src/contain/keeper/mod.rs); local shutdown and uninstall procedures are in
[runtimeOperations.md](runtimeOperations.md#uninstall).

Process completion permits console closure; output-reader completion proves the accepted final bytes were published.
The exit event also waits for the terminal authority to consume those bytes. Both public terminal views and the
local broker drain their remaining accepted output before publishing completion. A structural host failure remains
distinct from the process exit code, including when that code is zero; Studio retains the last provider screen and
reports the failure in the workbench. A generation transition does not stop a managed owner because its output is quiet or
because it has no viewers. Unused official attachment renderers can retire under their existing grace policy, and an
observed mirror can release its feed without stopping the external owner.

### Observed-owner input

An observed mirror can offer text input when its exact owning extension has registered a live input receiver.
The Runtime descriptor publishes this availability for the caller's current authority. The operation is
`terminals/sendText`; `terminals/write` remains the exact-byte PTY contract and refuses mirrors. Stop still belongs
to the original terminal owner.

The registration response contains a private proof for one integration, window and registration generation. The
owner binds a dedicated `windows/watchInput` connection using that proof. Offers carry only identity and sequence.
`windows/claimInput` checks the current sender and owner grants, approved filesystem roots, control lease, mirror,
registration, shell PID birth and execution before moving the transient text out of its one pending slot. The owner
checks its exact local terminal and execution again before calling the public input API. Neither a saved window
identifier nor a replacement Extension Host can claim an earlier registration's text.

The caller succeeds only after `windows/inputReceipt` confirms `ownerExtensionAccepted`. This means one public
owner-extension input call, not exact stdin bytes, shell processing or completed model work. The public API may
normalize newlines. A failed claim sends no input; a lost connection, expired delivery or uncertain API result leaves
an unknown outcome that must not be replayed. Repeating a completed mutation identity returns its structural receipt
without a second owner call. Pending text is bounded by existing terminal operation admission and released with its
receiver; the mutation ledger keeps only an authenticator and structural result. The executable contract is owned by
[`owner_input`](../crates/runtrol-daemon/src/runtime_terminal/owner_input/mod.rs) and
[`window_input`](../crates/runtrol-daemon/src/runtime_serve/window_input.rs).

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
3. A console another terminal host owns is never joined. Windows exposes `AttachConsole`, and an earlier build used
   it as a screen mirror; the accepted capability table makes an arbitrary external terminal focus-only (`PLAN-02`,
   2026-09-01), so the Runtime proves the window that owns the terminal instead and brings it forward on request
   (`providers/focusNative`).
4. Without one of those proofs, the process is observe-only. Runtime shows that it is live, blocks duplicate resume
   and deletion, and refuses to pretend that it can stream the session.

The provider driver reports this per-process fact as unavailable or official with an opaque target. Core
does not infer an attachment command from a provider name, a session identifier, terminal text, or a path. The opaque
official target may differ from the durable native conversation identity and is never persisted as transcript state.

Every external attachment is lazy for memory and process efficiency. Merely listing a live conversation retains only
its bounded roster record. Before the first open an official route allocates no renderer process, PTY, screen model
or output ring. Once opened, all viewers share the same
renderer and the executable per-terminal shared-state ceiling. Official attachments hold a
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
| `windows/register`, `windows/update`, `windows/list`, `windows/watchIndex` | A VS Code window registers itself (session identity, host generation, folders) and the ordinary terminals it observes (process id, shell integration, current command generation); every window reads or watches the index. One entry per window, bound to the registering connection, replaced by the same window's next registration |
| `terminals/detach`, `terminals/stop` | Detach one viewer or explicitly stop the live conversation through its owning route |
| `terminals/output`, `terminals/lagged`, `terminals/exited` | Stream ordered output, replace a lagged view from a complete snapshot, and report exit |

Open and attach return a terminal descriptor, a view ID, the current base64 screen, and an optional control lease.
Output sequence numbers are per view. A lag notification includes the complete replacement screen and next sequence,
so a client never attempts to reconstruct missing bytes semantically.

Studio forwards each received chunk through the public VS Code `Pseudoterminal.onDidWrite` event. That API has no
renderer write acknowledgement, so Studio cannot implement xterm write-callback watermarks or claim that firing the
event proves rendering completed. Runtime owns the bounded raw ring, per-view queues and explicit lag boundary;
VS Code owns buffering after the event. The native renderer measurement below observes actual consumption separately
and does not add a private renderer API to the extension.

`terminals/detach` ends only the selected view and returns its dedicated connection to ordinary request mode. An SDK
may open or attach another view on that same authenticated connection. Process exit, lost authority, malformed input,
and transport failure still end the connection.

Exactly one holder owns a terminal's control lease, which is input and resize authority together. Acquiring control
transfers it when the caller has at least the current holder's input precedence. The earlier holder's next write or
resize is refused with `controlConflict`. The descriptor carries `controlGeneration`,
a per-terminal count that climbs on every transfer and renewal, and `controlHeld`; the terminal index publishes a
change on every transfer and release, so every window sees who leads in order. Geometry follows the holder: a
follower cannot resize the process under another unexpired lease. A resize can acquire an unheld or expired lease
with `onlyIfFree`; the same Runtime lock checks the condition and grants control, so concurrent followers cannot
both win. A Runtime generation that does not support this condition refuses it without an unconditional retry.
A window that takes control by typing sends its own size once. Writes are serialized through the one PTY writer, so every viewer observes one
input order and one resulting output stream, and a refused write is never applied twice.

Generations advertising `terminalInputPriority` recognize the owner-approved `session.input.priority` scope.
It raises an integration above ordinary writers while it retains its current input grant and approved root. It does
not authorize input by itself or prove that bytes came from a physical keyboard. Equal-precedence writers may take
control from each other. Lower-precedence writers cannot displace a live higher-precedence holder, including through
initial open or attach. A refused initial lease leaves the view usable for observation. Revocation or removal of
input authority ends the protection; precedence is read from current authority rather than cached in the lease.
The authenticated owner-local CLI bridge uses that same holder table with interactive precedence. Its connection
cleanup releases only its own holder. Passive geometry updates still obey `onlyIfFree` at either precedence.

Studio requests this scope when the connected generation advertises support. An existing Studio identity reviews
the capability once: a previously complete Studio grant may add only the new scope using a generation-checked owner
administration call, preserving its roots and signing identity. A narrowed grant is left narrowed. The review is
persisted before requesting the change in a separate, identity-bound SecretStorage entry. Older windows can replace
their shared credential snapshot without erasing that decision, so a lost response or later owner removal cannot
silently elevate the identity again. An older generation defers the review until a supporting command connection
is established.
Draining generations retain their original authority ceiling; adding a scope at the successor never retroactively
changes the earlier generation's input policy.

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
window closes. Those observations finish at Studio's output callback, before VS Code paints the terminal. They do
not measure source-read to visible-frame latency. Summaries are recomputed from bounded sample series; historical
run values belong to their Git evidence, and the executable catalogue remains the release contract.

[`performance-budget.json`](../extensions/runtrol-vscode/performance-budget.json) owns the first-use ceiling, warm p95
ceiling, exact sample count, and installed-provider Runtime-client ceiling. The deterministic and real-provider gates
read that catalogue directly. [`vscodeMultiWindowTerminal.py`](../tests/audit/vscodeMultiWindowTerminal.py) rejects
missing samples, invalid summaries, a duplicate owner, a replaced process generation, or any task-owned survivor.
Documentation does not carry a second copy of those values.

A source-read-to-render measurement needs a separate, test-only observation boundary. Core's existing `test-support`
feature can observe a successful original reader return before coalescing, then its raw publication ordinal and the
exact checkpoint/live attachment origin. Public sequence one is relative to that attachment. A finite native probe
joins the exact terminal and two view IDs to the actual Studio decoder/filter output lengths, xterm's write callback,
and the following `onRender` watermark in both windows. The reported upper bound ends at the later renderer
acknowledgement. The write callback alone means parsed output; neither it nor `onRender` proves DXGI presentation or
physical display scanout. The probe verifies the native QPC and renderer clock relationship on the actual host.

Only identities, ordinals, lengths, eligibility flags and times enter this observation. Provider bytes, fragments and
content hashes are not retained. The actual decoder and presentation filters remain authoritative: split UTF-8 and
unfinished VT carries delay eligibility, and missing mappings, replacement checkpoints, overflow or unresolved final
output cannot count as successful samples. Report every read in the predeclared warm interval, with setup, ineligible
and unresolved counts separately. A first-read-per-input distribution is a different statistic and cannot replace
the complete eligible output distribution. Record observer overhead, GPU configuration and exact development build;
repeat after affected source changes. This is a bounded native acceptance tool, not a shipping trace or public timing
field.

Fresh open needs `session.start`; native resume needs `session.resume`; listing and viewing need
`session.output.read`; write and lifecycle mutations need the corresponding input or stop scope plus an unexpired
control lease. Canonical root checks and provider capabilities are the same boundaries used by structured sessions.
For a preserved Core-owned worker worktree, the [worktree controller](sessionDialogue.md#isolated-workers) binds
native resume and subsequent view/input authority to its original approved project and exact filesystem identity.

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

Filesystem identity is still part of authority. An admitted dedicated terminal view uses a recent proof of its pinned
root for input, output and control acquisition or renewal. These control operations check the exact view and current
grant again after obtaining the mutation lock. Views with the same integration, key and grant generations, approved
root identity and complete worktree binding share a pinned guard and one refresh in flight. The pool holds weak
references, so departed views retain neither authority nor a background poller. Background root proofs and index
root checks share a bounded blocking lane; quiet output-only views also receive these checks. Opening, rebinding and
ordinary requests outside a dedicated view retain their own canonical-root validation. Admission rechecks the
current grant and proof after waiting for control state, before issuing an initial lease or returning the first
screen snapshot.

A proof's lifetime begins when its filesystem check finishes, so delayed observation cannot renew old authority.
Each view schedules its next refresh from that shared completion time, including after admission or an inbound
response replaces its notification receiver.
A denied or failed filesystem check invalidates its shared proof. A refresh timeout provides no new authority: a
view can use its prior successful proof only until that proof's original expiration. Every output frame, including
exit drain and lag replacement, checks freshness before sending, and quiet views wake at the same absolute expiry.
An index check failure still closes its subscription. A successful result for a previous key, grant or worktree
binding cannot authorize the replacement binding. Static failure reasons identify the affected terminal view without
recording terminal bytes. Scheduling, timeout, concurrency and freshness limits belong to
[`root_proof`](../crates/runtrol-daemon/src/runtime_terminal/root_proof/mod.rs); relay ordering lives in the Runtime serving
modules.

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

Studio's output pump owns reattachment for its exact broken view. A delayed control failure or lease response from
that view cannot close the replacement or change its lease. The failed mutation keeps its original outcome; later
unsent input waits within the existing bounded input queue until reattachment settles.
If exact reattachment fails, Studio marks that tab as failed, preserves its last screen, and reports the original
connection failure together with the reattachment failure. The notification directs the person to reopen the
conversation from the sidebar. A healthy index connection cannot hide a failed terminal connection.

One atomic live-admission registry prevents a native conversation from having both a structured owner and a terminal
surface, including during generation handover. Runtime-owned TUI processes and official attachment renderers
export their terminal-surface claim. A terminal reservation is exported before process startup completes,
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
provider exits, Runtime drains the final frame before releasing the terminal. An explicit stop ends an owned PTY
directly; an official attachment invokes the paired provider stop command and then releases only
its attachment renderer. A draining Runtime generation releases a quiet observed mirror without stopping its external
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
expose another terminal host's original ConPTY byte pipes. `AttachConsole` exposes a console's current screen and
input queue, and an earlier build mirrored that screen; it is no longer joined, because an arbitrary external terminal
is focus-only: the Runtime proves the window that owns the terminal and brings it forward (`providers/focusNative`). On Unix, an arbitrary pre-existing PTY remains unattached
unless the provider or original terminal host exposes a supported official channel. Every unsupported row stays
observable rather than being restarted, migrated, or silently resumed.

## Human and machine surfaces

The TUI is the human surface. Managed processes exchange explicit opaque messages through
[the session courier](sessionDialogue.md), which the provider invokes through its ordinary shell tool after visible
activation. That process-scoped channel does not require a provider tool registration and does not claim a
provider-native conversation identity. The dialogue contract owns activation, delivery, replies and isolated workers.
Runtime never scrapes the screen, infers a reply, wakes an idle model through hidden input, or runs an agent loop.

## Deliberately absent

There is no transcript storage, screen interpretation, prompt rewrite, semantic routing, hidden model call, or API
key relay. Runtime carries bytes, authority, geometry, bounded replay, and process lifetime only.
