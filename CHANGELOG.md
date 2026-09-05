# Changelog

All notable changes to runtrol will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Entries are written from the user's point of view. Internal code names, plan numbers,
and refactoring that no user can observe do not belong here.

## [Unreleased]

### Added

- Observed mirrors: a provider started in an ordinary terminal of a Studio window (by absolute path, or by any
  spelling the transparent shim does not broker) appears as a hosted terminal fed by that window, with its origin
  and owner window in the descriptor; the shim now reports the processes above it (the invoking shell among them)
  so a command typed by name stays one row. Public protocol: `windows/mirrorOpen`, `windows/mirrorOutput`, `windows/mirrorEnd`,
  `TerminalDescriptor.origin` with the owner window fields, and `ProviderDescriptor.commandNames`.
- Owner reveal: clicking a row whose terminal another VS Code window owns asks that window to show the exact
  terminal and brings the window forward as far as Windows permits (flashing its taskbar button otherwise). Public
  protocol: `windows/reveal`, `windows/watchReveals` with `windows/revealRequested` and `windows/revealsEnded`, and
  `hostPid` on a window registration.
- Owner focus without shell integration: a provider typed into a Studio window's terminal that has shell
  integration turned off cannot be mirrored, but the Runtime still proves by process ancestry which window's
  terminal it runs in. Its row says `Focus owner`, and clicking it shows the terminal in that window and brings
  the window forward; nothing is opened or attached. A live conversation no window can reach now says
  `Observed only` instead of sharing `Elsewhere` with it, and a row with no live process says `Unavailable`.
  Public protocol: `NativeActivity.focusable` and `providers/focusNative`.
- Focus for an arbitrary external terminal: a provider running in Windows Terminal or a console window no VS Code
  window observes is `Focus owner` when the Runtime proves that its own process chain owns a desktop window, and
  clicking the row brings that window forward; `Observed only` otherwise. The proof repeats only when the live
  process set changes or after five seconds, never on every roster round.

### Removed

- The Windows console mirror: the `runtrol console-mirror` helper, the `consoleMirror` terminal origin, and the
  console route behind `attachable` in `providers/nativeActivity`. An arbitrary external terminal is focus-only under
  the accepted capability table: its row says `Focus owner` when a registered window is proved to own its terminal
  and `Observed only` otherwise, and nothing joins its console.
- The project actions and commands that turned Agent Tools on or off, the `tools enable` command, the `consult wire`
  command, and every Core request that could register an MCP entry in a provider's configuration. Runtrol no longer
  registers itself or one CLI inside another; only inventory, cleanup, and removal of earlier registrations remain.
- The `runtrol mcp` server, the `runtrol tools` and `runtrol consult` commands, and the Agent Tools crate. What earlier
  builds left in Claude, Codex, and the Runtrol home is read with `runtrol legacy inventory` and removed with
  `runtrol legacy cleanup`, which Studio runs once per Core image. Ordinary provider MCP entries are never touched.

### Changed

- A conversation tab's pane now holds the provider's bytes and nothing else. The Runtrol mark that was drawn while a
  tab opened, the clear-screen written before a replacement screen, and the exit and error sentences written into
  the pane are gone: opening shows as the window's progress indicator, an exit or failure changes the tab title, and
  a failure is also reported as a notification.
- The Core no longer rewrites provider terminal output: mouse-mode switches now reach every viewer exactly as the
  provider wrote them, and the Studio tab keeps its own selection and wheel by filtering that one control family at
  its own edge, checkpoints included.
- Exactly one window holds a conversation terminal's input and resize authority at a time. Typing in another window
  takes it over, visibly and in order (the terminal descriptor now carries `controlGeneration` and `controlHeld`),
  and a window that only changed size while another window was typing no longer resizes the shared process from
  under it; the size of the window that takes control is applied once when it does.
- The Core no longer switches mouse reporting on toward any viewer and no longer turns a viewer's mouse reports
  into arrow keys: what a viewer types reaches the provider exactly as written, and the private wire's viewer kind
  (terminal or touch) is gone with the translation it selected.

### Fixed

- Managed sessions can authenticate their shell tools to the local courier from their first command and after a
  Runtime generation handover. Windows verifies the pipe client's logon rather than rejecting a managed shell
  solely because its sandbox uses a different primary user. Foreign process trees, other logons and remote clients
  remain refused, and oversized frames are rejected before their body is allocated.
- A row click always does something a person can see (`STATE-04`): it opens the conversation, shows it in the
  window that runs it, or says in the row's own words why it cannot be opened. A conversation the Runtime is
  still stopping, one whose owner could not be rechecked, or one its service cannot reopen used to answer the
  click with a red error; nothing had failed, so the click now answers with that sentence as information.
- Open tab, closed tab, owner focus, mirror availability and Stop all read one generation record (`STATE-03`),
  measured from two real windows through every lifecycle path. A conversation whose process the Runtime was asked
  to stop keeps that record until the process ends: its row now says `Stopping`, offers neither a view the
  Runtime would refuse nor a second Stop, and no longer flips to `Observed only` for the second or two the
  provider's roster still lists the exiting process. A provider typed by absolute path into an ordinary VS Code
  terminal is one row everywhere: the Runtime now binds the provider's own process roster to the observed
  mirror by the same process-tree proof it uses for its own terminals, so the conversation's row carries the
  mirror's record and its owner window instead of standing beside an unnamed terminal row; that row offers no
  Stop, since the window that owns the terminal is what stops it.
- The Runtime publishes only proved generic states (`STATE-01`), replayed through the public protocol alone:
  process, owner, views, lease, output flow, checkpoint, lag, pending message and exit, each held to an exact
  event sequence. The public terminal descriptor now carries `viewerCount`, and the index republishes when a
  view attaches or ends, so a window can say a conversation is watched elsewhere without inferring it. The
  silence-based `looksStuck` hint is retired: silence is a diagnostic signal, not a state (a long tool call is
  quiet and not stuck), so a session row can no longer read as needing attention for being quiet; the field
  stays on the wire as `false` for older readers.
- A dead Runtime and a returning one leave no stale conversation behind (`EXT-08`), measured with three real
  owners of one provider (one started from the sidebar's `+`, one typed into a terminal, one on the desktop in
  Windows Terminal), ended, reopened, then the Runtime killed outright and restarted and the window restarted.
  When the Runtime stops, a conversation's RAM figure is a process's, so a row with no live process now shows
  none instead of keeping the last poll's number; and an attached tab whose transport was lost no longer raises a
  raw "Runtime closed during a frame" error over the sidebar's own "Cannot reach the Runtime Core" notice. When a
  new Runtime generation takes over, a window that the old generation knew re-registers itself with it, so another
  window can again reveal a terminal this one owns.
- One command generation is one row (`EXT-07`), measured with a real provider started from the sidebar's own `+`
  and another typed by name into a terminal of the same project. Clicking the placeholder row of a conversation the
  provider has not named yet now shows its own tab instead of starting a second provider. Studio no longer publishes
  its own conversation tabs (pseudoterminals) as observed terminals: VS Code answers their process id as `-1`, the
  Runtime refused the whole window update, and Studio treated the refusal as a lost connection, so the window's
  registration and every conversation tab on that connection were torn down and the tabs reopened fresh providers of
  their own. A refusal the Runtime answers now keeps the command connection; only a failure with no answer replaces it.
  A forced catalogue read asked while another forced read is in flight runs after it instead of being folded into a
  read that began before the change; a live conversation a Runtrol-owned terminal holds is asked for again like any
  other until the catalogue names it, so its placeholder promotes in place, and the placeholder's tab now follows
  the named row (the row reads as open and its click reveals that tab instead of opening a second one). The end
  of a mirror the transparent shim already replaced is no longer reported as a failed window publish.
- A conversation started in an ordinary terminal and first written down after its process was noticed (the usual
  case: a person opens the CLI, then types) reached the sidebar of no window, because the only catalogue refresh
  was the one its discovery had triggered. While a live conversation no row lists exists, its provider's catalogue
  is now asked again on a doubling wait (1 s to 15 s) until the row appears or the process ends.
- Pressing Escape alone in a conversation tab now reaches the provider at once. The Core's input boundary used to
  hold a lone Escape as an unfinished sequence and deliver it glued to the next key, which a CLI reads as an Alt
  chord. A mouse report a viewer sends is now forwarded to the provider exactly as written instead of dropped.
- A write into a conversation whose outcome is unknown (a short write, a broken pipe, a write the terminal never
  acknowledged) now ends that conversation terminal at once instead of leaving it open for more input on top of a
  partial line; nothing typed is ever written twice.
- A window no longer falls behind and gets its screen replaced during a fast provider burst: the Runtime host now
  reads a burst whole (waiting at most a millisecond for the rest of it) instead of publishing hundreds of tiny
  pieces, and a window that stops taking output altogether for ten seconds is closed explicitly rather than held
  open forever.
- The screen model that gives a late viewer its first picture no longer sits on the provider's output path. Every
  viewer receives each chunk the moment the host read it, a stalled or panicking screen model delays nothing and
  changes nothing for the provider or a viewer, and a late viewer whose checkpoint could not be trusted is told so
  instead of being handed a stale screen.
- A cursor position report now names the cursor where the provider asked, not where the host's read happened to
  end: the bytes before a question reach the screen model first and the answer observes that cursor. A question
  split across two reads is answered exactly once, when its last byte arrives, and only the unfinished question
  itself is carried between reads.
- `runtrol panic` now withdraws the daemon's own locator entry before it stops, so `runtrol status` and the standalone
  uninstaller no longer see a dead process listed after the panic button.

### Added

- Every Studio window now registers itself with the Runtime (its identity, folders, and the ordinary terminals it
  observes with their shell process ids, shell integration, and the command each is running) and keeps that record
  current from VS Code's own events, so another window can name exactly which window owns a terminal. The record
  leaves with the window's connection and a restarted Extension Host replaces it without a duplicate.
- The public Runtime protocol's terminal view opened and lagged messages now carry `checkpointAvailable`, which
  says whether the screen they deliver is the provider's current one; a client built before the field reads it as
  true, as every earlier Runtime meant.
- Uninstalling Runtrol Studio and restarting VS Code now runs a hook that stops the daemons Studio started, removes its
  Core images, provider shims, and other storage, and removes the Runtime state root unless a standalone Runtime
  install shares it. Provider profiles and conversations are never touched.

### Changed

- The first activation of each new Studio Core image removes the MCP registrations, Runtime grants, and local
  credentials that earlier Runtrol builds registered for Agent Tools and cross consult, through each provider's own
  removal command. An entry with the same name that Runtrol cannot prove it owns is reported and left untouched.
  `runtrol tools cleanup` runs the same pass by hand.

## [0.1.44] - 2026-09-01

### Changed

- Conversation rows and editor tabs now use the provider's actual glyph with one exact project accent. Open idle
  conversations remain static, only provider-proven model work spins the sidebar glyph, and the old left colour bars
  are removed.

## [0.1.43] - 2026-09-01

### Changed

- Idle Runtime recovery now uses one provider-neutral bounded schedule, and account usage readers start only when a
  client asks for usage. Immediate provider filesystem notifications remain the primary discovery path.

### Fixed

- Two distinct provider conversations started in separate VS Code terminals under the same project now replace their
  own provisional sidebar rows independently. Shell launchers are matched to their exact provider descendants by
  current process ancestry and creation identity instead of requiring the provider PID to equal the terminal root.
- Simultaneous terminal tabs no longer inherit the shared content-named Core executable label. Each tab identifies its
  provider and hosted terminal process, while ambiguous process mappings remain unresolved instead of merging two
  conversations.

## [0.1.42] - 2026-09-01

### Added

- Conversation tabs can be arranged into an editor grid. When another known VS Code window owns a conversation,
  Runtrol can reveal that window without starting another coding CLI.

### Changed

- One live conversation now keeps one provider process and one Runtime terminal across VS Code windows. Every
  attached window receives the same terminal output, and a remaining window takes writer ownership when the earlier
  writer closes.
- Sidebar colour and motion now mean provider-proven model work. An open but idle conversation stays still and
  monochrome, while attention and failure states remain visible without pretending the model is running.
- Runtrol Studio ships Claude Code and Codex in this release. Recent provider conversations render before slower
  history completes, with bounded titles, compact loading chrome, comma-formatted repository counts, and a wider
  project colour palette.

### Fixed

- Stale "Elsewhere" labels, ghost activity, the inactive project add button, exact-generation terminal reattachment,
  input after reopening an externally created conversation, and delayed discovery of new conversations are corrected.
- POSIX windows reuse a running Core and serialize first startup. New macOS machine identities use a login-user
  Keychain policy that survives content-named Core updates, and an existing path-bound identity is recovered after
  the Runtrol home is recreated.

