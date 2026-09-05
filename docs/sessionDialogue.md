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
unanswered wait exits unsuccessfully. Calling `courier` without a verb only checks admission.

For example, in a PowerShell shell tool, set that invocation's output encoding to UTF-8 before piping a body:

```powershell
$OutputEncoding = [System.Text.UTF8Encoding]::new($false)
'An explicit request' | & $env:RUNTROL_COURIER_EXE courier ask TARGET_SESSION --timeout 30
```

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

## Authority and lifetime

The named pipe rejects remote clients. Windows admission requires the current effective logon, an inherited
process token, the exact managed process tree, and kernel Job Object containment. A token copied to an unrelated
local process grants no access. Each connection proves its session again; an envelope cannot impersonate another
source. The courier endpoint and admission proof are private to their Runtime generation.

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
