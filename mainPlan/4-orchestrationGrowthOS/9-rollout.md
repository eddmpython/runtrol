# Rollout

## Phase order

| Phase | Production result | Entry | Exit |
|---|---|---|---|
| 0. Falsify the contract | No production feature | `3-embeddableAgentRuntime` public contract stable | Thin boundary, instruction identity, state model, bounds, and topology survive failure probes |
| 1. Ledger | Existing single session can produce metadata-only Run evidence | Phase 0 | Restart, retention, secret exclusion, update rollback, and memory gates pass |
| 2. Single-task Mission | One reviewed Task prepares, waits for local Send, verifies, and recovers | Phase 1 | Real provider journey and VS Code minimal surface pass |
| 3. Bounded DAG | Two isolated branches, independent review, one repair, manual integration | Phase 2 | Two-provider product journey and performance ratchet pass |
| 4. Candidate inbox | One project-only non-executable capability can be approved and rolled back | Phase 3 | Verification, local approval, tamper detection, and remote denial pass |
| 5. Measured reuse | One later Mission explicitly references the approved version | Phase 4 | Controlled baseline proves user value without performance or security regression |

No phase leaves a stub, hidden feature flag, unused dependency, or uncalled gate in production. Work begins under
`tests/_attempts/orchestrationGrowthOS/` and graduates one coherent slice at a time through the repository's eight
steps.

## Files

Files change only when their owning phase graduates. The tables below name each production and attempt path so the
implementation can proceed without rediscovering ownership or mixing later Growth work into the Mission kernel.

### Phase 0

| Path | Change |
|---|---|
| `tests/_attempts/orchestrationGrowthOS/README.md` | Experiment question, environment, measurements, failure mutations, rejected designs, and graduation ledger |
| `tests/_attempts/orchestrationGrowthOS/` probes | Instruction byte identity, state recovery, DAG bounds, canonical Receipt encoding, and topology experiments |
| `mainPlan/4-orchestrationGrowthOS/` | Update only when measurements overturn a design fact |

Phase 0 compares the proposed crate boundaries against the current dependency gate. It does not modify `crates/`,
the VS Code extension, public claims, or North Star scores.

### Phase 1

| Path | Change |
|---|---|
| `crates/runtrol-ledger/` | IDs, Mission and Task states, legal transitions, canonical Receipt, redb schema, queries, quotas, compaction, and recovery snapshot |
| `crates/runtrol-core/src/home/` | One discovered ledger path inside Runtrol home |
| `crates/runtrol-daemon/src/compose.rs` | Open store and ledger once, establish locks, and report fail-closed damage |
| `Cargo.toml` and `Cargo.lock` | Add the graduated member using existing workspace dependencies only |
| `tests/audit/dependencyDirection.rs` | Register the exact new layer and forbidden transitive paths |
| `tests/audit/` | Evidence completeness, boundary, retention, canonical encoding, memory, and update rollback gates |

### Phase 2

| Path | Change |
|---|---|
| `crates/runtrol-orchestrator/` | Closed Mission parser, validator, scheduler, reservation, provider-neutral effects, gate coordination, Handoff checks, and recovery reconciliation |
| `crates/runtrol-security/src/scope.rs` | Local and device Mission scopes with no grant path for local-only actions |
| `crates/runtrol-ipc/src/wire.rs` | Versioned Mission and Task request, snapshot, event, and typed refusal DTOs |
| `crates/runtrol-daemon/src/compose.rs` | Compose Core, ledger, and orchestrator |
| `crates/runtrol-daemon/src/dispatch.rs` | Dispatch exact Mission requests through the scope wall |
| `crates/runtrol-daemon/src/scope.rs` | Map each new request to one scope requirement |
| `crates/runtrol-daemon/src/serve.rs` | Run one bounded scheduler task, execute typed gate effects, and coalesce Mission event fan-out |
| `crates/runtrol-cli/src/` | Diagnostic list, get, validate, start, pause, cancel, and watch surfaces |
| `extensions/runtrol-vscode/src/protocol.ts` | Mission wire schema validation |
| `extensions/runtrol-vscode/src/mission/` | Snapshot state, controller, tree provider, commands, and reusable editor protocol |
| `extensions/runtrol-vscode/src/extension.ts` | Register the minimal Mission view and commands |
| `tests/audit/` | Contract, instruction identity, state, DAG, scheduler, recovery, Handoff, gate registry, remote denial, and UI gates |

### Phase 3

| Path | Change |
|---|---|
| `crates/runtrol-core/src/project.rs` | Reuse and, only if required, extend canonical claims with an explicit Mission Run owner |
| `crates/runtrol-core/src/session/manager.rs` | Bind a Run to the existing exact session and workspace admission path |
| `crates/runtrol-orchestrator/src/` | Worktree lifecycle intent, two-branch scheduling, independent review, bounded repair, and integration readiness |
| `crates/runtrol-daemon/src/` | Execute exact Git and provider effects under existing process ownership |
| `extensions/runtrol-vscode/src/mission/` | Graph, evidence, worktree, and integration review presentation |
| `tests/audit/` | Workspace isolation, two-provider journey, hard-restart matrix, and Mission performance |

