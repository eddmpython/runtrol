import type { MissionSnapshot } from "../protocol";
import { hasMissionMomentumWork, type MissionMomentum } from "./momentum";

/// A person can review this many exact Mission digests in one modal without the bulk action becoming opaque.
export const MAX_FLIGHT_DECK_MISSIONS = 8;

export type MissionFlightEntry = {
  readonly snapshot: MissionSnapshot;
  readonly momentum: MissionMomentum;
};

export type MissionFlightDeck = {
  readonly batch: readonly MissionFlightEntry[];
  readonly remainingReady: readonly MissionFlightEntry[];
  readonly waiting: readonly MissionFlightEntry[];
  readonly manual: readonly MissionFlightEntry[];
  readonly stopped: readonly MissionFlightEntry[];
};

/// Select the exact ordinary Missions one local confirmation can safely advance now.
///
/// The existing Mission Momentum classifier remains the authority for Task transitions. This layer only groups its
/// results across projects, keeps expired reviews and explicit recovery out, and bounds what one person must inspect.
export function missionFlightDeck(
  entries: readonly MissionFlightEntry[],
  nowUnixMs: number,
): MissionFlightDeck {
  const ready: MissionFlightEntry[] = [];
  const waiting: MissionFlightEntry[] = [];
  const manual: MissionFlightEntry[] = [];
  const stopped: MissionFlightEntry[] = [];

  for (const entry of entries) {
    const { snapshot, momentum } = entry;
    if (snapshot.mission.completion_policy !== "allTasks" || momentum.stopped) {
      stopped.push(entry);
      continue;
    }
    if (
      snapshot.mission.state === "validated"
      && snapshot.approval_expires_unix_ms <= nowUnixMs
    ) {
      manual.push(entry);
      continue;
    }
    if (momentum.manual.length > 0) {
      manual.push(entry);
      continue;
    }
    if (hasMissionMomentumWork(momentum)) {
      ready.push(entry);
      continue;
    }
    if (momentum.waiting.length > 0) {
      waiting.push(entry);
      continue;
    }
    stopped.push(entry);
  }

  ready.sort(compareReady);
  return {
    batch: ready.slice(0, MAX_FLIGHT_DECK_MISSIONS),
    remainingReady: ready.slice(MAX_FLIGHT_DECK_MISSIONS),
    waiting,
    manual,
    stopped,
  };
}

function compareReady(left: MissionFlightEntry, right: MissionFlightEntry): number {
  const statePriority = Number(left.momentum.start) - Number(right.momentum.start);
  if (statePriority !== 0) return statePriority;
  return left.snapshot.mission.project.localeCompare(right.snapshot.mission.project)
    || left.snapshot.mission.name.localeCompare(right.snapshot.mission.name)
    || left.snapshot.mission.mission_id.localeCompare(right.snapshot.mission.mission_id);
}
