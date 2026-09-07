import type { Conversation } from "./conversationList";
import { conversationArchival } from "./conversationArchival";
import { conversationDeletion } from "./conversationDeletion";
import type { ProviderCapabilities } from "./runtimeTypes";

/// Provider-owned archive and deletion share the same project-wide owner and confirmation boundary.
export type ProjectConversationAction = "archive" | "delete";

export type ProjectConversationPlan = {
  readonly action: ProjectConversationAction;
  /// Idle provider-owned records with the requested capability.
  readonly eligible: readonly Conversation[];
  /// Running under Runtrol's own supervision and eligible once stopped.
  readonly stoppable: readonly Conversation[];
  /// Running where Runtrol holds no handle to stop them. Always skipped, and said.
  readonly runningElsewhere: readonly Conversation[];
  /// Rows that must stay, grouped by the exact reason from the individual action's decision.
  readonly unsupported: ReadonlyMap<string, number>;
};

export function planProjectConversations(
  action: ProjectConversationAction,
  rows: readonly Conversation[],
  capabilitiesOf: (providerId: string) => ProviderCapabilities | null,
): ProjectConversationPlan {
  const eligible: Conversation[] = [];
  const stoppable: Conversation[] = [];
  const runningElsewhere: Conversation[] = [];
  const unsupported = new Map<string, number>();
  for (const row of rows) {
    if (row.live && !row.canStop) {
      runningElsewhere.push(row);
      continue;
    }
    // The individual action's rule, asked about the row as it would stand once stopped. Liveness is what the
    // stop button changes; the capability and the provider-owned identity are what it cannot.
    const decide = action === "archive" ? conversationArchival : conversationDeletion;
    const once = decide({ ...row, live: false }, capabilitiesOf(row.providerId));
    if (once.kind === "unsupported") {
      unsupported.set(once.why, (unsupported.get(once.why) ?? 0) + 1);
      continue;
    }
    if (row.live) stoppable.push(row);
    else eligible.push(row);
  }
  return { action, eligible, stoppable, runningElsewhere, unsupported };
}

export type ProjectConversationQuestion = {
  readonly message: string;
  readonly detail: string;
  /// The action for what is idle now, with its exact count, or null when nothing is.
  readonly idle: string | null;
  /// The action that stops supervised rows first, or null when none run.
  readonly stopAndApply: string | null;
};

/// The confirmation, with every number a person is about to act on. Null when there is nothing it could do.
export function projectConversationQuestion(
  projectName: string,
  plan: ProjectConversationPlan,
): ProjectConversationQuestion | null {
  const idle = plan.eligible.length;
  const running = plan.stoppable.length;
  if (idle === 0 && running === 0) return null;
  const total = idle + running + plan.runningElsewhere.length
    + [...plan.unsupported.values()].reduce((sum, count) => sum + count, 0);
  const archive = plan.action === "archive";
  const verb = archive ? "Archive" : "Delete";
  const applied = archive ? "archived" : "deleted";
  const lines: string[] = [
    archive
      ? "Each conversation leaves this list through its provider's archive surface. Restore it with that provider. Runtrol keeps no recovery copy."
      : "This removes each provider-owned conversation and its known related history records. Runtrol keeps no recovery copy.",
  ];
  if (running > 0) {
    lines.push(`${running} ${plural(running, "is", "are")} running here and ${plural(running, "is", "are")} ${applied} only if stopped first.`);
  }
  for (const [why, count] of plan.unsupported) {
    lines.push(`${count} ${plural(count, "stays", "stay")}: ${why}`);
  }
  if (plan.runningElsewhere.length > 0) {
    lines.push(`${plan.runningElsewhere.length} ${plural(plan.runningElsewhere.length, "is", "are")} running outside Runtrol and ${plural(plan.runningElsewhere.length, "is", "are")} skipped.`);
  }
  return {
    message: `${archive ? "Archive" : "Permanently delete"} ${idle + running} of ${total} ${plural(total, "conversation", "conversations")} in ${projectName}?`,
    detail: lines.join("\n"),
    idle: idle > 0 ? `${verb} ${idle} idle` : null,
    stopAndApply: running > 0 ? `Stop ${running} and ${plan.action} ${idle + running}` : null,
  };
}

export function projectConversationKept(plan: ProjectConversationPlan, includeRunning: boolean): string[] {
  const kept = [...plan.unsupported].map(([why, count]) => `${count}: ${why}`);
  if (plan.runningElsewhere.length > 0) kept.push(`${plan.runningElsewhere.length} running outside Runtrol`);
  if (!includeRunning && plan.stoppable.length > 0) kept.push(`${plan.stoppable.length} running here (Stop was not selected)`);
  return kept;
}

export type ProjectConversationOperations = {
  /// Resolves only after the exact selected owner has stopped. A refusal never authorizes the provider action.
  stop(row: Conversation): Promise<void>;
  /// Rechecks the current live row and invokes the provider with the original exact native identity.
  apply(row: Conversation): Promise<void>;
  report(message: string): void;
};

export async function applyProjectConversations(
  plan: ProjectConversationPlan,
  includeRunning: boolean,
  operations: ProjectConversationOperations,
): Promise<{ intended: number; completed: number; refused: string[] }> {
  const stopping = includeRunning ? plan.stoppable : [];
  const refused: string[] = [];
  let completed = 0;
  for (const row of [...plan.eligible, ...stopping]) {
    try {
      if (row.live) {
        operations.report(`stopping ${row.title}`);
        await operations.stop(row);
      }
      operations.report(row.title);
      await operations.apply({ ...row, live: false });
      completed += 1;
    } catch (error) {
      refused.push(`${row.title}: ${error instanceof Error ? error.message : String(error)}`);
    }
  }
  return { intended: plan.eligible.length + stopping.length, completed, refused };
}

function plural(count: number, one: string, many: string): string {
  return count === 1 ? one : many;
}
