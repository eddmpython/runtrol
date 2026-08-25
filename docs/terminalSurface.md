# The terminal surface

The conversation surface is the coding service's own terminal interface, run by the Core on a pseudo
terminal it owns and shown by any number of viewers at once. Runtrol carries the screen; it does not draw
one of its own.

## Why

Every service ships a terminal interface and keeps it current. A page of ours has to follow each of them
feature by feature and is always behind. Carrying the service's screen costs nothing per feature: when a
service adds one, it is there the same day. It is also the thin principle as a surface: bytes travel, and
nothing between the service and the person reads them.

## Shape

- **The Core is the terminal.** `runtrol-childproc::pty` makes the pseudo terminal (`ConPTY` on Windows,
  `openpty` elsewhere, owned code, no third-party terminal crate). `runtrol-core::terminal` hosts the CLI on
  it: one reader thread, a bounded output ring shared by every viewer, a screen model (`vt100`, no
  scrollback) so a viewer that attaches late is handed the current picture.
- **The Core answers the terminal's questions.** Measured 2026-08-25: Claude Code asks XTVERSION and the
  cursor position at start and draws nothing until both are answered; Codex and Grok ask the cursor. The
  host answers as xterm 378 would (`terminal::xterm`), so the screen exists before any viewer attaches and
  two viewers never answer twice. Answers a viewer's own terminal sends are dropped on the input path.
- **One mouse for every service** (`terminal::mouse`). The Core switches mouse reporting on toward the
  viewer only, never toward the CLI, and translates each report into keys on the screen model: a wheel
  notch is three arrows, a click on a row is the arrows that move the cursor's row there. The service sees
  keys, as from a keyboard. Claude Code's own mouse reporting is switched off by its manifest
  (`CLAUDE_CODE_DISABLE_MOUSE=1`) so there is one feel, not three.
- **The launch is the manifest's word.** `[tui]` declares `new`, `resume`, `env`, `env_unset`. Nothing in
  the Core, the daemon, the extension or the phone knows a service by name. `env_unset` exists because a
  daemon started from inside a service's own session inherits that session's markers (measured: Claude
  Code switched transcript saving off when it saw them).
- **PC:** an editor-area terminal tab per conversation (`extensions/runtrol-vscode/src/terminalTabs.ts`).
  The tab's pseudoterminal is one private-wire connection. Split, grid and full screen are VS Code's own.
- **Phone:** the same terminal over the public Runtime, drawn by xterm.js. Same screen, same keyboard.

## Wire

Private wire (`runtrol-ipc`): `terminalOpen { provider, native, workspace, cols, rows }` or
`terminalAttach { terminal, cols, rows }` turns the connection into a view. Down: `terminalOpened`, then
`terminalOutput { bytes }` (base64) repeatedly, `terminalLagged {}` when the viewer fell behind the ring
(the screen follows; clear first), `terminalExited { code }`. Up, while the view lasts: `terminalInput
{ bytes }` and `terminalResize { cols, rows }`. Opening a conversation whose terminal is already open joins
it rather than starting a second process.

Scope: opening needs `session.start` and passes the same provider and workspace boundary a session start
does; joining and typing need `session.inputWrite`; a resize needs `session.outputRead`. Nothing here is
reachable with a listing scope alone.

## Lifetime

A terminal lives while its CLI runs, whatever the viewers do; closing a tab detaches a viewer and the
conversation continues. A draining Core generation counts open terminals as live work, stays until they
end, and refuses to open new ones (the successor takes them). When the CLI exits, the Core lets the last
frame drain before releasing the terminal (measured on Windows: releasing on the exit itself lost it).

## What is deliberately absent

No transcript, no parsing of the screen for meaning, no prompt rewriting, no model call. The screen model is
geometry for the mouse and a snapshot for late viewers, and it is dropped with the terminal.
