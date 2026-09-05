# Managed session dialogue

The courier carries explicit, opaque UTF-8 messages between processes managed by one Runtime generation. It
never reads a provider transcript, chooses work, interprets an answer, or starts another model request.
The provider's normal shell tool and permission rules own execution of each courier command.

## Visible activation

A newly launched process has no dialogue mailbox. In the sidebar, choose **Enable dialogue for this live session**
on a live managed conversation. Its normal terminal opens and receives one ordinary visible instruction. The
installed command generates that instruction with `courier --guide`, using the same command help and executable
limits as `courier --help`. No provider configuration, hook, registration, or project instruction file is written.
The provider owns the instruction in its usual transcript and applies its own shell permissions.

The row shows when dialogue is enabled. **Disable dialogue for this live session** retires that mailbox, its calls,
and its pending waits. Starting the process again begins disabled. Re-enabling creates a fresh activation lifetime;
connections authenticated before that change cannot send, receive, or cancel work in the new lifetime. A failed
instruction write is disarmed under the same input lease, and any failure to disarm is reported explicitly.

The local public `terminals/setDialogue` operation requires the exact terminal input lease and current root grant.
It changes structural authority only. The Studio client sends the visible instruction through ordinary terminal
writes. The courier endpoint itself has no operation that can enable dialogue.

## Commands

Run the executable inherited as `RUNTROL_COURIER_EXE` with `courier --help` for the command syntax. The parser and
help text are owned by [`words.rs`](../crates/runtrol-cli/src/courier/words.rs). Bodies come from stdin. Command
arguments contain structural identifiers and deadlines only. Stdout contains one JSON answer; a refusal or an
unanswered wait exits unsuccessfully. The same reference explains identifier format and when an outgoing message ID
is generated. Identifier errors describe the rejected format without echoing the supplied value. Calling `courier`
without a verb only checks admission.

For a PowerShell shell tool, follow the invocation-local encoding setup in `courier --help` before piping a body.
Windows PowerShell can prepend a BOM when its console input encoding still has a preamble, even if output encoding
is already UTF-8 without a BOM. Its pipe also appends a newline. Callers that require an exact body without that
newline supply a raw UTF-8 input stream. The courier preserves these bytes; it does not normalize them.

The receiver explicitly invokes `courier wait`, reads the returned envelope, then supplies its answer on stdin to
`courier reply MESSAGE_ID`. An idle model cannot be awakened through a mailbox alone. It must already be running a
bounded wait or receive a new visible user instruction in its normal terminal.

`list` pages through enabled live managed process identifiers and root PIDs. Its cursor is exclusive. These are process
facts, not provider conversation IDs or inferred model states. `tell` returns the exact admission receipt. `inbox`
consumes one message immediately; `wait` consumes at most one before its deadline. A source filter leaves other
mail in order. `ask` admits a request and waits for its exact reply on one connection. `reply` names the received
ask, with its peer and deadline derived from the Runtime's call metadata. `cancel` withdraws the caller's call and
queues a bodyless notification when the bounded mailbox can admit it.

## Explicit rooms

The `room` commands group fixed participants for a bounded exchange. Opening a room makes the caller its owner and
first speaker. Only that owner may select another participant as speaker or close the room. A selected speaker
explicitly asks one participant, who replies through the ordinary exact-message reply command. The Runtime never
chooses the next speaker, starts another round, or wakes an idle model.

Each admitted round starts a fresh ask chain. An unanswered, cancelled, or abandoned round still consumes its
allowance; a refused ask does not. One round remains in flight until its reply is consumed or its call ends. The
final allowed reply remains readable. Closing a room, disabling or ending any participant, or reaching the room's
deadline immediately retires its calls and unread mail while preserving unrelated mail in order.

The [room core](../crates/runtrol-courier/src/courier/rooms/mod.rs) owns membership and speaker authority. Its
participant and round ceilings come from `Limits::INITIAL`; total rooms share the active-call ceiling. Completed
room metadata participates in the same expiry schedule, so an empty room cannot survive its deadline or accumulate
without a bound. Command words and their generated limit reference live in the
[room parser](../crates/runtrol-cli/src/courier/rooms.rs).

## Isolated workers