## [0.1.41] - 2026-08-30

### Changed

- Live provider conversations now use a measured capture ladder: the existing Runtime PTY first, a lazily allocated
  official provider TUI attachment second, a Microsoft Windows console mirror third, and an honest observe-only state
  otherwise. Every VS Code window shares one bounded terminal renderer and opening a live row never runs `resume`.
- While a conversation opens, the tab shows the Runtrol mark itself: the four curved arms, two coral and two in
  the terminal's own text colour, drawn from the brand's 32 px geometry, with a coral light passing over it
  until the coding service draws its first line. Until now the tab showed four bracket characters standing in
  for the mark, which read as brackets and not as the mark.

- A running conversation shows it from its edge as well: a light runs down the project colour band beside
  the row, in step with its turning icon, so a glance at the list finds the rows that are working without
  reading each 14 px icon. The band keeps its colour, so it still names the project.

### Fixed

- The opening mark stays up until the coding service draws something a person can see. It used to come
  down on the first clear-screen or cursor-hide the service sent, leaving the empty rectangle it exists to
  prevent.

## [0.1.40] - 2026-08-30

### Fixed

- A stored conversation in a project that has a terminal open again opens instead of failing. A Runtime
  generation on its way out could be holding a terminal whose conversation it was never able to name, and
  every stored conversation in that project was refused on the chance of being that one. The coding service is
  now asked directly, and a conversation it does not name as open is no longer held back by an unnamed
  terminal somewhere else.

- A Grok conversation that is already open is no longer offered as if it were closed. Grok publishes no list of
  what it has running, so every conversation looked stored and clicking one could have started a second process
  on the same conversation. Grok keeps a directory per conversation and holds a file inside it while that
  conversation is open, which the Runtime now reads: an open conversation is shown as open and bound to the
  process that has it. A coding service joins this by declaring where that evidence is, so nothing in the
  Runtime knows any service by name.

## [0.1.39] - 2026-08-30

### Fixed

- Conversations you start in a terminal now appear in the sidebar without waiting for a window to ask for them.
  The Runtime looked for coding sessions only while answering a window, so a machine with no Runtrol window open
  found none at all, and a window that had just opened watched its own list fill in. The Runtime now waits on
  each coding service's own record of what it has open and reacts the moment that changes, so a session started
  anywhere is already bound and ready to click by the time you look. Waiting costs nothing while nothing happens:
  an idle Runtime measured 63 milliseconds of processor time across ten seconds, inside its hundred.

- A Codex conversation you are working in no longer reads as running somewhere else. Codex keeps one lock file
  per open conversation, and the Runtime could see that a conversation was open but never which process had it,
  so a terminal the Runtime was itself hosting stayed unattached to the conversation you opened inside it and its
  row offered nothing to click. The Runtime now asks the operating system which process holds each lock, which
  binds the conversation to its terminal.

### Fixed

- A window connecting right after a Runtime update is no longer refused once. The old generation keeps serving
  its terminals while it drains, but it answers new connections against the successor's authority relay, and
  that relay arrives on its own one-second cadence; on a busy machine the first connection could land first and
  be turned away. The draining generation now waits out a few relay rounds before answering, so the first
  request after an update is a moment slower instead of failing.

## [0.1.38] - 2026-08-30

### Fixed

- The sidebar's refresh no longer stutters while the Runtime restamps the executable search path. Each list
  request past a one-second floor re-checked the PATH surface, and on Windows a cold re-check costs around a
  hundred milliseconds, which landed inside the refresh several times a window. The re-check now runs at most
  once per ten seconds; a newly installed coding service still appears within that time, and starting one
  never waits on this cache.

- Clicking a conversation that is open in the coding service's own editor panel now offers to reveal it there,
  and the Runtime refuses to resume it as a terminal. Resuming forked the conversation into a second process
  showing a frozen copy of that moment while the real session went on elsewhere; now one conversation stays
  one process, and the click leads to the surface where it actually lives.

- Your editor's Claude panel conversation now spins its sidebar icon while the model is answering, and stops
  the moment the turn ends. A panel session writes no run state into the process roster the way a terminal
  session does, so the sidebar had no way to know it was working; the turn is now read from the session's own
  transcript, from the markers the CLI already writes at each turn boundary, never its message text.

- The conversation your editor's Claude panel is running no longer flickers in and out of the sidebar. A panel
  session has no terminal of its own (the Claude extension drives it over a private pipe), but it announced
  itself the same way a real terminal session does, so the Runtime kept trying to mirror a console that was not
  there; each attempt appeared as a row and vanished a moment later. The Runtime now mirrors only sessions that
  own a real terminal, and shows a panel session as running in its own window instead.
- Running conversations now sit at the top of their project and stay put. The list ranked rows by how recently
  each was touched, and a conversation that is answering is touched on every streamed byte, so several running
  at once reshuffled the list continuously. Rows are now ordered by what their session is doing, and conversations
  in the same state hold a fixed order that streaming output cannot disturb.

- A Runtime update no longer ends the Claude Code sessions running in your own editor. When a new Runtime took
  over, the old one closed the conversations nobody was watching so it could exit, and that sweep also ended the
  processes it had only joined as mirrors: every update killed the editor's own Claude Code sessions with
  `exited with code 3221225473`, two at a time, minutes after the update. The old Runtime now lets a mirror go
  and ends only processes it started itself.

- The Runtime no longer rebuilds its provider inventory every second while a window is open. The sidebar's
  activity observation (four times a second per service) counted as a request that must recheck the executable
  search path, so the Runtime walked PATH for every service once a second for as long as a window lived; on
  Windows that walk slowed every other answer, and the sidebar's refresh took several times longer than its
  budget. Observing activity now rechecks nothing; installing a service is still noticed by the requests that
  can see it.

## [0.1.37] - 2026-08-29

### Added

- A project row now shows what its repository holds that is not committed or not pushed: lines added and
  removed, files git has not seen, and commits ahead of upstream, as `+120 -35 ?2 ↑3`. Committing takes the
  first three to zero and pushing takes the last to zero, and a clean, pushed project shows nothing. It is
  measured when an agent in that project writes and when the editor's git extension sees a change, never on a
  timer.
- Each service's usage panel shows the installed CLI release beside its name, and an "Update to X" button at the
  right end when a newer release is confirmed with an exact rollback. One press updates; no dialog.
- The build's version sits in the title bar beside "Runtrol" instead of on a line under it.

### Changed

- A running conversation's icon turns on its own; the ring around it is gone.
- Repaints update only what changed, so a turning icon no longer jumps back to its start every time a figure
  on the panel ticks.
- The usage panel drops "Within limits" and the sign-in method ("via claude.ai"); the bars and the plan say it.
  A model window's name is shown whole instead of cut to "GPT…Spark".
- The list's scrollbar sits on the panel's edge instead of 8px inside it.
- A right click on a project row opens its menu: everything the hover icons offer, plus "Delete all
  conversations". The confirmation carries the exact numbers: how many go, how many a service cannot delete,
  how many are running (deleted only if stopped first, by their own button) and how many run outside Runtrol
  and are skipped. Deletion stays permanent, per conversation, in the provider's own store.
- A signed-in account's usage panel carries a quiet "Sign out of ..." line when its service publishes its own
  sign-out command (all three do: measured against each installed CLI). It runs that command in a terminal
  exactly as sign-in does; Runtrol holds no credentials either way.
- The project palette holds twelve hues instead of five, so a machine with several projects stops handing two
  of them the same colour. The first six are the editor's own terminal palette; the six extras are band-only,
  and a tab then narrows a conversation to a colour family of two instead of settling it, which is the most
  the editor lets a tab icon say.
- A usage chip whose limits a team manages shows nothing under its ring instead of the words "team-managed",
  which read as a broken state. The detail panel still says why there is no number.

### Fixed

- The Runtime no longer dies when a coding CLI restores a saved cursor after its pane was made smaller. It
  did, twice in one afternoon, and every window then reported "the daemon connection closed" and "Runtime
  reconnect deadline expired" at once. The screen emulator (vt100) is updated to the release that fixed it,
  and a test pins the sequence.
- One conversation, one process. When a coding CLI moved to another conversation inside its own terminal
  (`/resume`, `/clear`), the Runtime kept the terminal filed under the old conversation, refused the new
  identity on every roster round (each answered with an error), and a click on the new conversation's row
  opened a second process on it. The terminal now follows the conversation its process is in, so the row
  joins the terminal that already shows it.
- A conversation running in a terminal Runtrol hosts but this window is not viewing shows that it is working.
  Its icon stood still whatever the coding service's own roster said, because the row's state was fixed at
  "ready" the moment it was built.
- A conversation tab on the computer never enters mouse mode. A coding CLI that switches terminal mouse
  reporting on had that switch reach the editor's terminal, so a click became arrow keys (which recalled
  earlier input in the prompt) and drag selection stopped working. The switch is now taken out of the stream
  before any viewer or the screen model sees it; the mouse remains a touch-screen concept for the phone only.
- Codex conversations now show that they are working. A conversation running in Codex outside Runtrol never
  turned its icon, because the driver had no way to see one; the sidebar showed it sitting still while a model
  was answering in it. Codex leaves two facts on disk for every process to read: which conversations a live
  process holds open, and whether the last turn in one has ended. The sidebar reads both, so a Codex
  conversation turns while it works and stops when it stops, whichever window or editor started it.

## [0.1.36] - 2026-08-29

### Fixed

- Old Runtime generations no longer linger after an update. A generation that has handed over now closes the
  conversations nobody is watching once they have been idle a short while, so it can finish and leave instead
  of holding them for hours. The current generation still keeps a viewerless session so a window or phone can
  reattach; only a generation that has been replaced lets an idle one go, and the coding service keeps the
  conversation either way.
- A session opened elsewhere is recognised as the one conversation it is, not drawn a second time. A session
  another generation already holds is no longer mirrored as if it were new, and the sidebar draws one row per
  conversation however many generations hold a terminal for it.
- A terminal stream ending because the grant generation moved (a deploy, a re-enrollment) no longer surfaces
  an error or marks the Core unreachable. The window re-reads the locator and reconnects; only a revoked
  integration is treated as a real stop.

## [0.1.35] - 2026-08-29

### Changed

- The symbol that shows while a conversation opens is now the Runtrol mark held still, with a coral light
  sweeping across it from left to right and repeating, instead of the mark spinning. It draws on a cleared
  pane, so there is no tile behind it.
- The sidebar shows the build version in small type at the top, under the "Runtrol" header.

### Added

- A conversation you started outside Runtrol on Windows can now be opened and driven inside it. When a coding
  CLI is already running in another window, another app, or a previous Runtime that an update left behind,
  Runtrol attaches a helper to that process's console, streams its screen into every Runtrol window, and sends
  your keystrokes back to it. The process keeps running where it was started; Runtrol becomes another view of
  the one session rather than starting a second copy, so clicking the row attaches instead of resuming. Linux
  needs process-trace permission and macOS has no such door; there, a session is one only when Runtrol started
  it. (Piped or SDK sessions have no screen to join and are left as they are.)

### Fixed

- The sidebar no longer prints "Update applies when the running conversations end" under its header. On a
  machine with several Runtime generations alive, and especially when this window sees none of them running, it
  read as coming from nowhere. The generation supervision still runs; it just no longer narrates itself here.

- Signing in to a coding service now completes. Pressing sign in used to type the service's login command
  into a terminal and stop there, so no login ever opened. It now runs the command, and the CLI opens its own
  browser flow for you to finish. Runtrol still holds no credential: the CLI authenticates itself, which is
  the boundary this product keeps.
- The usage detail panel is now reachable. Hovering a chip opened it, but moving the pointer down to the panel to press a button left the chip strip and closed it first. The chip and its panel are now one hover region, so you can travel into the panel and click.
- A coding service you are already signed in to no longer shows a "Sign in" button under its live usage. The
  button now appears only when the account is actually signed out or disconnected.
- The service choice that opens from the panel's new-conversation button withdraws when you click anywhere
  else, press Escape, or leave the panel. It used to stay open until answered.
- A conversation tab in VS Code behaves like the terminal it is. Clicking inside it no longer brings back an
  earlier prompt, the wheel no longer walks through prompt history, and dragging selects text again. Runtrol
  had switched mouse reporting on toward every viewer and turned each click into arrow keys, which is what a
  phone needs and exactly what an editor's terminal does not; it now does that only for a touch viewer.
- A conversation kept alive across an update opens again. An update leaves the previous Runtime running
  beside the new one for as long as its conversations last, and the panel followed only the new one, so every
  conversation the old one still held read as "running in a terminal Runtrol did not start" and refused to open
  (on one machine, eight conversations across five earlier Runtimes, all of them dead ends). The panel now
  follows every Runtime the machine lists, and pressing such a conversation attaches to the terminal that
  already runs it, in the Runtime that owns it, instead of refusing or starting a second copy.
