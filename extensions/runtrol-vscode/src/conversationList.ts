import { isProjectless } from "./projectlessWorkspace";
import type { ProjectRecord } from "./projects";
import type { NativeChatLine, ProviderLine, SessionLine, TerminalDescriptor } from "./runtimeTypes";
import { providerDisplayName, providerIcon, workspaceName } from "./sessionDisplay";
import { workspaceCovers, workspaceIdentity } from "./workspaceCollision";

import { NO_ACTIVITY, type SessionActivity } from "./sessionActivity";

/// Why a row refuses to open while its service is answering somewhere else.
///
/// Named once because two row shapes reach the same refusal, and because the row's tooltip and the notification
/// its click raises are both this sentence. Written twice it drifts, and then the panel says one thing where
/// the reader hovers and another where they click.
const RUNNING_ELSEWHERE =
  "This conversation is already running, but its live terminal is not available in this window.";
const PROCESS_STATUS_UNAVAILABLE =
  "Runtrol could not confirm whether this conversation is still running, so it will not open a second owner.";

/// What a conversation is doing, said the way a person would say it.
///
/// Ordered by how much it wants from the reader. `needsYou` is the only one that is actually urgent, and keeping
/// it a separate value from `attention` is deliberate: something that broke and something that is politely
/// waiting are different errands, and a surface that merged them would send the reader to the wrong one.
export type ConversationActivity =
  | "needsYou"
  | "attention"
  | "working"
  | "waitingOnQuota"
  | "ready"
  | "saved";

/// Where the conversation's process is right now.
///
/// This is the one fact that opening, stopping and deleting all turn on, and for a while it was five facts
/// folded into one `live` flag that every surface read its own way: a process alive in an older Runtime
/// generation read as "outside Runtrol", so its row would neither open nor stop, and a click had nowhere to go
/// (measured 2026-08-29, eight conversations). Every row states its presence once, and `live`, `canOpen`,
/// `canStop` and `blocked` are read off it in one place (`facts`) and nowhere else.
export type Presence =
  /// Runtrol hosts its terminal, in whichever generation opened it. Opens by attaching there; stops there.
  | { readonly kind: "hosted"; readonly terminal: TerminalDescriptor }
  /// Runtrol supervises a running structured session.
  | { readonly kind: "supervised"; readonly session: SessionLine }
  /// Runtrol started it a moment ago and the service has not described it yet.
  | { readonly kind: "starting" }
  /// A provider process is proven alive outside the terminal table. `openable` means its exact process also
  /// published a safe route that the first viewer can attach lazily; `focusable` means a registered VS Code window
  /// is proved to own the terminal it runs in, so that window can show it even when nothing can be opened here.
  | { readonly kind: "external"; readonly openable: boolean; readonly focusable: boolean }
  /// The last live owner could not be rechecked. It is not called live, but duplicate ownership stays denied.
  | { readonly kind: "unconfirmed" }
  /// No live process. The service stores it, and `openable` says whether the service can reopen it.
  | { readonly kind: "stored"; readonly openable: boolean };

/// What a presence lets a person do. The only place these four are decided.
function facts(presence: Presence): {
  readonly live: boolean;
  readonly canOpen: boolean;
  readonly canFocus: boolean;
  readonly canStop: boolean;
  readonly blocked: string | null;
} {
  switch (presence.kind) {
    case "hosted":
    case "supervised":
      return { live: true, canOpen: true, canFocus: false, canStop: true, blocked: null };
    case "starting":
      return { live: true, canOpen: true, canFocus: false, canStop: false, blocked: null };
    case "external":
      // Focus is never a way to open: a window showing its own terminal is the whole of it, and nothing here
      // starts a second owner of the conversation.
      return {
        live: true,
        canOpen: presence.openable,
        canFocus: !presence.openable && presence.focusable,
        canStop: false,
        blocked: presence.openable || presence.focusable ? null : RUNNING_ELSEWHERE,
      };
    case "unconfirmed":
      return {
        live: false,
        canOpen: false,
        canFocus: false,
        canStop: false,
        blocked: PROCESS_STATUS_UNAVAILABLE,
      };
    case "stored":
      return {
        live: false,
        canOpen: presence.openable,
        canFocus: false,
        canStop: false,
        blocked: presence.openable ? null : "This coding service cannot reopen this conversation.",
      };
  }
}

