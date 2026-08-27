import type { ProviderLine, ProviderUsageGauge, ProviderUsageWindow } from "./runtimeTypes";
import { awaitsVerification, isBroken } from "./providerHealth";
import { providerDisplayName, providerIcon } from "./sessionDisplay";

/// `signedOut` is its own state because it has its own action (sign in), the way `unavailable` has "fix".
export type UsageState = "available" | "checking" | "unavailable" | "disconnected" | "signedOut";

/// One provider-reported account window that can be drawn without inventing a denominator.
export type UsageMeter = {
  /// Stable identity within one provider row: the window's own name, as its service gave it.
  readonly key: string;
  /// What this bar is a bar of, in as few characters as name it.
  ///
  /// The service's own scope when it scoped the limit to one model, its own label when it named the bucket,
  /// and the window's length otherwise. A row of three anonymous bars is a row nobody can act on.
  readonly label: string;
  /// A bounded value suitable for the HTML progressbar contract.
  readonly percent: number;
  /// The complete spoken value, including the reset when one exists.
  readonly detail: string;
  /// The service says this is the window governing right now, which is the one to read first.
  readonly governing: boolean;
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
      detail: unmeteredDetail(gauge, account, nowMs),
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
  // The service was asked and answered that it has no usage surface at all. Nothing arrives later.
  if (account.status === "unpublished") return "No usage published";
  // Signed in, and the numbers are not here. Two different silences: one is the service answering that this
  // account is metered somewhere the reader cannot see, and the other is a question that did not come back.
  // Sending either of them to sign in would be a lie, and they are not the same thing to say.
  if (account.limitsAbsent?.kind === "unmetered") return account.limitsAbsent.why;
  if (account.limitsAbsent) return "Usage unreadable";
  // Signed in, and the number rides on the service's own turn events: it exists, it has simply not been
  // said yet in this home. Saying "no usage published" here would be wrong the moment somebody typed.
  return "Usage arrives with the first turn";
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
  // The sentence itself, whichever silence it is: the service's own words for an account metered elsewhere,
  // and ours for a question that did not come back.
  if (account.limitsAbsent) return account.limitsAbsent.why;
  return "No limit reported yet";
}

/// Numeric account windows become real progress bars, one per window the service described.
///
/// One bar per window rather than a summary, because the windows are not versions of each other. Measured on
/// a real account: the five-hour window read 13%, the whole-account week 95%, and the week scoped to one
/// model 100%. Only the last one was refusing work, and any single number would have been the wrong one.
///
/// A reset-only report remains useful text but never becomes a deceptive empty bar, since the provider did
/// not say how much of that window was used.
export function usageMeters(gauge: ProviderUsageGauge, nowMs: number): UsageMeter[] {
  return (gauge.windows ?? []).flatMap((window) => {
    const percent = boundedPercent(window.usedPercent);
    if (percent === null) return [];
    const resets = resetsIn(window, nowMs);
    return [{
      key: window.id,
      label: meterLabel(window),
      percent,
      detail: `${percent}% used${resets ? `, ${resets}` : ""}`,
      governing: window.governing === true,
    }];
  });
}

/// The whole-account seven-day window used by the compact sidebar row.
///
/// Some services report model-scoped seven-day windows before the general window. The compact row still represents
/// the account, so an exact `7d` wins regardless of provider order. A scoped week is only a fallback when no general
/// week exists; every reported window remains available in the hover and details popup.
export function primarySevenDayMeter(meters: readonly UsageMeter[]): UsageMeter | null {
  return meters.find((meter) => meter.label === "7d")
    ?? meters.find((meter) => meter.label.startsWith("7d "))
    ?? null;
}

/// One reported percentage as a proportion a bar can draw, or null when the service reported none.
///
/// The one place a percentage is bounded, so every surface that shows the same window shows the same
/// number. A value outside nought to a hundred is the provider's own overrun and stays in its payload; a
/// bar cannot be longer than its track and digits that disagree with the bar beside them are worse than
/// either alone.
export function boundedPercent(percent: number | null | undefined): number | null {
  if (typeof percent !== "number" || !Number.isFinite(percent)) return null;
  return Math.max(0, Math.min(100, Math.round(percent)));
}

/// What one bar is a bar of, in the service's own words.
///
/// Never a phrase composed here. A service that scoped a limit to one model named that model, and a service
/// that named the bucket gave that name; the length is the fallback and also the disambiguator, because one
/// service meters the same model over both a short window and a long one.
export function meterLabel(window: ProviderUsageWindow): string {
  // A bar needs some text beside it even for a window its service named nothing and dated nothing, which is
  // a shape older builds of one CLI still produce.
  return windowName(window) ?? "limit";
}

/// The same name, absent when the service said nothing this could be built from.
///
/// Separate from [`meterLabel`] because the muted line reads differently: a bar with no name still needs a
/// caption, but a line that said "limit 48%" would have put a word there that no service used.
/// The longest a service's own name for something may be before the middle of it is dropped.
///
/// A sidebar is narrow and these names are not chosen with one in mind: one service calls a model bucket
/// `Fable` and another calls one `GPT-5.3-Codex-Spark`.
const NAME_BUDGET = 12;