- The previous Runtime keeps answering after an update. It hands its store to the new Runtime and then
  refused every request, including a window's first connection, because it had nowhere to write its
  authorization record ("Runtime authorization audit storage is unavailable"). That is what made every
  conversation from before an update unreachable. It now keeps those records for the new Runtime, which
  collects them on its next poll and writes them into the one store.
- A refusal from a coding service now says why. Codex declining to resume a thread another window is writing,
  or Claude Code declining to delete a conversation a live process still has open, both arrived as one
  sentence ("refused this request for the conversation") and the Claude Code case even as "answered in a shape
  Runtrol cannot read". The service's own reason now rides with the refusal, so "another window has it" and
  "stop its process first" can be told apart and acted on.
- Stop now works on a conversation Runtrol hosts without supervising, which is every conversation kept alive
  across an update. It used to look for a supervised session and fail with "is not open yet". A conversation
  alive in a terminal Runtrol cannot reach is no longer offered a Stop that would fail.
- A second live process of one conversation, left behind when an earlier build resumed it again after an
  update, is shown as its own row under the conversation's title, so it can be opened and stopped rather than
  living on unseen.

- Pressing a conversation that is running in your own terminal now offers something to do. The panel can see
  those conversations, and it cannot open one: a terminal it did not start has no channel to take over. It used
  to answer with an error in protocol words and nothing else, which on a machine that keeps its CLIs open all
  day is most of the conversations in a folder. It now says where the conversation is, in a sentence written
  for a person, and offers to start another conversation in the same folder.

- A conversation running in your own terminal now shows that it is running. The panel used to decide from the
  conversation file's timestamp, and that file is written only when a message finishes: a turn that spent four
  minutes inside one command read as idle, and a turn that had just ended kept reading as busy. It now asks the
  service which of its own processes is answering, so the mark appears while the model works and goes when it
  stops.
- A running conversation is now unmistakable at a glance. An arc turns around its icon and the dot beside its
  name is blue. Turning the service's own icon was not enough on its own: Claude Code's icon is symmetric, so
  turning it looks exactly like standing still. The dot was painted with the editor's progress colour, which is
  grey in the dark theme this build of VS Code ships, so a running row looked like every other row.

## [0.1.34] - 2026-08-28

### Fixed

- The sidebar no longer prints "Update applies when the running conversations end" under its header. On a
  machine with several Runtime generations alive, and especially when this window sees none of them running, it
  read as coming from nowhere. The generation supervision still runs; it just no longer narrates itself here.

- A conversation already running in another window now opens as itself. Pressing it in a second window used to
  offer to resume it again, because the lookup that finds the live terminal compared folders as raw strings
  and the Runtime stores a path the window spells with a different drive case. It compares them as one folder
  now, and when a handover has left two generations naming it, takes the one still running.
- The name a service gives a conversation reaches its tab. A conversation started here is filed under a
  placeholder until its service writes it down, and only the list knew when that had happened, so the tab kept
  the folder's name for the rest of its life while the sidebar beside it showed the real one.
- Typing no longer stops mid-conversation. Two windows of one profile share the control of a terminal, so the
  one that renewed last leaves the other holding a retired generation, which the Runtime calls a control
  conflict. That was not counted as losing control, so the keystroke was dropped instead of asking again. An
  expired lease is also renewed no more: past its moment the only move is to take control again.
- Every usage bar was empty. Their widths were set on the element, and this page's own policy drops a style
  attribute without a word, so each bar drew at nothing. The same mistake had already cost the project colour
  band its colour; widths are stylesheet rules now, and a machine check refuses the attribute outright.
- The account panel offers its service's sign-in whatever the account looks like. A healthy account had no way
  to reach it at all, which is the one thing this surface is for when somebody is switching accounts.
- A project's name no longer disappears at a real panel width. The name could shrink to nothing while the
  chips beside it refused to shrink at all, and the hover buttons held a third of the row even while hidden.
- A service's own word for its account no longer runs into its neighbour's number.
- Conversations stop opening with a failure after an update. The panel installs the Core under a name made from
  its contents, so every update moves this program and takes the old copy away, leaving the tool entry a person
  switched on naming a program that is gone. Nothing was going to fix that on its own, because wiring happens
  when somebody presses the toggle and they already pressed it. The Core now re-points its own entry the moment
  it starts serving. It repairs and never creates: a project with no entry keeps none.

### Changed

- The panel stops rebuilding itself when a figure ticks. It used to rewrite its whole document every few
  seconds as memory readings changed, taking with it the detail panel a person had opened, the row they had
  focused and the place they had scrolled to. Only the content changes now.


## [0.1.33] - 2026-08-28

### Changed

- The symbol that shows while a conversation opens is now the Runtrol mark held still, with a coral light
  sweeping across it from left to right and repeating, instead of the mark spinning. It draws on a cleared
  pane, so there is no tile behind it.
- The sidebar shows the build version in small type at the top, under the "Runtrol" header.

### Added

- A conversation the panel did not start now turns its icon while its service is answering. Runtime gained a
  cheap question for it: which of a service's conversations were written in the last few seconds, answered by
  walking the service's own store for names and times without opening a single transcript. Until now the panel
  could only see a turn in a conversation it hosted, which on a real machine is the smaller half.

## [0.1.32] - 2026-08-28

### Fixed

- The sidebar no longer prints "Update applies when the running conversations end" under its header. On a
  machine with several Runtime generations alive, and especially when this window sees none of them running, it
  read as coming from nowhere. The generation supervision still runs; it just no longer narrates itself here.

- A conversation's tab now carries its project's colour. The colour was being handed to the editor beside a
  custom icon, which the editor draws as the image it is and tints not at all, so the tab kept the service's
  brand colour while the sidebar row beside it carried the project's. A conversation that belongs to a project
  trades the brand mark for the colour; one that belongs to no project keeps the mark, having no colour to show.

## [0.1.31] - 2026-08-28

### Fixed

- The sidebar no longer prints "Update applies when the running conversations end" under its header. On a
  machine with several Runtime generations alive, and especially when this window sees none of them running, it
  read as coming from nowhere. The generation supervision still runs; it just no longer narrates itself here.

- The mark that turns while a conversation opens now stays until the service itself draws. It was taken down
  when the Runtime answered, which is earlier: the answer carries an empty screen, so the tab went back to
  being the blank rectangle the mark exists to prevent.

### Changed

- The symbol that shows while a conversation opens is now the Runtrol mark held still, with a coral light
  sweeping across it from left to right and repeating, instead of the mark spinning. It draws on a cleared
  pane, so there is no tile behind it.
- The sidebar shows the build version in small type at the top, under the "Runtrol" header.

### Added

- A project row says which branch its folder is on, read straight from the repository rather than by running
  git, and says nothing when the folder is not in one.
- A project can be renamed from its own row, beside the actions that were already there.

## [0.1.30] - 2026-08-28

### Changed

- The sidebar spends its width on conversations. The title bar carries all three actions, so the strip that
  held one button under it is gone; the sentence about partial history is gone with it, and its answer moved
  into the title bar's menu. Everything below the header sits closer to the edges.
- A conversation's name stays on one line and fades where it runs out of room, instead of wrapping to two.
- A project shows five conversations and offers the rest behind one row, so one busy project cannot push the
  others off the screen.
- Usage stays at the bottom of the panel, and its detail gives each window's name a line of its own with the
  progress underneath, so a model's name is never the part that gets cut.

### Changed

- The symbol that shows while a conversation opens is now the Runtrol mark held still, with a coral light
  sweeping across it from left to right and repeating, instead of the mark spinning. It draws on a cleared
  pane, so there is no tile behind it.
- The sidebar shows the build version in small type at the top, under the "Runtrol" header.

### Added

- A conversation whose turn is running is marked by its icon turning, with a ring turning around it.
- Opening a conversation draws the Runtrol mark turning in the middle of the tab until the service's own
  screen arrives, so the wait is never a blank rectangle.
- A project added, renamed or removed in one window shows up in the others when they are next looked at.

## [0.1.29] - 2026-08-28

### Changed

- The symbol that shows while a conversation opens is now the Runtrol mark held still, with a coral light
  sweeping across it from left to right and repeating, instead of the mark spinning. It draws on a cleared
  pane, so there is no tile behind it.
- The sidebar shows the build version in small type at the top, under the "Runtrol" header.

### Added

- Runtime writes one line to `native-deletions.log` in its home whenever it removes a conversation from a
  coding service's own store, naming the integration that asked, the conversation and the folder. Deleting was
  already reversible by hand; now it is also answerable.

## [0.1.28] - 2026-08-28

### Fixed

- The sidebar no longer prints "Update applies when the running conversations end" under its header. On a
  machine with several Runtime generations alive, and especially when this window sees none of them running, it
  read as coming from nowhere. The generation supervision still runs; it just no longer narrates itself here.

- Each project's colour band is visible again beside its heading and its conversations. The colour was written
  onto the element, which the sidebar page's security policy drops, so the band was laid out at full width and
  painted nothing. The page's own stylesheet paints it now.

## [0.1.27] - 2026-08-28

### Fixed

- The sidebar no longer prints "Update applies when the running conversations end" under its header. On a
  machine with several Runtime generations alive, and especially when this window sees none of them running, it
  read as coming from nowhere. The generation supervision still runs; it just no longer narrates itself here.

- The sidebar lists a project's conversations on its own again. A coding service that became usable after the
  window had already asked (its CLI updating in the background, or the Runtime still starting) was never asked
  again, so the panel showed every project with nothing under it, said nothing about why, and stayed that way
  until Refresh Conversations was run by hand.

## [0.1.26] - 2026-08-28

### Fixed

- The sidebar no longer prints "Update applies when the running conversations end" under its header. On a
  machine with several Runtime generations alive, and especially when this window sees none of them running, it
  read as coming from nowhere. The generation supervision still runs; it just no longer narrates itself here.

- The sidebar no longer shows "Runtrol Runtime is not installed" when a healthy Runtime from an earlier build
  is still running. A window that installed a new build but whose own build has not taken over yet now uses the
  Runtime that is actually serving, instead of waiting for its exact build and then declaring nothing installed.

### Fixed

- The sidebar no longer prints "Update applies when the running conversations end" under its header. On a
  machine with several Runtime generations alive, and especially when this window sees none of them running, it
  read as coming from nowhere. The generation supervision still runs; it just no longer narrates itself here.

- The Runtime no longer goes quiet under load: starting or closing a conversation used to wait behind another
  session's process spawn or a durable session write on the Runtime's single control thread, which on slow disks
  meant commands timing out while the daemon looked healthy. Spawns and session writes now run on worker threads.

- Right after an update, a command that briefly could not reach the Runtime could start a second daemon that
  silently took over the socket and answered nobody. A daemon now refuses to bind an address that still answers.

- Thirty conversations starting at once no longer each probe the same coding CLI; the first probe is shared.

## [0.1.25] - 2026-08-27

### Fixed

- The sidebar no longer prints "Update applies when the running conversations end" under its header. On a
  machine with several Runtime generations alive, and especially when this window sees none of them running, it
  read as coming from nowhere. The generation supervision still runs; it just no longer narrates itself here.

- While several Runtime generations coexist right after an update, the locator file is rewritten often, and a
  security check that landed mid-rewrite made Studio pop "Runtrol Runtime is not installed" although the daemon
  was healthy. The check now retries through that instant; a real security verdict still refuses.

## [0.1.24] - 2026-08-27

### Changed

- The sidebar is one page Studio draws itself, with three zones that have visible edges: projects, conversations
  outside every project, and usage. The title bar keeps `Add Project` and `New Conversation`; every other action
  appears on the row it belongs to when the row is hovered, or behind the vertical dots at the top of the page.
  There is no second view and no second "Runtrol" header any more.

- Each project's colour now marks its own conversation rows and the terminal tabs those conversations open in, so
  the tab and the row say the same project at a glance. Projects reorder by drag.

- A conversation named by its first prompt (Codex does this when a thread has no name) wraps to two lines instead
  of being cut short.

- Hovering is calmer: the usage panel is the one detail surface (the browser tooltip that floated over it is
  gone), a conversation row keeps a tooltip only for the reason an open would be refused, and inside the panel
  only the window that is actually limiting you keeps its sentence; the others are a bar and a number.

### Changed

- The symbol that shows while a conversation opens is now the Runtrol mark held still, with a coral light
  sweeping across it from left to right and repeating, instead of the mark spinning. It draws on a cleared
  pane, so there is no tile behind it.