/// One conversation, whichever half of the system currently holds it.
///
/// Runtrol distinguishes a session it supervises from a chat the coding service owns on disk. That distinction is
/// real inside the daemon and meaningless to the person reading the list: both are one conversation they had. This
/// type is where the two halves become one row, so nothing downstream has to ask which kind it is.
export type Conversation = {
  /// Stable across the moment a provider-owned chat becomes a supervised session.
  ///
  /// Keyed on the conversation itself rather than on whichever record currently describes it, so opening a saved
  /// chat updates a row in place instead of removing one and inserting another somewhere else.
  readonly key: string;
  /// The key this row carried before its service announced its own identity for the conversation, when that
  /// moment has already passed.
  ///
  /// A conversation is keyed by the service's identity once it exists, and by the local session until then, so
  /// the key changes on the first turn. Anything remembered against a row (a pin, a nickname) was written under
  /// whichever key was current, and would be orphaned by that change without this.
  readonly legacyKey: string | null;
  readonly providerId: string;
  readonly serviceName: string;
  /// The glyph that stands for that service, which is what tells two rows apart without reading them.
  readonly serviceIcon: string;
  readonly title: string;
  /// The project the person selected. For a Core-owned linked worktree this is its base checkout.
  readonly homeWorkspace: string;
  /// The exact directory where the provider process runs.
  readonly workspace: string;
  readonly folder: string;
  /// Whether it runs in the scratch folder, which is what a conversation started with no project is.
  ///
  /// Such a row never files under a heading and never repeats its folder, because the folder was never the
  /// person's choice; it sits as a plain row beneath the projects, the way the chat apps people already use
  /// show a conversation nobody filed anywhere.
  readonly projectless: boolean;
  /// When the coding service last touched it, when the service reports that at all.
  readonly updatedAtMs: number | null;
  readonly activity: ConversationActivity;
  /// The tool the provider says is running right now (its own name or classification, the line the page draws
  /// for that call), or null. Read off the provider's events by the activity watch; never inferred.
  readonly tool: string | null;
  /// Whether the provider said this conversation needs the operator to sign in.
  readonly signInNeeded: boolean;
  /// Where its process is. `live`, `canOpen`, `canStop` and `blocked` are this, read out (`facts`).
  readonly presence: Presence;
  /// Whether a provider process is alive for it, wherever that process is.
  readonly live: boolean;
  /// Whether Runtrol can end its process: it hosts the terminal or supervises the session.
  readonly canStop: boolean;
  readonly open: boolean;
  /// Whether the operator pinned this conversation to keep it at the top of its list. A local ordering choice,
  /// remembered per machine; it never changes the conversation itself.
  readonly pinned: boolean;
  readonly session: SessionLine | null;
  readonly native: NativeChatLine | null;
  /// The exact daemon generation and terminal to attach when this conversation is already running.
  readonly hostedTerminal: TerminalDescriptor | null;
  /// The transient row key this hosted process had before the provider published its conversation identity.
  readonly hostedKey: string | null;
  readonly canOpen: boolean;
  /// A registered VS Code window is proved to own the terminal this conversation runs in, so that window can
  /// show it and be brought forward. Never a way to open it here.
  readonly canFocus: boolean;
  /// Why it cannot be opened, for the one row where that is true.
  readonly blocked: string | null;
};

/// A conversation runtrol has just opened, before the service has written anything down about it.
///
/// The service is what names a conversation and what files it, and neither has happened the moment a person
/// presses new: the CLI is starting, and it writes its store on its own schedule (measured 2026-08-26: a Grok
/// conversation with a completed turn was still absent from Grok's own listing). Between those two moments the
/// person is looking at a list that does not contain the thing they just made, which reads as "it did not work".
/// So runtrol shows what it started, using what it does know: the service, the folder, and that it is running.
export type StartedConversation = {
  /// Distinguishes two conversations started in the same folder with the same service.
  readonly id: string;
  readonly providerId: string;
  readonly workspace: string;
  /// What the tab is called until the service names the conversation.
  readonly title: string;
  /// When runtrol opened it, which is how the row it becomes is recognised.
  readonly startedAtMs: number;
  /// The exact Runtime terminal this placeholder opened, once the public open response arrives.
  readonly runtimeGeneration?: string;
  readonly terminalId?: string;
};