Git worktree creation uses explicit argument vectors and verified canonical paths. Production code does not build a
shell command string. Cleanup removes only a worktree whose Mission ownership and current Git metadata both match.

### Phase 4

| Path | Change |
|---|---|
| `crates/runtrol-growth/` | Candidate schema, bounded file validation, verification state, local digest trust index, activation, tamper detection, quarantine, and rollback |
| `crates/runtrol-security/src/scope.rs` | Local-only capability lifecycle scopes |
| `crates/runtrol-ipc/src/wire.rs` | Candidate snapshot and local action DTOs |
| `crates/runtrol-daemon/src/` | Compose Growth without giving it provider event or transcript access |
| `extensions/runtrol-vscode/src/capability/` | Candidate inbox controller, native diff links, evidence view, and actions |
| `tests/audit/` | Activation, no implicit use, tamper, rollback, provenance, remote denial, and uninstall gates |

### Phase 5

| Path | Change |
|---|---|
| Mission schema and review UI | Add exact capability version references already supported by the contract |
| Receipt schema | Record selected capability version IDs |
| `tests/audit/` and operator campaign | Fixed baseline and explicit reuse outcome |
| `docs/` | Graduate stable Mission, evidence, and capability contracts only after the user journey passes |
| `README*.md` and North Star board | Change only if a registered user-visible gate justifies a new claim or axis |

PWA code is not part of these phases. `5-pwaConnection` and `6-pwaSurface` consume only the stable bounded Mission
status and allowed remote actions after both initiatives reach their own entry conditions.

## Symbols and contracts

### Ledger

- `MissionId`, `TaskId`, `RunId`, `GateRunId`, `ReceiptId`, `ArtifactId`
- `MissionState`, `TaskState`, `RunOutcome`, and transition error types
- `MissionRecord`, `TaskRecord`, `RunRecord`, `GateRunRecord`, `ArtifactRecord`, `Receipt`
- `Ledger::open`, `Ledger::transition`, `Ledger::snapshot`, `Ledger::seal_receipt`, `Ledger::compact`
- quota and schema constants backed by measured SSOT values

### Orchestrator

- `MissionSpec`, `TaskSpec`, `MissionLimits`, `InstructionRef`, `ProviderSelector`, `WorkspaceMode`
- `MissionValidator`, `ValidatedMission`, and typed validation findings
- `Scheduler`, `Reservation`, `ResourceBudget`, `Eligibility`, and `SchedulerEffect`
- `GateDefinition`, `GateRegistry`, `GateRequest`, and `GateOutcome`
- `Handoff`, `ArtifactManifest`, `IntegrationReadiness`, and recovery decisions

The scheduler returns typed effects. It does not call a driver by name. Daemon composition maps effects to existing
provider-neutral Core APIs.

### Core and daemon

- `ProjectIdentity` and `WorkspaceClaim` remain the canonical workspace SSOT
- `SessionManager` remains the only owner of provider session lifecycle
- daemon `Composed` gains ledger and scheduler owners
- IPC `Request` and `Response` gain versioned Mission variants with explicit IDs
- scope mapping remains exhaustive, so a new request cannot omit authorization silently

### Growth

- `CapabilityId`, `CapabilityVersion`, `CandidateState`, `CapabilityKind`, and `CapabilityScope`
- `CandidateManifest`, `VerificationPlan`, `ApprovedDigest`, `TrustIndex`, and `ReuseOutcome`
- `propose`, `verify`, `approve`, `reject`, `quarantine`, `rollback`, and `archive` state methods

Capability APIs accept file metadata and digests, not provider conversation events.

### VS Code

- `MissionSnapshot`, `MissionTaskRow`, `MissionSelection`, `MissionController`, and `MissionTreeProvider`
- one reusable Mission editor protocol separate from the conversation renderer protocol
- `CandidateSnapshot` and `CandidateController` only after Phase 4
- daemon snapshot remains the state SSOT, with no optimistic completion

## Tests

Implementation follows this order for every gate:

1. Add the failure mutation or selftest and prove red.
2. Add the clean production behavior and prove green.
3. Register the gate in local preflight.
4. Register the correct hosted matrix or mark operator-only explicitly.
5. Run the focused unit and integration tests.
6. Run formatting, clippy with warnings denied, dependency direction, provider isolation, silent failure, workspace
   hygiene, no forbidden dash, and the full relevant preflight class.
7. Run the real product journey where the claim requires it.

Phase-specific required gates are listed in [Gates](8-gates.md). Each phase also reruns existing session, workspace,
process, security, memory, VS Code performance, update rollback, and uninstall journeys because those are the most
likely regression surfaces.

