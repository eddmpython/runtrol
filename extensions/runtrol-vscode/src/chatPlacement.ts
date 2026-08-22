import type { WorkspaceAccess } from "./runtimeTypes";

export type StartDecision = WorkspaceAccess | "isolated";

/// Placement implied by asking additional services the same first message.
///
/// A real project always receives one Core-owned linked worktree per service. The projectless scratch directory
/// is the only shared case because it is not a Git checkout and represents no selected project tree.
export function multiProviderPlacement(
  additionalProviders: number,
  projectless: boolean,
): StartDecision | null {
  if (additionalProviders <= 0) return null;
  return projectless ? "shared" : "isolated";
}