/// Every conversation on this machine, in the order to show them.
///
/// `projectlessRoot` is the scratch folder conversations without a project run in (null when this surface
/// has none); rows inside it are marked so the tree keeps them loose instead of inventing a heading.
export function conversations(
  sessions: readonly SessionLine[],
  providers: readonly ProviderLine[],
  nativeChats: readonly NativeChatLine[],
  selectedSessionId: string | null,
  projectlessRoot: string | null = null,
  activities: ReadonlyMap<string, SessionActivity> = new Map(),
  isolatedWorkspaceHomes: ReadonlyMap<string, string> = new Map(),
  pinnedKeys: ReadonlySet<string> = new Set(),
  renamedTitles: ReadonlyMap<string, string> = new Map(),
  started: readonly StartedConversation[] = [],
  /// Reserved positional input from the former screen-byte activity heuristic. Screen output is not model
  /// activity: an idle TUI may repaint its cursor, menus and prompts indefinitely.
  _screenOutput: ReadonlySet<string> = new Set(),
  /// Conversations with a model answering, hosted or external. Runtime names these from the provider's
  /// structural turn state rather than by interpreting terminal output.
  activeNative: ReadonlySet<string> = new Set(),
  /// Conversations owned by a provider process outside the daemon's PTY registry.
  observedNative: ReadonlySet<string> = new Set(),
  /// Daemon-owned provider terminals, pushed at process birth and exit.
  terminals: readonly TerminalDescriptor[] = [],
  /// Provider owners whose last live proof could not be refreshed. They block duplicate resume without reading
  /// as live or Elsewhere until one successful roster round resolves them.
  unconfirmedNative: ReadonlySet<string> = new Set(),
  /// Live provider owners whose exact process publishes a safe terminal route.
  attachableNative: ReadonlySet<string> = new Set(),
  /// Live provider owners whose terminal a registered VS Code window is proved to own.
  focusableNative: ReadonlySet<string> = new Set(),
): Conversation[] {
  const nativeByKey = new Map<string, NativeChatLine>();
  for (const chat of nativeChats) {
    nativeByKey.set(conversationKey(chat.providerId, chat.nativeSessionId), chat);
  }
  const terminalByConversation = new Map<string, TerminalDescriptor>();
  for (const terminal of terminals) {
    if (terminal.processState !== "running" || !terminal.nativeSessionId) continue;
    const key = conversationKey(terminal.providerId, terminal.nativeSessionId);
    const prior = terminalByConversation.get(key);
    if (!prior || terminal.openedAtMs > prior.openedAtMs) terminalByConversation.set(key, terminal);
  }

  const rows: Conversation[] = [];
  const claimed = new Set<string>();
  const claimedTerminals = new Set<string>();
  for (const session of sessions) {
    const sessionKey = `session:${encodeURIComponent(session.sessionId)}`;
    const key = session.nativeSessionId
      ? conversationKey(session.providerId, session.nativeSessionId)
      : sessionKey;
    const legacyKey = key === sessionKey ? null : sessionKey;
    const hosted = terminalByConversation.get(key) ?? null;
    const externalKey = session.nativeSessionId
      ? nativeProcessKey(session.providerId, session.nativeSessionId)
      : null;
    const observed = externalKey !== null && observedNative.has(externalKey) && hosted === null;
    const unconfirmed = externalKey !== null && unconfirmedNative.has(externalKey) && hosted === null;
    if (hosted) claimedTerminals.add(terminalKey(hosted));
    claimed.add(key);
    rows.push(supervised(
      session,
      nativeByKey.get(key) ?? null,
      key,
      providers,
      selectedSessionId,
      projectlessRoot,
      activities.get(session.sessionId) ?? NO_ACTIVITY,
      isolatedWorkspaceHomes,
      pinnedKeys.has(key) || (legacyKey !== null && pinnedKeys.has(legacyKey)),
      renamedTitles.get(key) ?? (legacyKey === null ? undefined : renamedTitles.get(legacyKey)),
      legacyKey,
      externalKey !== null && activeNative.has(externalKey),
      hosted,
      observed,
      externalKey !== null && attachableNative.has(externalKey),
      externalKey !== null && focusableNative.has(externalKey),
      unconfirmed,
    ));
  }
  for (const [key, chat] of nativeByKey) {
    // A chat the daemon already supervises is the same conversation, not a second one. The service half only
    // contributes its title and timestamp, which the supervised row above has already taken.
    if (claimed.has(key) || chat.alreadyManagedAs) continue;
    const hosted = terminalByConversation.get(key) ?? null;
    if (hosted) claimedTerminals.add(terminalKey(hosted));
    rows.push(providerOwned(
      chat,
      key,
      providers,
      projectlessRoot,
      pinnedKeys.has(key),
      renamedTitles.get(key),
      activeNative.has(nativeProcessKey(chat.providerId, chat.nativeSessionId)),
      hosted,
      observedNative.has(nativeProcessKey(chat.providerId, chat.nativeSessionId)) && hosted === null,
      attachableNative.has(nativeProcessKey(chat.providerId, chat.nativeSessionId)) && hosted === null,
      focusableNative.has(nativeProcessKey(chat.providerId, chat.nativeSessionId)) && hosted === null,
      unconfirmedNative.has(nativeProcessKey(chat.providerId, chat.nativeSessionId)) && hosted === null,
    ));
  }
  // A provider process is visible immediately, even before its own store publishes a conversation identity and
  // title. Once that row appears it claims this terminal and `hostedKey` lets an already open tab move in place.
  //
  // One conversation is one row. A conversation already placed above (as a supervised session or a service
  // row) has taken its terminal, and a terminal whose native identity is already on a placed row is the same
  // conversation seen through another generation, not a new one: it is skipped, never given a second row
  // (operator, 2026-08-29: a session showing twice is the bug, and hiding the second is not the fix. The one
  // terminal per conversation the row keeps is `terminalByConversation`, the most recently opened). A bare
  // terminal a person started outside Runtrol, whose conversation is on no other row, becomes its own row.
  const placedNatives = new Set(
    rows
      .map((row) => (row.native ? conversationKey(row.providerId, row.native.nativeSessionId) : null))
      .filter((key): key is string => key !== null),
  );
  for (const terminal of terminals) {
    const key = terminalKey(terminal);
    const conversation = terminal.nativeSessionId
      ? conversationKey(terminal.providerId, terminal.nativeSessionId)
      : null;
    if (
      terminal.processState !== "running"
      || claimedTerminals.has(key)
      || (conversation !== null && placedNatives.has(conversation))
      || started.some((pending) => startedCoversTerminal(pending, terminal))
    ) continue;
    const chat = conversation ? nativeByKey.get(conversation) ?? null : null;
    const working = terminal.nativeSessionId
      ? activeNative.has(nativeProcessKey(terminal.providerId, terminal.nativeSessionId))
      : false;
    rows.push(hostedRow(terminal, chat, providers, projectlessRoot, working));
    if (conversation !== null) placedNatives.add(conversation);
  }
  // What runtrol started and the service has not named yet. A placeholder gives way only to the row carrying its
  // exact Runtime generation and terminal id. Keeping both would show one conversation twice, and matching by a
  // shared provider, folder, or timestamp would let one simultaneous terminal consume another terminal's row.
  const named = namedPlaceholders(rows, started);
  for (const pending of started) {
    if (named.has(pending.id)) continue;
    rows.push(startedRow(pending, providers, projectlessRoot));
  }
  // Pinned rows first, then running conversations above idle ones, and inside each of those bands a fixed
  // order. Pinning is a placement choice, so it sorts ahead of everything else.
  return rows.sort((left, right) =>
    Number(right.pinned) - Number(left.pinned) || byRunningThenStable(left, right));
}

