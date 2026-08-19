import type { LimitTelemetry, UsageTelemetry } from "./telemetry";

/// What the conversation knows about itself, kept as values rather than as a screenful of cards.
///
/// Every field here was a labelled tile in a panel above the thread. None of them is worth a tile: they are read
/// in passing, between messages, and a tile that is read in passing is a tile that was in the way.
export type ConversationFacts = {
  service: string;
  model: string;
  effort: string;
  mode: string;
};

export type UsageFacts = {
  usage: UsageTelemetry | null;
  primary: LimitTelemetry | null;
  secondary: LimitTelemetry | null;
  reached: boolean;
};

export const NO_FACTS: ConversationFacts = { service: "", model: "", effort: "", mode: "" };
export const NO_USAGE: UsageFacts = { usage: null, primary: null, secondary: null, reached: false };

/// Who is being spoken to, and how they are configured.
export function agentLine(facts: ConversationFacts): string {
  // The mode is deliberately absent: it renders on its own chip, which is also its own switch.
  return [facts.service, facts.model, facts.effort]
    .map((part) => part.trim())
    .filter(Boolean)
    .join(" · ");
}

/// What this conversation has spent, shown only once a number actually exists.
///
/// Context share leads because it is the one a person acts on. A quota window only appears once it is close
/// enough to matter, so an untouched allowance never takes up the line.
export function usageLine(facts: UsageFacts, nowMs: number): string {
  const parts: string[] = [];
  const share = contextShare(facts.usage);
  if (share !== null) parts.push(`Context ${share}%`);
  const spend = spendOf(facts.usage);
  if (spend) parts.push(spend);
  const window = tightestWindow(facts);
  if (window) parts.push(limitPhrase(window, nowMs));
  return parts.join(" · ");
}

function contextShare(usage: UsageTelemetry | null): number | null {
  if (!usage || usage.used === null || usage.size === null || usage.size <= 0) return null;
  return Math.min(100, Math.round((usage.used / usage.size) * 100));
}

function spendOf(usage: UsageTelemetry | null): string {
  if (!usage || usage.amount === null || !usage.currency) return "";
  return `${usage.amount.toLocaleString(undefined, { maximumFractionDigits: 4 })} ${usage.currency}`;
}

/// The quota window closest to running out, and only once it is worth saying.
const WORTH_SAYING_PERCENT = 60;

function tightestWindow(facts: UsageFacts): LimitTelemetry | null {
  const windows = [facts.primary, facts.secondary].filter(
    (window): window is LimitTelemetry => window !== null,
  );
  if (windows.length === 0) return null;
  const tightest = windows.reduce(
    (worst, window) => (window.usedPercent > worst.usedPercent ? window : worst),
  );
  return facts.reached || tightest.usedPercent >= WORTH_SAYING_PERCENT ? tightest : null;
}

function limitPhrase(window: LimitTelemetry, nowMs: number): string {
  const left = `${Math.max(0, 100 - Math.round(window.usedPercent))}% of ${windowName(window)} left`;
  const until = window.resetsAt === null ? "" : resetPhrase(window.resetsAt, nowMs);
  return until ? `${left}, resets in ${until}` : left;
}

function windowName(window: LimitTelemetry): string {
  if (window.windowMinutes === null) return "limit";
  if (window.windowMinutes < 60) return `${window.windowMinutes}min limit`;
  if (window.windowMinutes < 1_440) return `${Math.round(window.windowMinutes / 60)}h limit`;
  return `${Math.round(window.windowMinutes / 1_440)}d limit`;
}

function resetPhrase(atMs: number, nowMs: number): string {
  const minutes = Math.round((atMs - nowMs) / 60_000);
  if (minutes <= 0) return "";
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.round(minutes / 60);
  return hours < 24 ? `${hours}h` : `${Math.round(hours / 24)}d`;
}
