# Changelog

All notable changes to runtrol will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Entries are written from the user's point of view. Internal code names, plan numbers,
and refactoring that no user can observe do not belong here.

## [Unreleased]

### Fixed

- Updating the extension no longer breaks against the still-running older Core. 0.1.8 failed its
  first hello on every machine with a running daemon ("InitializeResult violates the selected
  public schema"), because new required fields had joined a finalized protocol revision. The hello
  is now eternally compatible in both directions: fields added after a revision was finalized are
  optional forever, unknown fields are ignored, and a corpus of every shipped hello is enforced in
  CI so no future release can regress this.

### Added

- The Core now rolls itself forward. The daemon announces its own executable digest in the
  greeting; when the extension sees a daemon older than the Core it installed, it asks that daemon
  to retire (refused while any conversation still has a live process, retried once the machine is
  idle) and the next request starts the new build. A daemon too old to know "retire" gets a single
  explicit "Restart the Runtrol Core" button instead of silence. `runtrol retire` does the same
  from a terminal.

## [0.1.8] - 2026-08-20

### Added

- Conversations now open like files: each one gets its own editor tab, so several conversations can be
  on screen at once, split side by side, and rearranged like any editor. Switching tabs switches the
  sidebar highlight, prompts and interrupts go to the conversation whose tab they were typed in, and a
  window reload restores every conversation tab to its own session instead of collapsing them into one.

## [0.1.7] - 2026-08-20

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

- Returning to a conversation no longer shows an empty page. Leaving the tab (or switching sessions) used to
  resume the event stream after everything already delivered, so the fresh page had nothing to paint until
  the agent spoke again. A reopened conversation now replays the daemon's bounded recent window into the new
  page; anything older than that window stays with the service, exactly as before.

- A message from the conversation page that no handler recognizes is now dropped instead of falling through
  to the interrupt path. No shipped page ever sent such a message, but the fallback meant one malformed
  message away from stopping a running agent.

## [0.1.6] - 2026-08-20

### Fixed

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

- A conversation that would not start now says why. Every failure coming from the coding service itself, including a
  CLI that was simply not signed in, used to report "the session or native pointer changed after the caller observed
  it" and offer nothing. Not signed in, not installed, out of quota and capability absent are now four distinct
  answers, each with its own next step.
- Opening the Runtrol Chats view now shows the selected in-progress conversation immediately. Studio no longer leaves the editor empty until the session is clicked again, and existing provider chats appear without waiting five seconds after startup.

## [0.1.3] - 2026-08-16

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
