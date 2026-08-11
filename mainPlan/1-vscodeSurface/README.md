# VS Code primary surface

Status: active. This is the first product initiative after the immutable North Star reset.

## Outcome

One VS Code window controls every discovered local coding-agent CLI, project, session, and agent without making the
editor compete with provider output. Selecting a session binds the window to that session's exact workspace or
worktree. The provider CLI owns the conversation and repository changes. runtrol owns process supervision, bounded
event transport, session identity, workspace identity, and collision visibility.

## Implemented slice

The first end-to-end slice landed on 2026-08-11:

- `runtrol endpoint` ensures the daemon exists and reports the exact local IPC address from the Core SSOT
- the extension discovers a configured Core, a bundled Core, or `runtrol` on `PATH`
- the extension speaks the existing four-byte framed IPC directly over a named pipe or Unix socket
- native TreeViews show runtime-discovered providers and every session
- exactly one selected session has a live watch connection and reconnect cursor
- selecting a session opens its exact workspace in the same VS Code window and survives the reload by session id
- the active Webview bounds frames, characters, DOM nodes, and per-animation-frame work
- prompts, interruption, close, and provider-native approval choices cross the existing daemon boundary
- the extension stores no prompt, reply, draft, approval subject, or transcript copy
- the daemon pushes one current session index and then only list-visible changes, with one encoded snapshot shared by
  every surface and no session-list work for stable conversation content
- one greeted command connection is serialized and reused across refreshes, removing repeated discovery, handshake,
  and Windows named-pipe churn while watches remain independent
- an isolated real Extension Host ratchet measures ready activation, contributed-view opening, 40 refreshes, and RSS
  growth against `performance-budget.json` on every supported CI operating system
- the same real Webview run carries 3,000 raw frames per second while animation, typing, scrolling, Extension Host
  posting, renderer backlog, DOM nodes, visible characters, and memory remain inside one shared budget
- desktop and VS Code bundle one event-presentation SSOT for all 19 wire events; each surface owns only localized text,
  while a fault-injected gate rejects vocabulary drift and surface-local event-name maps
- starting another writer in the same, parent, or child workspace now shows the active collisions and offers focus,
  a separate known or browsed workspace or worktree, explicit continue, and cancel without scanning the repository
- the Core resolves bounded Git metadata into project and working-tree identity before provider discovery, then its
  single session owner atomically reserves that writer identity through opening, live, displacement, and closing;
  linked worktrees stay independent and only the surface's explicit continue action requests shared access
- the extension manifest owns release SemVer, while `release-targets.json` owns the six native Marketplace targets
- platform packaging includes exactly one matching Core and the repository license, while excluding source, tooling,
  dependencies, test budgets, and release metadata
- a fault-injected archive gate compares version, target, allowlisted entries, license, executable mode, and exact Core
  bytes before an isolated stable VS Code installs the package with no configured Core path
- the release workflow builds and clean-installs native Windows, macOS, and Linux packages before any publication job

The hosted `vscodeExtension` gate checks the thin boundary, no polling, no browser persistence, one selected watch,
queue and renderer bounds, TypeScript, framing tests, and production bundle size.
The hosted `vscodeHostPerformance` gate launches the product Core and production extension in a real isolated VS Code
profile. Its shared ratchet caps ready activation at 1,000 ms, view opening at 500 ms, refresh p95 at 50 ms, Extension
Host RSS growth at 48 MiB, Webview animation p95 at 40 ms, animation overrun from the unloaded native cadence at 8 ms,
input and scroll p95 at 50 ms, and renderer backlog at 1,024 frames while 15,000 raw frames cross the boundary in five
seconds.

## Module boundaries

| Module | Owns | Must not own |
|---|---|---|
| `core/locator.ts` | Core candidate order and one endpoint probe | provider names, session policy |
| `core/framing.ts` | bounded four-byte frame transport | request meaning, conversation rendering |
| `protocol.ts` | the TypeScript projection of the Rust wire | provider-specific fields |
| `state.ts` | session, provider, cursor, and selection metadata in memory | conversation frames |
| `controller.ts` | user actions, one watch lifetime, workspace binding | transcript discovery or agent loops |
| `conversationView.ts` | CSP and Webview transport | retained conversation state |
| `webview/` | bounded active rendering and input | durable storage, background sessions |

## Remaining gates before release

1. Extend the current 3,000-frame real Webview ratchet through session switching, workspace reload, and eight hot
   sessions.
2. Exercise start, prompt, approval, interrupt, reconnect, close, and workspace switching against installed real CLIs
   from an Extension Development Host.
3. Verify upgrade plus rollback without stopping active sessions. The hosted native release matrix already builds and
   clean-installs all six platform packages.
   Marketplace publication signs the complete platform VSIX, including the exact Core bytes already checked by the
   archive gate. A separate inner-binary signature is not claimed.
4. Publish `Runtrol Studio` to the Visual Studio Marketplace and verify a clean machine installs and runs it.

## Completion

This initiative is complete only when the Marketplace listing is public, a clean stable VS Code installs the matching
platform package, installed CLIs are discovered without configuration, the full real-provider journey passes, all
latency and memory ratchets are green, and no process started by the release verification remains alive.
