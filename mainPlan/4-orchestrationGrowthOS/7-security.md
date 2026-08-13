# Security

## Security objective

Mission automation increases the number of provider processes, workspaces, project files, and deterministic commands
that can be active without continuous user focus. It therefore narrows authority. It never turns scheduling metadata
into a new broad permission.

Existing default-deny, physical-presence, exact device grant, provider-native approval, panic stop, process ownership,
and model-key exclusion contracts remain authoritative.

## Threats

| Threat | Required response |
|---|---|
| Malicious Mission file | Closed schema, bounded fields, project-bound paths, local digest review, no shell text |
| Changed instruction after review | Digest mismatch returns Mission to `Draft` |
| Scheduler or remote device submits Task input | No scheduler effect or device-grantable scope can cross the local Send boundary |
| Two Tasks writing one tree | Atomic canonical workspace claim rejects the second reservation |
| Provider writes outside declared outputs | Post-run tree and filesystem evidence blocks passing, without claiming confinement |
| Provider edits an active capability | Approved digest mismatch marks it `Tampered` and disables reuse |
| Gate command substitution | Exact local registry reference and definition digest, no Mission command string |
| Gate output leaks a secret | Output remains bounded live data and is not persisted |
| Candidate contains a credential | No automatic output capture, bounded files, local diff review, and warning scan |
| Remote device widens a Mission | Creation, start, retry, integration, policy, gate, and capability actions are local-only |
| Crash duplicates work | Durable reservation before effect, process reconciliation, ambiguity becomes `Blocked` |
| Provider change invalidates assumptions | Runtime discovery and binary fingerprint change invalidate affected drafts and verifications |
| Capability supply-chain change | Exact content, parent, source Receipt, verification Receipt, and policy digests |

## Scope additions

New permissions are placed once in the existing `LocalScope` and `DeviceScope` walls.

Local-only capabilities:

- `MissionCreate`
- `MissionStart`
- `MissionRetryTask`
- `MissionSendTaskInstruction`
- `MissionIntegrate`
- `MissionArchive`
- `GateRegister`
- `PolicyWrite`
- `CapabilityPromote`
- `CapabilityRollback`
- `CapabilityArchive`

Potentially device-grantable capabilities, always per paired PC and approved project root:

- `MissionRead`
- `MissionPause`
- `MissionResumeSafe`
- `MissionCancel`
- `EvidenceReadSummary`

Provider-native approval responses continue through the existing risk-class contract. A Mission does not create a
second approval vocabulary. High-risk approval remains subject to the existing at-machine grant and expiry rules.

No conversion from a local-only capability to `DeviceScope` exists. Unknown scope values fail closed at wire decode
or the scope wall.

## Enforcement levels

Product text and gates must distinguish these levels:

| Control | v0 enforcement | Honest claim |
|---|---|---|
| Mission graph and retry bounds | Typed validator and state machine | Enforced |
| Concurrent writer separation | Canonical worktree claim and scheduler reservation | Enforced for scheduled Runtrol writers |
| Output root | Preflight claim plus post-run manifest and Git diff | Detected, not confined |
| Read root | Provider-native permission or OS sandbox only | Unavailable unless discovered and proven |
| Network deny | OS sandbox only | Unavailable unless proven on that platform |
| Command allowlist | Exact locally approved registry entry | Enforced for Runtrol gate launches |
| Provider child cleanup | Existing containment boundary | Enforced |
| Capability active trust | Local exact digest index | Enforced for Runtrol reuse, not filesystem immutability |
| Remote promotion and policy change | Missing device-grantable scope | Enforced by construction |

Worktree isolation is not a sandbox. A changed-files gate is not write prevention. A `network = deny` string in a
policy is not network denial. Features requiring a stronger level stay unavailable until the actual mechanism and a
failure mutation graduate.

## Mission approval

Local approval binds:

- Mission schema and file digest
- canonical project and base identity
- instruction paths and digests
- provider selections and discovered capability observations
- workspace and output claims
- GateDefinition IDs and digests
- policy digest
- capability version digests
- concurrency, retry, repair, artifact, and timeout bounds

Any change invalidates approval. An approval has a short start window and cannot be replayed for another Mission,
project, or digest. Mission approval authorizes preparation only. Each Task input still requires its own exact local
Send action.

## Gate registry

Only a local user can create or change a GateDefinition. The registry format is closed. Each entry specifies separate
program and argument fields and forbids shell interpolation. Working directories are symbolic project identities,
not arbitrary absolute paths.

The runner resolves the executable once, fingerprints it when supported, starts it through owned process containment,
applies timeout, and records the definition digest. A missing enforcement mechanism is a refusal, not a warning.

Mission authors can reference an existing gate but cannot widen it or register a new one.

## Capability activation

Candidate generation runs as a separate Task and inherits no implicit rights from the successful source Run. Its
reviewed instruction and output root are explicit. Without a proven sandbox, Runtrol does not claim the provider was
unable to edit other project files. Activation therefore relies on exact review, post-run diff evidence, and a local
digest pin.

A capability cannot define executable files in v0. It may reference only pre-existing GateDefinitions. A project
file edit cannot grant itself execution authority.

## Secrets and privacy

- Model credentials remain in provider-owned storage and never enter Mission configuration.
- Environment values are never copied into ledger records or receipts.
- Raw gate output is never durable by default.
- Provider events retain the existing bounded live transport and short replay behavior.
- Instruction and capability bodies stay in project files and are not duplicated in Runtrol databases.
- Receipt export is a local explicit action and includes metadata only.
- Error text uses typed categories and safe identifiers, not captured process output.

Hashing sensitive arbitrary output is also avoided. A digest can confirm a guessed secret. Only reviewed Artifact
files and fixed contract inputs receive durable digests.

## Remote behavior

The phone can reduce activity by pause, cancel, or panic stop. It cannot submit Task input or increase the set of
providers, workspaces, commands, instructions, retries, capabilities, or integration actions. Approval expiry is
denial.

The remote API returns bounded summaries. It never returns instruction bodies, candidate bodies, raw command output,
full diffs, or a capability approval action.

## Supply chain

Project capability files record content digest, parent digest, source Receipt, verification Receipt, policy digest,
license field, and optional upstream source. Import does not imply activation. Upstream updates create a new Candidate
and preserve the approved local version until separate verification and approval.

External signature and organization distribution are future concerns. v0 makes no trust claim beyond the local
approval and evidence chain.