- The sidebar shows the build version in small type at the top, under the "Runtrol" header.

### Added

- Every conversation row shows what its provider process holds in memory right now (`412 MB`), read from the
  Runtime every five seconds. The public Runtime contract carries it as `memoryBytes` on session and terminal
  descriptors, measured from the operating system and never estimated.

### Fixed

- The sidebar no longer prints "Update applies when the running conversations end" under its header. On a
  machine with several Runtime generations alive, and especially when this window sees none of them running, it
  read as coming from nowhere. The generation supervision still runs; it just no longer narrates itself here.

- Opening a conversation right after a Runtime update no longer fails with "the native catalogue observation
  expired": Studio reads the provider's catalogue again and opens with the proof it hands back now.

- Clicking a conversation row now opens its terminal tab in a window that has never selected a conversation
  before. The click used to fall through to "open the selected conversation" and do nothing on a fresh window.

- A conversation under a project heading now opens even when the window itself is open on a different folder:
  adding a project also asks for its folder, so the open is no longer refused with `rootDenied`.

- After a Runtime update, native conversations no longer stay refused with `legacyGenerationBusy` while a
  pre-update daemon is still winding down. A predecessor that cannot prove the claims handoff (it predates the
  protocol) stops blocking after a few seconds instead of for as long as it lives; one that proves it keeps the
  full cross-generation double-open protection.

## [0.1.23] - 2026-08-27

### Changed

- The symbol that shows while a conversation opens is now the Runtrol mark held still, with a coral light
  sweeping across it from left to right and repeating, instead of the mark spinning. It draws on a cleared
  pane, so there is no tile behind it.
- The sidebar shows the build version in small type at the top, under the "Runtrol" header.

### Added

- Applications can now open, list, watch, attach, control, resize, write, detach, and stop provider terminal
  sessions through the public Runtime protocol. Rust, TypeScript, and Python clients expose the same typed
  terminal contract. A reconnect is pinned to the Runtime generation that owns the terminal and never retries
  uncertain input or redirects it to another process.

- The public Python package is distributed as `runtrol-runtime-client` and imported as `runtrol_runtime`. It
  provides synchronous and asynchronous APIs, generated protocol types, typed Runtime exceptions, and one
  CPython 3.11 stable-ABI wheel for each of the six native targets. Release publication refuses source
  distributions and consumes every built wheel outside the repository before publishing.

- Runtime integrations can be reviewed, narrowed, listed, and revoked without Studio through interactive
  `runtrol integrations` commands. Exact local confirmation requests use `runtrol requests review`. Piped
  authority changes and command-line approval shortcuts are refused.

- Studio now uses one native `Runtrol` sidebar instead of separate Projects, Conversations, and Agent Usage
  view headers. Projects contain their conversations, projectless conversations remain top-level rows, and
  under the list a pinned usage strip draws every installed service as its icon inside a ring gauge with the
  seven-day percentage, so the account's position stays in view however long the list grows and the strip reads
  the same with three services or ten. Projects open by default so no conversation hides behind a chevron.
  Hovering, focusing, or pressing a chip opens that service's windows as thin bars (five-hour, weekly, and
  per-model), with plan, reset, report age, and any blocking limit.

- Studio activates faster. When the Runtime is already running, Studio reads its control endpoint from the
  Runtime locator it has just verified instead of starting a second Core process to ask, and it approves its
  own Runtime enrollment at once instead of waiting a quarter second for someone else to.

- Runtime now holds one atomic native-conversation claim across structured sessions, hosted terminals, and
  draining generations. Typed ownership failures prevent a second process or a newer Runtime generation from
  silently taking over a live native conversation.

- A row with no bar names which of four things is true, because they need four different responses: nobody is
  signed in, the service publishes no usage at all, the service was asked and the answer could not be read, or
  the check has not finished. Only the third is Runtrol's to retry, and the previous wording sent people to
  sign in when they already were.

- Agent Usage now says where each account stands before any turn runs. A service that publishes its own
  status (Claude Code's `auth status`, Codex's account methods) shows whether it is signed in and the plan it
  names in its own word (`max plan via claude.ai`); Codex's limit windows are read on request and drawn as bars
  without waiting for a turn. A signed-out service reads "Not signed in · Sign in" and pressing it types that
  service's own sign-in command into a terminal. A service that publishes no such surface is named as that,
  never shown as "Ready".

- Agent Usage is live. The Core pushes every account's position the moment a turn or a status read moves it
  (`providers/usageChanged` on the provider subscription; the phone reads the same lines on its session index
  watch), and asks each service again within seconds of a conversation opening or a turn ending instead of on
  a ten-minute clock. Codex now also shows today's tokens from its own `account/usage/read` beside its limit
  window. The phone app draws the same icon-plus-progress strip above its sessions, kept current by push.

- A conversation opens as the coding service's own terminal interface, in an editor-area tab. The Core hosts
  the CLI on a pseudo terminal it owns, answers the questions the CLI asks its terminal at start, keeps the
  screen for viewers that attach later, and turns the mouse into keys the same way for every service (a
  wheel notch scrolls, a click on a row selects it). Split, grid and full screen are VS Code's own. See
  `docs/terminalSurface.md`.

- The phone shows the same terminal: a conversation opens as the service's own screen, drawn by xterm.js
  (vendored, MIT), on the same hosted terminal the PC tab shows, with the same keyboard. Interrupt sends
  the terminal's own Ctrl+C. The phone's event list and composer are gone with it.

### Changed

- Runtrol ships the representative services only: Claude Code, Codex and Grok, with Gemini to follow once its
  CLI can be measured. Cline and OpenCode are no longer shipped as providers or offered in the catalogue. A
  provider is attached through its manifest's four declared surfaces (terminal, store, account, events) and an
  icon, and nothing else in the product changes when one is added.

- The Conversations panel no longer invents project headings. A heading exists because you added the folder
  (Add Project registers an existing folder; New Project Folder creates one first) or because this window is
  open on it. Adding a folder lists every conversation the services report inside it at once. Added projects
  can be pinned to the top, renamed, and removed; removing takes the heading off the list and never touches
  the folder. Conversations in a folder nobody added stay plain top-level rows, never indented.

- The chat page of ours is gone: no composer, no chips, no bottom-panel or side-bar chat places, no grid of
  pages. The conversation is the service's own terminal in an editor tab (above), and everything those
  chips did (model, mode, effort, project, service) is the service's own command in that terminal. New
  Conversation now asks only which service, and opens that service's terminal in the project.

### Removed

- Missions and everything under them are gone: the Missions view, its twenty five commands, Auto Flight,
  scheduling, landing review, the Receipt ledger, the Gate registry, Fleet Compare, and project Capability
  approval. The sidebar is the conversations list and the usage strip, and nothing else. A phone paired with
  Mission permissions keeps working; it simply no longer holds those, and no update is needed.

### Fixed

- The sidebar no longer prints "Update applies when the running conversations end" under its header. On a
  machine with several Runtime generations alive, and especially when this window sees none of them running, it
  read as coming from nowhere. The generation supervision still runs; it just no longer narrates itself here.

- Deleting a conversation now removes the row the moment it is clicked; the service's own deletion and its
  answer follow behind, and a refusal puts the row back with the reason. Before, every deletion re-read the
  service's whole store before the row left.
- A deleted conversation no longer lingers as a nameless row. Runtrol's own pointer to it is forgotten with
  the deletion, and a pointer left behind by an earlier version can now be deleted from its row like any
  other conversation.
- Installing a Runtrol update while its Core was running failed with "EPERM: operation not permitted, rename
  ... runtrol.exe" on Windows. Each build's Core now lives under its own content-named file and no file is
  ever written over.
- A Runtrol update no longer waits for the machine to go idle, and never again sits unapplied for days behind a
  Core that kept refusing to stop. The new Core starts beside the running one as its own generation, takes over
  the moment the old one hands over its store (milliseconds, whatever the agents are doing), and the old Core
  finishes only the turns already running and then exits by itself. Nothing is killed and nothing is asked.
- "Runtime reconnect deadline expired" after an update is gone with it: there is no moment any more where the
  old Core is down and the new one is not yet up.
- `runtrol status` lists every Core generation serving this machine: build, process, running turns, whether it is
  draining, and whether it still answers. The "Restart the Runtrol Core" button and the `retire` command are
  removed; a Core built before generations is drained once by the first newer Core to start and needs no button.
- A machine reboot no longer leaves a ghost generation behind. The first Core started after power returns
  binds the very endpoint its dead predecessor's entry names, so that entry answered the liveness probe and
  `runtrol status` showed the same build twice. A generation now clears every entry left on the endpoint it
  just bound: the endpoint is exclusive, so whoever named it before is gone.

### Changed

- The runtrol mark is now two-tone: two arms coral (`#F56565`) and two arms in the ink of the surface, white on
  a dark theme and graphite on a light one, so the mark no longer reads as a single orange blob. The accent
  colour in the sidebar, the phone app, and the public site follows the same coral. The Marketplace icon,
  favicon, and social cards are regenerated from the same geometry.
- The public site opens with an animated Runtrol Studio window instead of a static panel: the sidebar tree
  fills in, two conversations open as editor tabs, the running agent's icon spins, usage ticks, an approval
  asks for the user, and the phone toast arrives. Icons are Lucide; the header carries the GitHub, support,
  YouTube, and Threads channels.
- Every service now carries its vendor's current vector mark: Grok shows the mark xAI adopted in February 2025
  (the sidebar carried the retired 2023 slashed circle as a black tile), and the OpenAI and Cline marks are
  vectors rather than bitmaps, so all five stay sharp at any size and follow the editor theme.
- Opening a window no longer hashes the 15 MB Core twice before the sidebar can draw. The Core is recognised
  by its file identity from the previous activation, which saves about 120 ms of every start on this
  machine; a changed file is hashed again.
- Menus no longer say "Runtrol:" in front of every action. Inside Runtrol's own views the context is already
  known, so a row now reads "Rename Conversation" rather than "Runtrol: Rename Conversation". The Command
  Palette keeps the prefix, where it is what finds the commands among every extension's.
- The composer's context row no longer labels its chips. The project chip shows the project's name, the branch
  chip the branch, and the service chip the service's own mark beside its name, instead of "Project:",
  "Branch:" and "Agent:" in front of each.
- Model and reasoning effort are now one control, the way the ChatGPT and Claude composers do it: one chip
  reading "model · effort", and one menu with the models first and the current model's efforts under their own
  caption. Choosing a model no longer opens a second popover asking for the effort; the current effort is kept
  when the new model reports it. The menu marks what is answering right now and hangs from the chip itself
  instead of floating over the conversation.
- Choosing a coding service now shows each service's own mark beside its name, with a check on the one this
  conversation uses.
- The Runtrol update no longer waits for a fully idle machine. A replaced Core now takes over as soon as no
  agent is mid-turn: idle sessions close with the old Core and reopen from their own saved state, exactly as
  the manual restart always did. On a machine where agents run around the clock, the previous rule meant an
  update that never arrived, which is why new sidebar features could stay invisible for days.

## [0.1.22] - 2026-08-25

### Changed

- The symbol that shows while a conversation opens is now the Runtrol mark held still, with a coral light
  sweeping across it from left to right and repeating, instead of the mark spinning. It draws on a cleared
  pane, so there is no tile behind it.
- The sidebar shows the build version in small type at the top, under the "Runtrol" header.

### Added

- Conversations can now be pinned with an inline pin, and unpinned the same way. A pinned conversation leads the
  whole panel, above the project headings, and is drawn there instead of under its project rather than twice. The
  choice is remembered per machine and only reorders the list; it never touches the conversation itself.
- Claude Code conversations can now be deleted from the sidebar. Claude publishes no delete command, so instead
  of erasing anything Runtrol moves the conversation out of Claude's own store into a `runtrol-deleted` folder
  beside it: it leaves the sidebar and Claude's resume list at once, yet can be carried back by hand. The action
  is offered only at the machine, never from a paired phone.
- Agent Usage is now one line per service: the service icon and its usage, nothing else. A service that reports
  how much of a window it has used gets a real bar per window with the same figure beside it; one that reports
  only a running spend, or only when its window resets, shows that instead. No service ever gets an empty bar for
  a number it did not send. The service name and status move to the hover, so the strip stays a mark and a
  reading.
- Code an agent writes back is now coloured in the conversation, the way an editor colours it: comments,
  strings, numbers, keywords, types and called names, following the light or dark theme. A language Runtrol has
  no grammar for still gets its strings and comments.
- The conversation now reads nested lists, tables, and quoted lines the way they were written. A list indented
  under an entry stays under it, a table keeps its columns, and a quoted line is set off instead of showing its
  markers.

### Fixed

