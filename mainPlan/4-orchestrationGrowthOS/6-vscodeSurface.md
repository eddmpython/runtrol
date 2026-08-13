# VS Code surface

## Surface rule

Runtrol Studio remains the only PC surface. Sessions remain the primary navigation object. Mission controls must reuse
the current Core connection, selection store, exact workspace follow path, and one active conversation renderer.

No new desktop application, extension host process, local database, or duplicated provider subscription is allowed.

## Information architecture

The Activity Bar contribution contains these views:

```text
RUNTROL
  Sessions
  Missions
  Devices
```

Capabilities are not a permanent top-level view in v0. Candidate review is an inbox inside the selected Mission and
a command-palette entry. This keeps optional Growth concepts out of the normal single-session workflow.

## Mission list

Each row uses only daemon snapshot metadata:

- Mission display name from the Mission file
- canonical project label
- state
- passed Tasks over total Tasks
- running provider count
- approval wait count
- failed gate count
- last state transition time

The list does not read instruction, Handoff, report, Skill, diff, or conversation bodies. Search covers the displayed
metadata only. Stable sort keys prevent rows from moving while the user is acting on one.

## Mission editor

One reusable editor tab shows the selected Mission. It has four lightweight regions:

1. Task graph and current state
2. Selected Task contract and workspace
3. Gate and evidence summary
4. Local actions

The graph renders bounded state metadata only. Selecting a Task does not subscribe its provider stream. `Open Task
Session` switches the existing reusable conversation tab to that exact session and workspace through the current
selection path.

Only the visible conversation tab owns the full provider stream. A visible Mission editor may receive coalesced
Mission state events, but it never becomes a second conversation renderer.

## Mission review

Before `Start` is enabled, review shows:

- Mission digest and source file
- every Task instruction file with its digest
- discovered provider choice or required operator choice
- workspace mode and base identity
- declared input, output, and Handoff roots
- gate IDs and GateDefinition digests
- parallel, retry, repair, artifact, and hot process bounds
- requested local scopes
- validation warnings and hard refusals

Changed bytes invalidate the approval and return the Mission to `Draft`. The UI never keeps an optimistic local
approval after the daemon rejects or invalidates it.

## Runtime actions

| Action | Availability |
|---|---|
| Validate | Local, no execution authority |
| Start | Local only after exact review approval |
| Send Task Instruction | Local only for one exact `AwaitingInput` Task and reviewed digest |
| Pause | Local and scoped remote device |
| Resume | Local, or remote only when no scope expansion is required |
| Cancel | Local and scoped remote device |
| Panic stop | Always available through the existing security contract |
| Retry Task | Local only in v0 |
| Open Task Session | Local VS Code only |
| Review integration | Local only |
| Integrate | Local only and separately approved |
| Propose capability | Local only after a passed and accepted Run |

Every refusal names the missing state, scope, provider capability, workspace fact, or gate. There is no silent fallback
to a different provider or workspace.

## Evidence view

The evidence panel shows structured metadata and links to project artifacts:

- base and finish Git identity
- instruction and policy digest
- provider runtime observation
- declared artifacts and size
- gate status, duration, and typed failure
- receipt ID
- retry and repair history

Raw command output is live-only and bounded. If a report is a declared Artifact, VS Code opens the project file in a
normal editor. The extension does not copy it into extension storage.

## Integration view

The integration step uses native VS Code diff editors and source-control surfaces where possible. The Mission editor
shows which worktree and Receipt produced each candidate tree. It does not implement a second diff editor.

No automatic merge button exists in v0. A later product command may perform an exact reviewed integration, but it is
outside this initiative until a separate contract and rollback gate exist.

## Candidate inbox

The inbox is empty unless the user explicitly starts a proposal. A candidate card shows:

- kind, ID, project-only scope, and exact digest
- source Mission, Task, Run, and Receipt
- author and verifier runtime observations
- file diff in native VS Code diff editors
- referenced gates and verification results
- existing ID conflict
- destination path and rollback digest
- `Approve`, `Reject`, `Re-run verification`, and `Archive`

Approval is disabled when the project bytes change, the daemon trust snapshot is stale, the verification result is
incomplete, or the action is not local.

## PWA boundary

After `5-pwaConnection` and the base `6-pwaSurface` session journey graduate, the phone may add:

- Mission and Task status
- approval wait status
- pause, bounded resume, cancel, and panic stop
- non-sensitive gate outcome and receipt ID

The phone may not create or start a Mission, send a Task instruction, choose a provider, change an instruction, retry
a Task, integrate a tree, register a gate, approve a capability, change policy, or widen scope.

## Performance and accessibility

- 100 Missions and 1,000 Tasks use virtualized lists and incremental graph layout.
- Mission events are coalesced before extension host and webview updates.
- Hidden Mission views unsubscribe from live Mission events and recover from snapshot plus bounded replay.
- No polling timer updates relative time labels.
- Keyboard navigation, focus order, screen-reader labels, contrast, reduced motion, and zoom are release gates.
- Long titles and paths are truncated visually but remain available through accessible labels and copy actions.
- User typing, scrolling, file navigation, and conversation selection keep priority over graph layout and evidence work.

## Extension state boundary

Extension storage may retain only selected Mission ID and presentation preferences. Mission state, Task state,
receipts, instructions, artifacts, capability files, approval material, and provider observations remain with the daemon
or project owner.