An enabled lead can invoke `courier spawn PROVIDER` to open an ordinary managed terminal in a Core-owned linked
worktree of its approved Git project. The command accepts an optional discovered model and an optional stdin task.
The [spawn parser](../crates/runtrol-cli/src/courier/spawn.rs) owns syntax; the
[admission state](../crates/runtrol-daemon/src/courier_gate/spawning.rs) owns worker depth and capacity.
Pending launches and live workers consume the same bounds. A worker cannot spawn another worker.

Each spawn freezes the lead checkout's committed `HEAD` before allocating its worktree. Staged, unstaged and
untracked changes stay in the source checkout; Runtrol neither copies, stashes nor commits them. The spawn receipt
identifies the exact base commit. Separate spawns can observe different commits if the lead's `HEAD` moves between
them, so a client that needs one shared base checks their receipts before dispatching work. Runtime discovery selects
the executable and validates any requested model. The final launch checks the original activation, current integration authority,
project identity and deadline again before process creation. A disabled, revoked or expired request cannot acquire
authority by waiting for Git or provider discovery.

The new worker begins with dialogue disabled. Its optional initial task remains in the courier's bounded memory
until the person enables dialogue and the worker explicitly consumes it. Studio's ordinary visible instruction
names that initial receipt. It does not insert the task into terminal input. The public
[`TerminalDescriptor`](../crates/runtrol-runtime-protocol/src/terminal.rs) carries lineage and project ownership;
Studio orders related live rows together and identifies their lead in the tooltip. Ending the lead leaves its live
workers running and visible.

The [worktree controller](../crates/runtrol-daemon/src/isolated_workspace/mod.rs) owns the sole durable record of
reservation, process and filesystem identities. Its short transactions preserve other Runtime generations' rows.
Git operations retain their resource lock until their command and descendants have stopped. Clean, unchanged
worktrees are removed only after exact terminal exit; dirty or committed work is preserved. A finite restart sweep
also requires the recorded Runtime incarnation to have ended. Unknown or replaced ownership is retained and reported.

Selecting a preserved worker conversation resumes its native session in that exact worktree under the original
project's current `session.resume` grant. The worktree controller verifies the recorded project and filesystem
identities, then reserves the new Runtime and terminal occupant before process creation. An old exit callback or
restart sweep cannot reclaim a live resumed occupant. This binding keeps the original base commit but does not
invent new worker lineage. Dialogue starts disabled again; calls from the ended process are not replayed.
Within the current Runtime, the previous terminal's claim must retire before another occupant can start. A live
foreign Runtime without an exact terminal-retirement proof keeps the retained worktree unavailable for resume.
Opening a different subdirectory inside a retained worktree is refused; its recorded workspace is not silently
changed, and a fresh launch cannot bypass the retained worktree's cleanup owner.
Read-only checks use the atomically published registry independently of Git cleanup. Final reservation and
process binding keep their exclusive ownership checks.

The registry upgrades its ownership schema under the shared writer lock before it can record a resumed occupant.
Older writers reject that schema, so rolling back the executable must preserve the upgraded registry and its
worktrees. Rows whose original ownership cannot be verified remain preserved without resume authority.

Windows command cleanup validates the bounded PID list returned by a successful Job query, retaining exact process
handles and checking their membership and termination before releasing the operation. A finished process may remain
briefly in the assigned count after leaving that list; count inequality alone is not a failed completion proof.
Query errors, capacity overflow, invalid counts, and an unproven final Job state still preserve the resource lock.

On Windows, migration publishes the existing registry inside a nonempty directory at its original path. Publication
excludes the legacy commit source before reading the latest record, so an old process with cached metadata cannot
overwrite the new document. Old rows remain readable, but automatic mutation preserves rows whose original format
did not record operation and process identities. An older Runtime cannot read the migrated container; code rollback
must preserve that container and its unreconciled worktrees. The
[migration owner](../crates/runtrol-daemon/src/isolated_workspace/registry/migration.rs) implements this boundary.

## Authority and lifetime

The named pipe rejects remote clients. Windows admission requires the current effective logon, an inherited
process token, the exact managed process tree, and kernel Job Object containment. A token copied to an unrelated
local process grants no access. Each connection proves its session again; an envelope cannot impersonate another
source. The courier endpoint and admission proof are private to their Runtime generation.