- The sidebar no longer prints "Update applies when the running conversations end" under its header. On a
  machine with several Runtime generations alive, and especially when this window sees none of them running, it
  read as coming from nowhere. The generation supervision still runs; it just no longer narrates itself here.

- When Agent Usage cannot refresh, it now says why instead of only that it failed.
- Deleting a conversation from its row now says what was deleted and whose list it left, instead of the row
  simply disappearing, which looked the same as a misclick.

### Changed

- Every service now carries its own product mark: Claude Code, Codex, Cline, OpenCode, and Grok, taken from
  each vendor's own site rather than from the editor's icon font. Claude Code and OpenCode use vector marks, so
  they stay sharp at any size, and the OpenAI mark, which is white, flips to the dark badge the vendor itself
  uses on a light background instead of disappearing into it.
- Renaming a conversation is now instant and local. It no longer reopens a stored conversation and waits for its
  CLI in order to change a name; the name Runtrol shows is Runtrol's own, is remembered per machine, and never
  rewrites anything the coding service owns. Clearing the name restores the service's own title.
- A conversation named from its first prompt now passes over a leading slash command (such as `/model` or
  `/clear`), so its name is the first thing you actually asked rather than the control line you happened to type
  first. A conversation that only ever ran commands keeps its first command rather than going nameless.
- Rename is now an inline pencil on every conversation row, not only a buried right-click action, so a
  conversation can be given a name without hunting through a menu.

## [0.1.21] - 2026-08-24

### Changed

- Conversation tabs now use the active coding service's icon beside the actual conversation title. The Runtrol mark
  remains an extension-entry landmark and is no longer presented as the AI inside a chat.
- Conversation rows now expose one direct inline `X` only when the provider reports native deletion. It acts on that
  row without prior selection or confirmation; archive and close remain in the context menu, and unsupported services
  show no misleading delete action.
- Internal fallback names such as `Chat 8980` are no longer exposed. Provider titles and structured display previews
  remain authoritative, while a genuinely titleless provider record is labelled `Unnamed conversation`.
- Agent Usage now identifies services with their declared icons and full names instead of invented initials. Exact
  provider-reported percentages remain visible as progress bars.

## [0.1.20] - 2026-08-24

### Changed

- The symbol that shows while a conversation opens is now the Runtrol mark held still, with a coral light
  sweeping across it from left to right and repeating, instead of the mark spinning. It draws on a cleared
  pane, so there is no tile behind it.
- The sidebar shows the build version in small type at the top, under the "Runtrol" header.

### Added

- Conversation rows now expose provider-native archive beside delete when the installed CLI reports that capability.
  A supervised conversation is closed first, then the same confirmed action reaches the provider-owned record.
- Images pasted from the clipboard into the composer are attached to the next message with the same type, count, and
  size bounds as images chosen from disk.

### Changed

- The folder open in this VS Code window is now the first project heading and is available without project
  registration. It expands as soon as it has conversations. Other projects and conversations retain provider
  recency order.
- Conversation rows now show only the coding-service icon and the actual conversation title. The icon itself spins
  while that conversation is working; elapsed time and textual state labels no longer occupy the row. Incomplete
  history diagnostics also moved out of the permanent list header.
- One-off provider working directories no longer become project headings. Their conversations remain visible as
  ordinary rows, preventing temporary task and eye-test folder names from filling the sidebar.
- The real-window eye pass now requires a provider-owned cleanup surface and deletes every conversation it creates,
  preventing visual validation from adding test history to the operator's coding CLI.
- Conversation tabs use the actual conversation title without a `Runtrol` prefix. The extension containers are named
  simply `Runtrol`, live model and effort controls remain visible before the provider announces current values, and
  the send button now uses the compact VS Code foreground treatment.
- The fixed sidebar area is named `Agent Usage`. Every provider-reported numeric account window now has a bounded
  progress bar with its exact percentage and reset time, while a missing percentage remains `Ready` instead of
  implying zero usage. Compact name marks distinguish similarly named coding services instead of showing the same
  one-letter mark for both.
- Empty chats use a neutral greeting instead of repeating the project or product name. The project stays visible in
  the composer context as an explicitly labelled `Project`, beside labelled `Branch` and `Agent` targets. The message
  field names its selected coding service, and every generic panel, secondary-sidebar, and document title is `Chat`.

## [0.1.19] - 2026-08-24

### Changed

- Switching to an already supervised conversation now brings its tab and live event stream to the front before
  scrolling the sidebar selection. Webview readiness wakes the switch immediately instead of waiting for a polling
  interval, keeping the Windows release journey at 71.2 ms p95 without hiding the selected sidebar row.

## [0.1.18] - 2026-08-24

### Changed

- Switching between already supervised chats no longer pauses and restarts coding-service discovery, keeping the hot
  path focused on the selected conversation while background inventory work continues independently.
- Startup now allocates preparation lanes only for coding services that are actually being prepared. Hot ACP readers
  use a smaller fixed read buffer, and reconnect rings grow with real events instead of reserving their full ceiling,
  reducing fixed memory without weakening service serialization, line acceptance, or replay bounds.

## [0.1.17] - 2026-08-24

### Changed

- The symbol that shows while a conversation opens is now the Runtrol mark held still, with a coral light
  sweeping across it from left to right and repeating, instead of the mark spinning. It draws on a cleared
  pane, so there is no tile behind it.
- The sidebar shows the build version in small type at the top, under the "Runtrol" header.

### Added

- The fixed sidebar service area now exposes an `Add coding services` row backed by a generated snapshot of the
  official ACP Registry. Thirty safe local adapters, including GLM Agent, Qwen Code, and Gemini CLI, are available
  without provider branches in Studio or Core. Runtrol auto-discovers an already installed executable; an explicit
  service selection only places its exact install line in a terminal and never downloads or runs it automatically.
- Provider inventory now reuses one structural snapshot while PATH directories, the probe cache, and resolved
  executable identities remain unchanged. Provider-related requests return against the current complete snapshot
  immediately while one background task restamps the filesystem and publishes any change. A real probe write
  invalidates the snapshot immediately, keeping the larger service catalogue off refresh, start, and resume paths.

### Fixed

- The sidebar no longer prints "Update applies when the running conversations end" under its header. On a
  machine with several Runtime generations alive, and especially when this window sees none of them running, it
  read as coming from nowhere. The generation supervision still runs; it just no longer narrates itself here.

- Startup preparation now meets only coding services whose executable is installed. Missing entries from the service
  catalogue remain discoverable in the sidebar without creating background probe tasks or extending idle memory use.
- Clicking another saved chat in the same project now cools an idle provider process and switches immediately while
  preserving both conversations. A response that is genuinely still running gets only the explicit choices to stop
  and switch or keep both working, replacing the internal writer-overlap warning.
- Provider titles and list previews remain the visible conversation names. The final no-title fallback is now a
  compact unique `Chat` handle, and the rename action on a provider-owned saved row now stores the chosen name and
  returns the row to a cold state instead of failing because it was not already supervised.

## [0.1.16] - 2026-08-24

### Changed

- Rapid conversation switching no longer redraws the CLI status and usage tree when its visible and actionable state
  has not changed, keeping the fixed sidebar summary current without adding work to the switching path.

## [0.1.15] - 2026-08-23

### Fixed

- The sidebar no longer prints "Update applies when the running conversations end" under its header. On a
  machine with several Runtime generations alive, and especially when this window sees none of them running, it
  read as coming from nowhere. The generation supervision still runs; it just no longer narrates itself here.

- The fixed sidebar area now shows every installed coding-agent CLI's operational state and usage together. A service
  being checked remains visible, an unavailable service exposes its own fix action there, a failed refresh admits that
  the retained report is old, and the Conversations tree contains only projects and actual conversations.
- The Conversations title bar now keeps only New Conversation, Create Project, and Switch Conversation visible, so
  the title and list retain their reading width while less frequent actions remain in the overflow and Command Palette.

## [0.1.14] - 2026-08-23

### Fixed

- The sidebar no longer prints "Update applies when the running conversations end" under its header. On a
  machine with several Runtime generations alive, and especially when this window sees none of them running, it
  read as coming from nowhere. The generation supervision still runs; it just no longer narrates itself here.

- The machine-wide Conversations view no longer carries the current folder as a title qualifier. Every project remains
  a top-level sibling, and incomplete provider history is identified by service directly above the list instead of
  hiding that fact behind an information action.
- Every conversation row now contains an explicit state and time, including `Cannot reopen`, live `now`, and stopped
  `time unknown` states. A broken installed service exposes its provider-owned fix action directly from the row, while
  empty states distinguish a usable CLI, an executable still being checked, and a machine with no usable CLI.
- Screen readers now receive the provider represented by each conversation icon and the complete CLI usage row without
  duplicate state announcements. Composer choice lists expose expansion and active-option state, and Escape restores
  focus to the chip that opened a menu.

## [0.1.13] - 2026-08-23

### Fixed

- The sidebar no longer prints "Update applies when the running conversations end" under its header. On a
  machine with several Runtime generations alive, and especially when this window sees none of them running, it
  read as coming from nowhere. The generation supervision still runs; it just no longer narrates itself here.

- The Conversations sidebar now keeps every conversation-bearing folder as a top-level project sibling and omits
  an empty open folder, so another repository can no longer appear nested beneath the current workspace.
- Codex conversations without an explicit name now use the provider's own list preview as their visible title instead
  of repeating the project folder and an identifier. Conversation rows contain only the provider icon, title, exact
  running state, and time.
- CLI Usage is expanded at the bottom of the Runtrol sidebar and lists every connected usable CLI immediately, including
  an explicit `No report yet` state before a provider has supplied a usage gauge.

## [0.1.12] - 2026-08-23

### Changed

- Switching among active conversations now retires the previous watch without waiting for another service event.
  A hosted Extension Host gate holds selection changes below 175 ms p95, with the release candidate measuring 86.3 ms.
- Runtime validation metadata is projected to the keywords the client executes while the complete public schema remains
  packaged. The Studio bundle is 342.4 KiB, down from 388.0 KiB, with the same validation coverage.
- Eight idle hot conversations now have a live whole-daemon memory contract. The Windows release journey measured
  18.9 MiB at baseline, 22.9 MiB at peak, and 2.1 MiB of residual memory after cleanup.

## [0.1.11] - 2026-08-23

### Fixed

- The sidebar no longer prints "Update applies when the running conversations end" under its header. On a
  machine with several Runtime generations alive, and especially when this window sees none of them running, it
  read as coming from nowhere. The generation supervision still runs; it just no longer narrates itself here.

- Conversation rows and open tabs now adopt each coding service's own title when a native identity appears or a
  turn finishes. Provider catalogue refreshes are coalesced per service, operator names remain primary, and runtrol
  still never reads conversation content to invent a title.

## [0.1.10] - 2026-08-22

### Changed

- The symbol that shows while a conversation opens is now the Runtrol mark held still, with a coral light
  sweeping across it from left to right and repeating, instead of the mark spinning. It draws on a cleared
  pane, so there is no tile behind it.
- The sidebar shows the build version in small type at the top, under the "Runtrol" header.

### Added

- **Mission Flight Signals** turn Auto Flight state changes into one exact phone destination without putting that
  destination in Web Push. Core retains at most 64 structural `person`, `stopped`, or `landing` signals, filters them
  against the current workspace root, Mission digest, session binding, and Mission state, and returns them only after
  the phone reconnects through its existing authenticated `mission.read` authority. Studio uses a durable idempotent
  outbox, revokes automatic input while a signal is uncertain, and retries the same UUID after restart. The push body
  remains empty, the phone stores only an opaque cursor, and no remote start, arm, retry, Gate, integration, or
  completion authority was added. Real Extension Host and narrow-screen PWA states were inspected directly.

- **Mission Auto Flight** moves a reviewed ordinary DAG through every later proven-safe wave after one PC-local arm.
  Up to eight exact Mission digests can be armed together. Runtime lifecycle events drive the flow with no polling;
  each automatic Send stores the exact session generation before provider input, and fixed Gate verification waits
  for that same session to complete a real turn. Person or quota waits and pause retain the arm. Authority drift,
  ambiguous delivery, missing sessions, recovery, failure, comparison, cancellation, and arrival at integration
  remove it. Receipt Landing and final integration stay explicit. A real installed CLI completed two dependent
  waves, two Gate checks, and two Receipts with zero operator Continue actions in a photographed Extension Host.

