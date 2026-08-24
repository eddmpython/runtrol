import type { ProviderLine, ProviderUsageGauge, ProviderUsageWindow } from "./runtimeTypes";
import { awaitsVerification, isBroken } from "./providerHealth";
import { providerDisplayName, providerIcon } from "./sessionDisplay";

export type UsageState = "available" | "checking" | "unavailable" | "disconnected";

/// One provider-reported account window that can be drawn without inventing a denominator.
export type UsageMeter = {
  /// Stable identity within one provider row.
  readonly key: "primary" | "secondary";
  /// The provider's window duration when known, otherwise its structural position.
  readonly label: string;
  /// A bounded value suitable for the HTML progressbar contract.
  readonly percent: number;
  /// The complete spoken value, including the reset when one exists.
  readonly detail: string;
};

/// One line of the usage strip, ready to draw.
export type UsageRow = {
  /// Stable view identity.
  readonly key: string;
  /// The service's name, which is the row's label.
  readonly name: string;
  /// The editor glyph that stands for the service.
  readonly icon: string;
  /// The compact operational or usage summary.
  readonly detail: string;
  /// Every numeric account window the provider reported. No percentage means no empty bar.
  readonly meters: readonly UsageMeter[];
  /// Whether a limit is blocking right now, which decides the row's colour.
  readonly reached: boolean;
  /// The service's operational state. Usage is not allowed to hide a service that cannot currently run.
  readonly state: UsageState;
  /// The runtime identity used to validate an actionable unavailable row against the live provider snapshot.
  readonly providerId: string;
  /// The whole position for the hover, sentence by sentence.
  readonly tooltip: string;
};

/// Whether publishing the next snapshot would change anything visible or actionable in the tree.
/// Comparing complete rows keeps provider-owned fix arguments as fresh as their label and status.
export function usageRowsEqual(left: readonly UsageRow[], right: readonly UsageRow[]): boolean {
  return JSON.stringify(left) === JSON.stringify(right);
}

/// A compact text mark for the account meter Webview, which cannot reuse a TreeItem's editor glyph.
///
/// The mark comes from the discovered display name rather than a provider table. Two-word services use their
/// initials; a one-word service uses its first two characters. This keeps similarly named services such as
/// Claude Code and Codex visually distinct without making provider support part of this extension's source.
export function providerMark(name: string): string {
  const words = name.match(/[\p{L}\p{N}]+/gu) ?? [];
  if (words.length >= 2) {
    return words.slice(0, 2).map((word) => Array.from(word)[0] ?? "").join("").toLocaleUpperCase();
  }
  const characters = Array.from(words[0] ?? "");
  return characters.slice(0, 2).join("").toLocaleUpperCase() || "?";
}

/// Registry-backed coding services that are not installed and publish one exact operator-run install command.
export function installableProviders(providers: readonly ProviderLine[]): ProviderLine[] {
  return providers
    .filter((provider) => provider.installation.state === "missing" && Boolean(provider.help?.install))
    .sort((left, right) => left.displayName.localeCompare(right.displayName, "en"));
}

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
        meters: [],
        reached: false,
        state: "checking",
        providerId,
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
        meters: [],
        reached: false,
        state: "unavailable",
        providerId,
        tooltip: `${why}\n\nPress Enter for this service's fixes.`,
      };
    }
    if (!provider && gauge) {
      return {
        key: `usage:${encodeURIComponent(providerId)}`,
        name,
        icon: providerIcon(providerId, providers),
        detail: `Disconnected · ${usageDetail(gauge, nowMs)}`,
        meters: usageMeters(gauge, nowMs),
        reached: gauge.reached,
        state: "disconnected",
        providerId,
        tooltip: `${name}: disconnected; this is the last report\n${usageTooltip(name, gauge, nowMs)}`,
      };
    }
    if (!gauge) {
      return {
        key: `usage:${encodeURIComponent(providerId)}`,
        name,
        icon: providerIcon(providerId, providers),
        detail: "Ready",
        meters: [],
        reached: false,
        state: "available",
        providerId,
        tooltip: `${name}: ready. Usage appears here when the CLI reports an account limit.`,
      };
    }
    return {
      key: `usage:${encodeURIComponent(providerId)}`,
      name,
      icon: providerIcon(providerId, providers),
      detail: usageDetail(gauge, nowMs),
      meters: usageMeters(gauge, nowMs),
      reached: gauge.reached,
      state: "available",
      providerId,
      tooltip: usageTooltip(name, gauge, nowMs),
    };
  });
}

/// Numeric account windows become real progress bars. A reset-only report remains useful text but never becomes
/// a deceptive empty bar, since the provider did not say how much of that window was used.
export function usageMeters(gauge: ProviderUsageGauge, nowMs: number): UsageMeter[] {
  return ([
    ["primary", gauge.primary, "Current"],
    ["secondary", gauge.secondary, "Longer"],
  ] as const).flatMap(([key, window, fallback]) => {
    if (!window || typeof window.usedPercent !== "number" || !Number.isFinite(window.usedPercent)) return [];
    const percent = Math.max(0, Math.min(100, Math.round(window.usedPercent)));
    const resets = resetsIn(window, nowMs);
    return [{
      key,
      label: usageWindowLabel(window.windowMinutes, fallback),
      percent,
      detail: `${percent}% used${resets ? `, ${resets}` : ""}`,
    }];
  });
}

function usageWindowLabel(minutes: number | null | undefined, fallback: string): string {
  if (typeof minutes !== "number" || !Number.isFinite(minutes) || minutes <= 0) return fallback;
  if (minutes < 60) return `${minutes}m`;
  if (minutes < 2_880 && minutes % 60 === 0) return `${minutes / 60}h`;
  if (minutes % 1_440 === 0) return `${minutes / 1_440}d`;
  return `${minutes}m`;
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
