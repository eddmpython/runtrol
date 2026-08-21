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
four-byte header and payload. Public Runtime and private Studio transports end gracefully before Windows reuses their
named-pipe instances. Tests take the same enrollment path a person does: Studio settles its own enrollment with its
own key, and no test-only approval channel exists to route around.
Before settling its own enrollment, Studio briefly observes the exact pending decision so a decision already made
through another local administration surface is honoured instead of overridden. Studio self-approves only an
enrollment still pending after that window, by signing the exact pending identity with the key that requested it.

Studio becomes ready after one exact provider and managed-session inventory has populated navigation. Dedicated
provider and managed-session streams then replace that inventory in the background. While those streams remain
active, explicit refreshes reuse their latest snapshots instead of opening duplicate list requests. A lifecycle
mutation invalidates the session snapshot until the stream or an exact list request replaces it.
An installed executable without a verified probe remains visible while Studio starts its provider-neutral capability
probe in the background. A successful probe refreshes the Runtime snapshot automatically. A failed probe stays
unavailable and produces a visible warning without blocking unrelated sessions.
Core discovery and protected identity loading begin together. Studio then validates the public locator. On Windows
the exact selected Core executable reuses the Rust client's native owner and DACL checks, and the TypeScript SDK
compares the validated fields with the file it opens. Unix keeps the SDK's direct owner and mode checks. Opening the
conversation waits only until both the panel and VS Code's editor-tab model report that exact tab active; Webview
readiness then starts the selected-session stream without blocking the command response.

The bundled Core is copied by streaming digest into one stable extension-global path. A hard link preserves the mapped
image before atomic replacement. Extension Host reloads, official VSIX upgrades, and rollbacks therefore reconnect to
the original daemon and provider processes instead of making a versioned extension directory their lifetime owner.

## Session and workspace contract

- Fifteen sessions are the daily-use baseline and 30 sessions are the release load.
- At most eight sessions own hot provider processes.
- Exactly one selected session owns the full watch and Webview renderer.
- The sidebar is the one Paseo-style list of this machine: every project, every conversation of every installed
  coding service, the way the Codex and Claude desktop sidebars read. It lists conversations, never coding services. A
  service is a fact printed on a row, never a parent node the reader opens first, and there is no separate inventory
  view.
- Every folder a coding service holds conversations in is a project heading, with its conversations beneath it. A
  heading is created by the operator, or is the folder this window has open, or is discovered because a conversation
  names it (created > open > discovered, one heading per place). A discovered heading exists exactly as long as a
  conversation names it, so no heading is ever empty. Two folders with the same name are told apart by their parent.
  Grouping by coding service would sort by an implementation detail, since one repository driven by two CLIs is one
  piece of work.
- Conversations are listed machine-wide, in one question per service, from the service's own surface: a listing
  protocol method, a listing command, or (for a CLI that publishes neither, Claude Code) the names in the CLI's own
  store, read as identity, folder, the CLI's own title and last write, never a message. When a list is not everything
  the sidebar says so in one line above the list, and the services' own reasons sit behind an (i) in the view's
  title. A conversation started without a project runs in
  the extension's own scratch folder and is listed beneath the headings as a loose row, with no folder name and no
  collision question.
- The project this window is open on is first and open. Any project holding a conversation that stopped for the operator
  is open too, so no heading can hide the thing that wants them. Everything else is closed.
- A conversation is deleted from its row. A Runtime-supervised conversation is closed and forgotten here (the
  provider keeps its own record). A provider-owned stored conversation is deleted through the provider's own surface
  (`sessions/deleteNative`: Codex `thread/delete`, Cline `history delete`) after a modal question naming the service;
  a provider that publishes no such surface says so, in its own words, and nothing is attempted. Runtrol never removes
  a provider's files itself.
