# Core runtime

The runtrol core is a thin supervisor for coding-agent CLI processes. It starts and contains provider processes,
tracks lifecycle metadata, and transports their structured events. The provider CLI remains the owner of the
conversation and its transcript.

This document records the implemented runtime contract. Provider registration and discovery are described in
[providerArchitecture.md](providerArchitecture.md) and [providerDiscovery.md](providerDiscovery.md).

## Thin boundary

runtrol does not own, rewrite, index, or persist provider transcripts. Active routing uses provider-native identities
and official protocol or resume surfaces. A provider driver may make a bounded read-only scan of its provider-owned
store when the CLI exposes no catalogue surface, limited to native identity, workspace, timestamps, explicit title
records, and structured human-facing previews. It does not interpret prompts or replies to invent labels or keep a
conversation copy. Drivers otherwise normalize only the fields required for supervision, routing, consent, health,
usage, and continuity.

The ordinary redb database contains runtrol metadata only: runtrol and provider-native session identifiers, lifecycle
data, operator labels and pins, and local authorization metadata. A separate Mission ledger contains Mission, Task,
Run, Gate, Artifact, Receipt, and transition metadata. A bounded capability trust index contains project identity,
candidate state, exact digests, verification evidence, and retained approved versions. None of these stores contains
prompts, replies, event history, Gate output, environment values, credentials, or a transcript copy.

Mission files, instructions, handoffs, outputs, and capability bodies remain project files. Provider-native
conversations remain provider files. Deleting the Runtrol home removes session pointers, Mission evidence, and local
capability trust, but does not remove either class of owner data. The operational details are

## Runtime and admission

The daemon uses a current-thread Tokio runtime because supervision is I/O-bound. Blocking work is admitted through
a bounded pool sized from the maximum number of provider pipe operations plus a small fixed set of housekeeping
slots. Runtime features are enabled by the crates that use them instead of through a workspace-wide `full` feature.

Session admission is bounded separately. At most eight provider processes may be hot. A running turn is not evicted
to make room because doing so would discard work the operator is waiting for. The supervisor refuses admission when
all slots are protected by active work.

Provider structured transports use pipes rather than a terminal renderer. This preserves the provider's framing and
avoids turning console rendering behavior into a protocol dependency.

### Interactive housekeeping isolation

Filesystem root identity syscalls and process resident-memory queries do not execute on the current-thread async
executor. Root proofs use a bounded blocking lane and fail closed under the terminal contract. Resident-memory reads
return a cached bounded sample when available, schedule stale work on the blocking pool, and wake the relevant session
or terminal projection only when the observed value changes.

Post-invalidation provider inventory rebuilds run as coalesced background work. Each rebuild is revision-bound, so an
obsolete filesystem scan cannot replace a newer account or probe-cache state, and publication happens only after the
background result is current. On Windows, account-probe cleanup skips `EmptyWorkingSet` whenever any terminal is open.
The process-wide working-set release hook runs only at zero open terminals, so background account maintenance cannot
evict the PTY hot path's resident pages.

The terminal root-proof freshness and failure behavior are specified in
[terminalSurface.md](terminalSurface.md#live-authority-without-a-database-hot-path).

## Memory and idle CPU contract

The active gates measure the real debug daemon from outside the process. Provider fixtures and watch clients are not
charged to the daemon's RSS budget.

| Measurement | Windows | macOS | Linux |
|---|---:|---:|---:|
| Idle debug RSS | 20 MiB | 20 MiB | 48 MiB |
| Live debug RSS hard ceiling | 48 MiB | 48 MiB | 64 MiB |
| Eight hot idle sessions, peak increase from baseline | 5 MiB | 5 MiB | 5 MiB |
| One hot session plus four watchers, peak increase from baseline | 10 MiB | 10 MiB | 10 MiB |
| Residual increase after each live journey | 4 MiB | 6 MiB | 4 MiB |
| Idle process CPU during a 10 second window | 100 ms | 100 ms | 100 ms |

The Linux ceiling records the higher hosted debug measurement. The macOS residual allowance records measured
allocator retention. On macOS the daemon performs one early self-exec with the system allocator's
space-efficient policy. Its central supervised-command boundary restores the operator's original allocator
environment and removes private restart markers from every provider session and probe child. These numbers are
regression ceilings, not estimates of object sizes.

The first live journey holds eight independent provider sessions hot and idle at once, proving the full admission
set rather than extrapolating from one session. The payload journey admits one 900 KiB provider event and delivers
it completely to four real watchers. A separate
journey opens and closes three consecutive sessions whose provider emits a 15 MiB event. Each event is below the
framing parser's input limit but above the live event limit, so every watcher receives an explicit lag boundary and
the payload is not placed on the live wire.

After the final session closes, the gate allows at most eight consecutive 250 ms observation windows for allocator
settling. One complete window must stay at or below the platform residual ceiling. The byte ceilings remain exact,
and a failure reports the lowest complete-window sample observed during those two seconds.

After a provider session is fully released, GNU/Linux performs one explicit allocator trim and macOS requests
maximal pressure relief from all allocator zones at that lifecycle boundary. Neither operation adds a timer or
background worker.

The current evidence does not claim a release-build live ceiling or immediate return of every freed page to the
operating system.

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

## Generations: a running Runtime and the build that replaces it

A daemon is one generation of the Runtime: one build, identified by the SHA-256 of its executable. Every endpoint it
binds carries the first sixteen digits of that digest (`runtrol-<home>-<generation>` for private control,
`runtrol-runtime-<home>-<generation>` for the public Runtime), and every command run from an executable connects to
the endpoint named by that executable's own digest, starting that build when nothing listens there. Two builds
therefore never contend for a name, and a client never talks past the hello to a build other than its own.

The home's `runtime.locator.json` lists every generation currently serving (digest, both endpoints, version, process
id, start time, running turns, draining flag). Each daemon writes only its own entry, under the home's advisory lock,
and removes it at exit; an entry whose process no longer answers is dropped by the next generation that publishes.
`runtrol status` prints the list and probes each entry. The locator admits at most sixteen simultaneous live
generations. A seventeenth publish fails closed instead of creating an unbounded upgrade chain.

Installing an update writes a new content-named executable and no file is ever written over. The new generation
starts beside the running one and sends `drain` on the older generation's private endpoint. Drain is never refused:
the older daemon releases the durable store at once (the newer one is retrying its open and succeeds the moment the
file is free), stops taking new conversations, marks itself draining, and keeps serving the turns already running.
It exits by itself once no turn is running; idle processes end with it and reopen from the provider's own store under
the successor. Nothing is killed, nothing waits for an idle machine, and there is no gap between one daemon leaving
and the next arriving.

`drain` is permanently local. It carries `LocalScope::RuntimeDrain`, which no grant can hold, because choosing which
binary answers every later request is executable authority in the same sense as installing a provider.

A daemon built before generations listens on the bare home endpoint and publishes a locator of the earlier shape. A
starting generation recognises that shape, asks that daemon to retire on the bare endpoint, and keeps asking until it
exits; that path exists only for the one transition and carries nothing else.

The release gate `daemonCurrency` installs the exact VSIX into an isolated profile and requires that, within a bounded
time after activation, the newest generation listed for that profile's home is the build the VSIX bundles.

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
