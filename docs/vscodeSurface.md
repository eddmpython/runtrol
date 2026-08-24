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
provider and managed-session streams then replace that inventory in the background. Provider-related requests use the
current complete provider snapshot immediately and schedule one bounded filesystem restamp. A newly installed
catalogue service then arrives on the provider watch without a provider-specific path or a restart; unchanged PATH
directories, probe cache, and resolved executable identities publish nothing. Probe-cache writes invalidate the
snapshot directly. Managed-session refreshes reuse the current stream snapshot. A lifecycle
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
- The sidebar is the one list of this machine: every project and every conversation of every installed coding
  service. A service is the row icon, never repeated text and never a parent node. The row contains only the actual
  conversation name beside that icon. While a turn is working, the same service icon spins. A quiet row has no
  status label, badge, or elapsed-time label.
- A created project, this window's open folder, or a discovered folder with multiple conversations is a project
  heading, with its conversations beneath it. A one-off provider working directory remains a plain conversation
  instead of becoming a false project. A heading is created by the operator, or is the folder this window has open
  without registration, or is discovered because multiple conversations name it (created > open > discovered,
  one heading per place). The open folder is present even before its first conversation, so starting work there never
  requires adding a project. An explicitly created project may also remain while empty. Two folders with the same
  name are told apart by their parent.
  Grouping by coding service would sort by an implementation detail, since one repository driven by two CLIs is one
  piece of work.
- Conversations are listed machine-wide, in one question per service, from the service's own surface: a listing
  protocol method, a listing command, or (for a CLI that publishes neither, Claude Code) the names in the CLI's own
  store. A structured human-facing `name`, `title`, or `preview` may cross directly into the visible row without
  persistence or semantic parsing. When a list is not everything, the sidebar information action explains the
  affected services without placing a permanent status sentence above every conversation. A conversation started without a project runs in
  the extension's own scratch folder and is listed beneath the headings as a loose row, with no folder name and no
  collision question.
- The project this window has open is always first and expands as soon as it has conversations. Other projects remain
  ordered by their most recently updated conversation. Turn state never participates in ordering. The machine-wide
  view title has no current-folder qualifier that could make the other projects look nested under the window's folder;
  the first heading and its open-folder icon already identify the current project.
- Project summaries contain only a compact total count and an optional disambiguating qualifier. They never expose
  running, stopped, or attention state as heading text.
- Conversation rows have no textual state detail. A working conversation is shown by its spinning service icon; when
  it is not working, the icon is still. Provider, project, elapsed time, `Running`, `Ready`, and `Stopped` are not
  repeated beside the title. A conversation that cannot be reopened keeps the same disabled icon and one exceptional
  blocked mark because clicking it cannot perform the ordinary row action.
- Every installed CLI has a row in the fixed `Agent Usage` area at the bottom of the same sidebar. The area is
  expanded by default. A compact display-name mark distinguishes similarly named services there without a provider
  table (`CC` for Claude Code and `CO` for Codex). `Checking`, `Unavailable · Fix`, `Ready`, current usage, and a
  blocking limit are mutually honest states. Every provider-reported numeric account window is a real progress bar,
  bounded from zero to 100,
  with its window, exact percentage, and reset time beside it. Primary and secondary windows remain separate. A
  missing percentage never becomes an invented empty bar. `Ready` means the CLI is usable while no numeric account
  limit has been reported. A disconnected last report says so instead of looking current, and a reached limit uses
  the error colour rather than looking like ordinary capacity.
- The same fixed area ends with one `Add coding services` row when the generated official catalogue contains services
  that are not installed. Its count is visible without expanding another view. Selecting it opens a searchable list
  of missing services and their exact install lines. Selecting a service places the line in a terminal unexecuted;
  Studio never downloads, installs, signs in, or starts a service implicitly. Installed catalogue services disappear
  from that picker and appear as ordinary CLI status and usage rows through the same Runtime inventory.
- Provider-owned archive and delete actions sit together on every row whose discovered capability supports them.
  Either action first closes a Runtime-supervised pointer when necessary, then asks the provider's own surface to
  mutate its stored conversation. `sessions/deleteNative` relays Codex `thread/delete` and Cline `history delete`;
  `sessions/archiveNative` relays Codex `thread/archive`. A session without a provider-owned identity only forgets its
  local pointer. Runtrol never removes or edits a provider's files itself.
