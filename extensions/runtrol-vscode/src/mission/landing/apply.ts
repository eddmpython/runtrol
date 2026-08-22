import { lstat, mkdir } from "node:fs/promises";
import * as path from "node:path";

import * as vscode from "vscode";

import { writeAtomicLandingFile } from "./atomicFile";
import {
  landingByteDriftProblem,
  landingCompletionProblem,
  landingIdentity,
  missionLandingAuthority,
  missionLandingForSelection,
  sameBytes,
  type MissionLanding,
  type MissionLandingArtifact,
} from "./model";
import {
  readLandingArtifact,
  readLandingTarget,
  readMissionLanding,
  type ReviewedLandingArtifact,
  type ReviewedMissionLanding,
} from "./review";
import {
  applyLandingTransaction,
  createLandingDirectories,
  type LandingDirectoryIo,
  LandingTransactionError,
  removeLandingDirectories,
} from "./transaction";

export async function applyReviewedLanding(
  review: ReviewedMissionLanding,
  latestSnapshot: ReviewedMissionLanding["landing"]["snapshot"],
): Promise<void> {
  const latest = missionLandingForSelection(latestSnapshot, review.landing.selection);
  if (!latest || landingIdentity(latest) !== review.identity) {
    throw new Error("Mission or Receipt changed after Landing review");
  }
  const current = await readMissionLanding(latest);
  const drift = landingByteDriftProblem(review.artifacts, current.artifacts);
  if (drift) throw new Error(`${drift}; review the Landing again`);

  let createdDirectories: string[] = [];
  try {
    createdDirectories = await createLandingDirectories(
      await missingDirectoryPaths(review),
      directoryIo(),
    );
    const artifacts = new Map(review.artifacts.map((artifact) => [artifact.path, artifact]));
    const latestArtifacts = new Map(latest.artifacts.map((artifact) => [artifact.path, artifact]));
    await applyLandingTransaction(review.artifacts, {
      beforeWrite: async (entry) => {
        await assertForwardCurrent(latest, requireLatestArtifact(latestArtifacts, entry.path), entry);
      },
      read: async (artifactPath) => readLandingTarget(
        latest,
        requireLatestArtifact(latestArtifacts, artifactPath),
      ),
      write: async (artifactPath, bytes, expected) => {
        const reviewed = requireArtifact(artifacts, artifactPath);
        const artifact = requireLatestArtifact(latestArtifacts, artifactPath);
        await writeAtomicLandingFile(
          latest.snapshot.mission.project,
          reviewed.target.fsPath,
          bytes,
          reviewed.targetMode,
          async () => {
            if (sameBytes(bytes, reviewed.sourceBytes)) {
              await assertForwardCurrent(latest, artifact, { ...reviewed, targetBytes: expected });
            } else {
              await assertTargetCurrent(latest, artifact, expected);
            }
          },
        );
      },
      remove: async (artifactPath, expected) => {
        const artifact = requireLatestArtifact(latestArtifacts, artifactPath);
        await assertTargetCurrent(latest, artifact, expected);
        const target = requireArtifact(artifacts, artifactPath).target;
        try {
          await vscode.workspace.fs.delete(target, { recursive: false, useTrash: false });
        } catch (error) {
          if (!isVscodeMissing(error)) throw error;
        }
      },
    });
  } catch (error) {
    const directoryProblems = await removeLandingDirectories(createdDirectories, directoryIo());
    throw applyFailure(error, directoryProblems);
  }
}

async function assertForwardCurrent(
  latest: MissionLanding,
  artifact: MissionLandingArtifact,
  expected: { readonly path: string; readonly sourceBytes: Uint8Array; readonly targetBytes: Uint8Array | null },
): Promise<void> {
  const reread = await readLandingArtifact(latest, artifact);
  const problem = landingByteDriftProblem([expected], [reread]);
  if (problem) throw new Error(`${problem}; review the Landing again`);
}

async function assertTargetCurrent(
  latest: MissionLanding,
  artifact: MissionLandingArtifact,
  expected: Uint8Array | null,
): Promise<void> {
  const current = await readLandingTarget(latest, artifact);
  if (expected === null ? current !== null : current === null || !sameBytes(current, expected)) {
    throw new Error(`Project Artifact changed: ${artifact.path}; review the Landing again`);
  }
}

