import type { ProviderLine, ProviderUsageGauge, ProviderUsageWindow } from "./runtimeTypes";
import { awaitsVerification, isBroken } from "./providerHealth";
import { providerDisplayName, providerIcon } from "./sessionDisplay";

/// `signedOut` is its own state because it has its own action (sign in), the way `unavailable` has "fix".
export type UsageState = "available" | "checking" | "unavailable" | "disconnected" | "signedOut";

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
  /// The provider's own running spend, formatted for one glance, or null when it reported none. Drawn on its
  /// own line marked by the service glyph, never by repeating the service name.
  readonly cost: string | null;
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

/// What one coding service still needs before it can hold a conversation.
export type SetupState = "ready" | "signedOut" | "missing" | "unavailable";

/// One service in the set-up list, which is every service this build serves rather than a catalogue.
export type SetupRow = {
  /// The service's runtime identity, carried back with the action.
  readonly providerId: string;
  /// The service's own name.
  readonly name: string;
  /// The glyph that stands for it, the same one its usage row uses.
  readonly icon: string;
  /// What it needs.
  readonly state: SetupState;
  /// The single sentence under the name.
  readonly detail: string;
  /// Whether pressing this row does anything.
  readonly actionable: boolean;
};

/// Every service this build serves, with what each one still needs.
///
/// Every one, not only the absent ones: a set-up list that hides what is already working answers "which services
/// do I have" with silence, and that is the question someone opens it with. The set is what this build ships,
/// so it is short by design and nobody is offered a service that was never measured here.
export function setupRows(providers: readonly ProviderLine[]): SetupRow[] {
  return providers.map((provider) => {
    const base = {
      providerId: provider.providerId,
      name: provider.displayName,
      icon: providerIcon(provider.providerId, providers),
    };
    if (provider.installation.state === "missing") {
      const install = provider.help?.install ?? null;
      return {
        ...base,
        state: "missing" as const,
        detail: install ? "Not installed. Its command goes to your terminal" : "Not installed",
        actionable: Boolean(install),
      };
    }
    if (isBroken(provider)) {
      return {
        ...base,
        state: "unavailable" as const,
        detail: provider.installation.why ?? "Installed but cannot start",
        actionable: true,
      };
    }
    if (provider.account?.status === "signedOut") {
      return { ...base, state: "signedOut" as const, detail: "Not signed in", actionable: true };
    }
    return { ...base, state: "ready" as const, detail: "Ready", actionable: false };
  });
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
        cost: null,
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
        cost: null,
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
        cost: usageCost(gauge),
        tooltip: `${name}: disconnected; this is the last report\n${usageTooltip(name, gauge, nowMs)}`,
      };
    }
    const account = provider?.account ?? null;
    if (account?.status === "signedOut") {
      return {
        key: `usage:${encodeURIComponent(providerId)}`,
        name,
        icon: providerIcon(providerId, providers),
        detail: "Not signed in · Sign in",
        // A window a turn reported stays visible: the service's own number outranks its sign-in verdict.
        meters: gauge ? usageMeters(gauge, nowMs) : [],
        reached: gauge?.reached ?? false,
        state: "signedOut",
        providerId,
        cost: gauge ? usageCost(gauge) : null,
        tooltip: `${name} says nobody is signed in.\n\nPress Enter to sign in with this service's own command.`,
      };
    }
    const plan = accountLine(account);
    if (!gauge) {
      // No bar, so the row says why in as few words as it takes, and the rest goes to the hover. The plan is not
      // that reason: `max plan via claude.ai` on a row with no bar reads like the bar is coming, when what is
      // true is that the service publishes no number for one (measured on Claude Code 2.1.235 and 2.1.246: no
      // command, no stream field, no file). A row that states its cause can be acted on; a row that states a
      // plan cannot.
      return {
        key: `usage:${encodeURIComponent(providerId)}`,
        name,
        icon: providerIcon(providerId, providers),
        detail: usageAbsenceCause(account),
        meters: [],
        reached: false,
        state: "available",
        providerId,
        cost: null,
        tooltip: plan
          ? `${name}: ${plan}. ${accountAbsence(name, account)}`
          : `${name}: ${accountAbsence(name, account)}`,
      };
    }
    return {
      key: `usage:${encodeURIComponent(providerId)}`,
      name,
      icon: providerIcon(providerId, providers),
      // The bar is the row. The plan the service named is true but not what this line is for, and prefixing it
      // pushed the number a person came to read off the end of a narrow sidebar. It is in the hover.
      detail: usageDetail(gauge, nowMs),
      meters: usageMeters(gauge, nowMs),
      reached: gauge.reached,
      state: "available",
      providerId,
      cost: usageCost(gauge),
      tooltip: plan ? `${name}: ${plan}\n${usageTooltip(name, gauge, nowMs)}` : usageTooltip(name, gauge, nowMs),
    };
  });
}

