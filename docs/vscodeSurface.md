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

`runtrol endpoint` starts the daemon when needed and reports the exact local named pipe or Unix socket. The extension
uses the existing four-byte framed protocol directly. One greeted command connection is serialized and reused, while
the selected session owns one independent watch connection and reconnect cursor.

The bundled Core is copied by streaming digest into one stable extension-global path. A hard link preserves the mapped
image before atomic replacement. Extension Host reloads, official VSIX upgrades, and rollbacks therefore reconnect to
the original daemon and provider processes instead of making a versioned extension directory their lifetime owner.

## Session and workspace contract

- Fifteen sessions are the daily-use baseline and 30 sessions are the release load.
- At most eight sessions own hot provider processes.
- Exactly one selected session owns the full watch and Webview renderer.
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
| `state.ts` | provider, session, cursor, and selection metadata in memory | conversation frames |
| `selectionStore.ts` | one bounded selected-session identifier | prompts, replies, or provider state |
| `controller.ts` | user actions, one watch lifetime, workspace binding | transcript discovery or agent loops |
| `conversationView.ts` | CSP and Extension Host to Webview transport | retained conversation state |
| `webview/` | bounded active rendering and input | durable storage or background sessions |

## Performance contract

The real Extension Host gate runs the production extension and Core on hosted Windows, macOS, and Linux. The shared
ratchet currently caps:

| Measure | Ceiling |
|---|---:|
| Ready activation | 1,000 ms |
| Contributed view opening | 500 ms |
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

The public identity is `runtrol.runtrol-studio`. The extension manifest owns release SemVer and
`release-targets.json` owns the six native targets:

- `win32-x64`
- `win32-arm64`
- `darwin-x64`
- `darwin-arm64`
- `linux-x64`
- `linux-arm64`

Each VSIX contains exactly one matching native Core, the production bundles, canonical brand assets, and the repository
license. Source, tooling, development dependencies, performance budgets, and target metadata are excluded.

The release workflow builds on every native runner, compares the packaged Core bytes with the built binary, installs
the VSIX into a clean stable VS Code profile, activates through the bundled Core, exercises upgrade and rollback with
an active session, and uploads the package. It creates a tagged GitHub Release only after all six jobs pass.

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

## Verification entry points

| Gate or command | Contract |
|---|---|
| `vscodeExtension` | thin extension boundary, TypeScript, framing, storage, queue, renderer, and bundle limits |
| `vscodeHostPerformance` | real 30-session Extension Host and Webview responsiveness on three operating systems |
| `vscodeRealProviderJourney` | installed provider discovery and a complete real CLI control journey |
| `vscodePackage` | six-target SSOT, exact archive contents, Core bytes, workflow integrity, and listing metadata |
| `vscodeUpgradeRollback` | active-session continuity across official VSIX upgrade and rollback |
| `node tooling/installed-package.mjs --marketplace` | public Marketplace download, isolated activation, bundled Core, refresh, and view opening |

Every verifier uses an isolated profile marker and terminates only exact owned process identities. It must never close
unrelated VS Code windows, extension hosts, daemons, or provider sessions.
