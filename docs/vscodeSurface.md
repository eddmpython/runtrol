# VS Code surface

This document is the operational source of truth for Runtrol Studio.

## Product boundary

Runtrol Studio is the flagship graphical client for Runtrol Runtime and the only distributed desktop GUI. Runtime
remains independently usable through Rust, TypeScript, and Python clients plus the `runtrol` administration CLI.

Studio owns VS Code navigation, workspace following, one sidebar webview projection, terminal tabs, and explicit user
actions.
Runtime owns process supervision, session and workspace identity, public integration authority, and bounded transport.
Installed provider CLIs own accounts, conversations, terminal interfaces, model controls, approvals, and repository
changes.

Studio never persists a prompt, reply, draft, approval subject, terminal frame, transcript copy, or model API key,
and it never creates a second conversation store while transporting live terminal bytes.
Closing or updating VS Code does not transfer provider process ownership away from Runtime.

The release provider set comes from the tracked driver manifests. This release packages and verifies
[Claude](../crates/runtrol-drivers/manifests/claude.toml) and
[Codex](../crates/runtrol-drivers/manifests/codex.toml). Studio does not branch on either identifier, so changing the
release set remains a manifest and driver decision rather than a sidebar or terminal transport change.

## Runtime path

Core discovery follows this order:

1. explicit absolute `runtrol.corePath` for development;
2. the native Runtime bundled in a Marketplace package and materialized under extension global storage;
3. a `runtrol` executable discovered on `PATH`.

A failed discovery is retryable in the same window. Restore an unavailable configured executable at the same path,
then use the visible retry action; neither a settings change nor an Extension Host reload is required. Concurrent
actions share the current discovery attempt, and an older failed attempt cannot invalidate its replacement. Window
registration and other startup services begin after the first successful initialization, including this recovery path.
After a running Runtime becomes unreachable, the same retry action reopens the installed Runtime before replacing
the public command connection and its cached locator. Concurrent explicit recoveries share one attempt; window
registration resumes in the same Extension Host. An interrupted input or other effectful request is never replayed
as part of recovery.

The new-conversation command focuses the sidebar before showing its service picker, so the focus transition cannot
dismiss the question it just opened. The picker remains visible while Runtime or service discovery is unavailable
and offers a retry action. Once discovery succeeds, it shows the currently launchable services.

Studio uses one approved public Runtime identity for provider inventory, managed sessions, approvals, and terminal
views. It validates the owner-local locator before every new transport lifetime. A terminal tab owns a dedicated
public streaming connection, so terminal output never poisons the ordinary request-response client.

The private connection is limited to Runtime bootstrap and optional owner administration. Studio's private protocol
projection contains no terminal open, attach, input, resize, output, or exit variants.

