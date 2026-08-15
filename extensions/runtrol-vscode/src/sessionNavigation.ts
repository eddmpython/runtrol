import type { NativeChatLine, ProviderLine, SessionLine } from "./runtimeTypes";
import { sessionStateLabel } from "./runtimeProjection";
import { providerDisplayName, sessionContext, uniqueSessionTitle, workspaceName } from "./sessionDisplay";

export type SessionChoice = {
  label: string;
  description: string;
  detail: string;
  picked: boolean;
  session: SessionLine;
};

export type ChatService = {
  providerId: string;
  displayName: string;
  provider: ProviderLine | null;
  sessions: readonly SessionLine[];
  nativeChats: readonly NativeChatLine[];
  selected: boolean;
};

export function orderedSessions(
  sessions: readonly SessionLine[],
  selected: string | null,
): SessionLine[] {
  return sessions.slice().sort((left, right) => {
    const rank = sessionRank(left, selected) - sessionRank(right, selected);
    if (rank !== 0) {
      return rank;
    }
    return ordinalCompare(folderName(left), folderName(right))
      || ordinalCompare(left.workspace, right.workspace)
      || ordinalCompare(left.providerId, right.providerId)
      || ordinalCompare(left.sessionId, right.sessionId);
  });
}

export function sessionChoices(
  sessions: readonly SessionLine[],
  selected: string | null,
  providers: readonly ProviderLine[] = [],
): SessionChoice[] {
  return orderedSessions(sessions, selected).map((session) => ({
    label: `${icon(session, selected)} ${uniqueSessionTitle(session, sessions, providers)}`,
    description: sessionStateLabel(session),
    detail: `${sessionContext(session, providers)} · ${session.workspace}`,
    picked: session.sessionId === selected,
    session,
  }));
}

export function chatServices(
  sessions: readonly SessionLine[],
  providers: readonly ProviderLine[],
  selectedSessionId: string | null,
  nativeChats: readonly NativeChatLine[] = [],
): ChatService[] {
  const byProvider = new Map<string, SessionLine[]>();
  for (const session of sessions) {
    const existing = byProvider.get(session.providerId) ?? [];
    existing.push(session);
    byProvider.set(session.providerId, existing);
  }
  const managedNative = new Set(sessions.flatMap((session) => (
    session.nativeSessionId ? [`${session.providerId}\0${session.nativeSessionId}`] : []
  )));
  const nativeByProvider = new Map<string, NativeChatLine[]>();
  for (const chat of nativeChats) {
    if (chat.alreadyManagedAs || managedNative.has(`${chat.providerId}\0${chat.nativeSessionId}`)) {
      continue;
    }
    const existing = nativeByProvider.get(chat.providerId) ?? [];
    existing.push(chat);
    nativeByProvider.set(chat.providerId, existing);
  }

  const services = providers.map((provider) => chatService(
    provider.providerId,
    provider.displayName,
    provider,
    byProvider.get(provider.providerId) ?? [],
    nativeByProvider.get(provider.providerId) ?? [],
    selectedSessionId,
  ));
  const known = new Set(providers.map((provider) => provider.providerId));
  const unknown = new Set([...byProvider.keys(), ...nativeByProvider.keys()]);
  for (const providerId of [...unknown].filter((id) => !known.has(id)).sort()) {
    services.push(chatService(
      providerId,
      providerDisplayName(providerId),
      null,
      byProvider.get(providerId) ?? [],
      nativeByProvider.get(providerId) ?? [],
      selectedSessionId,
    ));
  }
  return services;
}

function chatService(
  providerId: string,
  displayName: string,
  provider: ProviderLine | null,
  sessions: readonly SessionLine[],
  nativeChats: readonly NativeChatLine[],
  selectedSessionId: string | null,
): ChatService {
  return {
    providerId,
    displayName,
    provider,
    sessions: orderedSessions(sessions, selectedSessionId),
    nativeChats: orderedNativeChats(nativeChats),
    selected: sessions.some((session) => session.sessionId === selectedSessionId),
  };
}

function orderedNativeChats(chats: readonly NativeChatLine[]): NativeChatLine[] {
  return chats.slice().sort((left, right) => (
    ordinalCompare(right.updatedAt ?? "", left.updatedAt ?? "")
    || ordinalCompare(left.title ?? "", right.title ?? "")
    || ordinalCompare(left.cwd, right.cwd)
    || ordinalCompare(left.nativeSessionId, right.nativeSessionId)
  ));
}

function sessionRank(session: SessionLine, selected: string | null): number {
  if (session.sessionId === selected) {
    return 0;
  }
  if (session.looksStuck) {
    return 1;
  }
  return session.hot ? 2 : 3;
}

function icon(session: SessionLine, selected: string | null): string {
  if (session.sessionId === selected) {
    return "$(check)";
  }
  if (session.looksStuck) {
    return "$(warning)";
  }
  return session.hot ? "$(circle-filled)" : "$(circle-outline)";
}

function folderName(session: SessionLine): string {
  return workspaceName(session.workspace);
}

function ordinalCompare(left: string, right: string): number {
  return left < right ? -1 : left > right ? 1 : 0;
}
