# Changelog

All notable changes to runtrol will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Entries are written from the user's point of view. Internal code names, plan numbers,
and refactoring that no user can observe do not belong here.

## [Unreleased]

## [0.1.2] - 2026-08-16

### Added

- Every New Chat entry now guides the user through service, workspace, the installed CLI's current model choices,
  and the reasoning efforts available for that exact model. Runtime rechecks explicit choices immediately before
  starting the provider process.
- The active conversation header now keeps the requested model, reasoning effort, provider mode, context use,
  provider-reported session cost, and short and long account limits visible together.
- Studio automatically lists provider-owned chats through each coding service's official Runtime catalogue, then
  resumes the selected chat without scanning or copying private transcript storage.
- The Chats menu includes an explicit Extension Host restart action for recovering extension state while preserving
  the supervised Runtime and provider processes.
- Sessions can be given a short name from the VS Code session list. Automatic names combine the project and installed CLI, and only colliding names receive a short discriminator.
- Local products can enroll with the owner-only Runtime endpoint and receive only the exact provider and project access approved in Runtrol Studio. Studio can review, narrow, deny, and revoke integrations without ending supervised sessions.

### Changed

- Studio now opens around a chat-first layout. Each available service exposes a permanent New chat row, secondary
  views start collapsed, the empty conversation has one primary action, and the composer shows only the action valid
  for the current chat state. The plus action and Command Palette now use the same guided New Chat flow, and choosing
  Provider default leaves the installed CLI's current model or reasoning setting unchanged.
- Selecting a session now opens one reusable conversation tab in the editor area. The wider layout keeps session state, workspace, output, approvals, and an expanding composer visible without squeezing the conversation into the sidebar.

### Fixed

- Studio populates navigation from one exact Runtime inventory before background change streams connect, keeping cold
  activation within its startup budget on every supported operating system. Session navigation initializes as soon
  as Runtime is ready while the independent Mission list continues loading.
- Opening a conversation now completes only after VS Code's panel state and editor-tab model agree on the active
  Runtrol tab, preventing intermittent focus on another editor. Studio also re-probes an already live Webview after
  hide and restore transitions, waits for its renderer before completing the command, and reloads one silent renderer
  instead of waiting for a startup notification that has already been delivered.
- Studio verifies a newly installed CLI in the background and refreshes its provider state automatically, so the CLI
  can become usable without an extra restart or manual refresh.
- Local Runtime and Studio connections retire Windows named pipes gracefully before an immediate reconnect, avoiding
  startup stalls while preserving the same owner-only endpoints.
- Windows Runtime listeners keep another named pipe ready while accepting the current client, so simultaneous Studio
  and public SDK connections no longer lose the daemon during connection churn.
- The TypeScript Runtime client now cancels stalled handshakes and subscription reconnects within the caller's exact
  deadline, including attempts that have not created a transport yet.
- Runtime event streams no longer rescan installed providers for unrelated authenticated requests, and disconnected
  idle streams are retired immediately. Studio activation, hot-session switching, and reload restoration remain
  within their enforced budgets on Windows.
- Studio retains an unexpired control lease across reconnects and same-Runtime workspace reloads without letting a
  slow editor secret store block a completed session action, preventing false control conflicts after restarts.
- Closing a session now invalidates any concurrently refreshed inventory and accepts a cooled session with no durable
  pointer as already closed, so removed sessions disappear from Studio immediately.
- Runtime retires disconnected local event and session-index watches even while their streams are quiet, then returns
  allocator pages after the watch task and bounded large live events fully leave the fan-out path, keeping settled
  daemon memory within the enforced budget.
- The phone app now permits its same-origin presentation contract under its content security policy, and its pairing
  screen reports readiness without exposing actions that require an already connected PC.

### Removed

- The standalone desktop GUI, its `runtrol gui` execution path, Tauri dependencies, and GUI-only build and test jobs. Runtrol Studio in VS Code is now the only PC user interface.

## [0.1.1] - 2026-08-12

### Fixed

- Installed CLI discovery no longer repeats one expensive help process for every candidate flag. The measured cold
  Core startup on Windows fell from about 30 seconds to about 4.3 seconds.