/// How near the top a conversation sits by what its process is doing, higher first.
///
/// A conversation a turn stopped for is the one worth a person's eye, then one a model is answering in, then one
/// merely alive, then one only stored. This is the "running conversations on top" the sidebar promises.
function activityRank(activity: ConversationActivity): number {
  switch (activity) {
    case "needsYou": return 5;
    case "attention": return 4;
    case "working": return 3;
    case "waitingOnQuota": return 2;
    case "ready": return 1;
    case "saved": return 0;
  }
}

/// Running conversations rank above idle ones, and within a running band the order never moves.
///
/// A running conversation's `updatedAtMs` bumps on every byte it streams, so ordering the live band by recency
/// made the rows reshuffle under the person while several sessions ran (operator, 2026-08-30). A live band is
/// held in a fixed identity order instead, so a conversation stays where the eye left it for as long as it runs.
/// Stored conversations do not stream, so recency there is both stable and the useful order.
function byRunningThenStable(left: Conversation, right: Conversation): number {
  const rank = activityRank(right.activity) - activityRank(left.activity);
  if (rank !== 0) return rank;
  if (activityRank(left.activity) > 0) {
    return compare(left.folder, right.folder)
      || compare(left.title, right.title)
      || compare(left.key, right.key);
  }
  return byMostRecentlyActive(left, right);
}

/// Every conversation that belongs to one created project, under one heading.
///
/// A project is a folder the operator chose to make one, which is what a person means by "what I am working
/// on". Grouping by the coding service instead would sort by an implementation detail: somebody running Claude
/// and Codex on the same repository is doing one piece of work, not two.
export type ProjectGroup = {
  /// Stable across casing and separator differences, and legal as a tree element id.
  readonly key: string;
  /// The folder name, which is what a person calls the project.
  readonly name: string;
  /// The path, for the hover and for opening it.
  readonly workspace: string;
  /// Why this heading exists: the operator added it, or this window is open on it. The tree offers different
  /// actions for each (an added project can be renamed, pinned and removed; the open folder can be added), and
  /// the rule that one heading is drawn per place does not depend on which kind won.
  readonly kind: ProjectKind;
  /// Whether the person pinned it to the top. Only an added project can be.
  readonly pinned: boolean;
  /// Whether this VS Code window is open on it.
  readonly current: boolean;
  /// The parent folder's name, set only when another heading has the same name, so two folders called
  /// "new-chat" in different places are told apart without renaming either.
  readonly qualifier: string | null;
  readonly rows: readonly Conversation[];
  /// How many of them have stopped and want the reader.
  readonly attention: number;
  /// How many have a provider process behind them right now.
  readonly live: number;
  /// Whether the conversation the reader currently has open is one of these.
  ///
  /// A heading that hides the open conversation is a heading that made the reader lose their place.
  readonly holdsOpen: boolean;
};

export type ProjectKind = "created" | "open";

/// Conversations gathered under this machine's projects: the ones the operator added, and the folders this
/// window has open.
///
/// A project is a decision, never a discovery (fixed 2026-08-25). The panel used to invent a heading for every
/// folder that held enough conversations, and the operator rejected the wall of folder names it produced. Now a
/// heading exists because the person added the folder or opened this window on it, and nothing else. Adding a
/// folder lists every conversation the coding services report inside it, at once: the CLI's own listing is the
/// authority on which folder a conversation belongs to, and `projectOf` files each one under the deepest added
/// project that covers it.
///
/// One heading per place. An added project wins over an open folder covering the same conversation, because
/// adding is the more deliberate act and one row must never appear twice. An added project with nothing in it
/// yet is still returned: it was made a moment ago and a heading that vanished would read as the creation
/// failing. Pinned projects come first, then this window's folder, then the rest by most recent conversation.
///
/// Conversations without a project (no added folder covers them, the scratch folder, or no folder at all) are
/// deliberately absent here: they are the plain rows `loose` returns, at the top level beneath the headings,
/// never indented under a heading nobody made.
export function projects(
  records: readonly ProjectRecord[],
  rows: readonly Conversation[],
  openWorkspaces: readonly string[],
): ProjectGroup[] {
  const filed = new Map<string, Conversation[]>(records.map((record) => [record.key, []]));
  for (const row of rows) {
    if (intrinsicallyLoose(row)) continue;
    const home = projectOf(records, row);
    if (home) filed.get(home.key)?.push(row);
  }
  // Only what somebody added. The window's own folder used to become a heading of its own, which made the
  // same machine look different in every window: open Runtrol here and this folder led the list, open it
  // there and another did. The panel is the machine's, not this window's (operator, 2026-08-26), so the open
  // folder is marked as current and nothing more.
  return qualified(records
    .map((record) => group(
      `project:${encodeURIComponent(record.key)}`,
      record.name,
      record.workspace,
      "created",
      openWorkspaces.some((folder) =>
        workspaceCovers(record.workspace, folder) || workspaceCovers(folder, record.workspace)),
      filed.get(record.key) ?? [],
      record.pinned,
    )))
    .sort(byAddedOrder(records));
}

/// Headings that share a name get their parent folder's name beside it.
/// Pinned projects first, then the order the person put them in.
///
/// Deliberately not "most recently used". A list that reorders itself under the reader is a list they cannot
/// learn, and the same machine would look different from one hour to the next. The order is theirs to set
/// (operator, 2026-08-26), so the only thing this does is honour it and lift the pinned ones.
function byAddedOrder(
  records: readonly ProjectRecord[],
): (left: ProjectGroup, right: ProjectGroup) => number {
  const placed = new Map(records.map((record, index) => [`project:${encodeURIComponent(record.key)}`, index]));
  return (left, right) => {
    if (left.pinned !== right.pinned) return left.pinned ? -1 : 1;
    return (placed.get(left.key) ?? 0) - (placed.get(right.key) ?? 0);
  };
}

