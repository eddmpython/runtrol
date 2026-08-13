# Product contract

## Governing promise

Runtrol remains a thin, local supervisor for provider-native coding-agent sessions. This initiative adds an explicit
operations contract around those sessions. It does not add a reasoning engine.

The user-visible promise is:

> Review one explicit work graph, let Runtrol keep each provider session in the correct workspace, and accept only
> results that pass the declared evidence gates.

The existing session workflow remains available. A user who wants one provider session must not learn Mission,
Task, Gate, Handoff, or Capability concepts.

## Boundary table

| Runtrol may | Runtrol must never |
|---|---|
| Validate the syntax, references, bounds, scopes, and graph shape of a Mission file | Decide what a natural-language goal means |
| Start or resume a provider-native session in an exact workspace | Implement a central planner or agent loop |
| Present the exact approved instruction artifact and transport it after a local `Send` action | Submit any Task instruction from a scheduler, timer, remote device, or background loop |
| Transport structured provider events through the existing bounded path | Parse conversation content to infer progress, lessons, or routing |
| Track state, process ownership, workspace claims, artifact hashes, and gate outcomes | Discover or copy a provider transcript |
| Run locally registered deterministic gates | Treat model self-report as a gate |
| Present project-owned candidate files for local approval | Promote a candidate because its author says it is good |
| Pin an exact approved capability digest for explicit reuse | Inject a capability into an unrelated task automatically |
| Store opaque runtime observations about the selected provider | Hardcode provider models, versions, flags, home paths, or task semantics |

## Explicit instruction contract

Every executable Task has one `instruction_ref` pointing to a project-owned UTF-8 file and one expected SHA-256
digest. The Mission review shows the file and digest before activation.

When an eligible Task is ready, Runtrol:

1. Resolves the path inside the canonical project boundary.
2. Rejects links, traversal, changed digest, oversize input, or invalid UTF-8.
3. Reads the bytes once after approval.
4. Opens or resumes the exact provider session and displays `Ready for input` without submitting anything.
5. Requires the user at the PC to press `Send Task Instruction` for that exact Task and digest.
6. Transports exactly those bytes through the same provider-neutral input boundary used by a normal user submission.
7. Stores only the path, digest, user-action receipt, and opaque structured acknowledgement observation when the
   discovered provider surface supplies one, never the bytes.

No scheduler component can submit or add a system message, status request, handoff summary, capability text, retry
explanation, or completion request. If a Task needs those words, its reviewed instruction artifact must contain them,
and the user must submit it locally. A repair Task uses a separately reviewed instruction artifact and explicit
artifact references.

This contract is the first falsification target. If useful Mission execution requires scheduler-submitted text,
undisclosed generated prompt text, or remote Task submission, the Mission runtime does not graduate.

## Mission authorship

A Mission may come from three explicit sources:

1. A user-authored TOML file.
2. A project file written by a provider session at the user's request.
3. A previously approved project playbook instantiated with typed parameters.

All three produce the same project artifact and pass the same validation and review. Runtrol records the source kind
but does not infer authorship from file contents. A remote device may view and pause a Mission later, but it cannot
create or widen one.

## Completion semantics

A provider event may signal that the session is idle or that a turn ended. It cannot set Task state to `Passed`.
Passing requires all of these facts:

- the Task still owns its exact workspace claim
- the base identity and current Git identity are known
- every required artifact exists inside an allowed output root
- each artifact has a bounded manifest and digest
- every required gate completed against the same input identity
- every required approval is current and bound to the same request digest
- the evidence receipt committed durably

If any fact is unknown, the Task becomes `Blocked` or `Failed`. It never silently passes.

## User workflow

The initial workflow has these user decisions:

1. Choose an existing Mission file.
2. Review validation results, instruction files, provider selections, workspaces, gates, and bounds.
3. Start the Mission locally so the first eligible Task is prepared.
4. Press `Send Task Instruction` for each exact eligible Task. Parallel Tasks can be reviewed together but each send
   remains a distinct local action.
5. Answer provider-native approvals as they occur.
6. Review the final diff and evidence.
7. Integrate or reject locally.

Candidate capability creation is a separate, optional action after integration. It is not inserted into the critical
path of every Mission.

## Non-goals

- natural-language plan generation in Core
- conversation memory, search, summarization, or transcript export
- automatic subagent prompts or recursive repair loops
- scheduler, timer, background, or remote submission of a Task instruction
- implicit current project, current session, or current task server behavior
- model quality ranking based on prose or hidden provider behavior
- automatic provider fallback after a task has started
- automatic merge, rebase, or conflict resolution
- a hosted account, cloud state store, or model-key proxy
- user-wide or organization-wide skill promotion in the first release
- background self-evolution

## Release claims

Before Slice 3, public messaging may say only that Mission execution is experimental. "Agent Operations OS",
"verified growth", "best provider", and similar product claims are forbidden until the corresponding real journey
and ratchet are registered in the evidence board.

No North Star score changes because code exists. A user-visible axis can be added only with its gate, runner, claim
text, and all language variants in the same change.