- Moving this window to a project is a button on its heading (and the live conversation's project chip), never a side
  effect of opening a conversation. Opening is the file-click grammar: the conversation's tab opens here, the window
  stays where it is, and the CLI runs in the conversation's own folder regardless.
- Heading order ignores what is running inside it, for the same reason row order does. The current folder remains
  first; position among other projects reflects provider recency.
- A heading's rows are built when that heading is first drawn, never before. Thirty projects means twenty-nine closed
  headings whose rows nobody is going to look at, and the tree provider answers parent queries from a map rather than
  from built items so revealing a row never depends on having built the rest.
- A Runtime-supervised session and the provider-owned chat it came from are one row. Row identity is the conversation,
  so opening a saved chat updates that row in place instead of removing one and inserting another.
- Every conversation is ordered by the most recent provider timestamp. Live state never overrides recency and turn
  state never participates, so a row moves only when its conversation timestamp changes.
- A row whose service asked a question carries the service's own first allow and decline options inline and every
  option under "Answer the Question...", so it is answered without opening the tab. Sign-in and provider-owned
  remedies stay available as actions without adding permanent state text to every conversation. One light watch per
  running session keeps the icon and actions current; the hot ceiling bounds how many.
- The window moves between projects from the keyboard: "Switch Window to Project..." (`Ctrl+K Ctrl+Shift+P`) and
  back with `Ctrl+K Ctrl+B`, the same window replaced each time; the previous project is one string in global state.
- A new chat's service chip and the `Also Ask Another Service` command add another installed service without a
  second prompt. For a Git project, the first message asks Core to create one linked worktree per chosen service at
  the exact clean base commit. Every Runtime session starts with exclusive access to its distinct path, while Studio
  keeps the chats under the selected base-project heading and the grid lines them up. A projectless scratch chat is
  the only shared-placement exception because it has no project files to isolate.
- A single-service collision offers `Start isolated` as the primary safe action. Core durably records creating,
  ready, bound, dirty-preserved, and released ownership. Studio rebinds a uniquely matching live Runtime session
  after a restart, removes only abandoned clean worktrees, and shows the exact path of any dirty worktree it preserves.
  No branch, merge, commit, prompt, reply, provider flag, or transcript enters that record.
- Every conversation retains one internal operational state for actions, navigation, accessibility, and the view
  badge. Only active work changes the visible service icon by spinning it. The row does not print the state name.
- A conversation whose turn stopped for a person contributes to the view count badge and the next-waiting command, so
  it stays reachable without adding a permanent label to every quiet row. An account limit is a separate state and
  never counts toward that badge, because nobody can answer it.
- One command opens the next conversation that stopped for the operator, and pressing it again walks the rest
  rather than returning to the same one. A conversation waiting on an account limit is never a destination, because
  nobody can answer it. This is the orchestration primitive: supervising several agents never requires reading a
  board to work out which one wants attention.
- While anything is waiting, the status bar carries the count and its warning colour from anywhere in the window,
  and activating it opens that conversation. With nothing waiting it returns to reporting running counts. A count of
  running agents is ambient; a count of agents that stopped for this person is a request.
- New Conversation, Switch Conversation, Show Open Conversation, and Open Next Waiting each have a keyboard chord,
  so the entry point never requires the mouse.
- The Conversations title bar keeps only New Conversation, Create Project, and Switch Conversation visible. Waiting,
  current-conversation, layout, refresh, service, device, access, and recovery commands remain in the overflow and
  Command Palette. The title and the list do not lose reading space to a row of rarely used toolbar icons.
- Every visible provider mark has an equivalent spoken provider name. Tree rows announce their internal state to
  assistive technology without placing provider or state text in the visible row. Composer chip and slash-command
  listboxes expose expansion and the active option to assistive technology;
  Escape closes a chip menu and returns focus to the chip that opened it.
- A coding service that is not installed produces no row. Every installed service appears in the fixed status and usage
  area. One that still cannot run says `Unavailable · Fix` there, and Enter opens the same provider-owned remedies as
  its inline action. The Conversations tree remains only projects and conversations. An empty tree distinguishes a
  usable CLI with no conversations from a machine with no usable CLI, and gives the correct next action in each case.
  A newly discovered executable says `Checking` in the fixed area until its verified probe completes; it is never
  briefly reported as missing or made invisible by another usable CLI.
- Each conversation opens in its own editor tab with a bounded renderer and composer, exactly as a file does: ten or
  twenty tabs are arranged, split and sized by VS Code's own editor groups, and the focused tab is the selected
  conversation. Prompts and interrupts travel only to the tab that sent them. Tabs survive a window reload by their
  session identity, and a tab whose session no longer exists is closed rather than guessed at.
- A conversation can live in any of the window's own places: an editor tab (the default), the bottom panel beside
  the terminals, or the secondary side bar beside the code. The two non-tab surfaces are both named `Chat`, so their
  view and container labels state their purpose without repeating the product name inside the conversation. Each
  place is a VS Code surface; Runtrol adds no pane system of its own. A conversation is in one place at a time and
  watched once; moving it is a row command ("Open Conversation in Panel / in Side Bar / as Tab"), and the conversation
  a place showed before a reload comes back to
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
- New chat opens as a draft tab: one neutral greeting, the composer, and chips for the project, the git branch, the coding
  service, the model, the reasoning effort and the access mode. Each chip is its own picker, nothing runs until the
  first message, and that message starts the conversation in the same tab with exactly those choices. The `+` on a
  project heading opens the same draft with the folder already answered; the defaults are this window's folder, the
  service used last, and the project's last explicit choices. The greeting does not repeat the project or product
  name because the composer labels its authoritative destination as `Project`, `Branch`, and `Agent`. The project hover
  gives the full path. A draft survives a window reload with its choices.
- The composer is the one standard card rather than an invention: a context row (project, branch, service), the
  message, and a bar with attach and access mode on the left and model, reasoning effort and send on the right. Images
  travel once as `sessions/submitBlocks` content and are never stored. There is no microphone, because no installed
  CLI takes audio. The message field names the selected coding service and tells the operator to choose one when
  none is selected, so the destination is clear before Enter can send.
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
- A hidden conversation pauses its watch at the last delivered cursor. Its dedicated SDK transport drains written
  bytes and destroys the unread side immediately, so rapid switching cannot strand a named-pipe slot. Reopening waits
  for the new Webview document to become ready before bounded replay continues.
- Conversation and sidebar activity watch handshakes share one foreground-priority gate. Only connection and
  subscription setup is serialized; acknowledged streams remain concurrent, and retired promise cleanup is not on
  the visible switch path.
- An operator name is stored as bounded session metadata. Without one, the provider's own catalogue title or
  structured display preview is used and refreshed after a native identity appears and after each turn settles. A
  project or provider name is never a conversation-title fallback. `Chat` with a short stable identity is the final
  fallback, and a short stable suffix appears only when actual titles collide. Renaming a provider-owned saved row
  adopts it only long enough to store the operator label and immediately cools the provider process again.
- The selected session remains first. One fuzzy switcher searches project, provider, state, and workspace metadata.
- Session-index subscribers receive one current snapshot and then only list-visible changes.
- Selecting a cold session gives immediate feedback, resumes through its provider-native identity, and follows its
  exact workspace. If the only overlapping chat is idle, Studio cools its provider process and switches with no
  dialog while preserving that chat. Only a turn that is still working asks whether to stop and switch or keep both
  writers active.
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
| `agentTools.ts` | exact Core enable, disable, and list commands plus in-memory project badge state | provider configuration, integration secrets, or Runtime grant policy |
| `chatPlacement.ts` | the provider-neutral decision between shared, exclusive, and isolated first-message placement | Git commands, provider names, or durable ownership |
| `isolatedWorkspace.ts` | exact Core isolation requests and the exact generated Runtime root grant and revocation | Git execution, broad root removal, prompt content, or provider policy |
| `pairingQr.ts`, `pairingQrVendor.ts` | pairing-time loading of the bounded QR encoder sibling bundle | activation-time encoder cost or pairing authority |
| `state.ts` | provider, session, cursor, and selection metadata in memory | conversation frames |
| `selectionStore.ts` | one bounded selected-session identifier | prompts, replies, or provider state |
| `controller.ts` | user actions, one watch lifetime, workspace binding | transcript discovery or agent loops |
| `conversationSurface.ts` | the place contract: tab and workbench-view surfaces, the empty-place page | conversation state, watches, or any place-specific rendering |
| `conversationView.ts` | one conversation page on one surface, CSP, and Extension Host to Webview transport | retained conversation state or a second live renderer |
| `conversationPanels.ts` | one binding per conversation (surface + watch), the two workbench places, the grid | provider branches or conversation content |
| `diffDocuments.ts` | declared changes as read-only virtual documents for VS Code's diff editor | files on disk or a transcript of changes |
| `webview/` | bounded active rendering and input | durable storage or background sessions |
| `mission/controller.ts`, `mission/schedule.ts`, `mission/autoFlight.ts`, `mission/momentum.ts`, `mission/recovery.ts`, `mission/waveRunner.ts`, and `mission/tree.ts` | Mission review, exact durable schedule review, bounded local Auto Flight authority, safe-wave continuation, exact interrupted-Mission recovery, the shared provider-neutral wave runner, reviewed fleet launch, Task rows, native Artifact comparison, and one native editor document | provider input without confirmed local authority, transcript inference, schedule timers, polling, or optimistic completion |
| `mission/landing/` | deterministic ordinary queue and explicit Fleet winner selection, Receipt-evidence-bound native review, fixed-allocation reads, exact atomic Artifact replacement, drift and link defenses, and verified rollback | semantic merge, conflict resolution, staging, commits, provider knowledge, or conversation content |
| `capability/controller.ts` | candidate inbox, native diff review, and exact local trust actions | capability text injection or user-wide trust |

## Mission and capability surface

The Missions tree is part of the existing Runtrol activity container. It lists Core snapshots and exposes actions only
when the matching state permits them. Validation selects a project Mission file. An ordinary validated or running
Mission offers `Arm Mission Auto Flight` as its primary path and `Continue Reviewed Mission` as its explicit one-wave
path. The arm modal shows each exact Mission digest, project, and runtime-discovered operator-choice provider. One
confirmation can arm up to eight ordinary Missions. Armed rows show a rocket and `AUTO`; disarm is their immediate
inline action.

A validated Mission also offers `Schedule Reviewed Mission...`. Presets cover 15 minutes, one hour, and tomorrow at
local 09:00; a strict local input covers an exact minute. The confirmation freezes Mission and policy digests, Task
instruction digests, selectors, workspace modes, complete runtime-discovered provider assignments, due instant, and
the pending schedule ID being replaced. A post-confirmation read must match before commit. Pending rows show a calendar
and local due time, expose exact cancellation, and the Mission document shows local and ISO time, schedule identity,
provider mapping, state, and any closed structural failure.

Neither `mission/controller.ts` nor `mission/schedule.ts` owns a due timer. Closing Studio leaves the schedule in the
Core ledger. Reopening Studio only reads Core state. The Extension never starts a scheduled provider itself, stores no
schedule authority outside Core, and adds no provider or transcript knowledge. The static extension gate injects a
timer and removes schedule authority and compare-and-swap fields to prove those regressions make it red.

Auto Flight uses only Mission scheduler state and bounded Runtime metadata. Runtime row changes trigger reevaluation,
with no polling. Before automatic provider input, Studio durably records the exact Task, session, and current
`sessionGeneration`. Automatic Gate verification requires that same session to return `hotIdle` at a greater
generation. Working, person-waiting, quota-waiting, and paused Missions keep their arm without advancing. Authority
drift, ambiguous submission, missing or replaced sessions, recovery states, failure, specialized comparison,
cancellation, and other stopped states disarm it. Arrival at `integrating` disarms and offers Receipt Landing. No
instruction, provider output, event, transcript path, Gate output, or Artifact content enters extension storage.

Before Auto Flight disarms for a person wait, safety stop, or Receipt Landing, Studio writes one random signal UUID
and closed structural kind to a bounded global-state outbox. Pending delivery revokes provider-input authority
immediately. Core records the exact UUID idempotently, and only that acknowledgement removes the outbox entry and
arm. An Extension Host restart retries the same UUID; rearming clears stale signals for that exact Mission digest.
No instruction, path, output, Receipt body, or push payload is added by this handoff.

The explicit Continue modal still shows the exact Mission digest and currently safe work. One confirmation starts a
validated Mission, verifies exact `Ready` Task sessions with fixed Gates, prepares newly eligible workspaces and
public Runtime sessions, and sends exact reviewed instructions. Both paths stop at unproven work and keep project
integration explicit.

Start, Prepare, Send, and Verify remain command-palette and Task-row recovery actions. A provider submission whose
success is unknown is persisted as ambiguous before transport and cannot be auto-verified, including after Extension
Host restart. `choose_one` Missions keep the specialized `Run All Reviewed Attempts` action.

A Core restart turns any Task that crossed an in-flight or verification boundary into `blocked` and removes its stale
session binding. A recoverable blocked Mission row exposes **Recover Interrupted Mission**. One focused Quick Pick
shows the full Mission digest, policy digest, project, each exact workspace, runtime-discovered provider assignment,
and the risk that prior external effects may be repeated. Esc performs no mutation. Confirmation re-fetches and
compares every launch-relevant fact, reopens only blocked Tasks, safely resumes the scheduler, starts fresh public
Runtime sessions, and rechecks each instruction before Send. The same shared wave runner launches Fleet and recovery,
so the two paths cannot drift into different provider or ambiguity rules. A stop between reopen and resume is
re-enterable from eligible or reserved state. Contract loss is shown as `unavailableAfterRestart` and has no recovery
action. Mission refresh updates an already open virtual document together with its tree row, preventing a stale
`running` document from contradicting the new `blocked` state.

The Missions view title exposes `Continue Ready Missions` for cross-project operation. It reviews up to eight exact
ordinary Mission digests at once, prioritizes safe work already running, and advances each through Mission Momentum.
Additional ready Missions stay visible for the next review. Waiting-only, expired, recovery, specialized comparison,
and integration states are counted but excluded. A failure is attached to its Mission and does not stop unrelated
reviewed work. All native conversation tabs started by the batch are placed through the existing VS Code grid once.
No new renderer, provider parser, scheduler, or transcript owner is involved.

The reusable `runtrol-mission:` editor document shows the Mission source and digest, approval expiry, progress, Task
state, instruction and policy digests, provider and session identities, workspace and base commit, selected capability
versions, Gate counts, and the latest passing Run and Receipt IDs. It uses VS Code's text document surface and does not
create another Webview or another provider stream.

Pause, safe resume, cancel, bounded retry, integrated-tree verification, completion, and archive are explicit local
commands. Ordinary Missions require every Task to pass. A reviewed `choose_one` Mission waits for every attempt to
finish, compares sealed Artifact paths in native diff editors, and completes only with one exact passing Task. The
extension never authors a merge, resolves conflicts, stages, or commits. A reviewed Landing writes only the exact
sealed bytes the operator just compared, either the ordinary all-Task set or one explicitly selected winner Receipt.

An ordinary integrating Mission exposes **Review and Apply Mission Landing** instead of direct completion as its primary
row action. The same command in the Missions title and command palette orders every eligible project deterministically
and asks for a Mission when more than one is ready. One native VS Code changes editor compares every sealed UTF-8
Artifact from its passing Receipts against the current project, up to a fixed 1,024 Artifact review bound. Review
sides are bounded read-only memory documents with an 8 MiB combined text ceiling and no conversation storage. The
explicit **Apply, run Gates and complete** action revalidates Mission and Receipt identities, sealed path, size and
SHA-256 evidence, exact source and target bytes, target existence, symbolic-link ancestors, and dirty text, notebook
or custom-editor tabs before any write. Reads verify stat bounds, file identity and EOF without unbounded allocation.
One cross-window project lease covers exclusive same-directory preparation, final compare, atomic replacement,
verified rollback, and Core completion. Core compares Receipt evidence before and after fixed Gates. Studio reports
how many other projects are ready and offers **Review next**. Gate failure retains the exact review and ordinary Git
changes for a completion-only retry. A lost completion response refreshes Core authority and exact applied bytes, then
converges without a second write. A busy older Core without Artifact evidence leaves Landing unavailable. The old direct
completion command remains available for recovery.

An integrating `choose_one` Mission exposes both candidate comparison and winner Landing. Selecting the Mission asks
for one passing Task; selecting a passed Task keeps that exact Task. The winner name appears in the native multi-diff
and confirmation. Review authority contains only that Task's Receipt and includes the selected Task ID. Apply uses the
same bounded read, atomic transaction, lease, rollback, Gate, drift, and completion recovery path, while Core receives
the same selected Task ID. A different candidate can neither enter the Artifact set nor replace the selection after
review. Core retains the selected Task and Receipt through terminal compaction. The completed Mission document shows
both identities, and response-loss recovery requires both before it accepts success even when two candidates have
identical bytes.

The Capability Candidate Inbox uses a native quick pick and Markdown review document. Verification and approval name
one exact project version. Approval opens the built-in VS Code file or diff review first. Reject, quarantine, rollback,
and archive are also modal local actions. Candidate bodies stay in project files, and no action injects those bodies
into a Task. The detailed contracts are [Mission operations](missionOperations.md) and
[project capability trust](capabilityTrust.md).

## Agent Tools surface

Every project heading offers **Enable Agent Tools for This Project** as an inline action. The heading appends
`Agent Tools` after Core confirms the canonical project root, and one `tools list` call restores all enabled badges
after activation, including in a window with no open folder. Disable is available from the same heading and the
command palette.

The Extension Host passes one absolute project path as one argv word to the exact managed Core. It does not receive
an integration secret, inspect provider configuration, or implement grant policy. Core owns official provider MCP
registration, Runtime enrollment, protected credential lifecycle, and revocation. Changes are serialized inside one
Extension Host, while the daemon's provider lanes serialize official configuration commands across windows. The full
authority and protocol contract is [Agent Tools](agentTools.md).

## Performance contract

The real Extension Host gate runs three isolated cold trials of the production extension and Core on hosted Windows,
macOS, and Linux. The fastest of three trials feeds the shared budget, while exact session counts and zero dropped frames must hold in
every trial. The shared ratchet currently caps:

| Measure | Ceiling |
|---|---:|
| Ready activation | 1,800 ms |
| Runtrol navigation and conversation opening | 1,000 ms |
| Refresh p95 | 50 ms |
| Extension Host RSS growth | 64 MiB |
| Loaded animation frame p95 | 40 ms |
| Load overrun above the runner's native cadence | 8 ms |
| Input and scroll p95 | 50 ms |
| Renderer backlog | 1,024 frames |
| Hot-session switch p95 | 175 ms |
| Cold provider-native resume | 3,500 ms |
| Full workspace reload restoration | 2,500 ms |
| Second-folder conversation arrival | 15,000 ms |

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
the VSIX into a clean VS Code 1.132.1 profile, activates through the bundled Core, opens Runtrol, opens and closes a
new-conversation composer through the public command, exercises upgrade and rollback with an active session, and
uploads the package. Hosted extension gates use that same exact tested version unless an operator explicitly supplies
another version. It creates a tagged GitHub Release only after all six jobs pass.

The current public release is [Runtrol Studio 0.1.15](https://github.com/eddmpython/runtrol/releases/tag/vscode-v0.1.15).
All six native packages are published under one
[Marketplace listing](https://marketplace.visualstudio.com/items?itemName=runtrol.runtrol-studio). Stable VS Code
1.132.1 downloads the exact public package into an isolated profile on each native release runner, activates with no
configured Core path, materializes the bundled Core, refreshes through it, opens the contributed Runtrol view, and
opens then closes the same new-conversation composer. Exact verifier processes and temporary profiles are removed
afterward.

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
| `node tooling/real-window-eye.mjs` | the eye pass: an isolated VS Code window and an isolated Runtime with the real installed CLIs, real folders and real conversations, photographed in the draft, working row, Agent Usage bars, conversation, tabs, reopened, grid, places, diff and diff-editor poses, with a real throwaway deletion; the pictures are the judgement. `RUNTROL_EYE_ENTRY=placeProbe` runs the focused place probe and `RUNTROL_EYE_ENTRY=agentToolsEye` runs the zero-model-turn Agent Tools enable and revoke proof in isolated provider homes |
| `RUNTROL_EYE_DRAFT_ONLY=1 node tooling/real-window-eye.mjs` | focused current-folder sidebar and composer photograph plus the same real project-switch and keyboard-back proof, with no provider turn |
| `RUNTROL_EYE_ENTRY=missionFlightDeckEye node tooling/real-window-eye.mjs` | at 1456 by 906, two isolated Git projects and reviewed Missions start together through one flight, reach integration together through the next flight, review existing and new files in native Receipt Landing multi-diffs, select the public product action, reject five drift or local-boundary failures plus one passing Gate mutation, retain retry state, apply exact bytes, and complete both Missions. Review, confirmation, first-completed/second-waiting, and next-review screenshots are inspected directly |
| `RUNTROL_EYE_ENTRY=missionAutoFlightEye node tooling/real-window-eye.mjs` | one reviewed two-wave dependency Mission is armed once, starts two real provider sessions, verifies two fixed Gates, reaches `integrating` with zero operator continuation actions, removes its own authority, and is photographed before arm, while armed, and after arrival |
| `RUNTROL_EYE_ENTRY=fleetEye node tooling/real-window-eye.mjs` | two real CLI attempts reach passing Receipts, native diffs compare their distinct output, `attempt-2` opens as the only winner Receipt, an `attempt-1` apply request is rejected, the public primary action writes exactly `attempt 2`, and the Mission reaches `completed` with its selected Task and Receipt visible. Comparison, winner review, confirmation, and completed screenshots are inspected directly at 1456 by 906 |
| `RUNTROL_EYE_ENTRY=missionRecoveryEye node tooling/real-window-eye.mjs` | one real provider Mission is photographed in flight, its exact Core process is terminated and replaced over the same isolated home, the Mission and Task are photographed blocked, Esc proves the focused recovery confirmation performs no mutation, Enter starts a distinct fresh Runtime session, and the running recovery is photographed at the declared viewport |
| `RUNTROL_EYE_ENTRY=safeParallelChatEye node tooling/real-window-eye.mjs` | one draft starts real Claude Code and Codex sessions in distinct Core-owned linked worktrees, proves the same Git store and base commit, keeps the base checkout unchanged, force-restarts Core, recovers exact ownership, and removes only the exact clean worktrees and Runtime roots |
| `node tooling/installed-safe-parallel-eye.mjs <exact-vsix>` | installs one exact VSIX in an isolated real VS Code profile, proves the bundled and managed Core digests match, then uses only the public new-chat and Also Ask commands to start two installed services in distinct worktrees and verify exact cleanup at 1456 by 908 |
| `agentToolsSmoke` | real installed provider CLIs, official MCP registration, modern and legacy discovery, fixed tool catalogue, root isolation, Runtime reads, complete revocation, and post-revocation default deny with zero model turns |
| `missionGrowthContracts` | Mission state, exact Send, evidence, integration, capability trust, local scope, tamper, and rollback |
| `missionLiveJourney` | two installed provider CLIs complete five reviewed Tasks and an explicit reuse, tamper, and rollback journey through production IPC |
| `vscodePackage` | six-target SSOT, exact archive contents, Core bytes, workflow integrity, and listing metadata |
| `crossPlatformContract` | one public command, automatic Core default, and unconditional first-run step across the shared six-target package contract |
| `crossPlatformMatrix` | exact VSIX installation, bundled Core discovery, Runtrol opening, new-conversation draft, and exact close on native Windows, macOS, and Linux |
| `vscodeUpgradeRollback` | active-session continuity across official VSIX upgrade and rollback |
| `channelVerdict` | confirmed provider package ownership and closed update arguments |
| `cliUpdateRehearsal` | failed provider target and exact verified rollback transaction |
| `node tooling/installed-package.mjs --marketplace` | public Marketplace download, isolated activation, bundled Core, refresh, view opening, and new-conversation draft opening and close |

Every verifier uses an isolated profile marker and terminates only exact owned process identities. It must never close
unrelated VS Code windows, extension hosts, daemons, or provider sessions.
