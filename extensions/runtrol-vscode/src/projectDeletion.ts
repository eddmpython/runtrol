import type { Conversation } from "./conversationList";
import { conversationDeletion } from "./conversationDeletion";
import type { ProviderCapabilities } from "./runtimeTypes";

/// What deleting every conversation of one project would actually do, decided before anything is asked.
///
/// One project's rows are not one case: some can go now, some are running here and can be stopped first, some
/// run where Runtrol has no handle, and some belong to a service that publishes no deletion at all. The
/// confirmation has to say those numbers exactly, because "delete all" that silently leaves rows behind reads
/// as a bug and one that silently kills running agents is one (operator, 2026-08-29).
export type ProjectDeletionPlan = {
  /// Deletable as they stand: idle, provider-owned, and the service publishes deletion.
  readonly deletable: readonly Conversation[];
  /// Running under Runtrol's own supervision and deletable once stopped.
  readonly stoppable: readonly Conversation[];
  /// Running where Runtrol holds no handle to stop them. Always skipped, and said.
  readonly runningElsewhere: readonly Conversation[];
  /// Idle rows nothing here may delete, counted per service so the sentence can name whose they are.
  readonly undeletable: ReadonlyMap<string, number>;
};

export function planProjectDeletion(
  rows: readonly Conversation[],
  capabilitiesOf: (providerId: string) => ProviderCapabilities | null,
): ProjectDeletionPlan {
  const deletable: Conversation[] = [];
  const stoppable: Conversation[] = [];
  const runningElsewhere: Conversation[] = [];
  const undeletable = new Map<string, number>();
  for (const row of rows) {
    if (row.live && !row.canStop) {
      runningElsewhere.push(row);
      continue;
    }
    // The one shared deletion rule, asked about the row as it would stand once stopped. Liveness is what the
    // stop button changes; the capability and the provider-owned identity are what it cannot.
    const once = conversationDeletion({ ...row, live: false }, capabilitiesOf(row.providerId));
    if (once.kind === "unsupported") {
      undeletable.set(row.serviceName, (undeletable.get(row.serviceName) ?? 0) + 1);
      continue;
    }
    if (row.live) stoppable.push(row);
    else deletable.push(row);
  }
  return { deletable, stoppable, runningElsewhere, undeletable };
}

export type ProjectDeletionQuestion = {
  readonly message: string;
  readonly detail: string;
  /// The button that deletes what is idle now, with its exact count, or null when nothing is.
  readonly deleteIdle: string | null;
  /// The button that stops the supervised running rows first and deletes them too, or null when none run.
  readonly stopAndDelete: string | null;
};

/// The confirmation, with every number a person is about to act on. Null when there is nothing it could do.
export function projectDeletionQuestion(
  projectName: string,
  plan: ProjectDeletionPlan,
): ProjectDeletionQuestion | null {
  const idle = plan.deletable.length;
  const running = plan.stoppable.length;
  if (idle === 0 && running === 0) return null;
  const total = idle + running + plan.runningElsewhere.length
    + [...plan.undeletable.values()].reduce((sum, count) => sum + count, 0);
  const lines: string[] = [
    "This removes each provider-owned conversation and its known related history records. Runtrol keeps no recovery copy.",
  ];
  if (running > 0) {
    lines.push(`${running} ${plural(running, "is", "are")} running here and ${plural(running, "is", "are")} deleted only if stopped first.`);
  }
  for (const [service, count] of plan.undeletable) {
    lines.push(`${count} ${plural(count, "stays", "stay")}: ${service} cannot delete stored conversations.`);
  }
  if (plan.runningElsewhere.length > 0) {
    lines.push(`${plan.runningElsewhere.length} ${plural(plan.runningElsewhere.length, "is", "are")} running outside Runtrol and ${plural(plan.runningElsewhere.length, "is", "are")} skipped.`);
  }
  return {
    message: `Permanently delete ${idle + running} of ${total} ${plural(total, "conversation", "conversations")} in ${projectName}?`,
    detail: lines.join("\n"),
    deleteIdle: idle > 0 ? `Delete ${idle} idle` : null,
    stopAndDelete: running > 0 ? `Stop ${running} and delete ${idle + running}` : null,
  };
}

function plural(count: number, one: string, many: string): string {
  return count === 1 ? one : many;
}