function qualified(groups: readonly ProjectGroup[]): ProjectGroup[] {
  const counts = new Map<string, number>();
  for (const heading of groups) counts.set(heading.name, (counts.get(heading.name) ?? 0) + 1);
  return groups.map((heading) => (
    (counts.get(heading.name) ?? 0) > 1
      ? { ...heading, qualifier: parentName(heading.workspace) }
      : heading
  ));
}

function parentName(workspace: string): string | null {
  const parts = workspace.replaceAll("\\", "/").split("/").filter(Boolean);
  return parts.length >= 2 ? parts[parts.length - 2] ?? null : null;
}

function group(
  key: string,
  name: string,
  workspace: string,
  kind: ProjectKind,
  current: boolean,
  rows: readonly Conversation[],
  pinned: boolean,
): ProjectGroup {
  return {
    key,
    name,
    workspace,
    kind,
    current,
    pinned,
    qualifier: null,
    rows,
    attention: rows.filter(needsYou).length,
    live: rows.filter((row) => row.live).length,
    holdsOpen: rows.some((row) => row.open),
  };
}

/// Whether a conversation has no project to file under: it runs in the scratch folder, or names no folder.
function intrinsicallyLoose(row: Conversation): boolean {
  return row.projectless || !row.homeWorkspace.trim();
}

/// The deepest open folder that covers a conversation, by identity, or null for none.
function openFolderOf(openWorkspaces: readonly string[], row: Conversation): string | null {
  if (!row.homeWorkspace.trim()) return null;
  let home: string | null = null;
  let homeLength = -1;
  for (const folder of openWorkspaces) {
    if (!workspaceCovers(folder, row.homeWorkspace)) continue;
    const identity = workspaceIdentity(folder);
    if (identity.length > homeLength) {
      home = identity;
      homeLength = identity.length;
    }
  }
  return home;
}

/// The created project a conversation belongs to, or null when nobody filed it anywhere.
///
/// Deepest folder wins when projects nest, because that is the one a person would call the conversation's home.
function projectOf(records: readonly ProjectRecord[], row: Conversation): ProjectRecord | null {
  if (!row.homeWorkspace.trim()) return null;
  let home: ProjectRecord | null = null;
  for (const record of records) {
    if (!workspaceCovers(record.workspace, row.homeWorkspace)) continue;
    if (!home || record.key.length > home.key.length) home = record;
  }
  return home;
}

/// The conversations that belong to no project: they run in the extension's scratch folder, or name no folder
/// at all. Their own section, beneath Projects (`docs/vscodeSurface.md`), in the order the rows already have.
///
/// A conversation discovered in a folder nobody added is NOT one of these. It has a project, that project is
/// simply not on this person's list, and showing it anyway is what made the sidebar a wall of other people's
/// work (operator, 2026-08-26: the standard is Paseo, the Claude app and the Codex app, where a folder you
/// never added is not on screen). Adding the folder is what brings its conversations in, all at once. The
/// consequence is worth stating plainly, because it is large: measured on this machine 2026-08-28, one service
/// alone held 176 conversations across 95 folders while two folders were on the list, so most of what exists is
/// deliberately not on screen until a folder is added.
///
/// Below the headings rather than above them, because a project is a place somebody chose and a loose
/// conversation is one they did not. No heading of their own: an earlier version filed them under "No project",
/// which turns an absence into a category and reads as a folder the person forgot about. Together `projects`
/// and this function split the list with nothing falling through and nothing drawn twice.
export function loose(rows: readonly Conversation[]): Conversation[] {
  return rows.filter(intrinsicallyLoose);
}

/// Pinned projects first in the order they were added, then the current window's project, then the other
/// projects by most recent conversation.
///
/// Deliberately blind to whether anything inside is running or waiting. Sorting on that would move a whole
/// heading, and everything under it, every time an agent started or finished a turn. Pinning is the person's
/// own placement, so it is stable above everything; the current folder is next because it is the work the
/// person opened this VS Code window to do.
function byPinnedThenMostRecent(
  records: readonly ProjectRecord[],
): (left: ProjectGroup, right: ProjectGroup) => number {
  const order = new Map(records.map((record, index) => [`project:${encodeURIComponent(record.key)}`, index]));
  return (left, right) => {
    if (left.pinned !== right.pinned) return left.pinned ? -1 : 1;
    if (left.pinned && right.pinned) return (order.get(left.key) ?? 0) - (order.get(right.key) ?? 0);
    return byMostRecentProject(left, right);
  };
}

function byMostRecentProject(left: ProjectGroup, right: ProjectGroup): number {
  if (left.current !== right.current) return left.current ? -1 : 1;
  const leftAt = latestActivity(left.rows);
  const rightAt = latestActivity(right.rows);
  if (leftAt !== rightAt) {
    if (leftAt === null) return 1;
    if (rightAt === null) return -1;
    return rightAt - leftAt;
  }
  return compare(left.name, right.name) || compare(left.key, right.key);
}

function latestActivity(rows: readonly Conversation[]): number | null {
  let latest: number | null = null;
  for (const row of rows) {
    if (row.updatedAtMs !== null && (latest === null || row.updatedAtMs > latest)) {
      latest = row.updatedAtMs;
    }
  }
  return latest;
}

/// The muted line beside a project heading.
export function projectDetail(group: ProjectGroup): string {
  const parts: string[] = [];
  if (group.qualifier) parts.push(`in ${group.qualifier}`);
  if (group.rows.length > 0) parts.push(String(group.rows.length));
  return parts.join(" · ");
}