- **Review and Apply Mission Landing** turns ordinary Mission integration into a cross-project, Receipt-first review.
  Every sealed UTF-8 Artifact from one passing Mission opens together in one native VS Code changes editor against
  the current project. One explicit action rechecks the Mission, Receipt path, size and SHA-256 evidence, source bytes,
  project bytes, symbolic-link boundary, and dirty text, notebook or custom-editor tabs. Each replacement is prepared
  and verified in an exclusive same-directory file, then atomically renamed only after a final compare. A bounded
  transaction verifies rollback, while a cross-window project lease prevents two Studio writers. Core compares the
  tree both before and after fixed Gates, so a passing Gate that edits an Artifact cannot complete the Mission. Gate
  failure deliberately leaves reviewed Git changes visible for repair and retry. A lost completion response converges
  from refreshed Core authority and exact applied bytes without rewriting files. An older busy Core that does not yet
  publish Artifact evidence leaves Landing unavailable instead of crashing or trusting paths alone. A real Extension
  Host selected the public apply action, applied existing and new files across two Git projects, rejected pre-review
  Receipt mutation, project and Receipt drift, a dirty editor, a symbolic-link swap and a Gate mutation, then recovered
  and completed.

- **Continue Ready Missions** turns the Missions view into a bounded multi-project flight deck. One local modal lists
  the exact digest and currently safe action for up to eight reviewed ordinary Missions, then advances each through
  the existing Mission Momentum requests. Running work is handled before a new start; expired review, recovery,
  waiting-only, comparison, and integration boundaries stay out. One failure cannot stop an unrelated Mission, one
  provider choice can cover every operator-choice Task in the run, and all newly started native conversations are
  arranged once. A real Extension Host advanced two separate Git projects from `validated` to `running` and then to
  `integrating` through two Flight Deck actions.

- An ordinary reviewed Mission now advances one safe wave at a time from one **Continue Reviewed Mission** action.
  The same confirmed local action starts a validated Mission, seals exact Ready Tasks with their fixed Gates,
  prepares newly eligible workspaces and Runtime sessions, and sends unchanged reviewed instructions. It stops at
  provider work, person or quota waits, failures, retries, missing identities, comparisons, and integration. Granular
  recovery commands remain available. An uncertain provider delivery is persisted before transport and disables
  automatic verification even after an Extension Host restart. A real two-stage Mission completed both waves through
  an installed CLI in an isolated Extension Host.

- Phone notifications now open the first session that is actually waiting for the operator. The phone session list
  shows a bounded `Needs you` count and cycles through every person wait while keeping account-limit waits separate.
  A normal narrow-screen launch stays on the list instead of opening an arbitrary first session. The generic Web Push
  still carries no session identifier or conversation content, and the real installed-CLI approval gate proves that
  the focus appears while approval is pending and clears after the answer.

- A project heading can enable **Agent Tools** in one click. The installed coding-agent CLIs receive one
  provider-neutral MCP server through their own official registration commands, then can discover providers and
  models, list project sessions, start exclusive work, send unchanged instructions, read bounded events, and stop
  exact sessions through the public Runtime. Each project has its own root-bound, OS-protected identity; approvals,
  shared starts, deletion, provider secrets, transcript copies, and a Runtrol-owned agent loop remain outside the
  tool catalogue. Disabling the last project removes provider registrations, Runtime authority, and local
  credentials. An existing or externally replaced registration is never overwritten or removed: exact ownership is
  proved through each provider's official readback, and a failed first enable rolls its new Runtime authority back.
  The project badge, real installed CLI smoke, outside-root denial, and complete revocation have been verified with
  zero model turns.

- The sidebar now lists conversations from folders this window has never opened. Each coding
  service is asked about the whole machine in one question instead of once per approved folder,
  which is what those services were already willing to answer: measured against the installed
  CLIs, four of the five list every conversation they know without being given a folder, and each
  one they return says which folder it belongs to. A service that genuinely can only answer one
  folder at a time says so and is asked that way, unchanged.

- When the list is not everything, the sidebar says why, in the service's own words, rather than
  letting an incomplete list read as complete. An empty project heading now says "nothing listed"
  instead of claiming the folder holds no conversations, which is something the heading cannot know.

- The sidebar says what each running conversation is doing, without opening it: the tool the coding
  service says is running ("Bash", "Run npm test", "Edit"), in the service's own word, on the row while it
  runs, gone when it ends. A service that asks the operator to sign in is said on its rows as "Sign in
  needed", and the row's key places that service's own sign-in command in the terminal (never run).

- A conversation that stopped for a question can be answered from its row: the "Needs you" row carries
  the service's own first allow and decline options as inline buttons, and "Answer the Question..." lists
  every option the service offered, in its words. The tab need not be opened; the row changes when the
  answer lands.

- Moving the window between projects is a keyboard round trip: "Switch Window to Project..."
  (`Ctrl+K Ctrl+Shift+P`) lists every project the sidebar knows and moves this window there, and
  `Ctrl+K Ctrl+B` brings it back to the project it was on before, in the same window. The heading's
  button and a conversation's project chip remember the same way.

- "Needs you" now actually lights up when a coding service asks a question. The sidebar's state, its badge,
  the status bar count and "Open Next Waiting" all read the Runtime's "waiting on a person" fact, and the
  Runtime derived that fact only from a turn event no installed service sends; the question itself now counts
  as the turn waiting on a person, and its withdrawal (or the next event after the answer) as the wait ending.
  The sidebar also repaints when only that fact changes; it used to call two snapshots equal when nothing
  but "waiting on a person" differed.

- "Start here anyway" and "Resume anyway" (a second agent on a folder another agent is writing in), and a
  second conversation with no project, now actually open. They asked the public Runtime for shared
  working-tree access, which it refused outright as a local operator action, and the refusal was shown as
  a sign-in problem. Shared opens now take the same road as the Runtime's other locally decided mutations:
  the public Runtime queues the exact request and answers `presenceRequired`, Runtrol Studio confirms the
  choice its person just made at the machine and sends the unchanged request again, and the conversation
  lives under the public Runtime as every other does. Any other integration's shared open shows up in
  "Review Runtime requests" with the service and the folder, for the operator to allow.

- One prompt to several services: the service chip of a new chat offers "Also ask <service>"; the first
  message then starts a conversation per chosen service, each in its own tab, and the grid lines them up.
  Several agents on one folder is the operator's explicit choice there, so they share the folder (the
  Runtime's working-tree contract); a service that cannot be asked says why on the first tab.

- Choosing a model, a reasoning effort, an access mode, a coding service or a project now happens in
  the composer, in a popover hanging from the chip that was clicked, the way every chat composer
  offers its menus, instead of a picker at the top of the window. Arrow keys move, Enter chooses,
  Escape dismisses, a click elsewhere closes. The slash-command menu was already there; now every
  choice the composer offers is answered where it was asked. The command palette keeps the same
  choices for a command invoked from the palette.

- The sidebar's "not every chat is listed" notice is one line; the services' own reasons moved
  behind an (i) in the Conversations title ("Why Are Some Chats Not Listed?"). Printed in full they
  had become a nine-line wall above the first conversation.

- A conversation can live in any of the window's own places, not only an editor tab: the bottom panel
  beside the terminals, or the secondary side bar beside the code. Right-click a conversation and
  choose "Open Conversation in Panel", "in Side Bar" or "as Tab"; the conversation moves, stays one
  conversation, and the place remembers it across a reload. VS Code 1.106 or newer is required from
  this version on, which is the version that lets an extension place a view in the secondary side bar.

- One command spreads the open conversation tabs over a grid of editor groups: "Arrange
  Conversations in a Grid" (`Ctrl+K Ctrl+G`), as square as the count allows (two by two, three by
  three), VS Code drawing and sizing the groups. Nine is the editor's column bound, and the command
  says when tabs were left where they were.

- A change a coding service declares opens in VS Code's own diff editor. The tab names the change
  with an "Open diff" button instead of drawing a coloured patch; before and after open as read-only
  documents side by side (a unified patch as a read-only `.diff` document). Nothing is written to disk.

- Reopening a stored Claude Code conversation shows the conversation, the way `claude --resume` draws
  it in a terminal: the last stretch of the stored exchange (operator messages, tool calls and their
  results, replies) appears in the tab before anything new, read from the CLI's own store and relayed
  through the same path as live frames. Claude Code's stream prints no history on resume; this reads
  back what the CLI itself stored, bounded to the most recent records, and keeps no copy. Codex threads
  already did this from their own resume answer, so reopened conversations now read alike on both.

- In a Claude Code conversation the operator's own messages now appear on the operator's side, as
  they do for Codex and the ACP services. Claude Code re-emits each message it was sent when asked
  with its own `--replay-user-messages` flag (measured on the installed CLI), and the driver asks
  whenever the CLI's parser confirms the flag; an older CLI without it simply shows replies only, as
  before. Nothing is echoed locally: the words shown are the ones the provider handed back.

- A conversation a coding service declines to open (measured: a Codex thread another Codex window is
  writing to) is reported as refused, not as a feature the service "does not offer". The feature is
  there; the service said no to this one request, and the sentence now says that.

- The sidebar's "not every chat is listed" notice says each reason once. A service that lists in
  pages repeated its omission sentence on every page, and the first page of a store carried two
  sentences joined together, so the notice had grown into a twelve-line wall above the list.

- Claude Code's stored conversations are listed, with the titles Claude Code gave them, under the
  folders they ran in. The CLI publishes no command for what it has stored, so the sidebar names them
  from the CLI's own store: the conversation's identity, its folder, its title and its last write,
  read as they are. No message is read, nothing is copied, and the store stays the only home of the
  conversation. Measured on the machine that built this: 265 conversations across 49 folders named
  in 1.4 s. The previous listing showed only the Claude Code processes running at that moment.

- The sidebar shows every project on this machine, the way the Codex and Claude sidebars do: every
  folder a coding service holds conversations in is a heading, with its conversations beneath it.
  This window's folder comes first, the operator's created projects keep their rename and remove,
  and any other folder can be made a project from its heading in one click. A heading never
  appears empty: a discovered folder exists exactly as long as a conversation names it. Two
  folders with the same name are told apart by their parent folder.

- New chat opens as a draft tab: a greeting, the composer, and chips for the project, the branch,
  the coding service, the model, the reasoning effort and the access mode. Each chip is its own
  picker, nothing runs until the first message, and that message starts the conversation in the
  same tab with exactly those choices. The + on a project heading opens the same draft with the
  folder already answered. A draft survives a window reload with its choices.

- A conversation can be started with no project at all. It runs in the extension's own scratch
  folder, sits beneath the project headings as a plain row, never repeats that folder's name, and
  never asks the writer-collision question.

- The composer is the card every chat app has converged on: where the conversation runs across the
  top, the message in the middle, the controls along the bottom with send at the right edge. The +
  attaches images to the next message (sent through the public Runtime's block input, never stored),
  and the branch of the conversation's folder is read off its own repository.

- Every conversation tab's chips say where it runs and who answers. On a live conversation, the
  project chip offers the one explicit move to that folder as the window, and the service chip
  offers a new conversation in the same folder with another service.

- A conversation the coding service stores can be deleted from the sidebar, by the service itself:
  the row's trash button asks the provider through its own deletion surface (Codex `thread/delete`,
  Cline `history delete`), after a question that names the service and promises no undo. A service
  that publishes no such surface (Claude Code) says so in its own words instead of offering an act
  that could not work. Runtrol holds no copy and deletes nothing itself. The public Runtime carries
  this as `sessions/deleteNative` under `session.delete`, and both SDKs call it.

- A reopened Codex conversation now reads as the conversation it is: the CLI hands over its recent
  turns on resume and they are shown, most recent turns only, so a long thread does not arrive as one
  enormous frame.

### Fixed

- The sidebar no longer prints "Update applies when the running conversations end" under its header. On a
  machine with several Runtime generations alive, and especially when this window sees none of them running, it
  read as coming from nowhere. The generation supervision still runs; it just no longer narrates itself here.

- An ordinary chat sent to several services no longer chooses linked worktrees automatically. Before any provider
  starts, the operator explicitly chooses separate Core-owned worktrees or a shared current checkout, with the
  collision consequence stated in the same local confirmation. Runtrol performs only that choice.

- A conversation listed from the whole machine can actually be opened. The proof the listing
  handed out was bound to a folder scope that no approved folder could ever match, so every such
  row refused to open; the proof now opens on the same owner-only terms the listing was given.

- A Claude Code reply no longer appears twice. The streamed pieces and the assembled message are
  recognised as one message by the message's own name, which the real CLI puts on the opening
  fragment and on the whole and on nothing in between; the deltas in between are given that name
  as they pass, and the whole replaces what the pieces built.

- Codex conversations no longer read as fifty-six years old. Codex reports its timestamps in
  seconds; they were read as milliseconds.

- When a service's list leaves conversations out, the sidebar says why ("their folders no longer
  exist, or they repeat or overrun entries") instead of an internal filtering phrase, once per reason
  rather than once per page.