The bundled Runtime is materialized as an immutable, content-named executable in extension-global storage.
[Automatic updates](automaticUpdates.md#ownership) routes delivery and generation lifetime to their owners.
Restarting the Extension Host detaches its views; Runtime retains the provider processes under their existing
generation, and opening a conversation again attaches to that exact owner.
The sidebar's restart command first asks for confirmation, then closes this host's exact conversation tabs while
the editor connection is still available. It waits for their close events before restarting extensions. Canceling
leaves the tabs untouched; an unconfirmed close cancels the restart. Tabs created by other extensions are preserved,
including tabs with the same label. This command does not stop Runtime or the provider processes.

## One sidebar page

Studio contributes exactly one view in the Runtrol container, `runtrol.sidebar`, and draws it itself as a webview.
One view, deliberately: VS Code draws a collapsible section header for every view in a container as soon as there
are two, and moves the title actions into those headers. With one view the container's title bar keeps the two
actions that start things (`Add Project`, `New Conversation`) and the page below draws everything else. A native
tree cannot draw the edges between zones, the gauges, or the row density this page has.
The packaged container and its only view both use `Runtrol <version>` as their native manifest title. VS Code merges
them into one header, so the operator sees the product version once, without a second page title or punctuation.

The page has three zones with visible edges, in this order:

- **Projects**: one row per folder the operator added (or has open in this window). A project row collapses,
  shows its conversation count, its attention and running counts, and on hover its actions: new conversation
  here, pin, open in a window, archive or delete every eligible provider-owned conversation after exact
  confirmation, or remove from the sidebar (the folder on disk stays). Projects
  reorder by drag. The existing project record retains its provider-glyph accent, so adding or rearranging projects
  keeps open tabs stable. Allocation uses an unused palette colour while one remains. A subfolder conversation uses
  its deepest registered project's accent in both its tab and its open sidebar row. Rows have no left colour bar.
  Closed rows keep their muted provider icon; project headings and conversation titles remain readable without colour,
  following [W3C's use-of-colour guidance](https://www.w3.org/WAI/WCAG22/Understanding/use-of-color.html).
  Project identity and repository details wrap according to their content width, keeping large formatted addition
  and deletion counts visible in a narrow sidebar. Conversation titles remain one line.
- **Conversations**: the conversations that belong to no project, as plain rows.
- **Usage**: one chip per installed service, its icon inside a ring gauge with the seven-day percentage; see below.

A conversation row is the service glyph, a normal-contrast one-line title with a fading tail, an optional worded
action state, and the memory the provider process holds right now from the Runtime's `memoryBytes`. The bounded
refresh cadence is executable in [`controller.ts`](../extensions/runtrol-vscode/src/controller.ts). Only a provider-proven
open model turn spins the provider glyph. Opening a tab applies its project accent but does not start animation. A live
or paused TUI stays static even when it repaints its prompt, menu, or cursor. `Needs you` and `Error` remain static
worded states. The presentation in [`sidebarPage.ts`](../extensions/runtrol-vscode/src/sidebarPage.ts) distinguishes
actions and missing evidence. `Focus owner` means a registered window is proved to own the terminal and clicking
shows it there; `Observed only` means a process is live but this window has no attachment route. `Unavailable` does
not prove process exit: it can represent an unconfirmed owner while resumption remains refused. `Activity unknown`
reports missing turn-state evidence without denying a separately verified live process. On hover the row shows its actions: pin, rename, stop when running, archive and delete
when the service reports those surfaces, allow and decline when a turn waits for the person. Rows are reached with
Tab and the arrow keys; Enter opens the conversation's terminal tab.

Everything rare lives behind the vertical dots at the top of the page: switching, refreshing, service set-up and
updates, phone pairing, Runtime integrations and requests, restarting the Extension Host. The empty list says
which of four things is true (connecting, unreachable, verifying the installed CLI, no CLI installed), each with
its own next step, and a first run offers the two starting actions as rows.

Project-wide archival and permanent deletion live in the project's context menu. Studio first applies the shared
[`conversationArchival`](../extensions/runtrol-vscode/src/conversationArchival.ts) or
[`conversationDeletion`](../extensions/runtrol-vscode/src/conversationDeletion.ts) decision to every current row;
[`projectConversations`](../extensions/runtrol-vscode/src/projectConversations.ts) owns the resulting plan and confirmation.
The confirmation distinguishes conversations that can go immediately, supervised processes that require an explicit
stop first, external live owners, and rows that cannot receive the chosen action. Retained rows show the actual reason,
including an unconfirmed process or a new process without a provider conversation identity. A provider that supports
the action is never blamed for those missing proofs. Cancel changes nothing. Choosing idle-only never stops a process.
Choosing stop-and-archive or stop-and-delete waits for Runtime to confirm exit, and each action still revalidates current ownership.
Provider refusal restores the row and is included in the final result. Removing the project heading is a separate
sidebar action that leaves both its folder and provider-owned history intact.

Project change counts are read by [`GitChangesWatch`](../extensions/runtrol-vscode/src/gitChanges.ts) after a real
change signal settles. A project has at most one active Git read and one coalesced follow-up; a removed project's
older result cannot populate a new row. A read failure shows `Git unavailable`, including its reason on hover,
rather than a clean working tree. The next change signal or **Look again** retries within the same process-rate
limits. Repositories created or first committed while the sidebar stays open can become measurable this way.
An idle project starts no retry timer. Git's [porcelain status contract](https://git-scm.com/docs/git-status#_porcelain_format_version_2)
distinguishes an unborn branch from a failed read; an absent executable or timeout is not an empty repository.

## Usage zone

Each installed provider is one chip: the provider's icon inside a ring gauge, the number under it. No provider
name is drawn; the icon is the label, so a chip's width never depends on a name and the zone reads the same with
three providers or ten. The ring is the seven-day window when the provider publishes one, otherwise the window the
provider says governs, otherwise an empty ring with its current cause. A blocking limit turns the ring and the
number the theme's error colour.

Hovering or focusing a chip previews that provider's detail panel under the strip without adding a competing browser
tooltip. Enter or click pins an informational panel, and Escape closes it. The panel lists the plan the provider
named, one thin bar per reported window with the provider's own label and reset, and the report age. A chip whose state
has one action performs that action instead. Studio never converts a missing percentage into zero
or derives account capacity from terminal text.

When an account request fails, the chip offers `Retry` and its panel explains the failure. Use `Retry usage` to
request a fresh provider report through the same bounded refresh path. A failed read is neither proof of sign-out
nor a provider declaring that it publishes no usage. If an earlier report exists, its figures and original report
age remain visible and are explicitly marked as the last report. A partial response that confirms sign-in but
cannot read limits uses the same retry behavior. A confirmed sign-out removes figures measured for the previous
account; only a later provider report may replace them. A successful report that publishes no numeric usage also
retires superseded figures. Failure retains evidence, while a confirmed absence replaces it.
Account observations apply independently of installation changes and usage pushes, so signing out clears older
figures immediately and a successful account refresh removes an obsolete retry action.

At a provider-reported reset, expired windows and the older aggregate blocking verdict disappear. Remaining windows
retain their values and report age, and the chip offers a retry. Reset is not evidence of zero consumption or
a renewed allowance. One timer targets the next reported reset so a quiet sidebar updates without another provider
query or periodic repaint.

Sign-in and sign-out use the provider's discovered command in a native VS Code task terminal. The CLI owns its
authentication and browser interaction. Completion of that exact task requests one structured account refresh;
neither an exit code nor an unrelated task proves sign-in or sign-out. A sign-out action appears only after confirmed
sign-in. The command handler and task listener are loaded on the first explicit account action.

The Runtime subscription is the refresh clock. Structured provider account events publish immediately to the shared
`providers/usageChanged` watch. Hosted terminal writes use a cheap quiet-edge clock only while a terminal is open, and
the provider-owned process roster supplies the same busy-to-quiet edge for a conversation started outside Studio.
Requests from multiple windows coalesce by provider. Each provider has one running query and one pending refresh;
its completion publishes independently, so another provider's slow or failed query cannot delay the result or its
next eligible refresh. A manifest-declared protocol account surface has a shorter
repeat floor than a process-backed reader because the latter can briefly use hundreds of MiB. With no open terminal
and no unread report, the supervisor sleeps until an activity wake or its slow backstop instead of polling while
idle. An activity edge inside a repeat floor stays in the same bounded provider set and runs when that floor expires;
it is never dropped into the backstop. [`account_probe.rs`](../crates/runtrol-daemon/src/account_probe.rs) owns every
executable deadline, floor, quiet interval, and backstop. A successful unsupported or unmetered result waits for an
actual activity edge, account action, or verified provider update instead of joining the slow sweep. An account
response from a replaced executable is refused, and the current installation is checked again.

## Terminal tabs

A conversation opens as the provider CLI's own terminal interface in an editor tab. The provider owns its composer,
model and effort controls, permissions, approvals, and history. Studio writes no prompt and parses no screen meaning.
The tab uses the provider's own glyph, accented with the same exact colour value as its open sidebar row. A generic
conversation codicon is never substituted for a project identity.

The sidebar uses the provider SVG while terminal tabs use registered color font glyphs generated from that SVG and
the shared project palette. The global icon registry keeps those glyphs available when an editor moves to a separate
window. On the pinned VS Code version, custom URI terminal styles are rebuilt from a transient editor-instance list;
their presence during a move cannot guarantee the tab icon remains visible. Studio does not inject workbench CSS or
change the user's theme to repair that boundary.

`terminalTabs.ts` uses the public TypeScript Runtime terminal client. On transport loss it re-reads the locator and
reattaches only to the descriptor's exact Runtime generation. The returned screen snapshot replaces the view. If an
open returns `terminalAlreadyLive`, Studio lists generations and attaches to that exact owner. It never redirects to
the newest generation, retries input, or falls back to private IPC.

Closing a tab detaches the viewer. Runtime keeps the provider terminal alive until its CLI exits or an authorized
explicit stop occurs. Split, grid, focus, and full-screen behavior belong to VS Code.

Arrange Open Conversations in a Grid distributes existing conversation tabs over native editor groups. It waits
for the target terminal and its native editor reveal before moving the active editor, including when conversations
share a title. Additional tabs share the layout; arrangement never creates another provider process. The public
layout and move commands are defined by [VS Code's editor command implementation](https://github.com/microsoft/vscode/blob/main/src/vs/workbench/browser/parts/editor/editorCommands.ts).

Studio activation never opens, continues, or resumes a conversation. It restores only the selected row and starts
the provider, session, and terminal index watches. A live terminal row attaches to its exact Runtime and terminal
generation; a cold provider-owned row starts only after the operator explicitly opens it.

Provider terminals started through an installed transparent command shim appear through the terminal index watch
without a catalogue poll. If the provider mints an identity and title after launch, Runtime binds the provider's
verified process record to that PTY and Studio rekeys the existing row and tab in place. The provider title replaces
the project placeholder, while the provider glyph and exact accent remain identical on the open row and tab.

A process already running outside the broker appears as live only while the current provider process roster proves
it. A failed roster read revokes `Elsewhere` and shows the prior owner as `Unavailable` until a successful round
resolves it; that deny-only state prevents a duplicate resume without claiming the process is still live. A terminal
descriptor is valid only while its exact generation stream remains connected. Studio does not attempt a duplicate resume.
Runtime marks the row openable only when the provider publishes an official live target; the first click allocates
the one shared attachment renderer, and observation alone allocates none. A row whose terminal a registered window is
proved to own says `Focus owner`, and a live row nothing can reach says `Observed only`. VS Code
windows are independent viewers, not process owners or operating-system capture boundaries. Shared Runtime producers
publish native changes to all watching windows. Source lifetime and the explicit compatibility polling boundary
follow [native observation](providerDiscovery.md#native-observation).

No published Studio version before this public terminal contract persisted a private terminal attachment identity,
so there is no discoverable legacy tab to migrate. Runtime's native claim registry and `legacyGenerationBusy` error
protect any older live owner without inventing a client-side bridge.

## Window registry

Every Studio window registers itself with the Runtime once it is ready and keeps that record current from VS
Code's own events (`windowRegistry.ts`, `windowRegistryState.ts`): the window's session identity (kept across an
Extension Host restart, renewed by a reload), a host generation minted per activation, its workspace folders, and
every ordinary terminal it observes with the shell process id, whether shell integration is attached, the working
directory, and the command generation shell integration reported as running. Nothing is polled: a terminal opened
or closed, shell integration attaching, a command starting or ending, and a folder change each publish the whole
set once, with one publish in flight and the latest state sent after it. A terminal's name is read again at publish
time because VS Code names it after the shell starts and raises no event for that. Each validated Runtime generation
has a dedicated registration connection, retained while that exact generation remains present. Replacing the generic
command connection does not discard another generation's registration. A failed registration connection invalidates
its group, and a restarted host registers with a higher registration generation and no duplicate. Retained-generation
lifetime follows [Runtime operations](runtimeOperations.md#update-and-rollback).
`tooling/window-registry-eye.mjs` proves registration on two isolated windows and a third,
development-mode window restarted by keys.

### Observed mirror

A provider started in an ordinary terminal of a Studio window is mirrored by that window (`windowRegistry.ts`,
`observedMirrorState.ts`). When shell integration reports a command whose program word is one of the inventory's
provider command names (`ProviderDescriptor.commandNames`, the manifest's own `bin.names`; the word is read past a
PowerShell call operator, unquoted, as a file name without a launcher extension), the window takes the execution's
output stream synchronously inside the start event (measured 2026-09-02: taken after any await it yields nothing),
opens a mirror through `windows/mirrorOpen`, feeds every captured chunk in order through `windows/mirrorOutput` (64
KiB per call, base64 of the exact UTF-8 bytes VS Code delivered), and ends it through `windows/mirrorEnd` with the
exit code when the command ends or the terminal closes. The feed has its own connection, and the open names the owner
window by its session identity rather than being inferred from the connection: chunks then never queue behind a
person's click, and a chunk the Runtime refuses closes only the feed, never the dedicated connection that holds this
window's registration in the registry. The Runtime hosts the mirror as a terminal whose child is
the feed (`runtrol-core::terminal::fed`): viewers, the raw lane, the checkpoint and the sidebar row apply
unchanged; the descriptor says `origin: observedMirror` with the owner window's session identity and terminal key.
Exact-byte viewer writes are refused and Stop answers that the owner window stops it. A provider
typed by name is brokered by the transparent shim instead: the shim sends the processes above it (the invoking
shell is among them, behind the `.cmd` launcher), the Runtime files the brokered terminal under each of them,
refuses a mirror open for that shell, and retires a mirror that opened first (measured: the mirror opens about 20 ms
after the command starts and the shim's brokered open retires it within the second), so one command generation
is one row. A mirror ends with the connection that feeds it. `tooling/observed-mirror-eye.mjs`
proves this on two isolated windows with the fixture TUI, real Claude and real Codex by absolute path, and Claude by
name through the shim.

When the exact registered owner has an active authorized input receiver, the row offers `Open input view here`.
This explicit action attaches the existing mirror in the current window. Typing uses the Runtime's separate
[observed-owner text contract](terminalSurface.md#observed-owner-input) and the owner's public `Terminal.sendText`
API. The ordinary row click still reveals the owner. The owner verifies the same terminal object and shell execution
after each claim, invokes its API once with automatic execution disabled, and returns a structural receipt. VS Code
owns newline conversion; the receipt does not assert shell execution or exact stdin bytes. Disconnect, stale
execution, cancellation and uncertain delivery never cause an automatic resend. Observation retains byte counts and
bounded structural failures, with no captured output head or conversation sample.

### Owner reveal

A row whose terminal another window owns (the descriptor's `ownerWindowSessionId` is not this window's) does not open
here. Its click asks the Runtime (`windows/reveal` with the owner's session identity and terminal key); the Runtime
sends the owner window a `windows/revealRequested` on the owner's reveal subscription (`windows/watchReveals`, a
dedicated connection every window keeps after it registers, reconnecting on its own), and the owner shows that exact
terminal the way a click on its tab would. The Runtime then brings the owner's editor window forward itself
(`runtrol-childproc::os_window`): a VS Code window belongs to the editor's main process, an ancestor of the Extension
Host whose pid the window registered, so the search walks that chain and stops at the nearest process that owns a
visible, unowned, titled top-level window (the chain of a window started from another editor's terminal reaches that
other editor too, measured 2026-09-02); among that process's windows it takes the one whose title carries the folder
name, or the only one. The window is restored if minimised and asked to the foreground; when Windows refuses (it
grants the foreground to the process that last sent input) the Runtime sends one input event of its own, a mouse move
of zero pixels that reaches no window, and asks once more; if that is refused too the taskbar button flashes instead.
The Runtime never attaches its thread's input queue to the editor's (`AttachThreadInput`), the usual third resort:
attached queues make two threads share one, so a stall in either freezes both, and the other one is the operator's
editor. A flashing taskbar button is the smaller loss. The answer says what happened (`delivered`,
`foreground` as `raised`, `flashed`, `notFound`, `ambiguous` or `unsupported`) and the clicking window says it in the
status bar. Nothing is typed into any window. `tooling/owner-reveal-eye.mjs` proves this on two isolated windows on
different projects clicking each other's provider rows.

## Session and workspace contract

- The exact managed-session release load and expected hot-process cardinality for this gate are owned by the
  `hostLoad` section of [`performance-budget.json`](../extensions/runtrol-vscode/performance-budget.json). Runtime's
  executable admission cap remains in [`session::tier`](../crates/runtrol-core/src/session/tier.rs), and the gate
  proves that the observed cardinality agrees.
- Each visible tab renders its own bounded view of one central PTY stream. Runtime never duplicates the provider
  process, output ring, or screen state per window.
- Projects retain the operator's order. Conversations move between attention, working, idle, and saved bands when
  their operational state changes, but streamed bytes never reorder rows within a band.
- Search uses project, provider, state, and workspace metadata without reading conversation content.
- Selecting a cold row resumes through the provider-native identity in its exact workspace.
- Equal, ancestor, and descendant writer roots collide atomically; separate linked worktrees do not.
- A bounded selected-session identifier may survive reload. Conversation content may not.
- A session waiting on the operator contributes to the view badge and **Open Next Waiting Conversation**. Quota waits
  do not pretend to be operator tasks.

## Legacy cleanup

The first activation of each Core image calls `legacyCleanup.ts`, which runs the exact Core `legacy cleanup` command
once. Core removes the provider MCP registrations, Runtime grants, and local credential slots that earlier Runtrol
builds created for the retired Agent Tools and cross-consult surfaces, through each provider's official CLI commands,
and reports every entry it preserved. Studio registers nothing and edits no provider configuration file.

## Uninstall

`package.json` declares the `vscode:uninstall` hook `dist/uninstall.js`. VS Code runs it with its own Electron as plain
Node on the start after Studio was removed. Every activation writes `uninstall.json` beside the hook naming the global
storage this Studio owned; the hook stops the daemons running from the managed Core directory through `runtrol panic`,
removes that global storage (Core images, provider shims, projectless scratch, digests), and removes the Runtime state
root unless a standalone Runtime install shares it. Provider profiles, provider processes Runtrol never started, and
provider-owned conversations are never read or touched. `runtrol panic` itself withdraws the daemon's locator entry
before termination, so nothing lists a dead process afterwards.

## Module boundaries

| Module | Owns | Must not own |
|---|---|---|
| `core/locator.ts` | Runtime candidate order and endpoint probe | provider names or session policy |
| `core/managedCore.ts` | digest verification and content-named Runtime materialization | session state or provider policy |
| `core/framing.ts`, `protocol.ts` | bounded private administration frames and their TypeScript projection | public terminal operations or provider fields |
| `runtimeClient.ts` | approved public identity, locator lifetime, inventory, sessions, approvals, terminal generations | provider credentials or transcript storage |
| `terminalFleet.ts` | merge exact terminal indexes from current and draining Runtime generations | opening, redirecting, or duplicating provider processes |
| `nativeActivityProjection.ts` | one current provider process-roster projection and failed-proof revocation | stored conversation discovery or terminal ownership |
| `sidebarView.ts` | VS Code webview host, Runtime-state projection, bounded view state, and command dispatch | provider calls from page code or conversation content |
| `sidebarPage.ts` | pure sidebar HTML, CSS, project and conversation row markup | Runtime access, provider policy, or durable state |
| `usageDisplay.ts`, `usageStrip.ts` | provider-neutral usage semantics, chips, gauges, and detail panels | inferred capacity or provider-specific branches |
| `providerAccountAction.ts` | explicit provider account tasks and exact completion refresh | credentials, browser authentication, or interpreting command output |
| `connectionActions.ts`, `connectionSurface.ts` | bounded command-only loading for phone, account, and extension restart actions | activation-time account work or provider discovery |
| `stateRows.ts` | exact row equality and incomplete-discovery notices | rendering, Runtime calls, or transcript inspection |
| `controller.ts` | explicit user actions, provider-neutral navigation, workspace binding | transcript discovery or an agent loop |
| `terminalTabs.ts` | one public Runtime terminal view per editor tab | reading, storing, rewriting, or retrying terminal input |
| `legacyCleanup.ts` | one exact Core cleanup run per Core image | provider configuration bytes or any registration |
| `core/uninstallRecord.ts`, `uninstall.ts` | the uninstall record and the post-uninstall hook that removes Runtrol's own residue | provider profiles, provider processes, or conversation content |
| `selectionStore.ts` | one bounded selected-session identifier | prompts, replies, terminal frames, or provider state |
| `pairingAdministration.ts` | local phone pairing and authority review | relay trust or conversation content |

## Performance contract

[`performance-budget.json`](../extensions/runtrol-vscode/performance-budget.json) is the executable catalogue for
Studio release responsiveness, Extension Host memory growth, release-load cardinality, simultaneous-window terminal
latency, and installed-provider delivery. The Extension Host gate
measures activation, opening, refresh, memory growth, native resume, hot-session switching, reload restoration, and
workspace-follow arrival against its `host` and `hostLoad` sections. Exact session counts must hold in every isolated
trial even when timing noise requires another trial.

The multi-window gate uses the separate `multiWindowTerminal` section. Its first sample includes VS Code's public
terminal input dispatch, while warm samples begin at the production pseudoterminal callback and cover Runtime, PTY
echo, fan-out to the other window, and writer handoff. Raw samples are retained only in the bounded test result. The
product stores no terminal transcript or performance trace. See [terminalSurface.md](terminalSurface.md) for the
measurement boundary.

## Brand

`assets/brand/` is the source of truth. The packaged Marketplace icon is the coral and white mark on graphite and is
copied into `resources/icon.png` during the build. The Activity Bar uses the canonical silhouette SVG because VS Code
masks contributed Activity Bar icons to the current theme foreground. Sidebar primary actions use the configurable
`runtrol.accent` color, whose dark default is canonical coral `#F56565`.

## Distribution

The public identity is `runtrol.runtrol-studio`. `release-policy.json` independently owns the Studio version while
the workspace `Cargo.toml` owns Runtime and SDK versions. `release-targets.json` owns the complete native package
matrix and runner mapping.

Each VSIX contains one matching native Runtime, production bundles, canonical brand assets, license, notice, and
Marketplace README. Source, tests, build tools, and development dependencies are excluded.

An exact release commit changes only the changelog and Studio release policy. After the Gates workflow succeeds for
that same main commit, the release workflow creates the governed annotated tag and a tag-bound draft GitHub Release
as durable staging. It repairs only missing or invalid target assets, audits the complete staged set, publishes and
verifies Marketplace packages, and runs public install journeys before exposing the GitHub Release. A failed-jobs
rerun reuses already verified draft assets. `publishExisting` reads an already public tagged release and can retry
Marketplace publication without rebuilding or changing its assets. [automaticUpdates.md](automaticUpdates.md) owns
the operator procedure and recovery rules.

## Verification entry points

Development builds require Node.js and `uv`. The font generator declares its exact build-only Python dependency in
[`providerFont.py`](../extensions/runtrol-vscode/tooling/providerFont.py). The ordinary extension build derives the
font, lookup map, and manifest registrations from the provider glyph inventory and project palette. Installed Studio
loads these assets and runs no font generator. A clean checkout builds the registrations without a prior generated
manifest, and CI installs the same required build tool before invoking the extension build.

| Gate or command | Contract |
|---|---|
| `npm --prefix extensions/runtrol-vscode run check` | TypeScript public and private boundary consistency |
| `npm --prefix extensions/runtrol-vscode test` | sidebar webview projection, usage, Runtime client, administration, and terminal behavior |
| `vscodeExtension` | one sidebar view, theme color, command, storage, package, and provider-neutral boundaries |
| `vscodeHostPerformance` | real release-load Extension Host responsiveness against the shared budget catalogue |
| `vscodeMultiWindowTerminal` | deterministic two-window identity, first-use dispatch, warm input and fan-out, handoff, and cleanup |
| `vscodeRealProviderJourney` | installed provider discovery and complete real CLI control journey |
| `node extensions/runtrol-vscode/tooling/real-window-eye.mjs` | isolated real VS Code visual journey and screenshots |
| `node extensions/runtrol-vscode/tooling/drag-select-eye.mjs` | a real pointer drag selects text in a Runtrol tab whose provider switched mouse reporting on, with screenshots and a public-wire screen comparison |
| `node extensions/runtrol-vscode/tooling/window-registry-eye.mjs` | two isolated windows and a development-mode third register with one Runtime, follow terminal open, command, and close, and survive an Extension Host restart as one entry each, read through the public wire |
| `node extensions/runtrol-vscode/tooling/provider-account-eye.mjs` | real Windows task completion refreshes the account once, an unrelated task does not, and the native task terminal is photographed without executing authentication |
| `vscodePackage` | complete target SSOT, exact archive contents, Runtime bytes, workflow, README, and brand metadata |
| `crossPlatformMatrix` | exact VSIX installation and first-run action on native Windows, macOS, and Linux |
| `vscodeUpgradeRollback` | active-session continuity across official VSIX upgrade and rollback |
| `node extensions/runtrol-vscode/tooling/installed-package.mjs --marketplace` | public Marketplace download, activation, bundled Runtime, view, and first-run command |

The Windows-only `tooling/inspect-vscode.mjs` development helper can list, capture, type into, or click a real VS Code
window after foreground verification. It is excluded from the VSIX and is not a product surface.
