import type { MissionSnapshot, MissionTaskLine } from "../protocol";

export type MissionLandingArtifact = {
  readonly path: string;
  readonly task: MissionTaskLine;
};

export type MissionLanding = {
  readonly snapshot: MissionSnapshot;
  readonly artifacts: readonly MissionLandingArtifact[];
};

const MAX_LANDING_ARTIFACTS = 1_024;

/// Build the exact project-side review named by one ordinary Mission's passing Receipts.
///
/// Every Artifact target appears once. Missing workspace or Receipt evidence refuses the whole review instead of
/// opening a partial diff that could look complete. The specialized choose-one policy stays on Fleet Compare.
export function missionLanding(
  snapshot: MissionSnapshot,
): MissionLanding | null {
  if (snapshot.mission.state !== "integrating" || snapshot.mission.completion_policy !== "allTasks") return null;

  const artifacts: MissionLandingArtifact[] = [];
  const targets = new Set<string>();
  for (const task of snapshot.tasks) {
    if (task.state !== "passed" || !task.workspace || !task.receipt_id || task.artifact_paths.length === 0) {
      return null;
    }
    for (const artifactPath of task.artifact_paths) {
      const target = artifactPath.toLowerCase();
      if (!safeArtifactPath(artifactPath) || targets.has(target)) return null;
      if (artifacts.length >= MAX_LANDING_ARTIFACTS) return null;
      targets.add(target);
      artifacts.push({ path: artifactPath, task });
    }
  }
  if (artifacts.length === 0) return null;
  artifacts.sort((left, right) => left.path.localeCompare(right.path));
  return { snapshot, artifacts };
}

export function missionLandingQueue(snapshots: readonly MissionSnapshot[]): MissionLanding[] {
  const ready: MissionLanding[] = [];
  for (const snapshot of snapshots) {
    const result = missionLanding(snapshot);
    if (result) ready.push(result);
  }
  ready.sort((left, right) =>
    left.snapshot.mission.project.localeCompare(right.snapshot.mission.project)
      || left.snapshot.mission.name.localeCompare(right.snapshot.mission.name)
      || left.snapshot.mission.mission_id.localeCompare(right.snapshot.mission.mission_id)
  );
  return ready;
}

export function safeArtifactPath(value: string): boolean {
  return value.length > 0
    && !value.startsWith("/")
    && !value.startsWith("\\")
    && !value.includes("\\")
    && !value.includes(":")
    && value.split("/").every((part) => part.length > 0 && part !== "." && part !== "..");
}
