import type { ProviderLine, SessionLine } from "./runtimeTypes";
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
): ChatService[] {
  const byProvider = new Map<string, SessionLine[]>();
  for (const session of sessions) {
    const existing = byProvider.get(session.providerId) ?? [];
    existing.push(session);
    byProvider.set(session.providerId, existing);
  }

  const services = providers.map((provider) => chatService(
    provider.providerId,
    provider.displayName,
    provider,
    byProvider.get(provider.providerId) ?? [],
    selectedSessionId,
  ));
  const known = new Set(providers.map((provider) => provider.providerId));
  for (const providerId of [...byProvider.keys()].filter((id) => !known.has(id)).sort()) {
    services.push(chatService(
      providerId,
      providerDisplayName(providerId),
      null,
      byProvider.get(providerId) ?? [],
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
  selectedSessionId: string | null,
): ChatService {
  return {
    providerId,
    displayName,
    provider,
    sessions: orderedSessions(sessions, selectedSessionId),
    selected: sessions.some((session) => session.sessionId === selectedSessionId),
  };
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
