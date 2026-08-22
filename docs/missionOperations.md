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
completion_policy = "all_tasks"

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

`completion_policy` defaults to `all_tasks`, which preserves the ordinary DAG contract: every Task must pass and
every passing Receipt contributes integration evidence. `choose_one` is the narrow comparison contract. It requires
two through four dependency-free Tasks with the same instruction, handoffs, output roots, Gates, and capability
versions. Every Task uses an isolated worktree, runs once, has no repair cycle, and terminal failure does not stop the
other attempts. Overlapping output roots are permitted only inside this contract. Integration review begins after
every attempt is terminal and at least one passed. Final verification names one passing Task and uses only that
Task's Receipt and Gates. If no attempt passes, the Mission fails.

The active bounds are:

| Boundary | Limit |
|---|---:|
| Mission file | 256 KiB |
| Instruction file | 256 KiB |
| Tasks in one Mission | 1,000 |
| Parallel Tasks | 2 normally, 4 for a validated `choose_one` comparison |
| Hot provider reservations | 8 |
| Runs per Task | 2 |
| Repair cycles | 1 |
| Output roots per Task | 32 |
| Gate references per Task | 64 |
| Capability references per Task | 32 |

## Review and local authority

Validation binds the exact Mission digest, instruction paths and byte digests, canonical project and Git identity,
Gate definition digests, provider runtime observations, output roots, limits, and selected capability versions.
Starting requires a second confirmed local action carrying the exact Mission digest. `Continue Reviewed Mission` is
that action for an ordinary Mission. That approval expires after five minutes.
Any changed Mission file, instruction byte, Gate definition, project identity, or capability trust state requires a new
validation.

The scheduler can reserve an eligible Task. It cannot prepare a workspace, start a provider, or submit text by itself.
One confirmed `Continue Reviewed Mission` action composes the currently safe local operations: it verifies exact
finished sessions with fixed Gates, prepares newly eligible workspaces, binds public Runtime sessions, rechecks the
instruction bytes, and transports them unchanged through the ordinary provider-neutral session input boundary.
Granular Start, Prepare, Send, and Verify commands remain available for explicit recovery. None is available to a
remote caller.

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

When every Task has a passing Receipt, the Mission enters `integrating`. Landing remains a local explicit action.
Core rechecks the current integrated project artifacts against all passing Receipts and reruns the distinct reviewed
Gate set on that tree. It collects and compares Artifact evidence before the Gates and again after them. Only both
exact matches with all Gates passing move the Mission to `completed`. Runtrol does not merge, rebase, resolve
conflicts, or select a branch automatically.

For an ordinary `all_tasks` Mission, **Review and Apply Mission Landing** replaces a blind jump from Receipts to final
verification. Studio rejects missing Receipt evidence, unsafe or non-canonical paths, case-folded overlapping project
targets, missing workspace Artifacts, Receipt size or SHA-256 mismatches, non-file project targets, symbolic links
below either reviewed root, dirty text, notebook or custom-editor tabs for an Artifact, non-UTF-8 Artifact text, and
review sides above 8 MiB combined before it opens a review. It preflights every stat size before allocation and reads
through fixed-length file handles that verify file identity and EOF. It combines
every sealed Artifact into one native VS Code changes editor, up to a fixed 1,024 Artifact bound. The left side is the
current project at review time and the right side is the exact passing Receipt result. Both are bounded read-only
in-memory snapshots and contain no conversation content.

**Apply, run Gates and complete** re-fetches the exact Mission and Receipt identities and re-reads every source and
target before the first write. Mission, Receipt, source, target, existence, link, or editor drift refuses the whole
operation. Studio holds one cross-window lease for the canonical project through Core completion. Each target is
prepared with exclusive creation in its verified parent, byte-checked, and atomically renamed only after one final
source and target compare. If a later replacement or verification fails, rollback changes a target only while it still
contains the exact bytes written by this action, then reads it again to prove restoration. It never stages or commits.
Core compares all live project bytes with every Receipt before and after the fixed Gates. Gate failure or Gate mutation
leaves visible Git changes and the Mission in `integrating`; the retained Landing can retry Core without rewriting
already exact files. If the Core commits completion but its response is lost, Studio refreshes the snapshot and accepts
success only when the Mission is `completed`, the reviewed Mission and Receipt authority is unchanged, and every
applied byte is still exact. A retry that already observes those facts performs no second write or completion request.
During managed Core supersession, an older busy Core without Artifact evidence makes Landing unavailable instead of
throwing or trusting Receipt paths alone. If other ordinary Missions are waiting across projects, Studio reports the
exact count and offers **Review next**.

