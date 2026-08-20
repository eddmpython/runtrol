import { record, type UnknownRecord } from "./presentation";

/// Bounded because a plan is a glance, not a document. A service that sends hundreds of entries still
/// gets its first page shown, and the transcript stays scrollable.
const MAX_PLAN_ENTRIES = 64;
const MAX_PLAN_CONTENT = 512;

/// The three states the Agent Client Protocol defines for a plan entry.
///
/// An unknown status renders as pending: not understood is never rendered as done.
export type PlanStatus = "pending" | "in_progress" | "completed";

export type PlanEntry = {
  content: string;
  status: PlanStatus;
};

/// The plan entries a service announced, in its own words and order.
///
/// Reads only `payload.entries[].content` and `.status` (the ACP plan shape). Anything else in the
/// payload stays untouched, and a payload without readable entries yields nothing so the caller can
/// fall back to the one-line notice instead of inventing a plan.
export function planEntriesOf(body: UnknownRecord): PlanEntry[] {
  const payload = record(body.payload);
  const entries = payload?.entries;
  if (!Array.isArray(entries)) return [];
  const plan: PlanEntry[] = [];
  for (const raw of entries) {
    if (plan.length >= MAX_PLAN_ENTRIES) break;
    const entry = record(raw);
    const content = entry?.content;
    if (typeof content !== "string" || !content.trim()) continue;
    plan.push({
      content: content.length > MAX_PLAN_CONTENT ? content.slice(0, MAX_PLAN_CONTENT) : content,
      status: statusOf(entry?.status),
    });
  }
  return plan;
}

export function planGlyph(status: PlanStatus): string {
  if (status === "completed") return "●";
  if (status === "in_progress") return "◐";
  return "○";
}

function statusOf(value: unknown): PlanStatus {
  return value === "completed" || value === "in_progress" ? value : "pending";
}
