# Mission Operations

Mission is the local, content-blind operations layer for provider-native coding sessions. It validates a reviewed
project graph, reserves exact workspaces, waits for local submission, and accepts completion only from sealed
artifacts and deterministic Gates. It is not a planner, an agent loop, or a conversation store.

## Ownership boundary

| Owner | Responsibility |
|---|---|
| Project | Mission TOML, instruction files, handoffs, output files, and reusable capability files |
| Provider CLI | Conversation, reasoning loop, credentials, native session record, and repository edits |
| Runtrol Core | Mission state, scheduler reservations, process and workspace ownership, Gate execution, and evidence metadata |
| Git | Base commit, linked worktree identity, tree state, and integration review |
| VS Code | Review and local actions through the existing Runtrol Studio extension |

Runtrol never generates, summarizes, expands, or stores a Task instruction. It never reads a provider transcript to
infer progress. Provider prose cannot pass a Task.

## Mission file

The closed schema is `runtrol.dev/mission/v1alpha1`. Unknown fields, malformed values, unsafe relative paths, cycles,
overlapping parallel output roots, unknown Gates, unavailable runtime IDs, changed instruction bytes, and unapproved
capability versions fail validation.

```toml
schema = "runtrol.dev/mission/v1alpha1"
name = "reviewed-change"
project_id = "operator-visible-project-label"
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
instruction_sha256 = "<lowercase sha256>"
workspace_mode = "read_only_base"
provider_selector = "operator_choice"
output_roots = [".runtrol/handoffs/investigation.txt"]
gate_refs = ["project-check"]

[[tasks]]
id = "implement"
depends_on = ["investigate"]
instruction_ref = "instructions/implement.md"
instruction_sha256 = "<lowercase sha256>"
workspace_mode = "isolated_worktree"
provider_selector = "runtime:<runtime-id>"
output_roots = ["src", "tests"]
gate_refs = ["project-check"]
```

`provider_selector` is either `operator_choice` or `runtime:<runtime-id>`, where the runtime ID is an exact current
observation. Provider names, versions, models, flags, and session paths are not schema constants.

The active bounds are:

| Boundary | Limit |
|---|---:|
| Mission file | 256 KiB |
| Instruction file | 256 KiB |
| Tasks in one Mission | 1,000 |
| Parallel Tasks | 2 |
| Hot provider reservations | 8 |
| Runs per Task | 2 |
| Repair cycles | 1 |
| Output roots per Task | 32 |
| Gate references per Task | 64 |
| Capability references per Task | 32 |

## Review and local authority

Validation binds the exact Mission digest, instruction paths and byte digests, canonical project and Git identity,
Gate definition digests, provider runtime observations, output roots, limits, and selected capability versions.
Starting requires a second local action carrying the exact Mission digest. That approval expires after five minutes.
Any changed Mission file, instruction byte, Gate definition, project identity, or capability trust state requires a new
validation.

The scheduler can reserve an eligible Task and prepare its workspace. It cannot submit text. A Task becomes ready for
input only after the operator binds an existing public Runtime session in the exact prepared workspace. `Send Task
Instruction` is a distinct local PC action and transports the reviewed UTF-8 bytes unchanged through the ordinary
provider-neutral session input boundary.

Mission creation, validation, start, Task preparation, Task Send, retry, integration, archive, Gate registration, and
all capability mutations are local-only requests. A paired remote device cannot widen a Mission or cause model input.

## Task and workspace lifecycle

The Mission state machine is:

```text
draft -> validated -> ready -> running -> integrating -> completed -> archived
                                  |              |
                                  |              +-> failure stays incomplete
                                  +-> paused, blocked, failed, or cancelled
```

Task scheduling follows dependency order. An eligible Task is reserved only within the Mission limits. Preparation
does one of two things:

- `read_only_base` binds the checked-out base worktree. Its declared outputs must be under `.runtrol/handoffs/`.
- `isolated_worktree` asks Git through fixed argument vectors to create one Mission-owned linked worktree and takes an
  exclusive writer claim for its canonical identity.

A linked worktree is collision isolation, not an OS sandbox. The Gate and provider processes still receive only the
enforcement that the operating system, provider, and existing Runtrol process boundary can actually prove.

## Gates, artifacts, and Receipts

A Gate is an exact local registry entry containing an ID, executable program, fixed argument vector, working-directory
mode, environment policy, and timeout. The maximum Gate timeout is 30 minutes. Runtrol does not construct a shell
command string and does not persist Gate stdout or stderr.