A `choose_one` Mission uses the same bounded and atomic Landing transaction with a narrower authority. The operator
selects one passing Task from the Mission or its Task row. Studio names that Task in the native winner multi-diff and
binds its Task ID, workspace, Run, Receipt, Artifact paths, sizes, and SHA-256 evidence into the review identity. No
other candidate contributes an Artifact. **Apply <winner>, run Gates and complete** writes only that Receipt, passes
the same Task ID to Core, reruns its fixed Gates, and completes only if the project still exactly matches the selected
Receipt. The terminal ledger record retains that selected Task and Receipt through compaction. Studio requires both
identities when recovering a lost completion response, so equal candidate bytes cannot make the wrong winner appear
successful. Changing the selected Task, its evidence, either side's bytes, or the Mission authority refuses the action.

## Recovery and retention

The Mission ledger is a separate redb file from ordinary session pointers. It holds at most 100 Missions in one
bounded query, up to 4,096 state transitions per Mission, and no conversation content. Completion and archive compact
transition detail only after a terminal checkpoint. Active recovery state is never compacted.

After restart, Core revalidates the Mission, instruction, Gate, runtime, capability, and workspace identities. Pending
or eligible work can be scheduled again. A reservation with no submission is released. An in-flight or verification
boundary is ambiguous, so the Task and Mission become blocked instead of duplicating provider work. Resume is an
explicit local safe-resume action.

For a blocked Mission whose reviewed contract still exists, Studio exposes **Recover Interrupted Mission** on the
Mission row. The focused confirmation names the full Mission and policy digests, project, exact Task workspaces, and
runtime-discovered provider assignments. It states that provider input before the restart may already have caused
external effects and that fresh sessions can repeat them. Esc closes the confirmation without changing Core state.
An already open Mission document is refreshed with the blocked snapshot at the same time as its tree row, so the
operator never sees a stale running document beside a blocked action.

After confirmation, Studio fetches the Mission again and requires the same digest, policy, Task states, instruction
digests, provider selectors, workspace modes, paths, and base commits. It reopens only blocked Tasks, safely resumes
the exact scheduler, starts fresh provider-native Runtime sessions, rechecks instruction bytes, and transports the
unchanged instructions through the ordinary Send boundary. Both `all_tasks` and `choose_one` use the same shared wave
runner. If Studio or Core stops between Task reopen and Mission resume, eligible or reserved Tasks remain part of the
same explicit recovery boundary, so the next action completes only the unfinished steps. A second uncertain provider
Send remains ambiguous and is never repeated automatically. `unavailableAfterRestart` means the reviewed contract
could not be reconstructed, so Studio refuses recovery and requires validation or cancellation instead.

Studio also persists the narrower boundary where Core committed a local Send intent but the public Runtime delivery
did not return success. Mission Momentum will not treat that session as finished after an Extension Host restart.
The operator must use the exact Task recovery action. No prompt or provider output is stored with this marker.

Deleting the Runtrol home removes Mission evidence and local trust state. It does not delete provider-native sessions,
the repository, instructions, handoffs, outputs, or capability files.

## VS Code workflow

Runtrol Studio contributes one Missions tree and native editor documents. It does not create another conversation
renderer. The normal sequence is:

1. Register fixed local Gates.
2. Validate a project Mission file.
3. Review the digest, limits, Tasks, providers, workspaces, Gates, and capability versions.
4. To start later without keeping Studio open, select `Schedule Reviewed Mission...`, choose the local due time and
   runtime-discovered providers, then confirm the exact Mission and policy authority. Core owns this one-shot wake.
5. For an attended ordinary Mission, select `Arm Mission Auto Flight` within the five-minute approval window. One exact
   local confirmation starts the first eligible wave and authorizes later safe waves while this Studio window is open.
   `Continue Reviewed Mission` remains the one-wave explicit path.
6. Auto Flight observes Runtime lifecycle rows. When a sent Task has completed one proven provider turn, it runs fixed
   Gates, seals the Receipt, and starts the next eligible DAG wave without another click.
7. For a Core-interrupted blocked Mission, use **Recover Interrupted Mission** once to reopen its exact Tasks, resume
   its scheduler, and start fresh sessions under the duplicate-effect warning. Use granular recovery for a failed,
   retryable, missing, or ambiguous Task. Working sessions, person or quota waits, comparison policy, and integration
   never advance by inference.
8. Open **Review and Apply Mission Landing**. For `all_tasks`, inspect the combined Receipt multi-diff. For
   `choose_one`, select one passing Task and inspect its winner multi-diff. Apply, rerun Gates, and complete in the one
   explicit action. Use direct completion only as a recovery path after a separately integrated tree.
