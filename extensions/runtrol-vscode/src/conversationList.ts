import { isProjectless } from "./projectlessWorkspace";
import type { ProjectRecord } from "./projects";
import type { NativeChatLine, ProviderLine, SessionLine } from "./runtimeTypes";
import { providerDisplayName, providerIcon, workspaceName } from "./sessionDisplay";
import { workspaceCovers, workspaceIdentity } from "./workspaceCollision";

/// What a conversation is doing, said the way a person would say it.
///
/// Ordered by how much it wants from the reader. `needsYou` is the only one that is actually urgent, and keeping
/// it a separate value from `attention` is deliberate: something that broke and something that is politely
/// waiting are different errands, and a surface that merged them would send the reader to the wrong one.
import { NO_ACTIVITY, type SessionActivity } from "./sessionActivity";

export type ConversationActivity =
  | "needsYou"
  | "attention"
  | "working"
  | "waitingOnQuota"
  | "ready"
  | "saved";

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
  /// Whether a provider process is currently supervising it.
  readonly live: boolean;
  readonly open: boolean;
  readonly session: SessionLine | null;
  readonly native: NativeChatLine | null;
  readonly canOpen: boolean;
  /// Why it cannot be opened, for the one row where that is true.
  readonly blocked: string | null;
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
): Conversation[] {
  const nativeByKey = new Map<string, NativeChatLine>();
  for (const chat of nativeChats) {
    nativeByKey.set(conversationKey(chat.providerId, chat.nativeSessionId), chat);
  }

  const rows: Conversation[] = [];
  const claimed = new Set<string>();
  for (const session of sessions) {
    const key = session.nativeSessionId
      ? conversationKey(session.providerId, session.nativeSessionId)
      : `session:${encodeURIComponent(session.sessionId)}`;
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
    ));
  }
  for (const [key, chat] of nativeByKey) {
    // A chat the daemon already supervises is the same conversation, not a second one. The service half only
    // contributes its title and timestamp, which the supervised row above has already taken.
    if (claimed.has(key) || chat.alreadyManagedAs) continue;
    rows.push(providerOwned(chat, key, providers, projectlessRoot));
  }
  return rows.sort(byMostRecentlyActive).map(disambiguated(rows));
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
  /// Why this heading exists: the operator created it, this window is open on it, or a coding service
  /// reported conversations in it. The tree offers different actions for each (a created project can be
  /// renamed and removed; a discovered folder can be made a project), and the rule that one heading is
  /// drawn per place does not depend on which kind won.
  readonly kind: ProjectKind;
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

export type ProjectKind = "created" | "open" | "discovered";

