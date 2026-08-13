# Orchestration kernel

## Architecture

The scheduler is a deterministic state machine above the existing Core. It does not own provider implementations,
process containment, session event content, or the workspace identity algorithm.

```text
VS Code and IPC
       |
       v
daemon composition
       |
       +-> orchestration scheduler -> ledger
       |
       +-> existing Core -> provider trait -> drivers
       |
       +-> security scope wall
```

The proposed production crates are:

- `runtrol-ledger`: Mission domain state, legal transitions, receipt encoding, persistence, and bounded queries
- `runtrol-orchestrator`: Mission validation, deterministic scheduling, reservations, effect intents, Handoff checks,
  gate coordination, retry policy, and integration readiness
- `runtrol-growth`: candidate schema, approved digest registry, verification lifecycle, explicit reuse outcome,
  quarantine, and rollback after Slice 4

The intended direct production edges are exact:

```text
runtrol-ledger       -> runtrol-provider
runtrol-orchestrator -> runtrol-provider, runtrol-security, runtrol-core, runtrol-ledger
runtrol-growth       -> runtrol-provider, runtrol-ledger
runtrol-daemon       -> runtrol-orchestrator, runtrol-ledger, runtrol-growth
```

The daemon keeps its existing edges and is the only composition root. Core never depends on the orchestrator, ledger,
or Growth. The orchestrator never depends on Growth. Ledger and Growth never depend on Core, drivers, IPC, transport,
daemon, or UI. The dependency-direction gate is the final SSOT if attempt measurements require a different edge.

The daemon executes scheduler effects through existing session, project, security, process-containment, and discovery
APIs and returns typed outcomes. Therefore a new provider remains a driver or manifest change and does not change the
Mission kernel.

## Activation flow

1. Load a Mission TOML file selected by the user.
2. Resolve the canonical project identity through the existing Core path.
3. Validate the closed schema and all referenced files.
4. Validate instruction, policy, GateDefinition, and Handoff digests.
5. Validate the DAG, conditions, retry bounds, parallel bounds, output claims, and workspace modes.
6. Ask runtime discovery which provider choices and structured input surfaces actually exist.
7. Present the complete resolved contract in VS Code.
8. Record local approval bound to the resolved Mission digest.
9. Move the Mission to `Ready` and allow the scheduler to reserve the first eligible Task.
10. Open or resume the exact session, enter `AwaitingInput`, and wait for the user at the PC to submit the reviewed
    Task instruction.

Remote callers cannot perform steps 1, 7, 8, 10, or policy-changing parts of step 6.

## Validation

A Mission is rejected when any of these is true:

- unknown schema key or enum value
- duplicate or missing Task ID
- dependency cycle or reference to a missing Task
- condition references a non-terminal or unrelated result
- zero or over-limit retry, repair, parallel, hot process, artifact, or gate bound
- instruction, policy, Handoff, or playbook path escapes the project
- instruction digest differs from the reviewed file
- two potentially parallel write Tasks claim one working tree or overlapping output roots
- a write Task does not request an isolated worktree
- a command gate is absent from the exact local registry
- an exact provider or model choice is not in current runtime discovery
- a requested provider capability cannot be established
- a remote-authored request names a local-only scope
- integration strategy is anything other than local manual review in v0

Validation is structural. It does not inspect whether a title, instruction, report, or Skill is sensible.

## Reservation

A Task reservation is all-or-nothing and obtains these resources in a single scheduler decision:

1. one Mission parallel slot
2. one global hot provider slot or a cold reservation that does not yet start a process
3. one canonical workspace claim
4. one output-root claim
5. required local scope evidence
6. one Run ID and durable `Reserved` transition

Failure releases every provisional resource and leaves the Task `Eligible` or moves it to `Blocked` with a typed
reason. Partial reservation never survives a scheduler tick.

Priority order is fixed:

1. user input and approval response
2. panic stop, cancel, and pause
3. existing provider I/O and process exit handling
4. gate completion and recovery reconciliation
5. foreground eligible Task
6. explicit capability verification
7. terminal compaction and statistics

The scheduler has no periodic polling loop. State changes and bounded timers wake it.

After reservation, daemon composition may open or resume the exact provider session and then durably enter
`AwaitingInput`. This is preparation, not Task execution. No background effect may cross the input boundary.

