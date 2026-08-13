# Domain model

## Ownership

The Mission domain owns work coordination metadata. Provider sessions still own conversation state, Git owns source
history, project files own instructions and reusable procedures, and the security crate owns grantability.

## Entities

| Entity | Meaning | Durable owner |
|---|---|---|
| `Project` | Canonical repository and working-tree identity already used by Core admission | Existing Core and store metadata |
| `Mission` | One reviewed, bounded graph that pursues one project goal | Ledger state plus project Mission file |
| `Task` | One schedulable unit with exact instruction, inputs, outputs, workspace mode, provider selector, and gates | Ledger state plus Mission file |
| `Run` | One attempt to execute one Task with one provider session and workspace claim | Ledger metadata |
| `Artifact` | A declared file or bounded directory manifest produced by a Run | Project filesystem, metadata digest in ledger |
| `GateDefinition` | A deterministic check referenced by stable ID | Project policy file or built-in closed registry |
| `GateRun` | One execution of a Gate against one exact input identity | Ledger metadata |
| `Handoff` | Explicit references from one passed Task to a dependent Task | Project file plus metadata digest |
| `Receipt` | Content-addressed statement joining Run identity, artifacts, gates, policy, and outcome | Bounded ledger record, optional export file |
| `CapabilityCandidate` | Untrusted project files proposed for explicit later reuse | Project worktree |
| `CapabilityVersion` | One locally approved digest and verification record | Project files plus local trust index |

## Identifiers

IDs are generated values and are distinct from display names.

```text
MissionId       msn_<uuidv7>
TaskId          tsk_<uuidv7>
RunId           run_<uuidv7>
GateRunId       gtr_<uuidv7>
ReceiptId       rcp_<sha256>
ArtifactId      art_<sha256>
CapabilityId    cap_<uuidv7>
CapabilityVer   cpv_<sha256>
```

Provider IDs, provider-native session IDs, model observations, and binary fingerprints use the existing runtime
discovery types. The Mission schema treats them as opaque values. It never parses a vendor name or version order.

## Mission file

The project format is TOML because the workspace already has one strict TOML parser. The schema is closed and
versioned.

```toml
schema = "runtrol.dev/mission/v1alpha1"
name = "provider-neutral-session-recovery"
project_id = "prj_..."
base_ref = "main"
require_clean_base = true

[limits]
max_parallel_tasks = 2
max_hot_providers = 4
max_runs_per_task = 2
max_repair_cycles = 1
stop_on_critical_failure = true

[[tasks]]
id = "investigate"
instruction_ref = "instructions/investigate.md"
instruction_sha256 = "..."
workspace_mode = "read_only_base"
provider_selector = "operator_choice"
output_roots = [".runtrol/handoffs/investigate"]
gate_refs = ["artifact-report"]

[[tasks]]
id = "implement"
depends_on = ["investigate"]
instruction_ref = "instructions/implement.md"
instruction_sha256 = "..."
workspace_mode = "isolated_worktree"
provider_selector = "operator_choice"
handoff_refs = ["investigate-report"]
output_roots = ["crates/runtrol-daemon", "tests/audit"]
gate_refs = ["daemon-tests", "diff-policy"]
```

The example is a shape, not a provider list. Provider choice is either an exact discovered runtime ID selected during
review or an explicit operator choice before reservation. The file cannot name a model that runtime discovery did
not return.

## Mission state

| State | Allowed next states | Entry fact |
|---|---|---|
| `Draft` | `Validated`, `Rejected` | File loaded, no execution authority |
| `Validated` | `Ready`, `Rejected` | Schema, graph, policy, path, digest, and bound checks pass |
| `Ready` | `Running`, `Cancelled` | Local review approved the exact Mission digest |
| `Running` | `Paused`, `Blocked`, `Integrating`, `Failed`, `Cancelled` | Scheduler may reserve eligible Tasks |
| `Paused` | `Running`, `Cancelled` | No new Task reservation is allowed |
| `Blocked` | `Running`, `Failed`, `Cancelled` | Exact blocker is durable and visible |
| `Integrating` | `Completed`, `Failed`, `Cancelled` | Required Tasks passed and final local review is pending |
| `Completed` | `Archived` | Integration approval and integration gates passed |
| `Failed` | `Archived` | No allowed recovery path remains |
| `Cancelled` | `Archived` | Cancellation completed and owned processes were reconciled |
| `Archived` | none | Immutable terminal summary retained under quota |

`Ready` approval binds the Mission file digest, every instruction digest, the policy digest, and the project identity.
Any change returns the Mission to `Draft`.

## Task state

| State | Allowed next states | Entry fact |
|---|---|---|
| `Pending` | `Eligible`, `Skipped`, `Cancelled` | Dependencies not yet satisfied |
| `Eligible` | `Reserved`, `Blocked`, `Cancelled` | Dependencies and condition are resolved |
| `Reserved` | `AwaitingInput`, `Eligible`, `Blocked`, `Cancelled` | Provider slot, workspace claim, output claim, and scope reserved atomically |
| `AwaitingInput` | `Running`, `Blocked`, `Cancelled` | Exact provider session and workspace are ready, but no Task instruction has been submitted |
| `Running` | `AwaitingApproval`, `Verifying`, `Blocked`, `Retryable`, `Failed`, `Cancelled` | Exact provider session and workspace are bound to a Run |
| `AwaitingApproval` | `Running`, `Failed`, `Cancelled` | Provider-native approval ID and digest are durable |
| `Verifying` | `Passed`, `Retryable`, `Failed`, `Cancelled` | Artifact manifest is sealed and gates are running |
| `Retryable` | `Eligible`, `Failed`, `Cancelled` | Failure class permits another Run and budget remains |
| `Blocked` | `Eligible`, `Failed`, `Cancelled` | External blocker is explicit |
| `Passed` | none | Required artifacts, gates, approvals, and receipt are complete |
| `Skipped` | none | Declared condition resolved false before reservation |
| `Failed` | none | Failure is terminal for this Task |
| `Cancelled` | none | No owned work continues |

No event may skip a state transition method. Duplicate events are idempotent by event ID and expected prior state.

## Run identity

A Run binds these facts once:

- Mission ID and Task ID
- Run number within the Task budget
- provider runtime ID and binary fingerprint observed at reservation
- provider-native session ID after attachment
- canonical project and working-tree identity
- base commit or non-Git snapshot identity
- instruction path and digest
- policy digest
- selected capability version digests
- local Task submission action ID and optional opaque structured acknowledgement observation when input begins
- start time and monotonic duration source

Changing provider, workspace, instruction, policy, or capability creates a new Run. It cannot mutate a past Run.

## Handoff

A Handoff contains only explicit project artifacts and questions:

```toml
schema = "runtrol.dev/handoff/v1alpha1"
mission_id = "msn_..."
source_task_id = "tsk_..."
source_run_id = "run_..."
base_commit = "..."
finish_tree = "..."
policy_sha256 = "..."
receipt_id = "rcp_..."

[[artifacts]]
path = ".runtrol/handoffs/investigate/report.md"
sha256 = "..."
media_type = "text/markdown"
```

There is no transcript, prompt, reply, hidden reasoning, raw environment, or raw command output field. A dependent
Task reads a Handoff only because its reviewed instruction and Mission explicitly reference it.

## Schema compatibility

- Unknown top-level keys fail validation during the alpha schema.
- Unknown enum values fail closed.
- A newer wire client may list an unsupported Mission as unreadable but cannot start it.
- Schema migration writes a new record before switching the active schema pointer.
- Old records remain readable for rollback until the release rollback window closes.
- IDs and digests are never reused across migrations.