The [admission worker](../crates/runtrol-daemon/src/courier_gate/admission.rs) performs containment and process
ancestry inspection outside the Runtime's event loop so the event loop can continue serving terminal input.
It owns the listener's existing greeting permit until inspection ends, including after its caller is cancelled.
Command admission still rechecks the exact activation after that inspection.

[`CallEnvelope`](../crates/runtrol-courier/src/envelope.rs), [`CallRef`](../crates/runtrol-courier/src/id.rs), and
the [wire command types](../crates/runtrol-courier/src/wire/commands.rs) own the serialized fields. Reply waits match
both the call ID and its original ask ID. Retained old replies cannot consume or cancel a later call that reuses an
ID. A receipt means mechanical admission only; it does not mean that a model read or completed the request.

[`Limits::INITIAL`](../crates/runtrol-courier/src/limits.rs) owns body, mailbox, call, deadline, and traversal
ceilings. The wire owns frame expansion, listing, connection, and waiter bounds. JSON framing accommodates the
worst-case escaping of a valid body. The length prefix is checked before reading its payload. Full mailboxes refuse
new messages and preserve existing mail. Long waits have separate slots from short commands and a per-session
allowance, so they cannot exclude the sends and replies that wake them.

Waiters hold no body and release the state lock while sleeping. The expiry task sleeps until the next actual
deadline; an empty courier has no polling timer. Receive, deadline, explicit cancellation, session exit, and
connection abandonment release the corresponding in-memory state. Disconnecting an admitted ask releases its
pending request even when the target mailbox is full. A reply already completed remains available to the asker's
inbox. A refused duplicate does not own cleanup of the original request.

Delivery is an at-most-once handoff: if a reader disconnects after consuming a message, the Runtime cannot roll
that delivery back. Bodies never enter Runtime files, audit rows, telemetry, or diagnostics. The provider may keep
ordinary shell input and output in its own transcript. Runtime neither copies nor edits that transcript.

## Verification

The real named-pipe journey extends the owned admission and generation-handover harness:

```text
node extensions/runtrol-vscode/tooling/courierAdmission.mjs --core DEVELOPMENT_EXE --next OTHER_BUILD_EXE --commands
```

It exercises Unicode, filtering, duplicates, full mailboxes, both reply directions, role refusal, exact cancellation,
deadline expiry, waiter saturation, disconnect cleanup, explicit room rounds and authority, generation continuity,
and body absence from task-owned
Runtime files. Real-provider visual verification uses the isolated native Extension Host in
[`courierProviderHost.mjs`](../extensions/runtrol-vscode/tooling/courierProviderHost.mjs).
Its optional `--project` names an existing absolute project directory. The host keeps its Runtime state, viewer
profile and coordination under its own temporary root; teardown never removes the supplied project. Native agents
still have their ordinary filesystem permissions, so a mission that preserves the source checkout must restrict
their task to the allocated worktrees and verify the source files and Git index before and after.

The same harness accepts `--input-latency` to measure terminal write acknowledgements during a finite load of real
courier admissions. It reads the sample count and warm-input ceiling from the extension's
[performance budget](../extensions/runtrol-vscode/performance-budget.json). This isolates Runtime transport cost;
the native Extension Host journey also measures the Studio input path.

With `--spawn`, the harness creates a disposable Git project and proves isolated workers, public lineage, visible
activation before initial delivery, depth and capacity refusals, clean-only recovery, and unchanged original files.
Its provider is a command fixture. Real-provider acceptance additionally uses the native Extension Host journey.

With `--lifecycle`, pass the accepted baseline as `--core` and the new development executable as `--next`.
The journey leaves exact asks pending across upgrade and rollback, ends the original Runtime before restoring its
image, and abruptly terminates the upgraded Runtime while another ask is pending. It checks contained-process exit,
closed peers, no replay into the replacement generation, timeout retirement, malformed-frame refusals, and body
absence after all Runtime handles close. This process fault does not invoke or test the Rust panic hook.

The [body-residue scanner](../extensions/runtrol-vscode/tooling/bodyResidue.mjs) checks explicit markers in raw,
JSON-escaped and Unicode-escaped UTF-8 and UTF-16LE. New lifecycle probes create unique ASCII sentinels in memory.
Its mutation tests run with the extension's ordinary tests and reject injected copies without printing their bodies.
