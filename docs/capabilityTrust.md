# Project Capability Trust

Runtrol can preserve one reviewed project procedure as an exact, locally trusted version for explicit later use. A
capability remains ordinary project text. Runtrol stores trust metadata and digests, never an injected prompt body.

## Scope and file contract

The initial scope is exactly `project`. There is no user-wide, organization-wide, hosted, or automatic promotion.
Supported non-executable kinds are `skill`, `gate_recipe`, and `playbook`.

A candidate directory contains:

```text
SKILL.md
capability.toml
verify.toml
references/
  optional.md
  optional.txt
  optional.toml
  optional.json
```

The closed schemas are `runtrol.dev/capability/v1alpha1` and
`runtrol.dev/capability-verification/v1alpha1`. Unknown fields fail. A candidate has at most 64 files and 1 MiB in
total. Every file must be regular UTF-8 text with no NUL byte. Links, executables, binary files, path traversal, and
unlisted file types fail inspection.

`capability.toml` binds the capability ID, kind, project scope, payload digest, source Mission, Task, Run, passing
Receipt, reviewed policy, optional parent version, and project-selected license. `verify.toml` binds a distinct author
Run and verifier Run, a project-relative replay instruction, a fixed fixture, and existing fixed Gate IDs.

The content digest covers `SKILL.md` and `references/`. The version digest covers the complete sorted tree, including
both closed metadata files. File paths, byte sizes, and file digests are part of the stable tree identity.

## Provenance requirements

Proposal is a local explicit action. Core accepts it only when the source Receipt exists in the Mission ledger and
matches all of the following:

- the canonical project worktree
- source Mission, Task, and Run IDs
- the exact source Receipt ID
- the reviewed policy digest
- an artifact path that contains the candidate output and binds its bytes

The verifier Run must be distinct from the author Run, have its own passing Receipt, and match the fixed replay and
fixture plan. Provider self-report and candidate text are not provenance.

## Lifecycle

```text
proposed -> candidate -> verifying -> verified -> active
                 |            |           |
                 +-> rejected +-> candidate on Gate failure

active -> tampered, quarantined, stale, or archived
tampered or quarantined -> candidate, rolledBack, or archived
```

Verification re-inspects the candidate bytes and runs the exact fixed Gate definitions. A passing verification creates
a separate verification Receipt in the local trust index. Approval is another modal local action that names the exact
version digest, source Receipt, and verification Receipt.

Activation atomically moves the candidate directory to
`.runtrol/capabilities/active/<capability-id>/`. Replacing an active version first moves the prior exact tree under the
project archive. At most eight approved versions are retained per capability.

## Explicit reuse

Approval does not inject capability text into a session. A later Mission must list both `capability_id` and the exact
`version_sha256` in a Task. Validation and local Send fail if that project version is absent, changed, quarantined,
stale, or otherwise not currently trusted. The selected version digest is sealed into the passing Task Receipt.

This makes reuse visible in the Mission review and evidence. It does not make Runtrol interpret the procedure or
decide which Task should use it.

## Tamper and rollback

Every trust listing re-inspects the active project tree. A one-byte change makes the capability `tampered` and removes
it from the approved selection set immediately. The changed bytes are never silently blessed.

Rollback is allowed only from `tampered` or `quarantined`. The operator chooses one retained approved digest. Core
atomically archives the displaced tree, restores the exact prior tree, verifies its digest, and records
`rolledBack`. A failed move restores the displaced tree instead of reporting success.

Quarantine, rollback, reject, approve, archive, proposal, and verification are local-only. A paired remote device has
no grant path for capability mutation.

## Storage and uninstall

Candidate, active, and archived bodies are project-owned files. The bounded trust index is Runtrol-owned metadata in
the Runtrol home. Removing Runtrol leaves all project files readable and leaves provider sessions with their native
owner. Removing the Runtrol home removes trust, so no project procedure remains silently active after reinstall.

## VS Code workflow

The Capability Candidate Inbox is part of Runtrol Studio. It uses native folder selection, Markdown review documents,
modal confirmations, and the built-in VS Code diff editor. It adds no Webview and no background verifier.

1. Propose one candidate directory inside the open project.
2. Review its project, kind, source Receipt, version digest, and current state.
3. Run independent fixed Gates.
4. Open the native file or diff review.
5. Approve the exact digest locally, or reject it.
6. Quarantine, archive, or roll back an exact version when needed.

## Verification and claim limit

The Growth unit and contract gates cover closed schemas, source and verifier Receipt binding, fixed Gates, exact
activation, explicit Mission selection, selected-version Receipt evidence, tamper unavailability, retained versions,
rollback, remote denial, and uninstall boundaries. The installed two-provider Mission journey exercises v1 approval,
later explicit reuse, v2 approval, one-byte tamper, and v1 rollback through production IPC.

This evidence proves the trust lifecycle. It does not claim that a capability improves model quality or choose a
capability automatically. Any comparative value claim requires a separate predetermined baseline campaign.

