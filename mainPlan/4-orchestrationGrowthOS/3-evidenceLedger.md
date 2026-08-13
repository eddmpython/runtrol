# Evidence ledger

## Purpose

The ledger answers whether declared work ran under the reviewed contract and whether deterministic checks accepted
its result. It is not a conversation history, command log, analytics warehouse, or replacement for Git.

The ledger serves both Mission recovery and later capability verification, but Mission execution must remain useful
without the Growth slice.

## Storage boundary

| Stored | Never stored |
|---|---|
| Mission, Task, Run, GateRun, Artifact, Receipt, and approval IDs | Prompt or reply text |
| Legal state transitions with event IDs | Provider transcript path or transcript copy |
| Canonical project and working-tree identities | Provider event payload or replay history |
| Base commit and finish tree | Raw stdout or stderr |
| Instruction path and digest | Instruction file bytes |
| Artifact path, type, size, and digest | Artifact body |
| Gate definition ID, definition digest, exit class, duration, and result | Arbitrary command line or shell source |
| Opaque provider runtime ID, binary fingerprint, model observation, and provider-native session ID | Environment variable name-value pairs |
| Policy digest, approval digest, scope class, and expiry | Bearer token, cookie, API key, device secret, or approval-sensitive body |
| Local Task submission action ID and optional opaque structured acknowledgement observation | Scheduler-generated or remotely supplied Task input |
| Capability ID and exact approved version digest | Skill body or hidden capability injection text |
| Adoption, rejection, quarantine, and rollback outcome | Semantic summary inferred from a conversation |

The ledger does not attempt heuristic redaction. Avoiding capture is the boundary. A token-like string cannot leak
through a field that does not accept arbitrary command arguments or output in the first place.

## Command gate evidence

A command gate stores a `command_ref` and the digest of the reviewed registry entry. The registry entry owns the
executable name, fixed arguments, working directory rule, timeout, and platform constraints. The ledger stores:

- command reference
- registry entry digest
- resolved executable binary fingerprint when available
- working directory identity, not an unrestricted path string
- start and finish time
- exit status class
- timeout, cancellation, or launch failure class
- declared output artifact digests

It does not store expanded arguments or output. If a test report is needed, the Mission declares a report file as an
Artifact and the user reviews that project file under the normal artifact contract.

## Receipt

A Receipt is a canonical, content-addressed statement. Canonical encoding is defined once in the ledger crate and is
covered by golden vectors on all three operating systems.

```json
{
  "schema": "runtrol.dev/receipt/v1alpha1",
  "mission_id": "msn_...",
  "task_id": "tsk_...",
  "run_id": "run_...",
  "project_id": "prj_...",
  "instruction_sha256": "...",
  "base_commit": "...",
  "finish_tree": "...",
  "provider_observation": {
    "runtime_id": "opaque-runtime-id",
    "binary_fingerprint": "sha256:...",
    "model": "opaque-runtime-observation",
    "native_session_id": "opaque-provider-session-id"
  },
  "artifacts": [
    {"path": "relative/path", "sha256": "...", "size": 1234}
  ],
  "gates": [
    {"id": "full-check", "definition_sha256": "...", "status": "passed"}
  ],
  "capability_versions": ["cpv_..."],
  "policy_sha256": "...",
  "outcome": "passed"
}
```

Time fields and durations live in the Run record and are excluded from the Receipt ID unless the canonical schema
explicitly includes them. The same logical evidence must hash identically on Windows, macOS, and Linux.

## Artifact manifests

Artifact collection is bounded before traversal begins.

- every Artifact root is relative to the canonical working tree
- links and junctions are resolved and rejected if they escape the root
- file count, total bytes, per-file bytes, path bytes, and traversal duration have hard limits
- a directory Artifact uses a sorted manifest with normalized relative paths
- Git tree IDs are preferred when the declared output is a Git-tracked tree at a known commit
- an oversize artifact fails with an explicit reason and is never partially represented as complete
- a changed artifact after sealing invalidates its GateRun input identity

Receipt creation does not copy Artifact bodies into Runtrol storage.

## Persistence and bounds

"Append-only" applies to the integrity of a live Run, not to infinite retention. The implementation uses these record
classes:

| Class | Retention behavior |
|---|---|
| Active Mission state | Retained until terminal and reconciled |
| State transition journal | Bounded per active Mission and compacted into a snapshot after durable checkpoints |
| Terminal Task and Run summary | Retained under a global record and byte quota |
| Receipt | Retained under the same quota, pin-able by explicit export |
| GateRun detail | Compacted after the rollback window, preserving final status and definition digest |
| Capability provenance | Retained while the approved version exists |

Exact quotas are frozen by the Slice 1 measurement campaign before production code graduates. Required properties
are fixed now:

1. No Mission can create an unbounded number of transitions, Runs, gates, or artifacts.
2. No query returns an unbounded result set.
3. Compaction is event-driven at lifecycle boundaries, never a polling worker.
4. Eviction is oldest-terminal-first and never removes active recovery state.
5. Eviction is explicit in the UI and leaves a tombstone summary, not a false complete history.
6. Export happens only on local user action and writes a plain project or chosen filesystem artifact.

## Crash recovery

The ledger commit order is:

1. Validate the expected prior state and event ID.
2. Write the next state and transition record in one database transaction.
3. Commit durably at lifecycle boundaries that can cause duplicate external work.
4. Only after commit, emit the scheduler effect or public event.

After restart, reconciliation compares durable intent with observed process, provider-native session, workspace claim,
artifact, and GateRun state. Ambiguity produces `Blocked`, never a guessed success or automatic duplicate Run.

Already passed gates may be reused only when instruction digest, artifact manifest, base and finish identity, gate
definition digest, policy digest, provider observation requirements, and capability versions all match.

## Database ownership

The proposed `runtrol-ledger` crate owns one database file and its schema lock. `runtrol-store` remains the owner of
session and device rows. The daemon is the only process that opens either database and establishes both locks before
recovery.

The ledger crate owns Mission domain states, legal transitions, canonical Receipt encoding, quotas, and queries. It
cannot depend on drivers, transports, UI, daemon, or provider transcript-capable event types.

## Removal and rollback

Deleting the Runtrol home removes local Mission state, receipts, and local capability trust decisions. It does not
remove provider sessions, Git history, Mission files, instruction files, Handoffs, or capability source files.

An older binary ignores a newer separate ledger file rather than downgrading it in place. A schema upgrade writes a
new generation and switches only after validation. Failed validation preserves the previous generation for update
rollback.
