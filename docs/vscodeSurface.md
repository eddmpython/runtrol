# VS Code surface

This document is the operational source of truth for Runtrol Studio.

## Product boundary

Runtrol Studio is the flagship graphical client for Runtrol Runtime and the only distributed desktop GUI. Runtime
remains independently usable through Rust, TypeScript, and Python clients plus the `runtrol` administration CLI.

Studio owns VS Code navigation, workspace following, native tree items, terminal tabs, and explicit user actions.
Runtime owns process supervision, session and workspace identity, public integration authority, and bounded transport.
Installed provider CLIs own accounts, conversations, terminal interfaces, model controls, approvals, and repository
changes.

Studio never stores a prompt, reply, draft, approval subject, terminal frame, transcript copy, or model API key.
Closing or updating VS Code does not transfer provider process ownership away from Runtime.

## Runtime path

Core discovery follows this order:

1. explicit absolute `runtrol.corePath` for development;
2. the native Runtime bundled in a Marketplace package and materialized under extension global storage;
3. a `runtrol` executable discovered on `PATH`.

Studio uses one approved public Runtime identity for provider inventory, managed sessions, approvals, and terminal
views. It validates the owner-local locator before every new transport lifetime. A terminal tab owns a dedicated
public streaming connection, so terminal output never poisons the ordinary request-response client.

The private connection is limited to Runtime bootstrap and optional owner administration. Studio's private protocol
projection contains no terminal open, attach, input, resize, output, or exit variants.

The bundled Runtime is copied by streaming digest into a stable extension-global location. A hard link protects any
mapped Windows image before replacement. Extension Host restart, VSIX upgrade, and rollback reconnect to the same
Runtime generations and provider processes.

## One native sidebar

Studio contributes one native view, `runtrol.sidebar`, named `Runtrol`. It does not contribute separate Projects,
Conversations, or Agent Usage views.

The tree order is:

1. first-run or contextual action rows when needed;
2. added project rows with provider-neutral conversation children;
3. conversations whose workspace is not represented by an added project;
4. one compact usage row for each installed provider.

A project is an operator-added folder, never a provider grouping. Adding a folder asks every discovered provider for
its native conversations and groups matching rows under that project. Conversations elsewhere remain top-level rows.
Provider identity is an icon on a conversation, not a heading. Adding a provider extends Runtime inventory and the
same tree without a Studio or Core edit.

Conversation rows keep one actual title and provider icon. The icon spins while that conversation is working. Quiet
rows do not repeat provider, project, `Ready`, or elapsed-time text. Pin, rename, archive, close, native delete,
provider remedy, and approval actions exist only when Runtime reports the matching capability or state.

The title toolbar keeps **Add Project** and **New Conversation** visible. Less frequent navigation and administration
remain in the overflow menu and Command Palette. All inline actions have command and keyboard-accessible equivalents.

## Usage strip

Under the list, pinned in the same container, the `runtrol.usage` view draws one chip per installed provider: the
provider's icon inside a ring gauge, with the number under it. No provider name is drawn; the icon is the label, so
a chip's width never depends on a name and the strip reads the same with three providers or ten. The ring is the
seven-day window when the provider publishes one, otherwise the window the provider says governs, otherwise an empty
ring with a one-word cause (`No report`, `Sign in`, `Fix`, `Checking`, `Offline`). A blocking limit turns the ring
and the number the theme's error colour.

Hovering or focusing a chip (chips are buttons, reached with Tab) opens that provider's panel under the strip; Enter
pins it and Escape closes it. The panel lists the plan the provider named, one thin bar per reported window with its
own name (`5h`, `7d`, `7d GPT-5.3-Codex`), each bar's reset and governing note, the report age, and the one action a
state offers (`Sign in`, `Fix`). This strip is the only webview Studio contributes; it renders host-built markup under
a nonce Content Security Policy and posts back nothing but the pressed action. Studio never converts a missing
percentage into zero or derives account capacity from terminal text.

Usage is pushed on the provider subscription and remembered briefly only as bounded operational telemetry so a new
window does not flash an empty strip. Expired, future-dated, or old-schema reports are refused.

## Terminal tabs

A conversation opens as the provider CLI's own terminal interface in an editor tab. The provider owns its composer,
model and effort controls, permissions, approvals, and history. Studio writes no prompt and parses no screen meaning.

`terminalTabs.ts` uses the public TypeScript Runtime terminal client. On transport loss it re-reads the locator and
reattaches only to the descriptor's exact Runtime generation. The returned screen snapshot replaces the view. If an
open returns `terminalAlreadyLive`, Studio lists generations and attaches to that exact owner. It never redirects to
the newest generation, retries input, or falls back to private IPC.

Closing a tab detaches the viewer. Runtime keeps the provider terminal alive until its CLI exits or an authorized
explicit stop occurs. Split, grid, focus, and full-screen behavior belong to VS Code.

No published Studio version before this public terminal contract persisted a private terminal attachment identity,
so there is no discoverable legacy tab to migrate. Runtime's native claim registry and `legacyGenerationBusy` error
protect any older live owner without inventing a client-side bridge.

## Session and workspace contract