- Opening a stored Grok (or any ACP) conversation longer than a few turns no longer fails. The
  protocol replays the whole history before answering the load, and a fixed ceiling refused every
  conversation past sixteen updates; the replay now keeps its tail and says once how much older
  history the service still holds.

- Opening a long Codex thread no longer takes every other Codex conversation down with it. The whole
  thread used to arrive in one frame; one real thread exceeded the transport's line limit, which ended
  the shared connection. The resume is bounded to the most recent turns, and an oversized frame on a
  shared connection is refused once and skipped instead of ending the connection.

- When a provider refuses to reopen a conversation, the sidebar now reports the provider's own kind
  of refusal (not installed, not signed in, over quota, unsupported, unreadable) instead of "the
  mutation may have happened and cannot be repeated safely".

## [0.1.9] - 2026-08-20

### Fixed

- The sidebar no longer prints "Update applies when the running conversations end" under its header. On a
  machine with several Runtime generations alive, and especially when this window sees none of them running, it
  read as coming from nowhere. The generation supervision still runs; it just no longer narrates itself here.

- Updating the extension no longer breaks against the still-running older Core. 0.1.8 failed its
  first hello on every machine with a running daemon ("InitializeResult violates the selected
  public schema"), because new required fields had joined a finalized protocol revision. The hello
  is now eternally compatible in both directions: fields added after a revision was finalized are
  optional forever, unknown fields are ignored, and a corpus of every shipped hello is enforced in
  CI so no future release can regress this. A second gate downloads the Core binaries out of the
  last three published packages, runs each as a real daemon, and requires this build's client to
  complete a real handshake against it, so the same break cannot reach anyone again.

### Changed

- The symbol that shows while a conversation opens is now the Runtrol mark held still, with a coral light
  sweeping across it from left to right and repeating, instead of the mark spinning. It draws on a cleared
  pane, so there is no tile behind it.
- The sidebar shows the build version in small type at the top, under the "Runtrol" header.

### Added

- The Core now rolls itself forward. The daemon announces its own executable digest in the
  greeting; when the extension sees a daemon older than the Core it installed, it asks that daemon
  to retire (refused while any conversation still has a live process, retried once the machine is
  idle) and the next request starts the new build. A daemon too old to know "retire" gets a single
  explicit "Restart the Runtrol Core" button instead of silence. `runtrol retire` does the same
  from a terminal.

## [0.1.8] - 2026-08-20

### Changed

- The symbol that shows while a conversation opens is now the Runtrol mark held still, with a coral light
  sweeping across it from left to right and repeating, instead of the mark spinning. It draws on a cleared
  pane, so there is no tile behind it.
- The sidebar shows the build version in small type at the top, under the "Runtrol" header.

### Added

- Conversations now open like files: each one gets its own editor tab, so several conversations can be
  on screen at once, split side by side, and rearranged like any editor. Switching tabs switches the
  sidebar highlight, prompts and interrupts go to the conversation whose tab they were typed in, and a
  window reload restores every conversation tab to its own session instead of collapsing them into one.

## [0.1.7] - 2026-08-20

### Changed

- The symbol that shows while a conversation opens is now the Runtrol mark held still, with a coral light
  sweeping across it from left to right and repeating, instead of the mark spinning. It draws on a cleared
  pane, so there is no tile behind it.
- The sidebar shows the build version in small type at the top, under the "Runtrol" header.

### Added

- The sidebar now shows every conversation on this machine, and any of them opens right here. Runtrol-managed
  sessions from folders this window never opened appear below the headings, clicking one opens its
  conversation as an editor tab in this window (the agent keeps working in its own folder), and moving VS Code
  to that project is a separate explicit button on the project's heading, never a side effect of a click. The
  enrollment-root boundary that used to hide and refuse them remains only where it is security: the phone.

- The reasoning effort has its own chip under the composer, and clicking it switches only the effort while
  keeping the answering model. While any switch (model, mode, or effort) is in flight, the chip keeps the
  provider's confirmed value and shows the request as a suffix ("gpt-5 → gpt-5-mini (requested)"); the
  suffix disappears exactly when the provider confirms, which is also when the change actually applies.

- A session can be started in a chosen permission mode (for example plan) through the public Runtime:
  `sessions/start` takes an optional `permission`, validated against the same switchable-mode boundary as
  mid-session switching, so modes that remove safety prompts stay unreachable for every caller. Resuming a
  session (`sessions/resume`) now also takes the optional model and reasoning effort the drivers already
  honored. "New Conversation with Service, Model and Effort..." now offers that starting mode as its last
  question, only for services that declare a switchable set.

- Removing a project shows an Undo toast instead of asking for confirmation up front: the misclick is
  covered without making every deliberate removal answer a question first.

- Images can travel with a prompt. The public Runtime gained `sessions/submitBlocks` (typed text and
  image blocks under the same lease and idempotency discipline as plain input; Runtime transports the
  frame and stores no attachment), and every driver speaks its own CLI's measured image surface: Claude
  Code's base64 content block, codex's data-URL input, and the ACP image block gated on the agent's own
  `promptCapabilities.image` announcement (cline announces true, grok false, and a service that cannot
  take an image refuses loudly instead of dropping a piece of the prompt).

- "Try One Instruction Several Ways..." lost its friction: the Gate question is now a pick from the
  registered Gates (with registering a new one chained in), the base ref is prefilled from the
  repository's own HEAD, and the writable directories are remembered per project instead of always
  resetting to `src`. The reviewed, unsaved Mission document contract is unchanged: registration is once
  per project, and after it a fan-out is one command.

- Opening the model picker twice no longer asks the provider twice. Model catalogues are remembered for
  five minutes against the exact installed binary (a login change still surfaces quickly, and a replaced
  binary is always re-asked), and starting a session with a chosen model reuses the same answer instead of
  re-spawning the provider the picker just asked.

- `providers/getCapabilities` now reports whether the model and the reasoning effort can be switched
  mid-session, per provider. The effort chip reads it before trying: a service that cannot switch (Claude
  Code refuses a mid-session effort change; the effort is an open-time flag there) says "from the next
  conversation" up front instead of failing after the pick.

- Codex conversations now show which model is answering. The open answer's own `model` field becomes the
  attach-time announcement, and the CLI's `thread/settings/updated` notification keeps the chip current
  mid-session, so the model chip no longer freezes on the requested value for this service. Measured from
  the CLI's own generated schema; the same measurement showed codex announces no slash-command catalogue,
  which is recorded in its manifest.

- Typing @ at the start of a word in the composer opens a workspace file picker, and the chosen path is
  inserted as plain text where the @ was. Nothing is interpreted: what an @path means stays the coding
  service's own business, exactly like a slash command's argument.

- Typing no longer waits for the agent. While a turn runs, the composer stays open and Enter queues the
  message onto a strip above it (up to eight, each with its own cancel); when the turn ends, queued
  messages go out one per turn, in order. The queue lives only in the open conversation page, exactly like
  unsent composer text: switching sessions or hiding the tab discards it, and nothing is ever written down.

- The deliberate start flow is findable now: "New Conversation with Service, Model and Effort..." sits on
  every project heading's right-click menu and as a second button on the empty conversation page, not only
  in the view's overflow menu. Each project also remembers its last explicit choice and leads every picker
  with it ("Last used here"), so the second configured start in the same project is three Enters. Nothing
  is skipped: the quick path still sends nothing, and the installed CLI's own settings stay the only
  automatic authority.

- A coding service that cannot start now offers its own remedies from its sidebar row: the wrench on the
  "needs attention" row lists the service's own sign-in, install, and diagnose commands, and the chosen
  line is placed in your terminal unexecuted, exactly like the start-failure dialog. Nobody has to attempt
  a conversation just to be told how to fix the service.

- Replies now render the light markdown coding agents actually write: fenced code blocks with a copy
  button, inline code, headings, lists, bold, italic, and web links that open in the browser. Plain text
  is untouched and costs nothing; anything the small grammar does not recognize stays exactly as typed.

- An agent's plan is now shown as the checklist it sent, updating in place as steps complete, instead of
  the fixed line "Plan updated". A plan event without readable entries still falls back to that line.

- A change the service declared as a change is now coloured inside the tool panel: added and removed text
  in the editor's own diff colours, for the two shapes services actually send (protocol diff blocks and
  unified diff text). Tool arguments that merely look like patches stay as plain detail, exactly as before.

- A running turn can be stopped from the keyboard. Escape in the conversation view stops the turn while the
  stop button is offered, and `Ctrl+K Ctrl+I` (`Cmd+K Cmd+I` on macOS) does the same from anywhere in the
  window, joining the existing conversation chords.

### Fixed

- The sidebar no longer prints "Update applies when the running conversations end" under its header. On a
  machine with several Runtime generations alive, and especially when this window sees none of them running, it
  read as coming from nowhere. The generation supervision still runs; it just no longer narrates itself here.

- Returning to a conversation no longer shows an empty page. Leaving the tab (or switching sessions) used to
  resume the event stream after everything already delivered, so the fresh page had nothing to paint until
  the agent spoke again. A reopened conversation now replays the daemon's bounded recent window into the new
  page; anything older than that window stays with the service, exactly as before.

- A message from the conversation page that no handler recognizes is now dropped instead of falling through
  to the interrupt path. No shipped page ever sent such a message, but the fallback meant one malformed
  message away from stopping a running agent.

## [0.1.6] - 2026-08-20

### Fixed

- The sidebar no longer prints "Update applies when the running conversations end" under its header. On a
  machine with several Runtime generations alive, and especially when this window sees none of them running, it
  read as coming from nowhere. The generation supervision still runs; it just no longer narrates itself here.

- The folders your window has open are conversation headings again. The 0.1.5 correction that stopped every
  folder from becoming a heading over-shot and flattened even the window's own project into a repeating list
  ("runtrol · ..." on every row). Opening a folder is your act exactly like creating a project: the open
  folder is now a collapsible heading with its conversations underneath, rows under any heading stop
  repeating the folder name, and folders you neither created nor opened stay plain rows as before.

- Opening a folder into a live window no longer reconnects everything to show that folder's conversations.
  Widening your own workspace grant now continues on the same authenticated connection (anything that removes
  or replaces authority still disconnects, exactly as before), conversation discovery reads the grant the
  daemon holds now instead of a stored snapshot, and resuming a cold conversation in the same run measured
  235 ms where the day started at ~2.4 s.

- The first meeting with each installed service got three times cheaper, and it now happens behind the boot
  instead of in front of you. A cold probe asked its questions one CLI start after another; they are asked
  together now, a probe that help already answered stops spawning control questions it will not use, and the
  daemon warms every usable service in the background (two at a time, so the warming never crowds out your
  own first request). Measured: resuming a cold conversation dropped from ~2.4 s to ~0.4 s, and a service
  asked a few seconds after startup answers from a warm cache in ~0.1 s.

## [0.1.5] - 2026-08-20

### Changed

- The symbol that shows while a conversation opens is now the Runtrol mark held still, with a coral light
  sweeping across it from left to right and repeating, instead of the mark spinning. It draws on a cleared
  pane, so there is no tile behind it.
- The sidebar shows the build version in small type at the top, under the "Runtrol" header.

### Added

- The model can now be switched in the middle of a conversation, from the conversation's own header. Click the
  agent chip, pick from what your session or your installed CLI actually offers, and the switch travels through
  each service's own surface: one CLI takes it immediately over its control channel, one applies it to the next
  message and every one after (its own documented behaviour), and one accepts it through its protocol. The
  header then shows the model the service says is answering, not the one that was merely requested, and a
  service that refuses says why in its own words. Reasoning effort rides along where the service accepts one
  mid-conversation; where it does not, Runtrol says so instead of silently dropping it.

- The permission mode can now be switched in the middle of a conversation, from its own chip in the
  conversation header, exactly like the model. The choices are what the service itself accepts: one CLI's
  control channel takes it immediately and announces the new mode itself, one carries it on the next message
  (its own documented surface), and protocol services that announce a mode set per session offer exactly that
  set. Modes that remove safety prompts entirely are deliberately not offered and are refused for every
  caller: turning questions off stays a deliberate act at the service's own surface, never a click in Runtrol.

- Conversations you started with Cline outside Runtrol now appear in the list. That CLI announces no session
  capabilities over the protocol its driver speaks, so Runtrol reported existing-conversation discovery as
  unsupported for it. The CLI lists them on its own command line, and Runtrol asks that command instead. Only the
  four things a row needs are read (identity, project, title, when it changed); the prompt text and the stored
  message path that arrive in the same record are never touched, and a test asserts that.