/// Conversations gathered under this machine's projects: the ones the operator created, the folders this
/// window has open, and every other folder a coding service reports conversations in.
///
/// **The panel shows the whole machine's projects** (operator contract, `memory/uxContract.md`, restated
/// 2026-08-20 against the Paseo, Codex and Claude sidebars: folder = project heading, conversations beneath
/// it). A folder a coding service ran in is a project the person worked on, and the CLI's own listing is the
/// authority on which folder a conversation belongs to (thin principle: the service files it, runtrol shows
/// it). So a folder holding conversations is a heading whether or not anybody created or opened it here.
/// What is still never invented is a heading with nothing in it: a discovered folder exists only while a
/// conversation names it, so a machine with no conversations shows no wall of empty folder names (the
/// 2026-08-19 regression was thirty empty auto-headings, and this rule keeps that impossible).
///
/// One heading per place. A created project wins over an open folder covering the same conversation, and
/// either wins over a discovered folder, because creation and opening are the more deliberate acts and one row
/// must never appear twice. A conversation inside nested created projects files under the deepest one, which
/// is the folder a person would call its home. A created project with nothing in it yet is still returned: it
/// was made a moment ago and a heading that vanished would read as the creation failing.
///
/// Conversations without a project (the scratch folder, or no folder at all) are deliberately absent here:
/// they are the plain rows `loose` returns, beneath the headings.
export function projects(
  records: readonly ProjectRecord[],
  rows: readonly Conversation[],
  openWorkspaces: readonly string[],
): ProjectGroup[] {
  const filed = new Map<string, Conversation[]>(records.map((record) => [record.key, []]));
  for (const row of rows) {
    if (unfiled(row)) continue;
    const home = projectOf(records, row);
    if (home) filed.get(home.key)?.push(row);
  }
  const groups: ProjectGroup[] = records.map((record) => group(
    `project:${encodeURIComponent(record.key)}`,
    record.name,
    record.workspace,
    "created",
    openWorkspaces.some((folder) =>
      workspaceCovers(record.workspace, folder) || workspaceCovers(folder, record.workspace)),
    filed.get(record.key) ?? [],
  ));
  const seen = new Set<string>();
  for (const folder of openWorkspaces) {
    if (!folder.trim()) continue;
    const identity = workspaceIdentity(folder);
    if (seen.has(identity)) continue;
    seen.add(identity);
    // A created project already standing for this place draws the one heading; either direction of cover
    // counts, so a project inside the open folder or around it never doubles up.
    const represented = records.some((record) =>
      workspaceCovers(record.workspace, folder) || workspaceCovers(folder, record.workspace));
    if (represented) continue;
    const folderRows = rows.filter((row) =>
      !unfiled(row) && !projectOf(records, row) && openFolderOf(openWorkspaces, row) === identity);
    // The current folder is already named by the VS Code window. Drawing it as an empty project row makes
    // every machine-wide project that follows look nested beneath it, even though the rows are siblings.
    // An open folder becomes a heading when it has a conversation to contain; an explicitly created project
    // still remains visible while empty through the record path above.
    if (folderRows.length === 0) continue;
    groups.push(group(
      `folder:${encodeURIComponent(identity)}`,
      workspaceName(folder) || folder,
      folder,
      "open",
      true,
      folderRows,
    ));
  }
  // Every other folder a conversation names, exactly as the service spelled it. Grouped by identity so one
  // folder reached by two casings is one heading; the first spelling seen is the one shown.
  const discovered = new Map<string, { workspace: string; rows: Conversation[] }>();
  for (const row of rows) {
    if (unfiled(row) || projectOf(records, row) || openFolderOf(openWorkspaces, row) !== null) continue;
    const identity = workspaceIdentity(row.homeWorkspace);
    const place = discovered.get(identity) ?? { workspace: row.homeWorkspace, rows: [] };
    place.rows.push(row);
    discovered.set(identity, place);
  }
  for (const [identity, place] of discovered) {
    groups.push(group(
      `discovered:${encodeURIComponent(identity)}`,
      workspaceName(place.workspace) || place.workspace,
      place.workspace,
      "discovered",
      false,
      place.rows,
    ));
  }
  return qualified(groups).sort(byNearestToTheReader);
}

/// Headings that share a name get their parent folder's name beside it.
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
): ProjectGroup {
  return {
    key,
    name,
    workspace,
    kind,
    current,
    qualifier: null,
    rows,
    attention: rows.filter(needsYou).length,
    live: rows.filter((row) => row.live).length,
    holdsOpen: rows.some((row) => row.open),
  };
}