## Instruction submission

Only the local `Send Task Instruction` action may move `AwaitingInput` to `Running`. The daemon rechecks Mission,
instruction, policy, workspace, provider session, and capability digests at that moment. It then transports the
approved instruction bytes without transformation. A gate compares the source digest, transported bytes, and provider
input fixture bytes.

The action commits a unique submission intent before transport and records any structured acknowledgement exposed by
the discovered provider surface after transport. Absence of that surface remains an explicit ambiguity, not a guessed
acknowledgement. Recovery never retries an ambiguous submission automatically. The Run becomes `Blocked` until the
user chooses how to reconcile it.

Provider-native session creation semantics are discovered or declared in the provider boundary and exercised by
real probes. The orchestrator cannot branch on a provider name. When a provider lacks a safe structured input surface,
the Task becomes `Blocked` and the UI explains the missing capability.

## Workspace isolation and honest claims

The existing `ProjectIdentity` and `WorkspaceClaim` remain the SSOT for writer admission.

An isolated Git worktree provides:

- a distinct working-tree identity
- separate index and checked-out files
- an explicit base commit
- deterministic diff and tree comparison
- collision prevention at scheduler admission

It does not provide:

- filesystem confinement against a malicious or mistaken process
- read restriction to declared inputs
- network denial
- process sandboxing

Output-root checks are preflight validation and post-run evidence. They detect an unexpected diff and block Task
passing. They do not claim that the provider could not write elsewhere. Hard confinement may be claimed only after a
provider-native permission mechanism or an OS sandbox is discovered, applied, and tested. Absence fails closed for a
Mission that requires hard confinement.

## Scheduling patterns in v0

Only four patterns ship initially:

```text
single:       implement -> verify -> local integration
investigate:  investigate -> implement -> verify -> local integration
parallel:     contract -> branch A + branch B -> verify -> local integration
review:       implement -> independent review -> one repair -> verify -> local integration
```

Parallel write branches are limited to two. Repair is limited to one cycle. The independent review uses a different
provider runtime when the operator selected one, but the system does not invent a fallback if none is available.

Competitive variants, red-team loops, automatic provider handoff, recursive decomposition, and automatic merge are
outside v0.

## Handoff

A source Task seals its declared Artifact manifest before dependent Tasks become eligible. A Handoff references that
manifest and Receipt. It does not copy provider conversation.

The dependent Task receives access because its Mission and instruction explicitly name the Handoff. Runtrol does not
append a generated summary to the instruction. If a human-readable summary is useful, it is one declared Artifact
written and reviewed like any other project file.

## Gate execution

Mission files reference stable gate IDs. Only a local project policy can map an ID to a fixed executable and argument
vector. No Mission field accepts shell text.

The orchestrator validates a gate and emits a typed launch effect. The daemon-owned gate executor:

- resolves the executable through the same probed-program boundary used elsewhere
- binds the exact working directory identity
- enforces timeout and owned process-tree cleanup
- captures only bounded live output for the selected UI and does not persist it
- records result metadata and declared report Artifacts
- rejects unknown platform or missing executable
- never treats output prose as a pass condition

An `exit 0` command is useful only if the GateDefinition says that exact exit class is sufficient. Structured report
gates validate a closed report schema from a declared Artifact.

Network denial is not promised by registry text alone. A gate requiring network denial remains unavailable until an
OS-enforced sandbox implementation graduates on that platform.

## Recovery

On restart, the daemon restores ledger state before scheduling anything. For each non-terminal Run it reconciles:

- provider process ownership
- provider-native session resumability
- current workspace identity and claim
- instruction and policy digests
- artifact seal state
- gate process ownership and result
- approval expiry
- retry budget

Ambiguous state becomes `Blocked`. Starting a replacement Run requires a local decision or a deterministic recovery
rule that proves the previous Run cannot still write.

## Integration

v0 never merges automatically. It also never sends an eligible or retry Task automatically. When required Tasks pass,
the Mission enters `Integrating`. VS Code shows source
worktrees, base and finish trees, diffs, receipts, failed then repaired gates, and remaining conflicts. A local user
chooses integration outside or through an explicitly approved product command.

The final integration gate runs against the actual integrated tree. A branch gate does not prove the integrated tree.
