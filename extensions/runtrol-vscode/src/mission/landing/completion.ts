import type { MissionSnapshot } from "../../protocol";

/// Complete exactly once while treating a lost response as an observation problem, not permission to rewrite files.
export async function completeLandingWithRecovery(
  latest: MissionSnapshot,
  complete: (snapshot: MissionSnapshot) => Promise<MissionSnapshot>,
  refresh: () => Promise<MissionSnapshot>,
  validateCompleted: (snapshot: MissionSnapshot) => Promise<void>,
): Promise<MissionSnapshot> {
  if (latest.mission.state === "completed") {
    await validateCompleted(latest);
    return latest;
  }

  let completed: MissionSnapshot;
  try {
    completed = await complete(latest);
  } catch (completionError) {
    let observed: MissionSnapshot;
    try {
      observed = await refresh();
    } catch {
      throw completionError;
    }
    if (observed.mission.state !== "completed") throw completionError;
    await validateCompleted(observed);
    return observed;
  }

  if (completed.mission.state !== "completed") {
    throw new Error(`Core returned ${completed.mission.state} after Mission Landing completion`);
  }
  await validateCompleted(completed);
  return completed;
}