/// Whether a conversation has no project to file under: it runs in the scratch folder, or names no folder.
function unfiled(row: Conversation): boolean {
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

/// The conversations that belong to no project, in the order the rows already have.
///
/// Exactly the ones started without a project (they run in the scratch folder) and the ones a service reported
/// with no folder at all. They sit at the top level beneath the project headings, not inside one. An earlier
/// version filed them under a heading called "No project", which turns an absence into a category and reads as
/// a folder the person forgot about. The chat apps people already use do not do that: a project is a place you
/// can put a conversation, and a conversation you did not put anywhere is simply a conversation.
///
/// Below the headings rather than above them, because a project is a place somebody chose and a loose
/// conversation is one they did not. Every conversation with a real folder is under a heading (`projects`), so
/// together the two functions split the list with nothing falling through and nothing drawn twice.
export function loose(rows: readonly Conversation[]): Conversation[] {
  return rows.filter(unfiled);
}

/// This window's project first, then whatever was touched most recently.
///
/// Deliberately blind to whether anything inside is running or waiting. Sorting on that would move a whole
/// heading, and everything under it, every time an agent started or finished a turn. The count in the
/// heading says who wants attention; the position says where the reader left it.
function byNearestToTheReader(left: ProjectGroup, right: ProjectGroup): number {
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
  if (group.rows.length === 0) return [...parts, "empty"].join(" · ");
  if (group.attention > 0) parts.push(`${group.attention} need you`);
  if (group.live > 0) parts.push(`${group.live} running`);
  parts.push(String(group.rows.length));
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

function supervised(
  session: SessionLine,
  native: NativeChatLine | null,
  key: string,
  providers: readonly ProviderLine[],
  selectedSessionId: string | null,
  projectlessRoot: string | null,
  activity: SessionActivity,
  isolatedWorkspaceHomes: ReadonlyMap<string, string>,
): Conversation {
  const homeWorkspace = isolatedWorkspaceHomes.get(workspaceIdentity(session.workspace)) ?? session.workspace;
  const projectless = isProjectless(homeWorkspace, projectlessRoot);
  return {
    key,
    providerId: session.providerId,
    serviceName: providerDisplayName(session.providerId, providers),
    serviceIcon: providerIcon(session.providerId, providers),
    title: session.label?.trim()
      || native?.title?.trim()
      || untitled(session.nativeSessionId || session.sessionId),
    homeWorkspace,
    workspace: session.workspace,
    folder: projectless ? "" : workspaceName(homeWorkspace),
    projectless,
    updatedAtMs: instant(native?.updatedAt),
    activity: activityOf(session),
    tool: activity.tool,
    signInNeeded: activity.signInNeeded,
    live: session.hot,
    open: session.sessionId === selectedSessionId,
    session,
    native,
    canOpen: true,
    blocked: null,
  };
}

function providerOwned(
  chat: NativeChatLine,
  key: string,
  providers: readonly ProviderLine[],
  projectlessRoot: string | null,
): Conversation {
  const resumable = chat.resume === "available" && Boolean(chat.adoptionToken);
  const projectless = isProjectless(chat.cwd, projectlessRoot);
  return {
    key,
    providerId: chat.providerId,
    serviceName: providerDisplayName(chat.providerId, providers),
    serviceIcon: providerIcon(chat.providerId, providers),
    title: chat.title?.trim()
      || untitled(chat.nativeSessionId),
    homeWorkspace: chat.cwd,
    workspace: chat.cwd,
    folder: projectless ? "" : workspaceName(chat.cwd),
    projectless,
    updatedAtMs: instant(chat.updatedAt),
    activity: "saved",
    tool: null,
    signInNeeded: false,
    live: false,
    open: false,
    session: null,
    native: chat,
    canOpen: resumable,
    blocked: resumable ? null : "This coding service cannot reopen this conversation.",
  };
}

function activityOf(session: SessionLine): ConversationActivity {
  if (session.lifecycle === "failed" || session.looksStuck) return "attention";
  // Waiting outranks running, because a turn that stopped for a person is the one fact worth interrupting them
  // for. Runtime reports it only while a turn is actually running, so it can never outlive its turn.
  if (session.waitingOn === "person") return "needsYou";
  if (session.waitingOn === "quota") return "waitingOnQuota";
  if (session.lifecycle === "hotRunning") return "working";
  if (session.lifecycle === "hotIdle") return "ready";
  return "saved";
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

/// Live conversations first, then whatever the coding service touched most recently.
///
/// Turn state deliberately does not participate. Sorting on it would move rows under the pointer every time an
/// agent started or finished thinking, and a list that rearranges itself while being read is not a list.
function byMostRecentlyActive(left: Conversation, right: Conversation): number {
  if (left.live !== right.live) return left.live ? -1 : 1;
  if (left.updatedAtMs !== right.updatedAtMs) {
    if (left.updatedAtMs === null) return 1;
    if (right.updatedAtMs === null) return -1;
    return right.updatedAtMs - left.updatedAtMs;
  }
  return compare(left.folder, right.folder)
    || compare(left.title, right.title)
    || compare(left.key, right.key);
}

/// Two conversations that read identically are two rows a person cannot tell apart.
function disambiguated(rows: readonly Conversation[]): (row: Conversation) => Conversation {
  const seen = new Map<string, number>();
  for (const row of rows) {
    const name = `${row.title}\0${row.serviceName}\0${row.folder}`;
    seen.set(name, (seen.get(name) ?? 0) + 1);
  }
  return (row) => {
    const name = `${row.title}\0${row.serviceName}\0${row.folder}`;
    return (seen.get(name) ?? 0) > 1
      ? { ...row, title: `${row.title} · ${shortIdentity(row)}` }
      : row;
  };
}

function shortIdentity(row: Conversation): string {
  return shortened(row.native?.nativeSessionId || row.session?.sessionId || row.key);
}

/// The last four usable characters of an identifier, which is what tells two otherwise identical rows apart.
function shortened(identity: string): string {
  const compact = identity.replaceAll(/[^A-Za-z0-9]/gu, "");
  return (compact.slice(-4) || identity.slice(-4)).toUpperCase();
}

/// What to call a conversation that the service did not name.
function untitled(identity: string): string {
  return `Untitled · ${shortened(identity)}`;
}

/// The muted second line: current state and the service's timestamp, with no repeated provider or project name.
export function conversationDetail(row: Conversation, nowMs: number, _grouped = false): string {
  return [
    conversationStatus(row),
    elapsed(row.updatedAtMs, nowMs),
  ].filter((part): part is string => Boolean(part)).join(" · ");
}

/// The smallest complete state vocabulary for a conversation row.
export function conversationStatus(row: Conversation): string {
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
/// arrives here: the Agent Client Protocol and cline print ISO 8601, Claude Code prints milliseconds since the
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
