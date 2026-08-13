# Orchestration and Growth OS

Status: design complete, implementation waits for `3-embeddableAgentRuntime` to graduate.

Order 4: this initiative follows the stable update and rollback contract plus the public Runtime consumer boundary,
then precedes the remaining phone connection and phone surface work. Existing phone security and transport
foundations stay valid.

## One sentence

**Operate an explicit, user-approved graph of provider-native sessions in exact workspaces, accept completion only from
deterministic evidence, and optionally preserve an approved project procedure for later reuse without reading or
rewriting a conversation.**

The "OS" label means an operations layer for sessions, tasks, workspace claims, permissions, evidence, and reusable
project procedures. It does not mean a general operating system, an LLM planner, or a new agent framework.

## Product placement

The existing North Star remains unchanged:

> One VS Code window operates every project, session, and agent immediately.

Mission operation deepens the word "operates". It does not replace the North Star or authorize a second product
surface. Sessions remain the primary product object, provider CLIs retain their own reasoning loops and transcripts,
and Runtrol remains the thin supervisor.

Mission is also a first-party proof that higher-level products can build on the public Runtime contract. Its VS Code
surface uses the same public client SDK for ordinary provider and session behavior. Mission-specific scheduling and
evidence methods extend the public contract only after their own gates pass. They do not create another private client
route to Core.

The first user-visible result is a `Mission` that binds explicit tasks to exact provider sessions and workspaces. The
second result, which may ship only after the Mission result is useful on its own, is a local approval inbox for a
project-owned capability candidate.

## Decisions after review

| Proposal | Decision | Reason |
|---|---|---|
| Explicit Mission and Task state machines | Accept | Hidden orchestration state cannot be recovered, audited, or bounded |
| Deterministic DAG scheduler | Accept | Runtrol can schedule declared work without becoming the reasoning agent |
| Evidence receipts | Accept with bounds | Completion needs evidence, but an unbounded append-only ledger would violate the resident-cost discipline |
| Provider-generated task plan | Accept only as an explicit project artifact | Runtrol validates shape and policy, never meaning |
| Task instruction delivery | Accept only as a local user submission of a reviewed `instruction_ref` | A scheduler submission is prompt injection even when the bytes came from a file |
| Worktree isolation | Accept as collision isolation | A worktree separates Git state but is not an OS sandbox and must never be described as one |
| Path and network scopes | Accept only when an OS or provider-native mechanism proves enforcement | Preflight checks and post-run diffs are detection, not confinement |
| Automatic learning triggers | Reject | Deciding that a conversation contains a lesson requires semantic inspection or untrusted self-report |
| Candidate capability | Accept after explicit user or Mission request | Candidate files are ordinary project artifacts until independent gates and local approval activate an exact digest |
| Automatic capability injection | Reject | Reuse must be an explicit Mission reference or user action, never hidden prompt material |
| Statistical provider routing | Defer | Five samples do not establish a reliable ranking, and task meaning cannot be inferred from conversation text |
| Self-evolution and competing variants | Exclude | It has no place before repeated real reuse proves that the simpler capability path helps |
| Automatic merge | Exclude | Initial integration remains a reviewed local action |
| Organization registry and sharing | Exclude | Personal local value must be proven first |
| YAML Mission format | Replace with TOML | TOML is already a workspace dependency and avoids a new parser and schema surface |
| Rich PWA Mission control | Defer to `6-pwaSurface` | The phone remains a bounded control surface after its connection layer exists |

## Invariants

1. Runtrol never generates a plan from natural language.
2. Runtrol never reads provider transcripts or interprets live conversation text.
3. Runtrol never creates, rewrites, summarizes, or augments task instructions.
4. A task instruction is a reviewed project file with a fixed digest and is submitted byte-for-byte only by an
   explicit local user action for that Task.
5. A provider statement that work is complete is not evidence of completion.
6. A task passes only after its declared artifacts and gates pass and a bounded receipt is durable.
7. Provider identifiers, models, flags, task surfaces, and versions remain runtime observations, not source constants.
8. A new provider still requires no Core edit.
9. Every retry, repair cycle, queue, replay window, hot process count, and stored record class is bounded.
10. The user can pause, cancel, inspect, and refuse integration at every stage.
11. Capability activation pins an exact approved digest. A later file edit is untrusted until reapproved.
12. Runtrol removal leaves provider sessions and project files usable without Runtrol.

