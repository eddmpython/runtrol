import type { NativeChatLine, ProviderLine, SessionLine } from "./runtimeTypes";
import { providerDisplayName, workspaceName } from "./sessionDisplay";

/// What a conversation is doing, said the way a person would say it.
export type ConversationActivity = "working" | "ready" | "attention" | "saved";

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
      : `session\0${session.sessionId}`;
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

function conversationKey(providerId: string, nativeSessionId: string): string {
  return `chat\0${providerId}\0${nativeSessionId}`;
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
  if (session.lifecycle === "hotRunning") return "working";
  if (session.lifecycle === "hotIdle") return "ready";
  return "saved";
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