/// A stable identity that is also legal as a tree element id.
///
/// Percent-encoded parts joined by a character encoding cannot produce. An earlier version separated them with NUL,
/// which reads fine as a map key and is not a legal element id: VS Code mangled it, could no longer resolve the
/// element, and every attempt to keep the open row selected rejected with "Cannot resolve tree item". Measured in
/// CI, fourteen rejections in one run against zero before, and the reveal retries showed up in session-switch time.
function conversationKey(providerId: string, nativeSessionId: string): string {
  return `chat:${encodeURIComponent(providerId)}:${encodeURIComponent(nativeSessionId)}`;
}

/// Provider-qualified identity for the cheap live-process roster.
export function nativeProcessKey(providerId: string, nativeSessionId: string): string {
  return `${encodeURIComponent(providerId)}:${encodeURIComponent(nativeSessionId)}`;
}

function supervised(
  session: SessionLine,
  native: NativeChatLine | null,
  key: string,
  providers: readonly ProviderLine[],
  selectedSessionId: string | null,
  projectlessRoot: string | null,
  activity: SessionActivity,
  isolatedWorkspaceHomes: ReadonlyMap<string, string>,
  pinned: boolean,
  name: string | undefined,
  legacyKey: string | null,
  providerWorking: boolean,
  hosted: TerminalDescriptor | null,
  observedExternal: boolean,
  attachableExternal: boolean,
  focusableExternal: boolean,
  unconfirmedOwner: boolean,
): Conversation {
  const homeWorkspace = isolatedWorkspaceHomes.get(workspaceIdentity(session.workspace)) ?? session.workspace;
  const projectless = isProjectless(homeWorkspace, projectlessRoot);
  // A cold supervised session is stored, and Runtrol itself can resume it, so it is openable.
  const presence: Presence = hosted
    ? { kind: "hosted", terminal: hosted }
    : observedExternal
      ? { kind: "external", openable: attachableExternal, focusable: focusableExternal }
      : unconfirmedOwner
        ? { kind: "unconfirmed" }
      : session.hot
        ? { kind: "supervised", session }
        : { kind: "stored", openable: true };
  return {
    key,
    legacyKey,
    providerId: session.providerId,
    serviceName: providerDisplayName(session.providerId, providers),
    serviceIcon: providerIcon(session.providerId, providers),
    title: name
      ?? (session.label?.trim()
        || providerTitle(native?.title, session.nativeSessionId || session.sessionId)),
    homeWorkspace,
    workspace: session.workspace,
    folder: projectless ? "" : workspaceName(homeWorkspace),
    projectless,
    updatedAtMs: instant(native?.updatedAt),
    activity: providerWorking ? "working" : activityOf(session),
    tool: activity.tool,
    signInNeeded: activity.signInNeeded,
    presence,
    ...facts(presence),
    open: session.sessionId === selectedSessionId,
    pinned,
    session,
    native,
    hostedTerminal: hosted,
    hostedKey: hosted ? terminalKey(hosted) : null,
  };
}

function providerOwned(
  chat: NativeChatLine,
  key: string,
  providers: readonly ProviderLine[],
  projectlessRoot: string | null,
  pinned: boolean,
  name: string | undefined,
  working: boolean,
  hosted: TerminalDescriptor | null,
  observedExternal: boolean,
  attachableExternal: boolean,
  focusableExternal: boolean,
  unconfirmedOwner: boolean,
): Conversation {
  const resumable = chat.resume === "available" && Boolean(chat.adoptionToken);
  const projectless = isProjectless(chat.cwd, projectlessRoot);
  const presence: Presence = hosted
    ? { kind: "hosted", terminal: hosted }
    : observedExternal
      ? { kind: "external", openable: attachableExternal, focusable: focusableExternal }
      : unconfirmedOwner
        ? { kind: "unconfirmed" }
      : { kind: "stored", openable: resumable };
  return {
    key,
    // A conversation the service already named has always been keyed by that name.
    legacyKey: null,
    providerId: chat.providerId,
    serviceName: providerDisplayName(chat.providerId, providers),
    serviceIcon: providerIcon(chat.providerId, providers),
    title: name ?? providerTitle(chat.title, chat.nativeSessionId),
    homeWorkspace: chat.cwd,
    workspace: chat.cwd,
    folder: projectless ? "" : workspaceName(chat.cwd),
    projectless,
    updatedAtMs: instant(chat.updatedAt),
    activity: working ? "working" : "saved",
    tool: null,
    signInNeeded: false,
    presence,
    ...facts(presence),
    open: false,
    pinned,
    session: null,
    native: chat,
    hostedTerminal: hosted,
    hostedKey: hosted ? terminalKey(hosted) : null,
  };
}

/// What a row says it is doing.
///
/// Managed-session lifecycle is used only when that provider session owns it. Provider-owned TUI conversations
/// take their working state from `activeNative` above. Terminal bytes are deliberately absent: an open, paused
/// TUI still paints, and `live` must never be confused with a model turn (operator, 2026-08-31).
function activityOf(session: SessionLine): ConversationActivity {
  // Only what the Runtime proved: a failed lifecycle. A silence-based "looks stuck" used to read as attention
  // here, and silence is not a state (a long tool call is quiet and not stuck); the Runtime no longer says it.
  if (session.lifecycle === "failed") return "attention";
  // Waiting outranks running, because a turn that stopped for a person is the one fact worth interrupting them
  // for. Runtime reports it only while a turn is actually running, so it can never outlive its turn.
  if (session.waitingOn === "person") return "needsYou";
  if (session.waitingOn === "quota") return "waitingOnQuota";
  if (session.lifecycle === "hotRunning") return "working";
  if (session.lifecycle === "hotIdle") return "ready";
  return "saved";
}