## Delivery slices

| Slice | User result | Entry condition | Exit condition |
|---|---|---|---|
| 0. Contract proof | A content-blind Mission can be activated without hidden prompt construction | `3-embeddableAgentRuntime` public contract is stable | Failure mutations prove the thin boundary and instruction byte identity |
| 1. Evidence | One existing session run receives a bounded, metadata-only receipt | Slice 0 | Crash recovery, secret exclusion, and memory gates pass |
| 2. Single-task Mission | One approved task opens or resumes in the exact workspace, waits for local Send, and passes declared gates | Slice 1 | Real provider journey completes through restart |
| 3. Bounded DAG | At most two write branches use independent worktrees and join at manual integration | Slice 2 | Two real providers complete investigation, implementation, and independent review |
| 4. Candidate inbox | The user can preserve one project procedure as an approved digest | Slice 3 | Candidate validation, independent replay, approval, tamper detection, and rollback pass |
| 5. Measured reuse | A later Mission explicitly reuses the approved procedure | Slice 4 | Reuse improves a predetermined gate result without more user operations |

Slices are graduation boundaries, not a license to place stubs in production. Each new capability starts under
`tests/_attempts/orchestrationGrowthOS/` and moves into production only after the repository's eight graduation steps.

## Documents

| Document | Authority inside this initiative |
|---|---|
| [Product contract](1-productContract.md) | Thin boundary, user promise, non-goals, and release claims |
| [Domain model](2-domainModel.md) | Mission, Task, Run, Gate, Handoff, Capability, IDs, and state transitions |
| [Evidence ledger](3-evidenceLedger.md) | Stored metadata, receipts, retention, recovery, and secret boundary |
| [Orchestration kernel](4-orchestrationKernel.md) | Validation, scheduling, workspace claims, provider execution, and integration |
| [Growth plane](5-growthPlane.md) | Candidate, verification, activation, explicit reuse, quarantine, and rollback |
| [VS Code surface](6-vscodeSurface.md) | Mission and candidate workflows without a second heavy renderer |
| [Security](7-security.md) | Scope wall, honest enforcement levels, remote denial, and supply chain |
| [Gates](8-gates.md) | Failing audits, real journeys, performance ceilings, and release evidence |
| [Rollout](9-rollout.md) | Impact files, symbols, tests, rollback, evaluation, and phase order |

## Completion

This initiative completes only when all of the following are true:

1. A real user goal is split into an explicit Mission and reviewed before execution.
2. Two different installed provider CLIs work in exact, non-overlapping worktrees.
3. After the user explicitly sends each eligible Task instruction, investigation, implementation, independent review,
   one bounded repair, and manual integration complete.
4. Daemon and VS Code restarts recover exact Mission state without duplicate provider work.
5. Declared gates, not model prose, decide task completion.
6. The evidence store contains no prompt, reply, transcript path, raw command output, environment value, or credential.
7. One explicitly requested project capability is verified, approved, reused, tamper-detected, and rolled back.
8. Existing input, scrolling, selection, memory, idle CPU, provider isolation, and uninstall gates remain green.
9. The resulting stable contracts graduate to `docs/` and this initiative folder is deleted.

## Kill criteria

Stop or reduce the initiative to session plus evidence if any of these remain true after the relevant slice:

1. Useful task activation requires transcript reading, semantic parsing, hidden prompt text, or a Core LLM call.
2. A normal provider process cannot be kept inside an honestly described permission boundary.
3. Mission operation takes more user actions or more waiting than operating the same sessions directly.
4. Mission UI breaks the existing input, scrolling, selection, hot process, memory, or idle CPU ceilings.
5. Worktree claims cannot prevent two scheduled writers from sharing one working tree.
6. Crash recovery can duplicate a task without an explicit conflict state.
7. Capability candidates create approval noise or fail to improve a predetermined outcome in repeated reuse.
8. Provider-native cross-provider operation makes the same workflow available with fewer user operations.
