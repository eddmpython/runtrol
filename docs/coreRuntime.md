# Core runtime

The runtrol core is a thin supervisor for coding-agent CLI processes. It starts and contains provider processes,
tracks lifecycle metadata, and transports their structured events. The provider CLI remains the owner of the
conversation and its transcript.

This document records the implemented runtime contract. Provider registration and discovery are described in
[providerArchitecture.md](providerArchitecture.md) and [providerDiscovery.md](providerDiscovery.md).

## Thin boundary

runtrol does not discover, derive, read, index, or persist provider transcript paths. It does not reconstruct missed
conversation from provider files. Drivers normalize only the fields required for supervision, routing, consent,
health, usage, and continuity. Everything else stays opaque.

The redb database contains runtrol metadata only: runtrol and provider-native session identifiers, lifecycle data,
operator labels and pins, and local authorization metadata. It contains no prompts, replies, event history, or
transcript copy. Deleting the runtrol home removes that metadata, not the provider's own conversation record.

## Runtime and admission

The daemon uses a current-thread Tokio runtime because supervision is I/O-bound. Blocking work is admitted through
a bounded pool sized from the maximum number of provider pipe operations plus a small fixed set of housekeeping
slots. Runtime features are enabled by the crates that use them instead of through a workspace-wide `full` feature.

Session admission is bounded separately. At most eight provider processes may be hot. A running turn is not evicted
to make room because doing so would discard work the operator is waiting for. The supervisor refuses admission when
all slots are protected by active work.

Provider structured transports use pipes rather than a terminal renderer. This preserves the provider's framing and
avoids turning console rendering behavior into a protocol dependency.

## Memory and idle CPU contract

The active gates measure the real debug daemon from outside the process. Provider fixtures and watch clients are not
charged to the daemon's RSS budget.

| Measurement | Windows | macOS | Linux |
|---|---:|---:|---:|
| Idle debug RSS | 20 MiB | 20 MiB | 48 MiB |
| Live debug RSS hard ceiling | 48 MiB | 48 MiB | 64 MiB |
| One hot session plus four watchers, peak increase from baseline | 10 MiB | 10 MiB | 10 MiB |
| Residual increase after the live journey | 4 MiB | 5 MiB | 4 MiB |
| Idle process CPU during a 10 second window | 100 ms | 100 ms | 100 ms |

The Linux ceiling accounts for the desktop runtime mapped into the shared executable. The macOS residual allowance
records measured allocator retention. On macOS the daemon performs one early self-exec with the system allocator's
space-efficient policy. Its central supervised-command boundary restores the operator's original allocator
environment and removes private restart markers from every provider session and probe child. These numbers are
regression ceilings, not estimates of object sizes.

The live journey admits one 900 KiB provider event and delivers it completely to four real watchers. A separate
journey opens and closes three consecutive sessions whose provider emits a 15 MiB event. Each event is below the
framing parser's input limit but above the live event limit, so every watcher receives an explicit lag boundary and
the payload is not placed on the live wire.

After a provider session is fully released, GNU/Linux performs one explicit allocator trim at that lifecycle
boundary. The trim adds no timer or background worker.

The current evidence does not claim a release-build live ceiling, GUI memory, or immediate return of every freed page
to the operating system.

## Bounded delivery and replay

Every live watcher has two ordinary queue bounds and one reserved control slot.

| Boundary | Limit | Overflow behavior |
|---|---:|---|
| Queued frames per watcher | 64 | Retire the watcher with its exact lag boundary |
| Ordinary retained bytes per watcher | 256 KiB | Retire the watcher with its exact lag boundary |
| One live provider payload | 1 MiB | Larger payloads create an explicit lag boundary without live encoding |
| Replay frames per session | 64 | Evict the oldest replay entry |
| Replay payload bytes per session | 64 KiB | Evict old entries or retain a fixed loss marker |

Payload buffers are reference-counted across watchers. Fan-out therefore adds queue envelopes and references rather
than one payload copy per watcher. A single event larger than the ordinary 256 KiB queue byte budget may occupy an
otherwise empty queue, but it must still fit the 1 MiB live payload limit.