- OpenCode now reports its models. The protocol its driver speaks has no way to enumerate them either, but that CLI
  has a command of its own that does, so Runtrol asks it instead of carrying a list. Measured: the model picker went
  from "this CLI exposes no selectable catalogue" to seven identifiers that come from the installed CLI at the moment
  of asking. Cline reports none, because that CLI has no such command and inventing one would be worse.
- One instruction can now be tried several ways at once, each attempt in its own worktree. Point at the instruction
  file, say how many attempts and which Gate judges them, and Runtrol composes the Mission that runs them in
  parallel. It opens that document for you to read and save rather than saving it itself: a Mission is bound to the
  exact bytes of a reviewed instruction, and one generated behind your back would keep the machinery and lose the
  point. Attempts are capped at four, because each owns a worktree and an agent process.

### Changed

- Project headings in the conversations panel are now yours to create. The list used to invent a heading for
  every folder that had ever held a conversation, which on a real machine becomes a wall of folder names nobody
  asked for. Now a heading exists only for a project you created (the new-folder button in the panel's title
  bar; pick one folder or several). Conversations file under the project whose folder contains them, and every
  conversation you did not put anywhere stays a plain row beneath the headings, exactly as it was. Right-click a
  project to rename or remove it; removing only takes the heading away and touches no conversation.

- Runtrol is now licensed under AGPL-3.0-only instead of MIT, and the source stays public. Using it changes
  nothing for you: it supervises agent CLIs as separate processes and places no license obligation on your own
  work. What changes is that a modified Runtrol has to stay open, including one offered to others over a
  network. The three packages other programs link against (`runtrol-runtime-protocol`,
  `runtrol-runtime-client`, `@runtrol/runtime-client`) move from MIT to Apache-2.0 and stay permissive on
  purpose, so integrating against Runtrol is unaffected.

- Installing more coding services no longer delays the moment the window is usable. Each service's executable was
  resolved one after another, so every supported CLI added its own search-path walk to startup. They are resolved
  at the same time now. Measured on the real Extension Host with four installed services: the fastest run went
  from 1,244 ms to 992 ms and the ratchet's activation budget went from red to green.

### Fixed

- The sidebar no longer prints "Update applies when the running conversations end" under its header. On a
  machine with several Runtime generations alive, and especially when this window sees none of them running, it
  read as coming from nowhere. The generation supervision still runs; it just no longer narrates itself here.

- A cold start no longer probes your installed services single file. The first time Runtrol meets each CLI it
  asks the binary who it is (hundreds of milliseconds each), and one global lock made five such introductions
  queue behind each other. Each service now has its own preparation lane, so first meetings overlap: measured,
  five cold services answered a model listing in 8.7 seconds where full serialization costs 18, with the same
  guarantee kept that one service is never probed twice at once and the shared answer cache cannot lose
  entries to a concurrent write.

- A folder's conversations no longer queue behind every other installed CLI. One internal gate serialized all
  existing-conversation listings, so the folder you just opened waited for whichever CLI was slowest to be
  probed and listed. Measured in a live window with five services installed: the second folder's conversation
  arrived after 13~17 seconds before, 9.7 seconds on a cold start now, and about 1.2 seconds once the daemon
  has met its providers. Listings now run a few at a time instead of one at a time.

- Opening another folder into a window now brings that folder's saved conversations with it. The reconnect that
  follows a widened workspace cleared the discovered-conversation list and never restarted discovery, so the new
  folder's conversations stayed invisible until a manual refresh. Proven in a live window by the new harness
  scenario that opens a second folder and watches its conversation arrive.

### Security

- A paired phone now acts only inside the workspace roots you approved for it. Every session command a phone
  sends (speaking, renaming, watching, interrupting, closing, answering an approval) is verified against the
  same live roots that bound its listing, so a session identity learned before a root was revoked stops
  working the moment the root does. Mission reads are bounded the same way: a phone holding the mission-read
  permission sees only the Missions of its approved roots, and one outside them answers exactly like one that
  does not exist.

- A paired phone now sees exactly the sessions inside the workspace roots you approved for it, and nothing else.
  The session list and its live updates used to be one shared snapshot, so any phone holding the listing
  permission received every session's absolute folder path, name, and activity, including projects that phone was
  never granted. Each phone's view is now projected through the same three-part verification that gates starting
  a session in a root (the grant still held, the path still resolving to itself, the directory still being the
  same project), revoking a root shrinks the phone's live view immediately, and local storage warnings stay on
  the machine. What you see at the PC is unchanged.

## [0.1.4] - 2026-08-17

### Changed

- The symbol that shows while a conversation opens is now the Runtrol mark held still, with a coral light
  sweeping across it from left to right and repeating, instead of the mark spinning. It draws on a cleared
  pane, so there is no tile behind it.
- The sidebar shows the build version in small type at the top, under the "Runtrol" header.

### Added

- Conversations now say when they are waiting for you. A coding agent that stops mid-turn for an approval or a
  question reads `Needs you` in the list, carries its own glyph, and raises a count badge on the Runtrol icon, so a
  blocked agent is visible without opening anything. Waiting on an account limit is shown as its own state and never
  asks to be answered.
- One key takes you to whichever agent is waiting for you. Run six at once and you never have to work out which
  one stopped: `Ctrl+K Ctrl+W` opens the next waiting conversation, and pressing it again walks the rest. An agent
  waiting on an account limit is skipped, because nobody can answer that.
- While anything is waiting, the status bar says so with a count and a warning colour from anywhere in the window,
  and clicking it goes straight there.
- New Conversation, Switch Conversation, Show Open Conversation, and Open Next Waiting have keyboard chords
  (`Ctrl+K Ctrl+N`, `Ctrl+K Ctrl+A`, `Ctrl+K Ctrl+O`, `Ctrl+K Ctrl+W`; `Cmd` on macOS).
- Cline and OpenCode join the coding services Runtrol supervises. OpenCode reports its existing conversations through
  the official protocol, so they appear in the list without being started here first.
- Conversations are grouped by project once the list gets long enough that grouping helps. The project this window is
  open on comes first and arrives open, any project holding a conversation that is waiting for you is open too, and the
  rest stay closed so eight projects do not become one long scroll. Each heading says how many conversations it holds
  and how many of them want you. A short list, or a single project, stays flat: a heading there would cost a click and
  shorten nothing.
- Typing `/` shows the commands the coding service you are talking to actually offers, with its own descriptions. Arrow
  keys move, Enter fills it in, and nothing is sent until you send it. This is also how you change model mid
  conversation: `/model` is the CLI's own command, so it takes effect in the CLI's own state rather than in a second
  opinion Runtrol keeps on the side.
- A coding service that will not start now offers its own way out. Not signed in, not installed, or installed and
  broken are three different problems, and Runtrol offers the matching command from that CLI itself: `claude auth
  login`, `codex doctor`, `npm install --global cline`, and so on. The command is typed into your terminal and left
  there unrun, so you read it and decide. Runtrol never installs or authenticates anything on your behalf.

### Changed

- You can now see what an agent is doing to your project. Tool activity used to render as the fixed sentence
  "Tool call started", so an agent that read three files, edited one and ran the tests showed five identical lines.
  It now reads `Edit src/main.rs`, `Run cargo test...`, `Run cargo clippy · failed`, updating in place as each call
  progresses, with a coloured rail for running, done, and failed. Only the service's own classification and label
  are shown; raw input, raw output and diffs stay untouched, because those are the conversation.
- Runtrol Studio no longer asks you to approve Runtrol Studio. Opening the extension for the first time used to
  interrupt you with a permission dialog, a scope picker, a project picker, and a phrase to transcribe, all so the
  extension could talk to the Core it had just installed and started itself. It now proves it is the enrollment it
  created by signing for it. Every other product still goes through the full local review, and narrowing what an
  integration may reach is still a decision you make.
- Installing more coding services no longer makes Runtrol slower to open. Each service is asked what it can do one
  at a time in the background instead of all at once. Measured with four installed services, where two of them take
  about three seconds each to answer: a refresh went from over five seconds back to immediate.
- Approving a command no longer looks like refusing one. Refuse is the solid button, granting is outlined, granting
  permanently is outlined in the warning colour, and a high-risk request says so in words. The styling follows what
  each option does rather than the order the coding service sent them in.
- The composer says when an agent is waiting for you or waiting on an account limit, instead of calling both of
  those "working".
- The sidebar is now one list of conversations. Clicking a row opens it. Coding services are no longer folders to open
  first, the separate services view is gone, and a service you have not installed no longer takes up a row.
- A conversation Runtrol is supervising and the saved chat it came from are one row instead of two, and opening a saved
  chat updates that row where it is rather than moving it.
- New Conversation no longer asks anything. It uses the coding service you used last, the project this window is open
  on, and whatever model and effort your installed CLI already defaults to. Choosing all three explicitly is still one
  command away.
- The conversation editor drops its session panel. Model, effort, context use, cost, and a quota window that is
  actually close now read as one line under the composer, and the reply column is centred for reading.
- Enter sends a message and Shift+Enter writes a new line.
- The Chats sidebar now marks the open conversation, keeps it selected, and labels provider-owned chats as Resume. The editor uses a quieter in-chat empty state, a named resume moment, and the brand accent so the open session is obvious at a glance.

### Fixed

- The sidebar no longer prints "Update applies when the running conversations end" under its header. On a
  machine with several Runtime generations alive, and especially when this window sees none of them running, it
  read as coming from nowhere. The generation supervision still runs; it just no longer narrates itself here.

- A conversation that would not start now says why. Every failure coming from the coding service itself, including a
  CLI that was simply not signed in, used to report "the session or native pointer changed after the caller observed
  it" and offer nothing. Not signed in, not installed, out of quota and capability absent are now four distinct
  answers, each with its own next step.
- Opening the Runtrol Chats view now shows the selected in-progress conversation immediately. Studio no longer leaves the editor empty until the session is clicked again, and existing provider chats appear without waiting five seconds after startup.

## [0.1.3] - 2026-08-16

### Changed

- The symbol that shows while a conversation opens is now the Runtrol mark held still, with a coral light
  sweeping across it from left to right and repeating, instead of the mark spinning. It draws on a cleared
  pane, so there is no tile behind it.
- The sidebar shows the build version in small type at the top, under the "Runtrol" header.

### Added

- The Marketplace page now gives an exact extension-ID search fallback, automatic-update recovery for earlier manual
  VSIX installs, local-workspace requirements, and short troubleshooting paths before development internals.
- Marketplace metadata now exposes release health and current-version badges, broader agent and session discovery
  keywords, and exact Workspace Trust and virtual-workspace safety boundaries.

### Changed

- Advancing the Studio patch version on `main` now starts the complete six-platform release automatically. The release
  publishes every package, verifies the public version, target set, and exact package digests, then installs and
  activates the public Marketplace package on every native release runner before creating the GitHub Release.
- Studio now runs only in the local UI Extension Host, matching its ownership of local CLI processes and bundled native
  Core. Untrusted and virtual workspaces are rejected with a direct explanation instead of failing after activation.

## [0.1.2] - 2026-08-16

### Changed

- The symbol that shows while a conversation opens is now the Runtrol mark held still, with a coral light
  sweeping across it from left to right and repeating, instead of the mark spinning. It draws on a cleared
  pane, so there is no tile behind it.
- The sidebar shows the build version in small type at the top, under the "Runtrol" header.

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

- The sidebar no longer prints "Update applies when the running conversations end" under its header. On a
  machine with several Runtime generations alive, and especially when this window sees none of them running, it
  read as coming from nowhere. The generation supervision still runs; it just no longer narrates itself here.

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

- The sidebar no longer prints "Update applies when the running conversations end" under its header. On a
  machine with several Runtime generations alive, and especially when this window sees none of them running, it
  read as coming from nowhere. The generation supervision still runs; it just no longer narrates itself here.

- Installed CLI discovery no longer repeats one expensive help process for every candidate flag. The measured cold
  Core startup on Windows fell from about 30 seconds to about 4.3 seconds.
- Webview performance and startup messages now wait for the renderer readiness handshake, avoiding lost messages on
  slower VS Code hosts.
- Selected-session persistence retries only short operating-system file locks within a bounded window, so a transient
  Windows scanner lock cannot abort a session switch.

## [0.1.0] - 2026-08-12

First public Runtrol Studio release for six native Windows, macOS, and Linux targets.

### Changed

- The symbol that shows while a conversation opens is now the Runtrol mark held still, with a coral light
  sweeping across it from left to right and repeating, instead of the mark spinning. It draws on a cleared
  pane, so there is no tile behind it.
- The sidebar shows the build version in small type at the top, under the "Runtrol" header.

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

- The sidebar no longer prints "Update applies when the running conversations end" under its header. On a
  machine with several Runtime generations alive, and especially when this window sees none of them running, it
  read as coming from nowhere. The generation supervision still runs; it just no longer narrates itself here.

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