Verification rechecks the provider-native session pointer from the durable session store, the workspace identity,
base commit, instruction and policy digests, declared outputs, and every Gate. A passing Receipt uses the canonical
project worktree identity and contains only:

- Mission, Task, Run, project, base, and finish-tree identities
- runtime ID, provider binary fingerprint, optional structured model observation, and native session ID
- instruction and policy digests
- sorted artifact paths, sizes, and digests
- sorted Gate IDs, definition digests, and passing status
- explicitly selected capability version digests

The Receipt schema is `runtrol.dev/receipt/v1alpha1`. Its canonical JSON bytes produce the `rcp_<sha256>` identity.
At least one artifact and one passing Gate are mandatory. One Run permits at most 256 artifacts, 512 MiB of declared
artifact bytes, and 64 Gate results. Prompt text, replies, transcript paths, command output, environment values, and
credentials are not Receipt fields.

## Integration

When every Task has a passing Receipt, the Mission enters `integrating`. Completion remains a local manual action.
Core rechecks the current integrated project artifacts against all passing Receipts and reruns the distinct reviewed
Gate set on that tree. Only an exact match with all Gates passing moves the Mission to `completed`. Runtrol does not
merge, rebase, resolve conflicts, or select a branch automatically.

## Recovery and retention

The Mission ledger is a separate redb file from ordinary session pointers. It holds at most 100 Missions in one
bounded query, up to 4,096 state transitions per Mission, and no conversation content. Completion and archive compact
transition detail only after a terminal checkpoint. Active recovery state is never compacted.

After restart, Core revalidates the Mission, instruction, Gate, runtime, capability, and workspace identities. Pending
or eligible work can be scheduled again. A reservation with no submission is released. An in-flight or verification
boundary is ambiguous, so the Task and Mission become blocked instead of duplicating provider work. Resume is an
explicit local safe-resume action.

Deleting the Runtrol home removes Mission evidence and local trust state. It does not delete provider-native sessions,
the repository, instructions, handoffs, outputs, or capability files.

## VS Code workflow

Runtrol Studio contributes one Missions tree and native editor documents. It does not create another conversation
renderer. The normal sequence is:

1. Register fixed local Gates.
2. Validate a project Mission file.
3. Review the digest, limits, Tasks, providers, workspaces, Gates, and capability versions.
4. Start within the five-minute approval window.
5. Prepare each reserved Task and bind its exact provider session.
6. Send each exact instruction locally.
7. Verify the Task and inspect its Run and Receipt IDs.
8. Retry only a blocked or retryable Task within the declared bound.
9. Verify and complete integration locally.
10. Archive the terminal Mission.

### Composing the parallel-attempt Mission

Step 2 assumes a Mission file exists. Writing one by hand is the part that kept the commonest shape of this flow
unused: one instruction, tried several ways at once, compared, and the best kept.

Studio composes that file. It takes an instruction file the operator already wrote, an attempt count, and the Gate
that judges an attempt, and produces a Mission whose Tasks carry no dependencies (so they run at once) and each
declare `isolated_worktree` (so no two write the same files). Attempts are bounded at four because each owns a
worktree and a hot provider. Each attempt runs once with no repair cycles: a fan-out compares attempts, and
retrying one silently would mean the thing being compared changed while being compared.

Three things it does not do, each for a stated reason:

- **It does not save the file.** Studio writes exactly two things to disk, and an instruction is the prompt an
  operator gives an agent. The composed document opens in an editor; the operator reads it, chooses where it lives,
  and saves it. Step 2 then proceeds normally.
- **It does not create the instruction.** A Task is bound to the exact bytes of its instruction because those bytes
  are meant to have been reviewed. The digest is taken over the file as it already exists, never over text this
  surface reformatted.
- **It does not invent a Gate.** Build commands differ per project, and a Gate that checks nothing produces a
  fan-out whose attempts all pass regardless of what they did. The operator names a registered Gate. Registration
  is per project, not per fan-out.

## Verification and claim limit

`missionGrowthContracts` proves state, scheduling, exact Send, recovery, evidence, integration, local scope, explicit
reuse, tamper detection, and rollback contracts. `missionLiveJourney` uses two installed provider CLIs and production
Core IPC to complete five reviewed Tasks, two integrations, two archives, exact capability reuse, tamper detection,
rollback, and cleanup against deterministic loopback model endpoints. `missionLedger` requires a snapshot of 100
Missions and 1,000 Tasks to remain at or below 50 ms p95.

These gates prove product wiring and deterministic evidence. They do not claim that one provider or capability is
better, that a linked worktree is a sandbox, or that loopback model output represents account-backed model quality.

