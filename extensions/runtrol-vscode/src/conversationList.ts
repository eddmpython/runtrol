import type { NativeChatLine, ProviderLine, SessionLine } from "./runtimeTypes";
import { providerDisplayName, workspaceName } from "./sessionDisplay";

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
  readonly title: string;
  readonly workspace: string;
  readonly folder: string;
  /// When the coding service last touched it, when the service reports that at all.
  readonly updatedAtMs: number | null;
  readonly activity: ConversationActivity;
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
export function conversations(
  sessions: readonly SessionLine[],
  providers: readonly ProviderLine[],
  nativeChats: readonly NativeChatLine[],
  selectedSessionId: string | null,
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
    rows.push(supervised(session, nativeByKey.get(key) ?? null, key, providers, selectedSessionId));
  }
  for (const [key, chat] of nativeByKey) {
    // A chat the daemon already supervises is the same conversation, not a second one. The service half only
    // contributes its title and timestamp, which the supervised row above has already taken.
    if (claimed.has(key) || chat.alreadyManagedAs) continue;
    rows.push(providerOwned(chat, key, providers));
  }
  return rows.sort(byMostRecentlyActive).map(disambiguated(rows));
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
): Conversation {
  return {
    key,
    providerId: session.providerId,
    serviceName: providerDisplayName(session.providerId, providers),
    title: session.label?.trim() || native?.title?.trim() || workspaceName(session.workspace),
    workspace: session.workspace,
    folder: workspaceName(session.workspace),
    updatedAtMs: instant(native?.updatedAt),
    activity: activityOf(session),
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
): Conversation {
  const resumable = chat.resume === "available" && Boolean(chat.adoptionToken);
  return {
    key,
    providerId: chat.providerId,
    serviceName: providerDisplayName(chat.providerId, providers),
    title: chat.title?.trim() || workspaceName(chat.cwd),
    workspace: chat.cwd,
    folder: workspaceName(chat.cwd),
    updatedAtMs: instant(chat.updatedAt),
    activity: "saved",
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
  const identity = row.native?.nativeSessionId || row.session?.sessionId || row.key;
  const compact = identity.replaceAll(/[^A-Za-z0-9]/gu, "");
  return (compact.slice(-4) || identity.slice(-4)).toUpperCase();
}

/// The muted second line: only the facts that separate this row from its neighbours.
export function conversationDetail(row: Conversation, nowMs: number): string {
  return [
    row.folder === row.title ? null : row.folder,
    row.serviceName,
    elapsed(row.updatedAtMs, nowMs),
  ].filter((part): part is string => Boolean(part)).join(" · ");
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

function instant(value: string | null | undefined): number | null {
  if (!value) return null;
  const parsed = Date.parse(value);
  return Number.isFinite(parsed) ? parsed : null;
}

function compare(left: string, right: string): number {
  return left < right ? -1 : left > right ? 1 : 0;
}