/// Whether this conversation has a proven live provider owner but no currently reachable terminal route.
///
/// The panel can see the service's own running processes, and a terminal it did not create has no attach
/// channel a public operating system call can take over. So the row says the conversation is alive and refuses
/// to open it, which is the truth rather than a tab that would fight the terminal already driving it.
export function runningElsewhere(row: Conversation): boolean {
  return row.presence.kind === "external" && !row.presence.openable;
}

/// Whether this conversation has stopped and cannot continue until the reader does something.
export function needsYou(row: Conversation): boolean {
  return row.activity === "needsYou" || row.activity === "attention";
}

/// How many conversations are waiting on the reader right now.
export function attentionCount(rows: readonly Conversation[]): number {
  return rows.filter(needsYou).length;
}

/// The next conversation that wants the reader, starting after the one they are looking at.
///
/// This is the whole orchestration primitive. Running six agents means five of them are busy and one has stopped
/// for you, and the useful question is never "show me a board" but "take me to the one that wants me". Cycling
/// from the open row rather than always returning the first means pressing it repeatedly walks every waiting
/// conversation instead of bouncing between two.
///
/// Returns null when nothing is waiting, which the caller says out loud rather than opening something arbitrary.
export function nextNeedingYou(
  rows: readonly Conversation[],
  openKey: string | null,
): Conversation | null {
  const waiting = rows.filter(needsYou);
  if (waiting.length === 0) return null;
  const openAt = openKey === null ? -1 : rows.findIndex((row) => row.key === openKey);
  if (openAt < 0) return waiting[0] ?? null;
  // Ordered by the list itself, so the walk follows what the reader sees.
  return waiting.find((row) => rows.indexOf(row) > openAt) ?? waiting[0] ?? null;
}

/// Whatever the coding service touched most recently first.
///
/// Turn state deliberately does not participate. Sorting on it would move rows under the pointer every time an
/// agent started or finished thinking, and it also put an old live process above a conversation touched moments
/// ago. A conversation list is chronological; state remains visible without rewriting that order.
function byMostRecentlyActive(left: Conversation, right: Conversation): number {
  if (left.updatedAtMs !== right.updatedAtMs) {
    if (left.updatedAtMs === null) return 1;
    if (right.updatedAtMs === null) return -1;
    return right.updatedAtMs - left.updatedAtMs;
  }
  return compare(left.folder, right.folder)
    || compare(left.title, right.title)
    || compare(left.key, right.key);
}

/// A provider's generic placeholder is the absence of a human title. Internal session identifiers are never
/// conversation names and must not leak into the sidebar as labels such as `Chat 8980`.
function providerTitle(title: string | null | undefined, _identity: string): string {
  const value = title?.trim();
  return value && value.toLocaleLowerCase("en-US") !== "untitled" ? value : "Unnamed conversation";
}

/// Conversation rows have no muted text.
///
/// The provider glyph identifies the coding service and spins while it runs. The title names the conversation.
/// Dates, service names and state words would only repeat those two visual facts.
export function conversationDetail(_row: Conversation, _nowMs: number, _grouped = false): string {
  return "";
}

/// The smallest complete state vocabulary for a conversation row.
export function conversationStatus(row: Conversation): string {
  if (!row.canOpen) return "Cannot reopen";
  if (row.signInNeeded) return "Sign in needed";
  switch (row.activity) {
    case "needsYou":
      return "Needs you";
    case "attention":
      return "Error";
    case "working":
      return "Running";
    case "waitingOnQuota":
      return "Limit";
    case "ready":
      return "Ready";
    case "saved":
      return "Stopped";
  }
}

/// Terse elapsed time, in the spelling a chat list uses.
export function elapsed(atMs: number | null, nowMs: number): string | null {
  if (atMs === null) return null;
  const seconds = Math.max(0, Math.round((nowMs - atMs) / 1_000));
  if (seconds < 60) return "now";
  const minutes = Math.round(seconds / 60);
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.round(minutes / 60);
  if (hours < 24) return `${hours}h`;
  const days = Math.round(hours / 24);
  return days < 7 ? `${days}d` : `${Math.round(days / 7)}w`;
}

/// A timestamp a coding service reported, in whichever way that service reports one.
///
/// The protocol asks a driver for the provider's own representation rather than a house format, so more than one
/// arrives here: Agent Client Protocol CLIs print ISO 8601, Claude Code prints milliseconds since the
/// epoch, and Codex prints seconds since the epoch (measured in the real window, 2026-08-20: read as
/// milliseconds, every Codex row said "2952w"). Reading only the first spelling left every row from the second
/// with no time at all, which pushed it below every dated row and stripped the elapsed part of its subtitle.
///
/// Reading all three is not interpretation. Each is an unambiguous machine format and none says anything about
/// what the conversation contains. Seconds and milliseconds are told apart by magnitude: no coding CLI existed
/// before 1973, so a bare number below a hundred billion can only be seconds.
function instant(value: string | null | undefined): number | null {
  if (!value) return null;
  if (/^\d+$/.test(value)) {
    const number = Number(value);
    if (!Number.isSafeInteger(number)) return null;
    return number < 100_000_000_000 ? number * 1_000 : number;
  }
  const parsed = Date.parse(value);
  return Number.isFinite(parsed) ? parsed : null;
}