The replay ring is a short latency window, not history. A payload larger than the ring's 64 KiB byte budget is never
retained there. A fixed marker makes a later reconnect report the missing range instead of treating an empty ring as
complete coverage.

## Cursor and gap semantics

A watch cursor is the boundary of the next required event and has three parts:

| Field | Meaning |
|---|---|
| `stream` | One live hub incarnation. A daemon restart creates a new value |
| `epoch` | One provider attachment inside that stream |
| `seq` | The next dense event sequence required by the watcher |

If every event after a cursor remains in the bounded replay ring, reconnect returns each event exactly once. If the
cursor names another stream or epoch, lies outside the retained window, moves ahead of the live boundary, or crosses
a deliberately unretained frame, the acknowledgement contains both the requested cursor and the current live
boundary as an explicit gap.

The terminal watch surface prints a runtrol-owned `watch event  next` cursor line before each provider payload. The
payload that follows is emitted byte-for-byte without interpretation or rewriting. This keeps reconnect positions
machine-verifiable without making the terminal surface understand conversation content.

The local resilience gate disconnects a real local IPC watcher, verifies exact replay inside the window, hard-kills
the daemon, and starts a replacement with the same home. The replacement preserves provider-native identity and
continues through the provider's official resume surface. The old cursor receives a new-stream gap before new live
events begin.

This is not a lossless history claim. It does not claim remote-network behavior, a phone endpoint, or transcript
recovery.

## Process containment and restart recovery

Containment is established before provider discovery or process launch.

On Windows, all supervised descendants join a job object configured to terminate them when its last handle closes.
The kernel therefore removes them when the daemon exits normally, panics, or is killed without cleanup.

On Unix, a small stable keeper leads one process group and starts the provider as its child. Before the provider is
reported ready, the keeper durably activates a bounded guard containing its PID, kernel start identity, and boot
identity. The executable is deliberately not part of that identity: an update replaces the file behind a live keeper
without touching the process, so a lookup by name stops matching while the keeper is still the same process. The
daemon retains the other end of one private inherited control socket. An explicit success or bounded
error frame closes the launch handshake. The registry holds at most 64 records.

Closing that control socket, including when the daemon is killed without cleanup, makes the live keeper signal its own
current group. The syscall contains no stored numeric PID or PGID, and the keeper is still a group member at that
instant, so a reused identifier cannot redirect termination. When the provider exits, the keeper sends its native exit
status to the daemon and then terminates its own group, including any residual descendants and itself. The daemon
reaps the keeper and removes the durable guard only after no group member can execute.

A replacement daemon holds the exclusive store lock before it examines crash records. It never signals a recorded
numeric process or group identifier. It revalidates the keeper PID, kernel start identity, and executable, waits only
for non-zombie members to disappear, and refuses an ambiguous live group. No process-name or environment scan is
used. Windows provides the corresponding kernel-owned cleanup through the job object.

## Storage boundary

redb is the metadata store. It has no background worker thread and requires no external database service. The daemon
holds the store's exclusive process lock before Unix orphan recovery, which prevents a second daemon from treating
the first daemon's live children as crash leftovers.

Conversation content remains outside this database. Live frames exist only in the bounded queues and replay rings
described above.

## Executable evidence

| Gate | What it establishes |
|---|---|
| `noTranscriptCopy` | The storage crate cannot name any event type capable of carrying conversation payload |
| `egressContract` | Production drivers and storage contain no vendor session-path discovery surface |
| `memoryBudget` | Platform and profile-specific idle daemon RSS ceilings |
| `liveMemoryBudget` | Real ACP event delivery, explicit oversize lag, peak and residual RSS ceilings |
| `idleFootprintRatchet` | The idle RSS source of truth plus at most 100 ms process CPU per 10 seconds |
| `resilienceFaultInjection` | Exact bounded local replay, hard restart gap, and provider-native resume |
| `orphanReaping` | Pending and active Unix crash windows, keeper control EOF, and real process-group removal |

The claim registry and hosted runner coverage are maintained in
[northStarEvidence.md](northStarEvidence.md).