- Webview performance and startup messages now wait for the renderer readiness handshake, avoiding lost messages on
  slower VS Code hosts.
- Selected-session persistence retries only short operating-system file locks within a bounded window, so a transient
  Windows scanner lock cannot abort a session switch.

## [0.1.0] - 2026-08-12

First public Runtrol Studio release for six native Windows, macOS, and Linux targets.

### Added

- Session-preserving VSIX upgrades and rollbacks. The bundled Core is materialized at one stable extension-global
  path and atomically replaced behind a preserved mapped image, so Extension Host reloads and official VS Code
  install or uninstall operations keep the original daemon, provider process, selected session, and workspace alive.
- A full installed-provider VS Code journey. A real Extension Host now auto-discovers Claude Code, starts sessions in
  two workspaces, carries a prompt and hidden approval denial, reconnects, interrupts, switches the same window,
  restores the exact selected session, and closes every exact process without storing conversation content.
- A 30-session VS Code control surface with selected-first stable ordering, fuzzy project, provider, state, and path
  search, immediate cold-row feedback, provider-native cold resume, and at most eight supervised hot processes. The
  real Extension Host ratchet waits for Core watch acknowledgement and Webview paint on every hot switch, bounds cold
  resume, and restores the exact selected session from one bounded scalar preference in a new VS Code process.
- Core-owned project and working-tree identity with atomic writer reservations. Separate folders in one Git worktree
  cannot race through opening, live, or closing states, linked worktrees remain independently usable, and only the
  existing explicit shared-start action can opt into overlapping writers.
- Workspace collision visibility in the VS Code start flow. Exact, ancestor, and descendant hot workspaces are shown
  before another writer starts, with focus-existing, choose-another workspace or worktree, explicit continue, and
  cancel outcomes.
- One event-presentation SSOT shared by desktop and VS Code, with presentation kind, message side, and localization
  keys for all 19 wire events and fault-injected coverage against the Rust vocabulary.
- A real VS Code Webview burst ratchet at 3,000 raw frames per second, including animation, input, scroll, queue,
  DOM, character, RSS, activation, view-open, and refresh budgets.
- Platform-specific `Runtrol Studio` release packaging with one exact bundled Core, license and archive allowlist
  verification, clean stable VS Code installation, automatic bundled-Core discovery, and native release automation.
- A change-only session-index subscription. Surfaces receive one current snapshot and then only list-visible changes;
  stable conversation content causes no list rebuild, and one encoded snapshot is shared across every subscriber.
- The first `Runtrol Studio` VS Code slice. It discovers a configured, bundled, or PATH Core,
  lists runtime-discovered CLIs and sessions in native views, keeps one selected framed watch,
  switches the same window to the session workspace, renders a bounded live conversation, and
  carries prompts, interruption, close, and provider-native approval choices without persistence.
- A real VS Code Extension Host performance ratchet for ready activation, view opening, refresh p95,
  and RSS growth. Refreshes reuse one serialized greeted command connection, removing repeated
  endpoint handshakes and rapid Windows named-pipe churn.
- A Core-owned `runtrol endpoint` command that starts the daemon when needed and reports the exact
  named-pipe or Unix-socket address without duplicating home or endpoint rules in native surfaces.
- A provider-neutral desktop application that keeps every discovered session in one list, opens hot
  sessions from their bounded live tail, and uses the provider's native surface for cold resume.
- Confirmed removal that deletes only the runtrol list pointer and leaves the provider-owned
  conversation available through its original CLI.
- A nonblocking composer with Korean IME commit protection, selection and copy support, and an
  editable next draft while the previous prompt is still running.
- Bounded live rendering that retains the newest reply tail and status under sustained output, with
  measured interaction, frame-time, GUI memory, WebView process-tree, and cleanup contracts.
- A single Windows product binary that embeds the desktop bundle, preserves an inherited shared
  console, hides only its own private console, and leaves supervised sessions with the daemon when
  the window closes.
- A provider-neutral `runtrol answer` command that binds an approval choice to its session,
  approval identifier, option, and exact subject digest.