export async function assertReviewedLandingApplied(
  review: ReviewedMissionLanding,
  latestSnapshot: ReviewedMissionLanding["landing"]["snapshot"],
): Promise<void> {
  const latest = missionLandingAuthority(latestSnapshot, review.landing.selection);
  if (!latest || landingIdentity(latest) !== review.identity) {
    throw new Error("Mission or Receipt changed after Landing apply");
  }
  if (latestSnapshot.mission.state === "completed") {
    const completionProblem = landingCompletionProblem(latest);
    if (completionProblem) throw new Error(completionProblem);
  }
  const current = await readMissionLanding(latest);
  if (current.artifacts.length !== review.artifacts.length) {
    throw new Error("the applied Artifact set changed; review the Landing again");
  }
  for (const [index, artifact] of review.artifacts.entries()) {
    const now = current.artifacts[index];
    if (
      !now
      || now.path !== artifact.path
      || !sameBytes(now.sourceBytes, artifact.sourceBytes)
      || now.targetBytes === null
      || !sameBytes(now.targetBytes, artifact.sourceBytes)
    ) {
      throw new Error(`Applied Artifact changed before Core completion: ${artifact.path}`);
    }
  }
}

async function missingDirectoryPaths(review: ReviewedMissionLanding): Promise<string[]> {
  const needed = new Set<string>();
  const root = path.resolve(review.landing.snapshot.mission.project);
  for (const artifact of review.artifacts) {
    let current = root;
    for (const part of artifact.path.split("/").slice(0, -1)) {
      current = path.join(current, part);
      try {
        const stat = await lstat(current);
        if (stat.isSymbolicLink() || !stat.isDirectory()) {
          throw new Error(`Unsafe project directory appeared: ${artifact.path}`);
        }
      } catch (error) {
        if (!isMissing(error)) throw error;
        needed.add(current);
      }
    }
  }
  return [...needed].sort((left, right) => left.length - right.length);
}

function requireArtifact(
  artifacts: ReadonlyMap<string, ReviewedLandingArtifact>,
  artifactPath: string,
): ReviewedLandingArtifact {
  const artifact = artifacts.get(artifactPath);
  if (!artifact) throw new Error(`Landing transaction named an unknown Artifact: ${artifactPath}`);
  return artifact;
}

function requireLatestArtifact(
  artifacts: ReadonlyMap<string, MissionLandingArtifact>,
  artifactPath: string,
): MissionLandingArtifact {
  const artifact = artifacts.get(artifactPath);
  if (!artifact) throw new Error(`Mission no longer names Artifact: ${artifactPath}`);
  return artifact;
}

function directoryIo(): LandingDirectoryIo {
  return {
    ensure: async (directory) => {
      try {
        const stat = await lstat(directory);
        if (stat.isSymbolicLink() || !stat.isDirectory()) {
          throw new Error(`Unsafe project directory appeared: ${directory}`);
        }
        return false;
      } catch (error) {
        if (!isMissing(error)) throw error;
      }
      try {
        await mkdir(directory);
        return true;
      } catch (error) {
        if (!isAlreadyExists(error)) throw error;
        const stat = await lstat(directory);
        if (stat.isSymbolicLink() || !stat.isDirectory()) {
          throw new Error(`Unsafe project directory appeared: ${directory}`);
        }
        return false;
      }
    },
    exists: async (directory) => lstat(directory).then(
      () => true,
      (error) => {
        if (isMissing(error)) return false;
        throw error;
      },
    ),
    remove: async (directory) => {
      await vscode.workspace.fs.delete(vscode.Uri.file(directory), { recursive: false, useTrash: false });
    },
  };
}

function applyFailure(error: unknown, directoryProblems: readonly string[]): Error {
  const detail = error instanceof Error ? error.message : String(error);
  const rollbackProblems = [
    ...(error instanceof LandingTransactionError ? error.rollbackProblems : []),
    ...directoryProblems,
  ];
  const suffix = rollbackProblems.length > 0
    ? ` Rollback needs attention: ${rollbackProblems.join("; ")}`
    : " Every changed file and newly created directory was restored and verified.";
  return new Error(`Landing apply failed: ${detail}.${suffix}`);
}

function isMissing(error: unknown): boolean {
  return error instanceof Error && "code" in error && error.code === "ENOENT";
}

function isAlreadyExists(error: unknown): boolean {
  return error instanceof Error && "code" in error && error.code === "EEXIST";
}

function isVscodeMissing(error: unknown): boolean {
  return error instanceof Error && "code" in error && (error.code === "FileNotFound" || error.code === "ENOENT");
}
