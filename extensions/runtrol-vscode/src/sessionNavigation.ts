import type { ProviderLine, SessionLine } from "./runtimeTypes";
import { sessionStateLabel } from "./runtimeProjection";
import { sessionContext, uniqueSessionTitle, workspaceName } from "./sessionDisplay";

export type SessionChoice = {
  label: string;
  description: string;
  detail: string;
  picked: boolean;
  session: SessionLine;
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
