import path from "node:path";

import type { SessionLine } from "./protocol";

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
      || ordinalCompare(left.provider, right.provider)
      || ordinalCompare(left.session, right.session);
  });
}

export function sessionChoices(
  sessions: readonly SessionLine[],
  selected: string | null,
): SessionChoice[] {
  return orderedSessions(sessions, selected).map((session) => ({
    label: `${icon(session, selected)} ${folderName(session)}`,
    description: `${session.provider} | ${session.doing}`,
    detail: session.workspace,
    picked: session.session === selected,
    session,
  }));
}

function sessionRank(session: SessionLine, selected: string | null): number {
  if (session.session === selected) {
    return 0;
  }
  if (session.looks_stuck) {
    return 1;
  }
  return session.hot ? 2 : 3;
}

function icon(session: SessionLine, selected: string | null): string {
  if (session.session === selected) {
    return "$(check)";
  }
  if (session.looks_stuck) {
    return "$(warning)";
  }
  return session.hot ? "$(circle-filled)" : "$(circle-outline)";
}

function folderName(session: SessionLine): string {
  return path.basename(session.workspace) || session.workspace;
}

function ordinalCompare(left: string, right: string): number {
  return left < right ? -1 : left > right ? 1 : 0;
}
