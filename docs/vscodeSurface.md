# VS Code Surface

This document is the operational source of truth for the public PC product.

## Product boundary

`Runtrol Studio` is the only distributed PC surface. It is a VS Code extension, not a separate desktop application.
The extension owns navigation, workspace following, bounded rendering, and user actions. The Rust Core owns process
supervision, session identity, workspace identity, and bounded event transport. Installed provider CLIs own their
conversations, credentials, native session records, and repository changes.

The extension never stores a prompt, reply, draft, approval subject, transcript copy, or model API key. Closing or
updating VS Code does not transfer session ownership away from the Core.

## Runtime path

Core discovery follows one ordered runtime contract:

1. an explicit absolute `runtrol.corePath` used for development;
2. the native Core bundled in a Marketplace package and materialized under extension global storage;
3. a `runtrol` executable discovered on `PATH`.

`runtrol endpoint` starts the daemon when needed and reports the exact private local named pipe or Unix socket. One
greeted private command connection is serialized and reused only for local administration that is not part of the
public Runtime protocol. Provider inventory, managed sessions, lifecycle actions, approvals, and event streams use the
public TypeScript Runtime SDK with an approved Studio integration identity.

Studio validates the owner-local public Runtime locator once for a healthy Runtime instance and reuses that exact
validated locator across its command, inventory, and selected-session stream connections. A connection or terminal
stream failure discards it, so the next attempt repeats locator ownership and permission validation before accepting
a replacement Runtime. Initial discovery gives Core a bounded window to finish its atomic owner-only locator
publication after private IPC becomes reachable. Public SDK frames use one bounded local transport write for the
four-byte header and payload.
Before opening the local enrollment review, Studio briefly observes the exact pending decision so an approval already
completed through another local administration surface does not produce a stale duplicate prompt.

Studio becomes ready after both dedicated provider and managed-session streams have delivered their first snapshots.
While those streams remain active, explicit refreshes reuse their latest snapshots instead of opening duplicate list
requests. A lifecycle mutation invalidates the session snapshot until the stream or an exact list request replaces it.
Core discovery and protected identity loading begin together. Studio then validates the public locator. On Windows
the exact selected Core executable reuses the Rust client's native owner and DACL checks, and the TypeScript SDK
compares the validated fields with the file it opens. Unix keeps the SDK's direct owner and mode checks. Opening the
conversation waits only until VS Code activates the editor tab; Webview readiness then starts the selected-session
stream without blocking the command response.

The bundled Core is copied by streaming digest into one stable extension-global path. A hard link preserves the mapped
image before atomic replacement. Extension Host reloads, official VSIX upgrades, and rollbacks therefore reconnect to
the original daemon and provider processes instead of making a versioned extension directory their lifetime owner.

## Session and workspace contract

- Fifteen sessions are the daily-use baseline and 30 sessions are the release load.
- At most eight sessions own hot provider processes.
- Exactly one selected session owns the full watch and Webview renderer.
- The sidebar owns session navigation. The selected conversation opens in one reusable editor tab with a bounded renderer and composer.
- A hidden conversation pauses its watch at the last delivered cursor. Reopening waits for the new Webview document to become ready before replay continues.
- An operator name is stored as bounded session metadata. Without one, the visible title is the workspace name plus the runtime-discovered provider name. A short stable suffix appears only when titles collide.
- The selected session remains first. One fuzzy switcher searches project, provider, state, and workspace metadata.
- Session-index subscribers receive one current snapshot and then only list-visible changes.
- Selecting a cold session gives immediate feedback, resumes through its provider-native identity, and follows its
  exact workspace.
- One bounded selected-session scalar survives workspace reload. It contains no conversation content.
- Core-owned project and working-tree identity prevents concurrent writers in equal, ancestor, or descendant paths.
  Linked worktrees remain independent, and only an explicit user action permits shared access.

## Module boundaries

| Module | Owns | Must not own |
|---|---|---|
| `core/locator.ts` | Core candidate order and one endpoint probe | provider names or session policy |
| `core/managedCore.ts` | digest verification, stable Core path, mapped-image preservation, atomic replacement | session state or update policy |
| `core/framing.ts` | bounded four-byte frame transport | request meaning or conversation rendering |
| `protocol.ts` | TypeScript projection of the Rust wire | provider-specific fields |
| `runtimeClient.ts` | approved public Runtime identity, validated-locator lifetime, inventory, lifecycle, and streams | provider credentials or conversation storage |
| `state.ts` | provider, session, cursor, and selection metadata in memory | conversation frames |
| `selectionStore.ts` | one bounded selected-session identifier | prompts, replies, or provider state |
| `controller.ts` | user actions, one watch lifetime, workspace binding | transcript discovery or agent loops |
| `conversationView.ts` | one editor panel, CSP, and Extension Host to Webview transport | retained conversation state or a second live renderer |
| `webview/` | bounded active rendering and input | durable storage or background sessions |
| `mission/controller.ts` and `mission/tree.ts` | Mission review, local actions, Task rows, and one native editor document | provider input without local Send or optimistic completion |
| `capability/controller.ts` | candidate inbox, native diff review, and exact local trust actions | capability text injection or user-wide trust |

## Mission and capability surface

