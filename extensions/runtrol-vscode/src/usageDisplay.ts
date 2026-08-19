import type { ProviderLine, ProviderUsageGauge, ProviderUsageWindow } from "./runtimeTypes";
import { providerDisplayName, providerIcon } from "./sessionDisplay";

/// One line of the usage strip, ready to draw.
export type UsageRow = {
  /// Stable tree identity.
  readonly key: string;
  /// The service's name, which is the row's label.
  readonly name: string;
  /// The editor glyph that stands for the service.
  readonly icon: string;
  /// The muted line beside the name.
  readonly detail: string;
  /// Whether a limit is blocking right now, which decides the row's colour.
  readonly reached: boolean;
  /// The whole position for the hover, sentence by sentence.
  readonly tooltip: string;
};

/// The strip's rows, in the order the Runtime reported them.
///
/// Only providers that have reported appear. An account that has said nothing since the Runtime started is not
/// a green light and not a red one; it is absent, and the view's empty text says so in words.
export function usageRows(
  gauges: readonly ProviderUsageGauge[],
  providers: readonly ProviderLine[],
  nowMs: number,
): UsageRow[] {
  return gauges.map((gauge) => {
    const name = providerDisplayName(gauge.providerId, providers);
    return {
      key: `usage:${encodeURIComponent(gauge.providerId)}`,
      name,
      icon: providerIcon(gauge.providerId, providers),
      detail: usageDetail(gauge, nowMs),
      reached: gauge.reached,
      tooltip: usageTooltip(name, gauge, nowMs),
    };
  });
}

/// The muted line: the account's position in the provider's own numbers, nothing invented.
///
/// A provider that reports a percentage shows it. One that reports only when the window resets shows that. One
/// that reports a blocking limit says so first, because that is the fact the reader acts on.
export function usageDetail(gauge: ProviderUsageGauge, nowMs: number): string {
  const parts: string[] = [];
  if (gauge.reached) parts.push("limit reached");
  const window = gauge.primary ?? gauge.secondary;
  const percent = window?.usedPercent;
  if (typeof percent === "number") parts.push(`${percent}%`);
  const resets = resetsIn(window, nowMs);
  if (resets) parts.push(resets);
  if (parts.length === 0) parts.push("within limits");
  return parts.join(" · ");
}

/// When the governing window resets, as a wait a person can plan around.
function resetsIn(window: ProviderUsageWindow | null | undefined, nowMs: number): string | null {
  const at = window?.resetsAtMs;
  if (typeof at !== "number" || at <= nowMs) return null;
  const minutes = Math.round((at - nowMs) / 60_000);
  if (minutes < 1) return "resets now";
  if (minutes < 60) return `resets in ${minutes}m`;
  const hours = Math.round(minutes / 60);
  if (hours < 48) return `resets in ${hours}h`;
  return `resets in ${Math.round(hours / 24)}d`;
}

/// The hover, with both windows when both were reported.
function usageTooltip(name: string, gauge: ProviderUsageGauge, nowMs: number): string {
  const lines = [`${name}: ${gauge.reached ? "a limit is blocking right now" : "within limits"}`];
  for (const [label, window] of [
    ["Current window", gauge.primary],
    ["Longer window", gauge.secondary],
  ] as const) {
    if (!window) continue;
    const pieces: string[] = [];
    if (typeof window.usedPercent === "number") pieces.push(`${window.usedPercent}% used`);
    if (typeof window.windowMinutes === "number") pieces.push(`${window.windowMinutes} minute window`);
    const resets = resetsIn(window, nowMs);
    if (resets) pieces.push(resets);
    if (pieces.length > 0) lines.push(`${label}: ${pieces.join(", ")}`);
  }
  const age = Math.max(0, Math.round((nowMs - gauge.atMs) / 60_000));
  lines.push(age < 1 ? "Reported just now" : `Reported ${age}m ago`);
  return lines.join("\n");
}