function compare(left: string, right: string): number {
  return left < right ? -1 : left > right ? 1 : 0;
}

/// How far a service's own clock may sit behind this one before a row stops being recognised as the
/// conversation runtrol just started.
const SERVICE_CLOCK_SLACK_MS = 60_000;

/// Which placeholders the service has now named, and the conversation each one turned out to be.
///
/// A conversation started here has no name until its service writes one, so the list stands a placeholder in
/// its place and the tab is filed under that placeholder. The moment the service writes the row, two things
/// have to happen together: the list drops the placeholder, and the tab it belongs to moves onto the real
/// conversation. They were two separate judgements, so the tab kept the folder name for the rest of its life
/// even though the sidebar was already showing the real one (operator, 2026-08-28: the name the service gives
/// has to reach the tab). One answer, read by both.
///
/// The match is exact: the provider-owned row must carry the same Runtime generation and terminal id the fresh
/// tab opened. Provider, folder and timestamp are not identities. A different recent conversation in the same
/// repository can satisfy all three and used to rename a new Codex tab after an unrelated old chat.
export function namedPlaceholders(
  rows: readonly Conversation[],
  started: readonly StartedConversation[],
): ReadonlyMap<string, string> {
  const byTerminal = new Map<string, Conversation>();
  for (const row of rows) {
    if (row.hostedTerminal !== null) byTerminal.set(terminalKey(row.hostedTerminal), row);
  }
  const named = new Map<string, string>();
  for (const pending of started) {
    if (
      pending.runtimeGeneration === undefined
      || pending.terminalId === undefined
    ) continue;
    const row = byTerminal.get(terminalKey({
      runtimeGeneration: pending.runtimeGeneration,
      terminalId: pending.terminalId,
    }));
    if (row === undefined) continue;
    named.set(pending.id, row.key);
  }
  return named;
}

/// The row for a conversation runtrol opened and the service has not described yet.
///
/// It carries no service record, so nothing downstream can offer to resume, rename or delete it: those all need
/// the identity the service has not published. It is a place in the list, honest about being new.
function startedRow(
  pending: StartedConversation,
  providers: readonly ProviderLine[],
  projectlessRoot: string | null,
): Conversation {
  const projectless = isProjectless(pending.workspace, projectlessRoot);
  return {
    key: `started:${encodeURIComponent(pending.id)}`,
    legacyKey: null,
    providerId: pending.providerId,
    serviceName: providerDisplayName(pending.providerId, providers),
    serviceIcon: providerIcon(pending.providerId, providers),
    title: pending.title,
    homeWorkspace: pending.workspace,
    workspace: pending.workspace,
    folder: projectless ? "" : workspaceName(pending.workspace),
    projectless,
    updatedAtMs: null,
    // Starting is not working: the service has not been asked anything yet, so claiming it is busy would put a
    // spinner on a conversation nobody has spoken to.
    activity: "ready",
    tool: null,
    signInNeeded: false,
    presence: { kind: "starting" },
    ...facts({ kind: "starting" }),
    open: true,
    pinned: false,
    session: null,
    native: null,
    hostedTerminal: null,
    hostedKey: null,
  };
}

/// A daemon-owned process whose provider conversation identity has not been published yet.
function hostedRow(
  terminal: TerminalDescriptor,
  chat: NativeChatLine | null,
  providers: readonly ProviderLine[],
  projectlessRoot: string | null,
  /// Whether the service says a model is answering in this conversation right now, from its process roster.
  working: boolean,
): Conversation {
  const key = terminalKey(terminal);
  const projectless = isProjectless(terminal.workspace, projectlessRoot);
  return {
    key,
    legacyKey: null,
    providerId: terminal.providerId,
    serviceName: providerDisplayName(terminal.providerId, providers),
    serviceIcon: providerIcon(terminal.providerId, providers),
    title: chat
      ? providerTitle(chat.title, chat.nativeSessionId)
      : workspaceName(terminal.workspace) || "New conversation",
    homeWorkspace: terminal.workspace,
    workspace: terminal.workspace,
    folder: projectless ? "" : workspaceName(terminal.workspace),
    projectless,
    updatedAtMs: terminal.openedAtMs,
    // A hosted terminal turns when the service says it is answering. It used to be fixed at "ready", so a
    // conversation running in a terminal Runtrol hosts but this window is not viewing never showed it was
    // working (operator, 2026-08-29: a running session read as not running).
    activity: working ? "working" : "ready",
    tool: null,
    signInNeeded: false,
    presence: { kind: "hosted", terminal },
    ...facts({ kind: "hosted", terminal }),
    open: false,
    pinned: false,
    session: null,
    native: chat,
    hostedTerminal: terminal,
    hostedKey: key,
  };
}

function terminalKey(terminal: Pick<TerminalDescriptor, "runtimeGeneration" | "terminalId">): string {
  return `terminal:${encodeURIComponent(terminal.runtimeGeneration)}:${encodeURIComponent(terminal.terminalId)}`;
}

function startedCoversTerminal(pending: StartedConversation, terminal: TerminalDescriptor): boolean {
  if (pending.runtimeGeneration !== undefined && pending.terminalId !== undefined) {
    return terminal.runtimeGeneration === pending.runtimeGeneration
      && terminal.terminalId === pending.terminalId;
  }
  return pending.providerId === terminal.providerId
    && workspaceIdentity(pending.workspace) === workspaceIdentity(terminal.workspace)
    && terminal.openedAtMs >= pending.startedAtMs - SERVICE_CLOCK_SLACK_MS;
}
