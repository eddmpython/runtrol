import path from "node:path";

import type { SessionLine } from "./runtimeTypes";

export type WorkspaceRelation = "same" | "candidateContainsSession" | "sessionContainsCandidate";

export type WorkspaceCollision = {
  session: SessionLine;
  relation: WorkspaceRelation;
};

/// The collisions that cannot be made safe by quietly cooling an idle provider process.
export function workingCollisions(
  collisions: readonly WorkspaceCollision[],
): WorkspaceCollision[] {
  return collisions.filter(({ session }) => session.lifecycle === "hotRunning");
}

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

/// The one value that decides whether two paths are the same project.
///
/// Exported because grouping conversations by project asks exactly the question collision detection asks,
/// and two answers to it would disagree on the case a person is most likely to hit: the same folder
/// reached by a different casing or separator on Windows, which would split one project into two headings
/// while collision detection still treated them as one.
export function workspaceIdentity(
  value: string,
  paths: typeof path.posix | typeof path.win32 = path,
  platform: NodeJS.Platform = process.platform,
): string {
  const resolved = paths.resolve(value);
  return platform === "win32" ? resolved.toLocaleLowerCase("en-US") : resolved;
}

/// Whether the identity `candidate` is `base` itself or sits anywhere inside it.
///
/// Both arguments must already be identities. The one containment predicate: root following, project grouping
/// and collision detection all answer "is this folder inside that one" through this line, so they can never
/// disagree about it.
export function identityCovers(base: string, candidate: string, separator: string = path.sep): boolean {
  return candidate === base || candidate.startsWith(base + separator);
}

/// Whether `folder` is `root` itself or sits anywhere inside it, as paths a person wrote.
export function workspaceCovers(
  root: string,
  folder: string,
  paths: typeof path.posix | typeof path.win32 = path,
  platform: NodeJS.Platform = process.platform,
): boolean {
  return identityCovers(
    workspaceIdentity(root, paths, platform),
    workspaceIdentity(folder, paths, platform),
    paths.sep,
  );
}

function normalize(
  value: string,
  paths: typeof path.posix | typeof path.win32,
  platform: NodeJS.Platform,
): string {
  return workspaceIdentity(value, paths, platform);
}