/// A name too long for the row, shortened from the middle so the end of it survives.
///
/// Cut from the end, `GPT-5.3-Codex-Spark` becomes `GPT-5.3-Co…`, which is the half every one of that
/// service's buckets shares and none of the half that says which bucket. Cut from the middle it becomes
/// `GPT…Spark`, the vendor's word and the distinguishing word both. Nothing is renamed: the whole name
/// is on the row's hover and in the data, and this is only what fits on the row.
export function shortened(name: string | null): string | null {
  if (name === null || name.length <= NAME_BUDGET) return name;
  const head = name.slice(0, 3).replace(/[-_. ]+$/, "");
  const tail = name.slice(-5).replace(/^[-_. ]+/, "");
  return head + "…" + tail;
}

export function windowName(window: ProviderUsageWindow): string | null {
  const named = shortened(window.scope ?? window.label ?? null);
  const length = usageWindowLabel(window.windowMinutes, null);
  // Length first, because it is the short half and the one that tells two windows of the same model apart.
  // Measured the other way round in a real sidebar: one service meters the same model over five hours and
  // over a week, both bars read `GPT-5.3-Codex-...`, and the two lines were indistinguishable.
  if (named && length) return `${length} ${named}`;
  return named ?? length;
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

function usageWindowLabel(minutes: number | null | undefined, fallback: null): string | null;
function usageWindowLabel(minutes: number | null | undefined, fallback: string): string;
function usageWindowLabel(
  minutes: number | null | undefined,
  fallback: string | null,
): string | null {
  if (typeof minutes !== "number" || !Number.isFinite(minutes) || minutes <= 0) return fallback;
  if (minutes < 60) return `${minutes}m`;
  if (minutes < 2_880 && minutes % 60 === 0) return `${minutes / 60}h`;
  if (minutes % 1_440 === 0) return `${minutes / 1_440}d`;
  return `${minutes}m`;
}

/// The muted line: the account's position in the provider's own numbers, nothing invented.
///
/// One line for a service with several windows, so it names the window it is about. The one it is about is
/// the service's own governing window when it marks one, and otherwise the fullest, because that is the one
/// about to bite. Reading the shortest window instead showed 13% on an account that was already refusing
/// work on a window it never mentioned.
export function usageDetail(gauge: ProviderUsageGauge, nowMs: number): string {
  const parts: string[] = [];
  if (gauge.reached) parts.push("limit reached");
  const window = governingWindow(gauge);
  const named = window ? windowName(window) : null;
  // The same bounding the bar does, because the line and the bar are two readings of one number and a row
  // that said `250%` beside a bar drawn at full disagreed with itself.
  const percent = window ? boundedPercent(window.usedPercent) : null;
  if (percent !== null) {
    parts.push(named ? `${named} ${percent}%` : `${percent}%`);
  } else if (named) {
    // No percentage, so the line names the window itself. Measured: a service that publishes its usage
    // period and no number for it read as a bare "resets in 4d", which says when without saying what.
    parts.push(named);
  }
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

/// The line for a service that reported windows, with its own reason first when none of them carry a number.
///
/// A period with a reset and no bar reads as something that failed. Measured on a real account: one service
/// answers about the plan and the period and states no percentage at all, because that account is metered by
/// a team the operator cannot see. The service has a word for that and the row leads with it, because it is
/// the answer to the question the missing bar asks.
export function unmeteredDetail(
  gauge: ProviderUsageGauge,
  account: ProviderLine["account"] | null | undefined,
  nowMs: number,
): string {
  const detail = usageDetail(gauge, nowMs);
  const metered = (gauge.windows ?? []).some((window) => boundedPercent(window.usedPercent) !== null);
  // Only the service's own reason leads a line. A failure of ours is a thing to fix, not a caption for a
  // number the service did give.
  if (metered || account?.limitsAbsent?.kind !== "unmetered") return detail;
  return `${account.limitsAbsent.why} · ${detail}`;
}

/// The one window this row's line is about.
///
/// The service's own word first: it marks the window governing right now, and measured, it marks one that is
/// not the fullest when the two meter different things. Failing that, the fullest, then whatever exists.
export function governingWindow(gauge: ProviderUsageGauge): ProviderUsageWindow | null {
  const windows = gauge.windows ?? [];
  const declared = windows.find((window) => window.governing === true);
  if (declared) return declared;
  let fullest: ProviderUsageWindow | null = null;
  for (const window of windows) {
    if (boundedPercent(window.usedPercent) === null) continue;
    if (!fullest || (window.usedPercent ?? 0) > (fullest.usedPercent ?? -1)) fullest = window;
  }
  return fullest ?? windows[0] ?? null;
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

/// The hover: every window the service described, spelled out.
///
/// The row has room for one line and a stack of bars; this is where the rest goes, including the length of
/// each window and the model a scoped limit applies to.
function usageTooltip(name: string, gauge: ProviderUsageGauge, nowMs: number): string {
  const lines = [`${name}: ${gauge.reached ? "a limit is blocking right now" : "within limits"}`];
  for (const window of gauge.windows ?? []) {
    const pieces: string[] = [];
    const percent = boundedPercent(window.usedPercent);
    if (percent !== null) pieces.push(`${percent}% used`);
    if (typeof window.windowMinutes === "number") {
      pieces.push(`${window.windowMinutes} minute window`);
    }
    const resets = resetsIn(window, nowMs);
    if (resets) pieces.push(resets);
    if (window.governing === true) pieces.push("governing now");
    lines.push(pieces.length > 0 ? `${meterLabel(window)}: ${pieces.join(", ")}` : meterLabel(window));
  }
  const age = Math.max(0, Math.round((nowMs - gauge.atMs) / 60_000));
  lines.push(age < 1 ? "Reported just now" : `Reported ${age}m ago`);
  return lines.join("\n");
}