- A watch subscription acknowledgement, so callers know the event boundary is installed
  before sending work that may immediately ask for approval.
- A hosted-safe real Claude Code approval journey. It uses a local deterministic model endpoint,
  denies the real hidden stdio tool request, requires the provider's `end_turn`, and proves the
  denied file and provider child process are absent afterward.
- Lazy production probes that hand the exact inspected program to its driver, include interpreted entry
  files in cache identity, bound captured output before allocation, run outside the session event owner,
  refuse missing required flags, and never silently drop an explicit optional choice. Model calls, process opens,
  and command writes also stay outside the event owner, while guarded reservations keep opening and cleanup work
  counted against the bounded session-process slots.
- Credential-free hosted model discovery. Codex enumerates its live protocol catalogue, while Claude
  exposes stable aliases plus an honest partial catalogue from provider-owned read-only state. Hosted CI
  proves that file-backed path through an isolated sentinel and scans all production source for leaks.
- A cross-consult toggle. One switch registers Codex as a consultable server inside Claude Code using
  only the CLIs' own official commands, verified against the server's own tool list before wiring and
  against the registering CLI's own answer after, so a Claude turn can ask Codex for an opinion and
  relay it back. The reverse direction reports honestly as unsupported with the measured reason, flipping
  is never order-sensitive, turning it off restores the configuration exactly, and the switch works only
  at the machine, never from a paired device.

- North Star with a scored checklist. Every axis began at 0. Manual evidence can establish
  the manual tier, while every higher score requires its evidence gate to run in hosted CI.
- Architecture decisions across eight initiatives, each recorded with the measurement
  or source reading that produced it.
- Contract gates introduced before product code: workspace hygiene, forbidden
  folder names, silent failure detection with a self test, and AI attribution blocking.
- A scoreboard that computes rather than declares. Each axis score is derived from a base
  evidence tier, additives that only attach once the evidence is real and complete, and caps
  for gates that skipped or that nothing runs. The README in all four languages is held to
  the computed board, so a translated copy cannot keep yesterday's number.

- The logo, as vectors. A symbol, a wordmark, and three lockups in SVG, plus the favicon,
  app icon, tray icon, and social card sizes that cannot be vectors. The mark keeps one
  colour on light and dark backgrounds, so only the wordmark has a theme variant.

### Security

- The PC pairing identity persists across restarts only as a Windows CurrentUser DPAPI blob, so the
  raw Noise private key never rests on disk and other machine accounts cannot read it.

### Fixed

- Typing and scrolling no longer rerender the full multi-session rail during sustained output. Frame parsing stays
  bounded, and disabled production tracing no longer schedules hidden DOM checkpoint work.
- The conversation pane no longer freezes after one reply chunk larger than the reconnect window.
  A reconnect that lands behind such a chunk now shows the explicit gap and still receives the
  retained tail of the conversation, including the turn's ending, instead of waiting forever for
  output that had already finished.

### Changed

- Reduced Extension Host to Webview calls under sustained output while preserving the existing bounded queue and
  responsiveness budgets.
- Native VSIX packaging now resolves Core paths from the repository root and assembles each package in an isolated
  temporary directory, so a running development Core is never replaced or deleted by a release build.
- The VS Code Webview ratchet now records the isolated runner's native animation cadence before load, then enforces
  both a 40 ms absolute frame ceiling and an 8 ms p95 load-overrun ceiling. A 30 Hz virtual display no longer looks
  like application jank, while output-induced degradation still fails.
- The VS Code surface batches Extension Host delivery, coalesces only adjacent deltas of the same visible message,
  segments long streaming text, isolates offscreen layout, and avoids forced layout in its render loop.

- The immutable North Star now targets one responsive VS Code window for every project, session,
  and supported installed agent. It binds session selection to the exact workspace or worktree,
  keeps renderer and stream costs bounded, and treats visible waiting or stutter as a release blocker.
- Modularity, clean code, security, hygiene, and budget are named gates on a pass or fail
  board instead of prose. They are deliberately not worth points: a floor rule at 7 out of 10
  is a floor rule being broken, and a total that rises without the user receiving anything is
  the inflation the scoreboard exists to prevent.
