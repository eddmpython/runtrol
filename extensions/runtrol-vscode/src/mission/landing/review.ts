import { createHash } from "node:crypto";
import * as path from "node:path";

import * as vscode from "vscode";

import { MAX_DIFF_TEXT, type DiffDocuments } from "../../diffDocuments";
import {
  landingIdentity,
  type LandingByteEvidence,
  type MissionLanding,
  type MissionLandingArtifact,
} from "./model";
import { inspectSafeLocalFile, readExactLocalFile, type SafeLocalFile } from "./localFile";

export const LANDING_SUFFIX = ": Receipt Landing";

export type ReviewedLandingArtifact = LandingByteEvidence & {
  readonly source: vscode.Uri;
  readonly target: vscode.Uri;
  readonly sourceText: string;
  readonly targetText: string;
  readonly targetMode: number | null;
};

export type ReadMissionLanding = {
  readonly landing: MissionLanding;
  readonly identity: string;
  readonly artifacts: readonly ReviewedLandingArtifact[];
};

export type ReviewedMissionLanding = ReadMissionLanding & {
  readonly tab: vscode.Tab | null;
};

export type LandingArtifactRead = LandingByteEvidence & {
  readonly source: vscode.Uri;
  readonly target: vscode.Uri;
  readonly targetMode: number | null;
};

type LandingArtifactInspection = {
  readonly artifact: MissionLandingArtifact;
  readonly source: vscode.Uri;
  readonly target: vscode.Uri;
  readonly sourceFile: SafeLocalFile;
  readonly targetFile: SafeLocalFile | null;
};

export async function openMissionLanding(
  landing: MissionLanding,
  documents: DiffDocuments,
): Promise<ReviewedMissionLanding> {
  const review = await readMissionLanding(landing);
  const tabsBefore = new Set(vscode.window.tabGroups.all.flatMap((group) => group.tabs));
  const title = `${landing.snapshot.mission.name}${LANDING_SUFFIX}`;
  const resources: Array<[vscode.Uri, vscode.Uri, vscode.Uri]> = review.artifacts.map((artifact) => [
    artifact.target,
    documents.snapshot(artifact.targetText, `landing/${artifact.path}`),
    documents.snapshot(artifact.sourceText, `receipt/${artifact.path}`),
  ]);
  await vscode.commands.executeCommand(
    "vscode.changes",
    title,
    resources,
  );
  const tabs = vscode.window.tabGroups.all.flatMap((group) => group.tabs);
  const tab = tabs.find((candidate) => candidate.label === title && !tabsBefore.has(candidate))
    ?? tabs.find((candidate) => candidate.label === title && candidate.isActive)
    ?? null;
  return { ...review, tab };
}

export async function readMissionLanding(landing: MissionLanding): Promise<ReadMissionLanding> {
  const decoder = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true });
  const artifacts: ReviewedLandingArtifact[] = [];
  let heldBytes = 0;
  const inspected: LandingArtifactInspection[] = [];
  for (const artifact of landing.artifacts) {
    const inspection = await inspectLandingArtifact(landing, artifact);
    heldBytes += inspection.sourceFile.size + (inspection.targetFile?.size ?? 0);
    if (heldBytes > MAX_DIFF_TEXT) throw new Error("Landing exceeds 8 MiB");
    inspected.push(inspection);
  }
  for (const inspection of inspected) {
    const read = await readInspectedLandingArtifact(inspection);
    const { source, target, sourceBytes, targetBytes, targetMode } = read;
    artifacts.push({
      path: inspection.artifact.path,
      source,
      target,
      sourceBytes,
      targetBytes,
      targetMode,
      sourceText: decodeArtifact(decoder, sourceBytes, inspection.artifact.path, "Receipt"),
      targetText: targetBytes === null
        ? ""
        : decodeArtifact(decoder, targetBytes, inspection.artifact.path, "project"),
    });
  }
  return { landing, identity: landingIdentity(landing), artifacts };
}

export async function readLandingArtifact(
  landing: MissionLanding,
  artifact: MissionLandingArtifact,
): Promise<LandingArtifactRead> {
  const inspection = await inspectLandingArtifact(landing, artifact);
  if (inspection.sourceFile.size + (inspection.targetFile?.size ?? 0) > MAX_DIFF_TEXT) {
    throw new Error("Landing Artifact exceeds 8 MiB");
  }
  return readInspectedLandingArtifact(inspection);
}

