# Changelog

All notable changes to runtrol will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Entries are written from the user's point of view. Internal code names, plan numbers,
and refactoring that no user can observe do not belong here.

## [Unreleased]

Core implementation phase. No release yet. The repository holds the working supervisor,
its measured architecture, and the gate harness.

### Added

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

- The conversation pane no longer freezes after one reply chunk larger than the reconnect window.
  A reconnect that lands behind such a chunk now shows the explicit gap and still receives the
  retained tail of the conversation, including the turn's ending, instead of waiting forever for
  output that had already finished.

### Changed

- The immutable North Star now targets one responsive VS Code window for every project, session,
  and supported installed agent. It binds session selection to the exact workspace or worktree,
  keeps renderer and stream costs bounded, and treats visible waiting or stutter as a release blocker.
- Modularity, clean code, security, hygiene, and budget are named gates on a pass or fail
  board instead of prose. They are deliberately not worth points: a floor rule at 7 out of 10
  is a floor rule being broken, and a total that rises without the user receiving anything is
  the inflation the scoreboard exists to prevent.
