# Growth plane

## Hypothesis

A successful Mission sometimes creates a project-specific procedure, verification recipe, playbook, failure
signature, or context document that should survive the provider session. Preserving it is useful only when later
reuse improves a predetermined result without more user operations.

Growth is optional and follows a useful Mission runtime. It never justifies weakening the thin boundary.

## Candidate sources

A candidate can be created only by:

1. A local user choosing `Propose capability from this Run`.
2. A reviewed Mission containing a separate, bounded candidate Task with its own `instruction_ref`, submitted through
   the same local per-Task action.
3. A user-authored project file imported into the candidate inbox.

There is no automatic trigger based on tool-call count, failures, corrections, conversation text, or model claims.
Runtrol may display deterministic facts such as "this Run added a new registered gate", but the user decides whether
to start candidate creation.

## Capability kinds

v0 supports three non-overlapping kinds:

| Kind | Purpose | Executable payload |
|---|---|---|
| `skill` | Human-readable procedure and references | None |
| `gate_recipe` | References to existing locally registered gates | None |
| `playbook` | Parameterized Mission template with closed typed fields | None |

Failure signatures, automatic routing rules, executable scripts, user-wide packages, organization packages, and
offline variants remain excluded. An executable verification belongs in the local GateDefinition registry and is
approved separately. A capability cannot introduce a new command by hiding it beside `SKILL.md`.

## Project format

```text
.runtrol/capabilities/
  candidates/
    <candidate-id>/
      SKILL.md
      capability.toml
      verify.toml
      references/
  active/
    <capability-id>/
      SKILL.md
      capability.toml
      verify.toml
      references/
  archive/
```

`SKILL.md` follows the public Agent Skills shape where compatible. `capability.toml` carries Runtrol-specific scope,
ownership, provenance, compatibility observations, and exact content digest. `verify.toml` can reference only existing
GateDefinition IDs.

The files remain ordinary, portable project files. Local trust is not inferred from their directory name.

## Trust model

The local registry activates an exact digest after approval. This distinction is essential because an ordinary
provider process working in the project may be able to edit `.runtrol/capabilities/active`.

- a file in `active/` whose digest has no local approval is untrusted
- an approved file whose digest changes becomes `Tampered` and is unavailable
- reapproval creates a new immutable capability version
- an old approved version remains available for rollback under quota
- Runtrol never claims it physically prevented the provider from editing project files

The trust index stores only IDs, digests, state, approval, and evidence references. It does not store the Skill body.

## State machine

| State | Allowed next states | Meaning |
|---|---|---|
| `Proposed` | `Candidate`, `Rejected` | Files exist but have no verification claim |
| `Candidate` | `Verifying`, `Rejected` | Closed schema, path, size, and duplicate ID checks pass |
| `Verifying` | `Verified`, `Candidate`, `Rejected` | Independent declared gates run in a bounded verification Run |
| `Verified` | `Active`, `Rejected` | Evidence is complete, local approval still required |
| `Active` | `Tampered`, `Quarantined`, `Stale`, `Archived` | Exact approved digest may be explicitly referenced |
| `Tampered` | `Candidate`, `RolledBack`, `Archived` | Project bytes no longer match the approved digest |
| `Quarantined` | `Candidate`, `RolledBack`, `Archived` | Reuse evidence indicates harm or incompatibility |
| `Stale` | `Candidate`, `Archived` | Runtime or project observation changed and revalidation is required |
| `RolledBack` | `Archived` | A prior approved version became active |
| `Rejected` | `Archived` | Candidate was not accepted |
| `Archived` | `Candidate` | Explicit local restoration only |

No transition to `Active` is remotely grantable. There is no `Trusted` marketing state in v0. Evidence is shown as
counts and exact gate outcomes instead of a broad trust label.

## Verification

Candidate verification checks:

- closed metadata schema and bounded file tree
- no path escape, link escape, hidden executable, binary, or oversized file
- all GateDefinition references already exist and are locally approved
- source Receipt, policy digest, and parent version are present
- candidate author and verifier Runs are distinct
- replay instruction and fixed input fixture are explicit project artifacts
- all required gates pass against the candidate digest
- no provider transcript, credential path, environment value, or raw command output is included

Secret scanning can be an additional warning gate, but it is not the primary guarantee. The design prevents arbitrary
runtime output from entering a candidate automatically and requires local diff review.

## Approval

The local inbox shows:

- exact file diff
- candidate kind and project-only scope
- source Mission, Task, Run, and Receipt
- author and verifier provider observations
- every verification GateRun
- existing capability ID conflict
- current and rollback digests
- files and commands that later reuse may reference

Approval pins the exact digest and moves the reviewed files into the project active location using an atomic local
operation. If the destination already differs, activation stops rather than overwriting it.

## Explicit reuse

A capability can affect a Task only when the reviewed Mission contains its exact `capability_id` and version digest,
or the user adds it during Mission review. Runtrol exposes the project file as a referenced resource. It does not
prepend the Skill to task input or choose one by reading task prose.

The Task instruction is responsible for telling the provider to read a referenced capability when needed. The
Receipt records only the capability ID and version digest.

## Outcome attribution

Reuse is counted only against a predetermined evaluation contract. A Task passing is not enough to prove the
capability caused success.

For the first measured reuse campaign:

1. Freeze the task fixture, provider observation, instruction, gates, and environment class.
2. Run a baseline without the capability reference.
3. Run with the exact capability version.
4. Compare deterministic gate outcome, repair count, elapsed time, and user operations.
5. Record external failures separately.
6. Repeat enough times to expose variance before a product claim.

No automatic provider ranking or capability selection ships from this data. A later routing initiative would need a
separate decision, sample-size contract, confidence method, drift policy, and failure mutation.

## Quarantine and rollback

Any of these facts removes a version from selection immediately:

- project bytes no longer match the approved digest
- a required GateDefinition was removed or changed
- a reuse campaign reproduces a capability-caused gate regression
- compatibility observations no longer match the current runtime or project contract
- local user quarantines it

Rollback reactivates an already approved prior digest and invalidates every Mission draft that selected the replaced
version. It does not rewrite a running Task. A running Task finishes under its pinned version or is cancelled.

## Deferred work

These require separate initiatives and are not latent flags:

- semantic candidate extraction
- hidden capability injection
- user-wide and organization-wide promotion
- cloud registry or telemetry
- statistical provider routing
- automatic stale decisions based only on elapsed days
- capability-generated commands
- competing variants and self-evolution
- automatic policy modification

## Growth kill gate

After at least three distinct real project tasks and repeated controlled reuse, remove the Growth slice if it does not
reduce repair cycles, user operations, or deterministic gate failures. A directory of approved files is not product
value by itself.