The Missions tree is part of the existing Runtrol activity container. It lists Core snapshots and exposes actions only
when the matching state permits them. Validation selects a project Mission file. Start shows the exact Mission digest,
Task count, and the fact that no instruction is sent automatically. Each reserved Task must be prepared, bound to its
exact public Runtime session, and submitted through its own local `Send Task Instruction` action.

The reusable `runtrol-mission:` editor document shows the Mission source and digest, approval expiry, progress, Task
state, instruction and policy digests, provider and session identities, workspace and base commit, selected capability
versions, Gate counts, and the latest passing Run and Receipt IDs. It uses VS Code's text document surface and does not
create another Webview or another provider stream.

Pause, safe resume, cancel, bounded retry, integrated-tree verification, completion, and archive are explicit local
commands. Integration remains unavailable until every Task has passed. The extension does not merge or edit project
files.

The Capability Candidate Inbox uses a native quick pick and Markdown review document. Verification and approval name
one exact project version. Approval opens the built-in VS Code file or diff review first. Reject, quarantine, rollback,
and archive are also modal local actions. Candidate bodies stay in project files, and no action injects those bodies
into a Task. The detailed contracts are [Mission operations](missionOperations.md) and
[project capability trust](capabilityTrust.md).

## Performance contract

The real Extension Host gate runs three isolated cold trials of the production extension and Core on hosted Windows,
macOS, and Linux. One median feeds the shared budget, while exact session counts and zero dropped frames must hold in
every trial. The shared ratchet currently caps:

| Measure | Ceiling |
|---|---:|
| Ready activation | 1,000 ms |
| Runtrol navigation and conversation opening | 500 ms |
| Refresh p95 | 50 ms |
| Extension Host RSS growth | 48 MiB |
| Loaded animation frame p95 | 40 ms |
| Load overrun above the runner's native cadence | 8 ms |
| Input and scroll p95 | 50 ms |
| Renderer backlog | 1,024 frames |
| Hot-session switch p95 | 100 ms |
| Cold provider-native resume | 1,500 ms |
| Full workspace reload restoration | 1,500 ms |

The Webview carries 15,000 raw frames over five seconds and must drop zero raw frames while animation, input, scroll,
DOM, visible characters, queue growth, and memory remain bounded.

## Distribution

The public identity is `runtrol.runtrol-studio`. Workspace package SemVer in `Cargo.toml` is the release version source
for both Core and the generated extension manifest, while `release-targets.json` owns the six native targets:

- `win32-x64`
- `win32-arm64`
- `darwin-x64`
- `darwin-arm64`
- `linux-x64`
- `linux-arm64`

Each VSIX contains exactly one matching native Core, the production bundles, canonical brand assets, and the repository
license. Source, tooling, development dependencies, performance budgets, and target metadata are excluded.

The release workflow builds on every native runner, compares the packaged Core bytes with the built binary, installs
the VSIX into a clean VS Code 1.132.1 profile, activates through the bundled Core, exercises upgrade and rollback with
an active session, and uploads the package. Hosted extension gates use that same exact tested version unless an
operator explicitly supplies another version. It creates a tagged GitHub Release only after all six jobs pass.

The first public release is [Runtrol Studio 0.1.0](https://github.com/eddmpython/runtrol/releases/tag/vscode-v0.1.0).
All six native packages are published under one
[Marketplace listing](https://marketplace.visualstudio.com/items?itemName=runtrol.runtrol-studio). Stable VS Code
1.132.1 downloaded the public `win32-x64` package into an isolated profile, activated with no configured Core path,
materialized the exact bundled Core, refreshed through it, and opened the contributed Runtrol view. Exact verifier
processes and temporary profiles were removed afterward.

The attempted credentialless Marketplace OIDC exchange returned `404` from the Marketplace token endpoint. Until a
supported credentialless contract is available, Marketplace publication is a deliberate operator step using the
pinned `vsce` client and a short-lived Marketplace-only credential. The credential must be removed from `vsce` and
revoked after publication. No Marketplace secret is stored in the repository or release workflow.

Extension and provider update ownership, safe scheduling, exact rollback, and the local-only update command are
specified in [automatic updates](automaticUpdates.md).

## Verification entry points

| Gate or command | Contract |
|---|---|
| `vscodeExtension` | thin extension boundary, TypeScript, framing, storage, queue, renderer, and bundle limits |
| `vscodeHostPerformance` | real 30-session Extension Host and Webview responsiveness on three operating systems |
| `vscodeRealProviderJourney` | installed provider discovery and a complete real CLI control journey |
| `missionGrowthContracts` | Mission state, exact Send, evidence, integration, capability trust, local scope, tamper, and rollback |
| `missionLiveJourney` | two installed provider CLIs complete five reviewed Tasks and an explicit reuse, tamper, and rollback journey through production IPC |
| `vscodePackage` | six-target SSOT, exact archive contents, Core bytes, workflow integrity, and listing metadata |
| `vscodeUpgradeRollback` | active-session continuity across official VSIX upgrade and rollback |
| `channelVerdict` | confirmed provider package ownership and closed update arguments |
| `cliUpdateRehearsal` | failed provider target and exact verified rollback transaction |
| `node tooling/installed-package.mjs --marketplace` | public Marketplace download, isolated activation, bundled Core, refresh, and view opening |

Every verifier uses an isolated profile marker and terminates only exact owned process identities. It must never close
unrelated VS Code windows, extension hosts, daemons, or provider sessions.
