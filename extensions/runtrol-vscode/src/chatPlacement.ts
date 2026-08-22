import type { WorkspaceAccess } from "./runtimeTypes";

export type StartDecision = WorkspaceAccess | "isolated";

export type ParallelPlacementRequirement = "single" | "sharedOnly" | "ask";

/// Whether a parallel start has a real workspace choice to ask. A scratch directory has no project checkout to
/// isolate, while a real project never receives an automatic worktree decision from Runtrol.
export function parallelPlacementRequirement(
  additionalProviders: number,
  projectless: boolean,
): ParallelPlacementRequirement {
  if (additionalProviders <= 0) return "single";
  return projectless ? "sharedOnly" : "ask";
}