9. Archive the terminal Mission.

A `choose_one` comparison keeps its specialized **Run All Reviewed Attempts** and **Compare Passing Results** flow;
winner selection and application then use the same exact Landing safety boundary as an ordinary Mission.

When several ordinary Missions are ready across the operator's projects, `Continue Ready Missions` is the bounded flight deck.
It reads the current exact snapshots, puts running safe work before a new start, and lists every selected project,
full Mission digest, and safe action in one local modal. One confirmation advances at most eight Missions through the
same requests described above. More ready Missions stay counted for the next review. An expired review, a Task that
needs recovery, a waiting-only Mission, `choose_one`, and integration never enter the batch. One failed Mission is
reported without preventing an unrelated Mission from reaching its own exact safe result. Operator-choice Tasks may
share one runtime-discovered provider selected for that flight, or keep individual choices. Newly started native
conversation tabs are arranged once after the batch. This is local composition, not a Core scheduler or remote start
surface.

### Core-owned scheduled start

A validated Mission can hold one durable one-shot schedule. Studio freezes the schedule ID, optional replaced schedule
ID, due instant, Mission and policy digests, every reviewed Task identity and instruction digest, workspace mode,
provider selector, and complete Task-to-runtime-provider mapping. The final confirmation shows the local and ISO due
times plus that authority. Studio fetches the current snapshot again after confirmation before it asks Core to commit.

Core accepts schedule and exact cancellation only from the machine-local Mission-start scope. A new schedule either
observes no pending schedule or names the exact pending schedule it replaces. Cancellation binds the current Mission
digest and schedule ID. Repeating the same schedule ID with the same authority is idempotent; reusing it for different
authority is refused. Due time is bounded from one second through 366 days.

The schedule actor is owned by Core, not the Extension Host. It stores only reviewed structural authority in the
Mission ledger, wakes from the wall clock, and rechecks current capability, Mission, policy, Gate, Task, provider, and
workspace authority. A successful claim atomically enters `launching`, starts the existing Mission scheduler, and
reserves eligible Tasks before any provider process or input. The actor then connects to Core as a local client and
uses the existing `MissionPrepareTask`, `Start`, `MissionBindSession`, `MissionSendTaskInstruction`, and `Prompt`
requests. It contains no provider-specific flags, transcript parser, conversation storage, or model credential.

Core restart preserves `pending`. A reclaimed `launching` schedule may continue only while provider input is still
provably unsent. Send intent is durable before `Prompt`; loss after an ambiguous submission changes the schedule to
`attention` instead of repeating input. Changed authority becomes `refused` with a closed structural reason. Pending,
launching, started, cancelled, refused, and attention states are visible in the Mission document and tree.

### Mission Auto Flight

Auto Flight is one bounded PC-local arm for reviewed ordinary Missions. The Missions title action can arm up to eight
exact Mission digests in one modal. A row action arms one Mission. Operator-choice Tasks share the one
runtime-discovered provider selected at arm time; reviewed fixed selectors remain fixed. The row shows a rocket and
`AUTO`, and its inline stop action revokes future provider input immediately.

The arm persists only the Mission ID and SHA-256, the optional runtime-discovered provider choice, and Task, session,
and lifecycle-generation identifiers. It stores no instruction, provider output, event, transcript path, Gate output,
or Artifact bytes. Runtime row changes drive reevaluation, so Auto Flight adds no timer or polling loop and does not
become another scheduler or agent loop.

An automatic Send follows a strict order. Core first commits the exact reviewed Send intent. Studio then records the
bound session's current `sessionGeneration` in extension global state before any provider input is submitted. That
Task can be verified automatically only after the same session returns to `hotIdle` with a greater generation. A
still-idle session is never completion evidence. If the process stops between any of these steps, the existing
ambiguous-submission marker or generation marker makes recovery explicit instead of duplicating or optimistically
verifying work.

Working Tasks and person or quota waits retain the arm. Pausing a Mission retains it without advancing. Expired or
changed authority, ambiguous delivery, a missing or replaced session, Gate failure, retry or recovery state,
comparison flow, cancellation, and any other stopped state disarm it. Reaching `integrating` also disarms it and
offers Receipt Landing. Exact reviewed Artifact application, final Gate verification, and Mission completion share
one explicit operator action. Semantic merging and conflict resolution remain outside Runtrol.