- Moving this window to a project is a button on its heading (and the live conversation's project chip), never a side
  effect of opening a conversation. Opening is the file-click grammar: the conversation's tab opens here, the window
  stays where it is, and the CLI runs in the conversation's own folder regardless.
- Heading order ignores what is running inside it, for the same reason row order does. Waiting counts are printed in the
  heading; position reflects where the reader left it.
- A heading's rows are built when that heading is first drawn, never before. Thirty projects means twenty-nine closed
  headings whose rows nobody is going to look at, and the tree provider answers parent queries from a map rather than
  from built items so revealing a row never depends on having built the rest.
- A Runtime-supervised session and the provider-owned chat it came from are one row. Row identity is the conversation,
  so opening a saved chat updates that row in place instead of removing one and inserting another.
- Live conversations lead, then whatever the coding service touched most recently. Turn state never participates in the
  order, so no row moves because an agent started or finished thinking.
- Every row carries one of six states, and the same glyph means the same thing in the sidebar and in the switcher:
  needs you, needs attention, working, waiting on a limit, ready, saved. The point of the vocabulary is that a list of
  running agents can be read without opening any of them.
- A conversation whose turn stopped for a person reads `Needs you` before anything else on the row, and the view
  carries a count badge so a blocked agent stays visible from another view entirely. An account limit is a separate
  state and never counts toward that badge, because nobody can answer it.
- One command opens the next conversation that stopped for the operator, and pressing it again walks the rest
  rather than returning to the same one. A conversation waiting on an account limit is never a destination, because
  nobody can answer it. This is the orchestration primitive: supervising several agents never requires reading a
  board to work out which one wants attention.
- While anything is waiting, the status bar carries the count and its warning colour from anywhere in the window,
  and activating it opens that conversation. With nothing waiting it returns to reporting running counts. A count of
  running agents is ambient; a count of agents that stopped for this person is a request.
- New Conversation, Switch Conversation, Show Open Conversation, and Open Next Waiting each have a keyboard chord,
  so the entry point never requires the mouse.
- A coding service that is not installed produces no row. Only an installed service that still cannot run appears, and
  only once its capability probe has finished.
- Each conversation opens in its own editor tab with a bounded renderer and composer, exactly as a file does: ten or
  twenty tabs are arranged, split and sized by VS Code's own editor groups, and the focused tab is the selected
  conversation. Prompts and interrupts travel only to the tab that sent them. Tabs survive a window reload by their
  session identity, and a tab whose session no longer exists is closed rather than guessed at.
- A conversation can live in any of the window's own places: an editor tab (the default), the bottom panel beside
  the terminals, or the secondary side bar beside the code. Each place is a VS Code surface; Runtrol adds no pane
  system of its own. A conversation is in one place at a time and watched once; moving it is a row command ("Open
  Conversation in Panel / in Side Bar / as Tab"), and the conversation a place showed before a reload comes back to
  it. One command ("Arrange Conversations in a Grid", `Ctrl+K Ctrl+G`) spreads the open conversation tabs over
  editor groups as square as they come (two by two, three by two, three by three; nine is the editor's column
  bound and the command says when tabs were left in place). VS Code draws, sizes and lets the operator drag them.
- A change a coding service declares (an ACP diff block, a Codex unified change) is named in the tab with an
  "Open diff" button and opened in VS Code's own diff editor: two read-only virtual documents for before and after,
  or a read-only `.diff` document for a unified patch. The page draws and colours no diff; the texts are held in
  memory only, bounded, and never written to disk.
- When the Conversations view becomes visible and no conversation tab exists, Studio opens that selected in-progress
  chat without blocking ready. A restored editor tab is reused as VS Code left it.
- Existing provider-owned chats start loading as soon as the first inventory is ready, instead of waiting for a later idle window.
- New chat opens as a draft tab: a greeting, the composer, and chips for the project, the git branch, the coding
  service, the model, the reasoning effort and the access mode. Each chip is its own picker, nothing runs until the
  first message, and that message starts the conversation in the same tab with exactly those choices. The `+` on a
  project heading opens the same draft with the folder already answered; the defaults are this window's folder, the
  service used last, and the project's last explicit choices. A draft survives a window reload with its choices.
- The composer is the one standard card rather than an invention: a context row (project, branch, service), the
  message, and a bar with attach and access mode on the left and model, reasoning effort and send on the right. Images
  travel once as `sessions/submitBlocks` content and are never stored. There is no microphone, because no installed
  CLI takes audio.
- The conversation editor carries no session panel. The service, the model the service says is answering, the
  requested reasoning effort, context use, provider-reported cost, and the tightest account-limit window appear as
  chips beneath the composer, and the permission mode has a chip of its own. Missing provider telemetry remains
  visibly absent and is never estimated, and an untouched quota window says nothing.
- The conversation shows what the provider gives and nothing else: streamed replies named by the provider's own
  message identity, tool calls and results under the provider's own tool names, approvals as the provider asked them,
  and on resume whatever history the provider's own resume surface hands over (Codex replays its recent turns; Claude
  Code's stream prints none, and the tab is the quiet empty state rather than a reconstruction). Runtrol authors no
  line of it.
- A conversation the Runtime released to keep the running set small (eight hot processes) reads as paused in its tab,
  in one sentence, and watches itself again as soon as the session is hot; it is not an error and it is not retried
  in red.
- Enter sends and Shift+Enter writes a new line.
- A message opening with `/` offers the commands the attached coding service announced, with that service's own
  descriptions. Only a leading slash opens the menu, since a slash inside a sentence belongs to a path. Choosing fills
  the composer and sends nothing: some of these commands take an argument, and sending on selection would make those
  unusable and the rest premature. Only the announced name and description are read; a command's argument schema stays
  the service's business, because the value of passing a slash command through untouched is that the service decides
  what it means.
- Every choice the composer offers (project, service, model, reasoning effort, access mode) is answered in the
  composer: the chip's popover opens where the chip is, keyboard and mouse both work, and a click elsewhere
  dismisses it. No picker appears at the top of the window for a chip; the command palette keeps the same
  choices for commands invoked from the palette. The slash menu is the same kind of popover.
- Changing the model or the permission mode mid conversation is the chip that displays it. The pick travels
  `sessions/setModel` or `sessions/setMode` to the service's own switch surface (a control channel, the next turn's
  own override field, or the protocol's announced call, whichever that CLI actually has), and the chip then shows
  what the service says back, never what was merely requested. The choices are the session's own announced set or
  the service's measured vocabulary; modes that remove safety prompts are not offered and are refused for every
  caller. The service's own `/model` style commands remain available through the slash menu and stay authoritative:
  a switch made there surfaces through the same announcement events the chips read.
- The command list belongs to one conversation and is dropped on switching. A previous service's commands offered to
  the next one is worse than none, because it looks authoritative.
- A coding service that cannot start a conversation is answered with that service's own remedy, chosen by the public
  error category and what discovery already knows: not signed in, not installed, or installed and failing. The exact
  command is placed in the operator's terminal unexecuted. Runtrol never runs it. Fetching and executing on somebody's
  behalf is refused, and an install button that installs is that refusal reversed under a friendlier label.
- A hidden conversation pauses its watch at the last delivered cursor. Reopening waits for the new Webview document to become ready before replay continues.
- An operator name is stored as bounded session metadata. Without one, the visible title is the workspace name plus the runtime-discovered provider name. A short stable suffix appears only when titles collide.
- The selected session remains first. One fuzzy switcher searches project, provider, state, and workspace metadata.
- Session-index subscribers receive one current snapshot and then only list-visible changes.
- Selecting a cold session gives immediate feedback, resumes through its provider-native identity, and follows its
  exact workspace.
- One bounded selected-session scalar survives workspace reload. It contains no conversation content.
- Core-owned project and working-tree identity prevents concurrent writers in equal, ancestor, or descendant paths.
  Linked worktrees remain independent, and only an explicit user action permits shared access.

## Surface and driver contracts (what a new place or a new service has to implement, and nothing else)

Two contracts keep the conversation window from growing a branch per place or per service.

- **A place is one binding surface.** `ConversationSurface` (`conversationSurface.ts`) is all the page, the
  bindings and the controller know about where a conversation is shown: a webview to post to, `visible` to pause
  the watch by, a title, `reveal`, and two lifetime events (visibility changed, disposed). An editor tab and a
  workbench view both implement it; adding a place (a new view, an editor in another column) is one implementation
  and two lines of `package.json`, with no change to the page, the watch, the chips or the controller. The
  per-place state the workbench does not keep (which conversation a view showed) is the binding layer's, stored
  as a session identity only.
- **A service is one driver.** The Runtime's provider trait and manifest are the whole of what a coding service
  contributes; the window reads the provider-neutral event vocabulary (messages, tool calls, approvals, turns,
  notices, usage) and the provider's own words inside it. A new service reaches every place, the sidebar, the
  chips and the diff editor without a line in the extension (the isolation gate `tests/audit/providerIsolation.rs`
  guards the Runtime half; the extension has no provider branch to guard).

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
| `conversationSurface.ts` | the place contract: tab and workbench-view surfaces, the empty-place page | conversation state, watches, or any place-specific rendering |
| `conversationView.ts` | one conversation page on one surface, CSP, and Extension Host to Webview transport | retained conversation state or a second live renderer |
| `conversationPanels.ts` | one binding per conversation (surface + watch), the two workbench places, the grid | provider branches or conversation content |
| `diffDocuments.ts` | declared changes as read-only virtual documents for VS Code's diff editor | files on disk or a transcript of changes |
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
macOS, and Linux. The fastest of three trials feeds the shared budget, while exact session counts and zero dropped frames must hold in
every trial. The shared ratchet currently caps:

| Measure | Ceiling |
|---|---:|
| Ready activation | 1,350 ms |
| Runtrol navigation and conversation opening | 1,000 ms |
| Refresh p95 | 50 ms |
| Extension Host RSS growth | 48 MiB |
| Loaded animation frame p95 | 40 ms |
| Load overrun above the runner's native cadence | 8 ms |
| Input and scroll p95 | 50 ms |
| Renderer backlog | 1,024 frames |
| Hot-session switch p95 | 125 ms |
| Cold provider-native resume | 1,500 ms |
| Full workspace reload restoration | 1,750 ms |

The Webview carries 15,000 raw frames over five seconds and must drop zero raw frames while animation, input, scroll,
DOM, visible characters, queue growth, and memory remain bounded. Its measurement protocol bounds startup and result
acknowledgements separately and retries one transient Webview document reload within the outer gate timeout.

## Distribution

The public identity is `runtrol.runtrol-studio`. `release-policy.json` is the extension version source, independently
of the Runtime and Rust SDK version in `Cargo.toml`. Studio is permanently on the `0.1.x` release line from `0.1.1`
onward. Every release increments only the patch component by exactly one. The policy, complete changelog sequence,
and release workflow's exact predecessor-tag check enforce that rule before packaging. `release-targets.json` owns
the six native targets:

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

The current public release is [Runtrol Studio 0.1.3](https://github.com/eddmpython/runtrol/releases/tag/vscode-v0.1.3).
All six native packages are published under one
[Marketplace listing](https://marketplace.visualstudio.com/items?itemName=runtrol.runtrol-studio). Stable VS Code
1.132.1 downloads the exact public package into an isolated profile on each native release runner, activates with no
configured Core path, materializes the bundled Core, refreshes through it, and opens the contributed Runtrol view.
Exact verifier processes and temporary profiles are removed afterward.

Advancing `release-policy.json` by one patch on `main` starts the release automatically. GitHub Actions holds one
Marketplace-only PAT as the `VSCE_PAT` repository secret. The pinned publisher client receives it only through the
publication step environment, publishes all six packages, and verifies the public identity, version, target set, and
archive digests before the public installation matrix and tagged GitHub Release can complete. `publishExisting` is a
manual recovery input for an already tagged version and cannot rebuild or retag artifacts.

Extension and provider update ownership, safe scheduling, exact rollback, and the local-only update command are
specified in [automatic updates](automaticUpdates.md).

## Verification entry points

| Gate or command | Contract |
|---|---|
| `vscodeExtension` | thin extension boundary, TypeScript, framing, storage, queue, renderer, and bundle limits |
| `vscodeHostPerformance` | real 30-session Extension Host and Webview responsiveness on three operating systems |
| `vscodeRealProviderJourney` | installed provider discovery and a complete real CLI control journey |
| `node tooling/real-window-eye.mjs` | the eye pass: an isolated VS Code window and an isolated Runtime with the real installed CLIs, real folders and real conversations, photographed in the draft, conversation, tabs, reopened, grid, places, diff and diff-editor poses, with a real throwaway deletion; the pictures are the judgement. `RUNTROL_EYE_ENTRY=placeProbe` runs the focused place probe instead |
| `missionGrowthContracts` | Mission state, exact Send, evidence, integration, capability trust, local scope, tamper, and rollback |
| `missionLiveJourney` | two installed provider CLIs complete five reviewed Tasks and an explicit reuse, tamper, and rollback journey through production IPC |
| `vscodePackage` | six-target SSOT, exact archive contents, Core bytes, workflow integrity, and listing metadata |
| `vscodeUpgradeRollback` | active-session continuity across official VSIX upgrade and rollback |
| `channelVerdict` | confirmed provider package ownership and closed update arguments |
| `cliUpdateRehearsal` | failed provider target and exact verified rollback transaction |
| `node tooling/installed-package.mjs --marketplace` | public Marketplace download, isolated activation, bundled Core, refresh, and view opening |

Every verifier uses an isolated profile marker and terminates only exact owned process identities. It must never close
unrelated VS Code windows, extension hosts, daemons, or provider sessions.