Schema tests include round trip, unknown key, unknown enum, corrupt row, old version, new unsupported version,
cross-platform canonical digest, and rollback generation. Recovery tests inject termination before and after each
durable boundary.

## Rollback

Rollback preserves provider-owned sessions and project-owned artifacts first. Each phase graduates as one explicit
path-scoped commit, so an unshipped slice can be reverted without mixing unrelated work or schema generations.

### Development rollback

Before a slice graduates, all code lives under its attempt category. A failed attempt is removed without production
migration or public compatibility debt. Production files are changed only by the coherent graduation commit.

### Released ledger rollback

- Ledger data uses a separate file and schema generation from existing session and device storage.
- A previous binary ignores the new file and continues normal session supervision.
- Upgrade writes a new generation before switching the active pointer.
- A failed migration leaves the prior generation intact.
- No downgrade rewrites a newer generation.

### Mission runtime rollback

- Pausing admission stops new reservations without killing unrelated existing sessions.
- Running Mission-owned sessions remain ordinary provider-native sessions and can be resumed outside Mission mode.
- Mission cancellation releases only claims and processes whose exact ownership is proven.
- Removing the VS Code Mission view does not remove project Mission files or provider sessions.

### Capability rollback

- Every activation retains the prior approved digest under quota.
- Rollback changes the local selected digest atomically and invalidates affected Mission drafts.
- Running Tasks keep their pinned version or are cancelled. They never switch version mid-Run.
- Removing Runtrol leaves project capability files readable but removes local trust state, so nothing remains silently
  active.

### Portfolio rollback

If Mission operation is useful but Growth is not, graduate Mission and evidence contracts to `docs/`, remove Growth
code and UI, and close the initiative without claiming verified growth. If Mission operation itself fails a kill
criterion, preserve any independently useful evidence boundary and return product priority to PWA connection.

## Evaluation

Evaluation combines an architecture and failure-mode review with a product-scope and user-value review. Both must
pass before the measured campaigns can authorize graduation.

### Developer review

Technical approval requires exact dependency direction, one state-transition SSOT, provider-neutral effects, bounded
queues and storage, byte-identical instruction transport, fail-closed recovery, honest enforcement claims, and no
new path to transcript or secret storage. Edge cases include link and junction escape, non-Git projects, changed
instruction bytes, scheduler send attempts, remote send attempts, provider drift, PID reuse, daemon death at every durable boundary, partial artifact traversal,
gate timeout, stale worktree base, cancelled approval, schema downgrade, and capability tamper during a Run.

The architecture is rejected if it needs a provider-name branch in Core, a second session owner, a semantic parser,
an unbounded journal, a shell command string, or documentation that claims confinement where only detection exists.

### Product review

Product approval requires the fixed North Star to become easier to use: fewer manual session and workspace switches,
no new concept burden for a single session, no hidden automation, local review before authority expands, and an
obvious exit when a Mission blocks. Scope is sufficient only when the real two-provider journey completes through
manual integration and restart. Scope is excessive if Growth enters the critical path before Mission value is proven.

Priority remains `autoUpdate -> local Mission proof -> phone connection -> phone surface`. Acceptance is based on the
measured operations, latency, collisions, duplicate work, gate results, and controlled reuse below, not on the number
of new types or screens.

### Mission value campaign

Use one frozen, real repository task that requires investigation, implementation, independent review, and tests.
Perform the workflow directly with two provider CLIs, then through the reviewed Mission, with the same providers,
instructions, base tree, and gates.

Record:

- user clicks and command submissions
- manual provider, session, and workspace switches
- time to first useful Artifact
- total wall time
- approval count
- repair count
- gate failures
- workspace collisions
- duplicate Task submissions
- abandoned Runs
- input, scrolling, and selection latency
- daemon and extension memory plus idle CPU

Slice 3 graduates only when it produces zero workspace collisions and duplicate submissions, preserves every existing
performance ceiling, reduces manual session and workspace operations by at least 50 percent, and does not increase
time to first useful Artifact by more than 10 percent. A faster result with missing gates does not count.

### Growth value campaign

Choose at least three distinct real project tasks for which one approved project capability has a plausible,
predetermined effect. For each task run a frozen baseline and explicit reuse at least twice, separating external
provider failures.

Slice 5 graduates only if reuse reduces at least one of deterministic gate failures, repair cycles, or user operations
without worsening the others, and creates no extra approval action on Tasks that do not select the capability.

The campaign reports raw counts and environment observations. It does not label a provider or capability "best".

### Final decision

- If Mission passes and Growth passes, graduate both stable contracts and add only claims backed by active gates.
- If Mission passes and Growth fails, ship Mission plus evidence and remove Growth.
- If Mission fails, stop before PWA surface changes and return to the existing session supervisor North Star.
- If security enforcement requires claims stronger than actual OS or provider mechanisms, remove the claim or the
  feature. Documentation cannot be the sandbox.
