import type { MissionSnapshot, MissionTaskLine } from "../../protocol";

export type MissionLandingArtifact = {
  readonly path: string;
  readonly task: MissionTaskLine;
  readonly evidence: NonNullable<MissionTaskLine["artifacts"]>[number];
};

export type LandingSelection =
  | { readonly kind: "allTasks" }
  | { readonly kind: "chooseOne"; readonly taskId: string };

export type MissionLanding = {
  readonly snapshot: MissionSnapshot;
  readonly selection: LandingSelection;
  readonly artifacts: readonly MissionLandingArtifact[];
};

export type LandingByteEvidence = {
  readonly path: string;
  readonly sourceBytes: Uint8Array;
  readonly targetBytes: Uint8Array | null;
};

export const MAX_LANDING_ARTIFACTS = 1_024;

/// Build the exact project-side review named by one ordinary Mission's passing Receipts.
///
/// Every Artifact target appears once. Missing workspace or Receipt evidence refuses the whole review instead of
/// opening a partial diff that could look complete.
export function missionLanding(snapshot: MissionSnapshot): MissionLanding | null {
  return missionLandingForSelection(snapshot, { kind: "allTasks" });
}

/// Build one mutually exclusive Fleet winner review. No other passing Task contributes an Artifact.
export function missionWinnerLanding(snapshot: MissionSnapshot, taskId: string): MissionLanding | null {
  return missionLandingForSelection(snapshot, { kind: "chooseOne", taskId });
}

export function missionLandingForSelection(
  snapshot: MissionSnapshot,
  selection: LandingSelection,
): MissionLanding | null {
  if (snapshot.mission.state !== "integrating") return null;
  return missionLandingAuthority(snapshot, selection);
}

/// Recover the immutable Mission and Receipt authority after Core has crossed from integrating to completed.
/// The lifecycle state is deliberately not part of the authority identity because a successful Landing changes it.
export function missionLandingAuthority(
  snapshot: MissionSnapshot,
  selection: LandingSelection = { kind: "allTasks" },
): MissionLanding | null {
  if (snapshot.mission.state !== "integrating" && snapshot.mission.state !== "completed") return null;
  if (selection.kind === "allTasks" && snapshot.mission.completion_policy !== "allTasks") return null;
  if (selection.kind === "chooseOne" && snapshot.mission.completion_policy !== "chooseOne") return null;

  const tasks = selection.kind === "allTasks"
    ? snapshot.tasks
    : snapshot.tasks.filter((task) => task.task_id === selection.taskId);
  if (selection.kind === "chooseOne" && tasks.length !== 1) return null;

  const artifacts: MissionLandingArtifact[] = [];
  const targets = new Set<string>();
  for (const task of tasks) {
    if (task.state !== "passed" || !task.workspace || !task.receipt_id || task.artifact_paths.length === 0) {
      return null;
    }
    if (!Array.isArray(task.artifacts)) return null;
    const evidenceByPath = new Map(task.artifacts.map((evidence) => [evidence.path, evidence]));
    if (evidenceByPath.size !== task.artifact_paths.length || task.artifacts.length !== task.artifact_paths.length) {
      return null;
    }
    for (const artifactPath of task.artifact_paths) {
      const target = artifactPath.toLowerCase();
      const evidence = evidenceByPath.get(artifactPath);
      if (
        !safeArtifactPath(artifactPath)
        || targets.has(target)
        || !evidence
        || !Number.isSafeInteger(evidence.size)
        || evidence.size < 0
        || !/^[0-9a-f]{64}$/.test(evidence.sha256)
      ) return null;
      if (artifacts.length >= MAX_LANDING_ARTIFACTS) return null;
      targets.add(target);
      artifacts.push({ path: artifactPath, task, evidence });
    }
  }
  if (artifacts.length === 0) return null;
  artifacts.sort((left, right) => left.path.localeCompare(right.path));
  return { snapshot, selection, artifacts };
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

/// The exact Core and Receipt authority reviewed by a person. File bytes are checked separately because neither the
/// Mission digest nor a Receipt identity proves that a workspace or project file stayed unchanged after review.
export function landingIdentity(landing: MissionLanding): string {
  const { snapshot } = landing;
  return JSON.stringify({
    missionId: snapshot.mission.mission_id,
    missionSha256: snapshot.mission_sha256,
    policySha256: snapshot.policy_sha256,
    project: snapshot.mission.project,
    completionPolicy: snapshot.mission.completion_policy,
    selection: landing.selection,
    artifacts: landing.artifacts.map((artifact) => ({
      path: artifact.path,
      taskId: artifact.task.task_id,
      taskState: artifact.task.state,
      workspace: artifact.task.workspace,
      baseCommit: artifact.task.base_commit,
      runId: artifact.task.run_id,
      receiptId: artifact.task.receipt_id,
      receiptSize: artifact.evidence.size,
      receiptSha256: artifact.evidence.sha256,
    })),
  });
}

/// Refuse ambiguous completion recovery unless Core durably names the exact selected winner and Receipt.
export function landingCompletionProblem(landing: MissionLanding): string | null {
  if (landing.selection.kind !== "chooseOne") return null;
  const selected = landing.artifacts[0]?.task;
  const integration = landing.snapshot.integration;
  if (
    !selected
    || integration?.selected_task_id !== landing.selection.taskId
    || integration.selected_receipt_id !== selected.receipt_id
  ) {
    return "Core completion used a different selected Task Receipt";
  }
  return null;
}

export function landingByteDriftProblem(
  reviewed: readonly LandingByteEvidence[],
  current: readonly LandingByteEvidence[],
): string | null {
  if (reviewed.length !== current.length) return "the reviewed Artifact set changed";
  for (const [index, before] of reviewed.entries()) {
    const now = current[index];
    if (!now || now.path !== before.path) return "the reviewed Artifact order changed";
    if (!sameBytes(now.sourceBytes, before.sourceBytes)) return `Receipt Artifact changed: ${before.path}`;
    if (now.targetBytes === null !== (before.targetBytes === null)) {
      return `Project Artifact existence changed: ${before.path}`;
    }
    if (now.targetBytes !== null && before.targetBytes !== null && !sameBytes(now.targetBytes, before.targetBytes)) {
      return `Project Artifact changed: ${before.path}`;
    }
  }
  return null;
}

export function sameBytes(left: Uint8Array, right: Uint8Array): boolean {
  if (left.byteLength !== right.byteLength) return false;
  for (let index = 0; index < left.byteLength; index += 1) {
    if (left[index] !== right[index]) return false;
  }
  return true;
}