export async function readLandingTarget(
  landing: MissionLanding,
  artifact: MissionLandingArtifact,
): Promise<Uint8Array | null> {
  const target = artifactUri(landing.snapshot.mission.project, artifact.path);
  const targetFile = await inspectSafeLocalFile(landing.snapshot.mission.project, artifact.path, false);
  assertEditorsClean([target], artifact.path);
  return targetFile === null
    ? null
    : readExactLocalFile(targetFile, MAX_DIFF_TEXT, `Project Artifact ${artifact.path}`);
}

async function inspectLandingArtifact(
  landing: MissionLanding,
  artifact: MissionLandingArtifact,
): Promise<LandingArtifactInspection> {
  const source = artifactUri(artifact.task.workspace as string, artifact.path);
  const target = artifactUri(landing.snapshot.mission.project, artifact.path);
  const sourceFile = await inspectSafeLocalFile(artifact.task.workspace as string, artifact.path, true);
  if (!sourceFile) throw new Error(`Missing Artifact: ${artifact.path}`);
  if (sourceFile.size !== artifact.evidence.size) {
    throw new Error(`Receipt Artifact evidence mismatch: ${artifact.path}`);
  }
  const targetFile = await inspectSafeLocalFile(landing.snapshot.mission.project, artifact.path, false);
  assertEditorsClean([source, target], artifact.path);
  return { artifact, source, target, sourceFile, targetFile };
}

async function readInspectedLandingArtifact(
  inspection: LandingArtifactInspection,
): Promise<LandingArtifactRead> {
  const { artifact, source, target, sourceFile, targetFile } = inspection;
  const sourceBytes = await readExactLocalFile(sourceFile, MAX_DIFF_TEXT, `Receipt Artifact ${artifact.path}`);
  if (
    createHash("sha256").update(sourceBytes).digest("hex") !== artifact.evidence.sha256
  ) {
    throw new Error(`Receipt Artifact evidence mismatch: ${artifact.path}`);
  }
  const targetBytes = targetFile === null
    ? null
    : await readExactLocalFile(targetFile, MAX_DIFF_TEXT, `Project Artifact ${artifact.path}`);
  return {
    path: artifact.path,
    source,
    target,
    sourceBytes,
    targetBytes,
    targetMode: targetFile?.mode ?? null,
  };
}

function assertEditorsClean(uris: readonly vscode.Uri[], artifactPath: string): void {
  const dirtyText = vscode.workspace.textDocuments.some((document) =>
    document.isDirty && artifactUriMatches(document.uri, uris)
  );
  const dirtyNotebook = vscode.workspace.notebookDocuments.some((document) =>
    document.isDirty && artifactUriMatches(document.uri, uris)
  );
  const dirtyTab = vscode.window.tabGroups.all.some((group) => group.tabs.some((tab) =>
    tab.isDirty && tabInputUris(tab.input).some((uri) => artifactUriMatches(uri, uris))
  ));
  if (dirtyText || dirtyNotebook || dirtyTab) throw new Error(`Unsaved editor for Artifact: ${artifactPath}`);
}

function tabInputUris(input: unknown): readonly vscode.Uri[] {
  if (
    input instanceof vscode.TabInputText
    || input instanceof vscode.TabInputNotebook
    || input instanceof vscode.TabInputCustom
  ) return [input.uri];
  if (input instanceof vscode.TabInputTextDiff || input instanceof vscode.TabInputNotebookDiff) {
    return [input.original, input.modified];
  }
  return [];
}

function artifactUriMatches(candidate: vscode.Uri, expected: readonly vscode.Uri[]): boolean {
  return expected.some((uri) => sameFile(candidate, uri));
}

function sameFile(left: vscode.Uri, right: vscode.Uri): boolean {
  if (left.scheme !== "file" || right.scheme !== "file") return left.toString() === right.toString();
  return normalizedLocalPath(left.fsPath) === normalizedLocalPath(right.fsPath);
}

function normalizedLocalPath(value: string): string {
  const resolved = path.normalize(path.resolve(value));
  return process.platform === "win32" ? resolved.toLowerCase() : resolved;
}

function artifactUri(root: string, relative: string): vscode.Uri {
  return vscode.Uri.file(path.join(root, ...relative.split("/")));
}

function decodeArtifact(
  decoder: TextDecoder,
  bytes: Uint8Array,
  artifactPath: string,
  side: "Receipt" | "project",
): string {
  try {
    return decoder.decode(bytes);
  } catch {
    throw new Error(`${side} Artifact is not UTF-8: ${artifactPath}`);
  }
}
