import type { ProviderLine, ProviderUsageGauge, ProviderUsageWindow } from "./runtimeTypes";
import { awaitsVerification, isBroken } from "./providerHealth";
import { providerDisplayName, providerIcon } from "./sessionDisplay";

export type UsageState = "available" | "checking" | "unavailable" | "disconnected";

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
  /// The service's operational state. Usage is not allowed to hide a service that cannot currently run.
  readonly state: UsageState;
  /// The provider record carried by an actionable unavailable row.
  readonly provider: ProviderLine | null;
  /// The whole position for the hover, sentence by sentence.
  readonly tooltip: string;
};

/// The strip's rows. Every installed CLI is present, including one still being checked or one that needs a fix.
/// Missing CLIs stay absent. A last report whose provider disappeared remains as explicitly disconnected so a
/// known limit never silently becomes a healthy-looking omission.
export function usageRows(
  gauges: readonly ProviderUsageGauge[],
  providers: readonly ProviderLine[],
  nowMs: number,
): UsageRow[] {
  const byProvider = new Map(gauges.map((gauge) => [gauge.providerId, gauge]));
  const installed = providers.filter((provider) => provider.installation.state !== "missing");
  const providerIds = installed.map((provider) => provider.providerId);
  const providersById = new Map(installed.map((provider) => [provider.providerId, provider]));
  const seen = new Set(providerIds);
  for (const gauge of gauges) {
    if (seen.has(gauge.providerId)) continue;
    seen.add(gauge.providerId);
    providerIds.push(gauge.providerId);
  }
  return providerIds.map((providerId) => {
    const gauge = byProvider.get(providerId);
    const provider = providersById.get(providerId) ?? null;
    const name = providerDisplayName(providerId, providers);
    if (provider && awaitsVerification(provider)) {
      return {
        key: `usage:${encodeURIComponent(providerId)}`,
        name,
        icon: providerIcon(providerId, providers),
        detail: "Checking",
        reached: false,
        state: "checking",
        provider,
        tooltip: `${name}: checking the installed CLI`,
      };
    }
    if (provider && isBroken(provider)) {
      const why = provider.installation.why ?? `${name} cannot currently start a conversation.`;
      return {
        key: `usage:${encodeURIComponent(providerId)}`,
        name,
        icon: providerIcon(providerId, providers),
        detail: "Unavailable · Fix",
        reached: false,
        state: "unavailable",
        provider,
        tooltip: `${why}\n\nPress Enter for this service's fixes.`,
      };
    }
    if (!provider && gauge) {
      return {
        key: `usage:${encodeURIComponent(providerId)}`,
        name,
        icon: providerIcon(providerId, providers),
        detail: `Disconnected · ${usageDetail(gauge, nowMs)}`,
        reached: gauge.reached,
        state: "disconnected",
        provider: null,
        tooltip: `${name}: disconnected; this is the last report\n${usageTooltip(name, gauge, nowMs)}`,
      };
    }
    if (!gauge) {
      return {
        key: `usage:${encodeURIComponent(providerId)}`,
        name,
        icon: providerIcon(providerId, providers),
        detail: "No report yet",
        reached: false,
        state: "available",
        provider,
        tooltip: `${name}: no usage report yet`,
      };
    }
    return {
      key: `usage:${encodeURIComponent(providerId)}`,
      name,
      icon: providerIcon(providerId, providers),
      detail: usageDetail(gauge, nowMs),
      reached: gauge.reached,
      state: "available",
      provider,
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