- Fifteen sessions are the daily-use baseline and 30 sessions are the release load.
- At most eight sessions own hot provider processes.
- One selected session owns the foreground subscription and full renderer.
- Conversation and project ordering is stable and does not jump because turn state changes.
- Search uses project, provider, state, and workspace metadata without reading conversation content.
- Selecting a cold row resumes through the provider-native identity in its exact workspace.
- Equal, ancestor, and descendant writer roots collide atomically; separate linked worktrees do not.
- A bounded selected-session identifier may survive reload. Conversation content may not.
- A session waiting on the operator contributes to the view badge and **Open Next Waiting Conversation**. Quota waits
  do not pretend to be operator tasks.

## Agent Tools

The project sparkle calls `agentTools.ts`, which invokes exact Core enable, disable, and list commands. Runtime grants
are limited to the canonical project root and credentials remain in OS-protected storage. Provider registration uses
the provider's official CLI read and write surface. Studio does not edit provider configuration files directly.

## Module boundaries

| Module | Owns | Must not own |
|---|---|---|
| `core/locator.ts` | Runtime candidate order and endpoint probe | provider names or session policy |
| `core/managedCore.ts` | digest verification and stable bundled Runtime replacement | session state or provider policy |
| `core/framing.ts`, `protocol.ts` | bounded private administration frames and their TypeScript projection | public terminal operations or provider fields |
| `runtimeClient.ts` | approved public identity, locator lifetime, inventory, sessions, approvals, terminal generations | provider credentials or transcript storage |
| `trees.ts`, `usageDisplay.ts` | one native hierarchy, compact seven-day line, tooltip and detail facts | provider-specific branches or inferred usage |
| `controller.ts` | explicit user actions, provider-neutral navigation, workspace binding | transcript discovery or an agent loop |
| `terminalTabs.ts` | one public Runtime terminal view per editor tab | reading, storing, rewriting, or retrying terminal input |
| `agentTools.ts` | exact project enable, disable, and readback | provider configuration bytes or grant policy |
| `selectionStore.ts` | one bounded selected-session identifier | prompts, replies, terminal frames, or provider state |
| `pairingAdministration.ts` | local phone pairing and authority review | relay trust or conversation content |

## Performance contract

The Extension Host gate runs isolated production builds on Windows, macOS, and Linux. Thirty sessions, exact counts,
and zero dropped frames must hold in every trial. Current ceilings include:

| Measure | Ceiling |
|---|---:|
| Ready activation | 1,800 ms |
| Runtrol navigation and conversation opening | 1,000 ms |
| Refresh p95 | 50 ms |
| Extension Host RSS growth | 64 MiB |
| Loaded animation frame p95 | 40 ms |
| Input and scroll p95 | 50 ms |
| Hot-session switch p95 | 175 ms |
| Cold provider-native resume | 3,500 ms |
| Full workspace reload restoration | 2,500 ms |

## Brand

`assets/brand/` is the source of truth. The packaged Marketplace icon is the coral and white mark on graphite and is
copied into `resources/icon.png` during the build. The Activity Bar uses the canonical silhouette SVG because VS Code
masks contributed Activity Bar icons to the current theme foreground. Sidebar primary actions use the configurable
`runtrol.accent` color, whose dark default is canonical coral `#F56565`.

## Distribution

The public identity is `runtrol.runtrol-studio`. `release-policy.json` independently owns the Studio version while
the workspace `Cargo.toml` owns Runtime and SDK versions. Studio releases increment the `0.1.x` patch component by
exactly one. `release-targets.json` owns six packages: Windows, macOS, and Linux on x64 and ARM64.

Each VSIX contains one matching native Runtime, production bundles, canonical brand assets, license, notice, and
Marketplace README. Source, tests, build tools, and development dependencies are excluded.

Pushing a one-patch `release-policy.json` change to `main` starts the native matrix. Each runner builds Runtime,
packages and audits the exact VSIX, installs it into an isolated VS Code profile, completes the first-run journey,
and exercises active-session upgrade and rollback. Publication verifies all six Marketplace packages before creating
the tagged GitHub Release. `publishExisting` can republish only already tagged exact artifacts and cannot rebuild.

## Verification entry points

| Gate or command | Contract |
|---|---|
| `npm --prefix extensions/runtrol-vscode run check` | TypeScript public and private boundary consistency |
| `npm --prefix extensions/runtrol-vscode test` | native tree, usage, Runtime client, administration, and terminal behavior |
| `vscodeExtension` | one native view, theme color, command, storage, package, and provider-neutral boundaries |
| `vscodeHostPerformance` | real 30-session Extension Host responsiveness |
| `vscodeRealProviderJourney` | installed provider discovery and complete real CLI control journey |
| `node tooling/real-window-eye.mjs` | isolated real VS Code visual journey and screenshots |
| `vscodePackage` | six-target SSOT, exact archive contents, Runtime bytes, workflow, README, and brand metadata |
| `crossPlatformMatrix` | exact VSIX installation and first-run action on native Windows, macOS, and Linux |
| `vscodeUpgradeRollback` | active-session continuity across official VSIX upgrade and rollback |
| `node tooling/installed-package.mjs --marketplace` | public Marketplace download, activation, bundled Runtime, view, and first-run command |

The Windows-only `tooling/inspect-vscode.mjs` development helper can list, capture, type into, or click a real VS Code
window after foreground verification. It is excluded from the VSIX and is not a product surface.