/// Why this row has no bar, in the fewest words that name a cause rather than a symptom.
///
/// Three causes and three sentences, because they need three different things from the reader: one is theirs to
/// fix by signing in, one is the service's own limitation and nothing to act on, and one is a check that has not
/// finished. Collapsing them would send someone to sign in when they already are.
export function usageAbsenceCause(account: ProviderLine["account"] | null | undefined): string {
  // Not the bare "Checking" an unprobed install says: that one is about the executable, this one is about the
  // account, and a reader who saw the same word twice would not know which had stalled.
  if (!account) return "Checking usage";
  if (account.status === "signedOut") return "Not signed in · Sign in";
  return "No usage published";
}

/// The plan and sign-in method the service named, in its own tokens, or null when it named none.
///
/// "max plan" rather than a marketing name: the service said `max`, and that is the whole claim.
export function accountLine(account: ProviderLine["account"] | null | undefined): string | null {
  if (!account || account.status !== "signedIn") return null;
  const parts: string[] = [];
  if (account.plan) parts.push(`${account.plan} plan`);
  if (account.method && account.method !== account.plan) parts.push(`via ${account.method}`);
  return parts.length > 0 ? parts.join(" ") : "Signed in";
}

/// What to say when no usage number exists, in terms of what was actually asked.
///
/// Never "Ready": that claimed a state nobody had checked. Before the first check the line says so; a
/// service without a status surface is named as that; a signed-in service that reported no limit yet says
/// when one will show.
export function accountAbsence(name: string, account: ProviderLine["account"] | null | undefined): string {
  if (!account) return "Not checked yet";
  if (account.status === "unpublished") {
    return `${name} publishes no usage or sign-in status`;
  }
  return "No limit reported yet";
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

/// The account's running spend as the provider stated it, formatted for one glance.
///
/// The provider's own number and currency, never converted and never summed across sessions: this is the newest
/// report's figure. A currency the surface has no symbol for is shown with its code so nothing is misread.
export function usageCost(gauge: ProviderUsageGauge): string | null {
  const cost = gauge.cost;
  if (!cost || typeof cost.amount !== "number" || !Number.isFinite(cost.amount)) return null;
  if (cost.currency === "USD") {
    if (cost.amount > 0 && cost.amount < 0.01) return "<$0.01";
    return `$${cost.amount.toFixed(2)}`;
  }
  return `${cost.amount.toFixed(2)} ${cost.currency}`;
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
  if (typeof gauge.tokensToday === "number") parts.push(`${formatTokens(gauge.tokensToday)} today`);
  if (parts.length === 0) {
    // "Within limits" is a claim about a limit, so it is only made for a service that described one. A service
    // that reported nothing but a spend gets said as exactly that, rather than being credited with room it
    // never mentioned having.
    parts.push(window ? "within limits" : "no limit reported");
  }
  return parts.join(" · ");
}

/// Today's tokens by the service's own daily count, short enough for the strip.
export function formatTokens(tokens: number): string {
  if (tokens >= 1_000_000_000) return `${(tokens / 1_000_000_000).toFixed(1)}B tokens`;
  if (tokens >= 1_000_000) return `${(tokens / 1_000_000).toFixed(1)}M tokens`;
  if (tokens >= 1_000) return `${Math.round(tokens / 1_000)}k tokens`;
  return `${tokens} tokens`;
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
