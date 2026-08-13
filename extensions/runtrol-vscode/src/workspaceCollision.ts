import path from "node:path";

import type { SessionLine } from "./runtimeTypes";

export type WorkspaceRelation = "same" | "candidateContainsSession" | "sessionContainsCandidate";

export type WorkspaceCollision = {
  session: SessionLine;
  relation: WorkspaceRelation;
};

export function workspaceCollisions(
  candidate: string,
  sessions: readonly SessionLine[],
  platform: NodeJS.Platform = process.platform,
): WorkspaceCollision[] {
  const paths = platform === "win32" ? path.win32 : path.posix;
  const expected = normalize(candidate, paths, platform);
  const collisions: WorkspaceCollision[] = [];
  for (const session of sessions) {
    if (!session.hot) {
      continue;
    }
    const active = normalize(session.workspace, paths, platform);
    const relation = workspaceRelation(expected, active, paths);
    if (relation) {
      collisions.push({ session, relation });
    }
  }
  return collisions;
}

function workspaceRelation(
  candidate: string,
  active: string,
  paths: typeof path.posix | typeof path.win32,
): WorkspaceRelation | null {
  if (candidate === active) {
    return "same";
  }
  if (contains(candidate, active, paths)) {
    return "candidateContainsSession";
  }
  if (contains(active, candidate, paths)) {
    return "sessionContainsCandidate";
  }
  return null;
}

function contains(parent: string, child: string, paths: typeof path.posix | typeof path.win32): boolean {
  const relative = paths.relative(parent, child);
  return Boolean(relative)
    && relative !== ".."
    && !relative.startsWith(`..${paths.sep}`)
    && !paths.isAbsolute(relative);
}

function normalize(
  value: string,
  paths: typeof path.posix | typeof path.win32,
  platform: NodeJS.Platform,
): string {
  const resolved = paths.resolve(value);
  return platform === "win32" ? resolved.toLocaleLowerCase("en-US") : resolved;
}
