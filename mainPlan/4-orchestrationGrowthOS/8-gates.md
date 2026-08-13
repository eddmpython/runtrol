# Gates

## Gate doctrine

Every new claim starts with a failure mutation. The clean implementation is run only after the mutation proves the
gate can become red. A file existing under `tests/audit/` is not evidence until the active runner calls it and the
public claim registry names it.

Static contracts, deterministic fixture journeys, real provider journeys, performance benches, and operator-only
checks remain distinct. Operator evidence does not raise a North Star score.

## Required suite

| Gate | Kind | Clean assertion | Required failure mutation |
|---|---|---|---|
| `orchestrationThinBoundary` | contract | No model API, transcript path, semantic event parser, generated prompt, or provider-name branch enters Mission code | Add one forbidden field or dependency and require red |
| `missionInstructionIdentity` | contract and smoke | No scheduler or remote path can submit, and a local Send transports reviewed file bytes exactly while the ledger stores no body | Prepend one hidden byte or invoke a scheduler send and require red |
| `missionStateMachine` | contract | Every legal transition works, every illegal or duplicate conflicting transition fails | Skip `Reserved` or reuse an event ID with different data |
| `missionDagValidation` | contract | Cycles, missing dependencies, invalid conditions, overlapping output claims, and unbounded loops fail | Inject one defect of each class |
| `missionProviderNeutrality` | contract | Mission and scheduler use runtime provider observations and adding a fixture provider changes no Core source | Add a provider-name conditional |
| `boundedScheduler` | smoke and bench | Global and Mission limits, reservation atomicity, priority, fairness, pause, and cancel hold | Leak one slot or schedule background work before input |
| `missionWorkspaceIsolation` | smoke | Parallel scheduled writers receive different canonical worktrees and overlapping reservation is refused | Alias two paths to one working-tree identity |
| `missionRecovery` | fault smoke | Hard restart recovers state without duplicate Task input or lost claim | Kill before local Send, after durable submission intent, and before acknowledgement |
| `missionHandoffBoundary` | contract | Handoff schema carries artifact references and no conversation body field or provider-home access | Add a transcript or free-form captured-output field |
| `gateRegistry` | contract and smoke | Only exact local registry references launch fixed argv under timeout and containment | Add shell text, argument interpolation, or unknown gate ID |
| `evidenceCompleteness` | contract | A passed Run has exact input, artifact, gate, policy, provider observation, and Receipt identities | Remove each required fact in turn |
| `evidenceBoundary` | contract | Ledger cannot accept prompt, reply, event payload, instruction body, raw argv, output, env value, or secret | Add one forbidden durable field |
| `evidenceRetention` | bench | Transition, query, artifact, and global byte bounds compact terminal data without removing active recovery state | Exceed every quota and require explicit tombstone behavior |
| `capabilityActivation` | contract and smoke | Candidate needs independent verification and local exact-digest approval | Attempt remote promotion or author-only verification |
| `capabilityTamperRollback` | smoke | Changed active bytes disable reuse and rollback restores a prior approved digest | Modify one approved byte and keep selection cached |
| `capabilityNoImplicitUse` | contract | Only an exact reviewed Mission reference can select a capability | Add semantic search or prompt prepend behavior |
| `remoteMissionDenied` | contract and smoke | Device grants cannot create, start, retry, integrate, register, promote, or widen | Encode every local-only action in a device grant |
| `vscodeMissionPerformance` | bench | Mission UI keeps existing input, scrolling, selection, memory, and renderer ceilings | Render all Task nodes and subscribe every session |
| `missionLiveJourney` | real smoke | Two installed provider CLIs finish the reviewed investigate, implement, review, repair, verify, and local integration flow | Change one discovered provider surface and require visible refusal |
| `capabilityReuseJourney` | real smoke | One approved project capability is explicitly reused, measured, tamper-detected, and rolled back | Change version digest or gate definition between runs |
| `uninstallLeavesMissionArtifacts` | smoke | Removing Runtrol home leaves provider sessions, Mission, Handoff, and capability project files usable | Store the only copy of one required artifact in Runtrol home |

Each audit documents its owner, runner, platform matrix, runtime class, timeout, network need, cleanup contract, and
which public claim it can support.

## Product journeys

### Single-task journey

1. Create a clean temporary repository and explicit instruction file.
2. Validate and approve one Mission through production IPC.
3. Prepare a real or protocol-fixture provider in the exact workspace and require zero input before local Send.
4. Perform the local Send and prove byte-identical instruction input.
5. Produce one declared Artifact.
6. Run one registered gate.
7. Hard-kill and restart at each durable boundary in separate mutations.
8. Require one passed Receipt and zero duplicate submissions.

### Two-provider journey

1. Freeze one real repository task and its Mission before execution.
2. Locally send the investigation Task to provider A in read-only mode.
3. Locally send the implementation Task to provider B in an isolated worktree.
4. Locally send independent review to provider A in read-only mode.
5. Allow at most one repair in the implementation worktree.
6. Run branch gates and then integration gates on the reviewed integrated tree.
7. Require exact process cleanup and no overlap.

Account-backed work is operator evidence until a safe hosted real-provider runner exists. The fixture journey remains
the CI floor but cannot claim provider quality.

### Capability journey

1. Start proposal only from a local explicit action.
2. Write candidate files in a separate Run.
3. Verify with a distinct Run and fixed fixture.
4. Approve one exact digest locally.
5. Reference it explicitly in a later Mission.
6. Measure baseline and reuse outcomes.
7. Tamper with one byte and require immediate unavailability.
8. Roll back to the prior digest and repeat the fixed gate.

## Performance ceilings

The attempt campaign measures a clean baseline and freezes stricter ratchets before production graduation. These are
initial maximum release targets, not current claims:

| Measurement | Maximum |
|---|---:|
| Snapshot of 100 Missions and 1,000 Tasks | p95 50 ms |
| Ready Task scheduling decision | p95 20 ms |
| One in-memory state transition before durability | p95 10 ms |
| Initial bounded Mission graph presentation | p95 300 ms |
| Background capability verification concurrency | 1 |
| Idle Mission and Growth CPU | No timer-driven polling, existing daemon idle ceiling unchanged |
| Selected Task conversation transition | Existing hot and cold session ceilings unchanged |
| Full provider stream subscriptions | Exactly one |

Ledger RSS, database cache, transition quota, Receipt quota, artifact count, artifact bytes, Mission count, Task count,
and event replay bounds must receive numeric ceilings from the attempt measurements before Slice 1 graduates. No
placeholder adjective such as "small" or "bounded" can enter production documentation.

## Runner coverage

Every graduated gate is added to:

- the audit crate or audit command registry
- local preflight
- the relevant hosted workflow matrix
- `docs/northStarEvidence.md` only after it supports a public claim
- `tests/audit/northStar/board.toml` only when the scoring rules actually permit that evidence

The runner list is generated or coverage-checked so a new Mission category cannot be silently omitted. A skipped
platform or missing credential is printed as an explicit non-claim.

## Existing gates that must stay green

At minimum, changes remain blocked by provider isolation, dependency direction, no transcript copy, egress contract,
scope grantability, workspace overlap, process containment, resilience fault injection, memory budget, idle footprint,
VS Code extension performance, update rollback, workspace hygiene, formatting, clippy, and silent-failure gates.