Every person wait, safety stop, or arrival at Receipt Landing is staged in a durable Studio outbox before Auto
Flight gives up its input authority. The entry contains only a random signal UUID, Mission ID and digest, and one
closed structural kind. Studio submits that exact UUID to Core, removes the arm only after Core acknowledges the
idempotent record, and retries the same UUID after an Extension Host restart. Rearming clears older signals for that
exact Mission digest. Core validates the current Mission, digest, state, and bound session before an authenticated
phone can see a signal. This crash boundary cannot duplicate a wake or let uncertain delivery retain automatic
provider-input authority.

### Composing the parallel-attempt Mission

Step 2 assumes a Mission file exists. Writing one by hand is the part that kept the commonest shape of this flow
unused: one instruction, tried several ways at once, compared, and the best kept.

Studio composes that file. It takes an instruction file the operator already wrote, an attempt count, discovered
provider choices, and the Gate that judges an attempt. It produces a `choose_one` Mission whose Tasks carry no
dependencies and each declare `isolated_worktree`. Attempts are bounded at four because each owns a worktree and a
hot provider. Each attempt runs once with no repair cycles: a comparison treats failure as a result, and retrying one
silently would mean the thing being compared changed while being compared.

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

After the operator saves and validates the document, `Run All Reviewed Attempts` replaces the repeated start,
prepare, bind, and Send actions for this reviewed shape. One modal confirmation names the Mission digest and exact
provider assignment. Studio prepares every linked worktree and public Runtime session, rechecks every instruction,
submits the exact bytes, opens the native conversation tabs, and invokes the existing VS Code grid command.

After verification, `Compare Passing Results` groups the same sealed Artifact path across passing worktrees and opens
one native VS Code diff per attempt against the current project file. It does not merge files. The operator selects a
passing Task from the Mission or its Task row, reviews the exact winner Receipt, and uses one public action to apply
its bytes and complete. Core verifies the project against only the selected Receipt and retains its exact Task and
Receipt identities as terminal evidence.

## Verification and claim limit

`missionGrowthContracts` proves state, scheduling, exact Send, recovery, evidence, integration, local scope, explicit
reuse, tamper detection, and rollback contracts. `missionLiveJourney` uses two installed provider CLIs and production
Core IPC to complete five reviewed Tasks, two integrations, two archives, exact capability reuse, tamper detection,
rollback, and cleanup against deterministic loopback model endpoints. `missionLedger` requires a snapshot of 100
Missions and 1,000 Tasks to remain at or below 50 ms p95.

`fleetComparisonSmoke` runs two installed provider CLIs concurrently in distinct linked worktrees through production
IPC. It seals different content for the same Artifact, rejects the non-matching passing Task, accepts only the exact
selected Receipt already represented by the committed Core fixture, archives the Mission, and proves session cleanup.
The product application proof is the focused real Extension Host eye pass. It compares both candidates, selects
`attempt-2`, rejects an apply request naming `attempt-1`, opens the exact winner Receipt, invokes the public primary
action, changes the project to `attempt 2`, reaches `completed`, and cleans every owned session. The comparison,
winner review, confirmation, and completed screenshots are inspected directly at 1456 by 906. The Core unit journey
also proves that integration Gate requests use the selected Receipt Run and that selected Task and Receipt evidence
survives terminal ledger compaction.

The focused `missionFlightDeckEye` Extension Host journey also runs two separate Git projects through one installed
provider CLI. It opens all two-file Receipt Landings in native multi-diff editors, completes the first Mission while
the second stays `integrating`, opens the next queued Landing, completes both, and exits with every owned process
clean. The reviewed, completed, and next-Landing screenshots are inspected directly. This focused eye pass is product
evidence and does not create a new North Star score axis.

The focused `missionAutoFlightEye` Extension Host journey arms one two-wave dependency Mission once against an
installed provider CLI. It proves two provider sessions, two fixed Gate verifications, two sealed Receipts, zero
operator continuation actions, arrival at `integrating`, and automatic authority removal. The reviewed, armed, and
arrived screenshots are inspected directly. This is product evidence, not a new North Star score axis.

The focused `missionScheduleEye` journey opens VS Code 1.132.1 at 1456 by 908, schedules a validated Mission against an
installed provider CLI, captures the pending state, and closes the first Extension Host before the due instant. The
isolated Core stays alive with no Studio process. After the due instant a second Extension Host reconnects to the same
Core and observes the already started provider-native session, Mission `running`, schedule `started`, Task `running`,
and the real provider reply. Both screenshots are inspected directly. Core tests separately prove restart persistence,
one durable due claim, idempotent finish, and exact cancellation. This is product evidence, not a new North Star axis.

These gates prove product wiring and deterministic evidence. They do not claim that one provider or capability is
better, that a linked worktree is a sandbox, or that loopback model output represents account-backed model quality.
