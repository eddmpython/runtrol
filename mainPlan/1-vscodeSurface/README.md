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

The hosted `vscodeExtension` gate checks the thin boundary, no polling, no browser persistence, one selected watch,
queue and renderer bounds, TypeScript, framing tests, and production bundle size.
The hosted `vscodeHostPerformance` gate launches the product Core and production extension in a real isolated VS Code
profile. Its shared ratchet currently caps ready activation at 1,000 ms, view opening at 500 ms, refresh p95 at 50 ms,
and Extension Host RSS growth at 48 MiB.

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

1. Show workspace overlap and provider-offered worktree choices before starting conflicting sessions.
2. Extend the real Extension Host ratchet to the Webview under 3,000 frames per second, typing, scrolling, switching,
   workspace reload, and eight hot sessions.
3. Move shared event presentation rules out of the retired desktop surface so VS Code and any future phone surface use
   one vocabulary SSOT.
4. Exercise start, prompt, approval, interrupt, reconnect, close, and workspace switching against installed real CLIs
   from an Extension Development Host.
5. Build one signed Core per Marketplace target, package platform-specific VSIX files, install each into clean VS Code,
   and verify upgrade plus rollback without stopping active sessions.
6. Publish `Runtrol Studio` to the Visual Studio Marketplace and verify a clean machine installs and runs it.

## Completion

This initiative is complete only when the Marketplace listing is public, a clean stable VS Code installs the matching
platform package, installed CLIs are discovered without configuration, the full real-provider journey passes, all
latency and memory ratchets are green, and no process started by the release verification remains alive.
