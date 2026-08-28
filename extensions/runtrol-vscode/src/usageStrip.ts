/// The usage strip: one chip per installed coding service, a ring gauge around its icon, and a detail panel
/// that opens on hover, focus, or Enter.
///
/// The strip is the one graphic surface Studio draws itself. A native tree cannot draw a ring or a bar, and
/// the operator asked for a real gauge (2026-08-27): the icon is the whole label, so the chip's width never
/// depends on a service's name, and the ring reads at a glance however many services are installed. The
/// panel below the chips carries every window the service reported, each as a thin bar with its own name, so
/// a model-scoped week and a five-hour window are told apart without a click.
///
/// Everything here is a pure function of `UsageRow`: the host renders, the page only shows and hides. The
/// page never asks a provider anything and never sees a conversation; it is drawn from the same rows the
/// tooltip was.

import type { UsageMeter, UsageRow, UsageState } from "./usageDisplay";
import { primarySevenDayMeter } from "./usageDisplay";

/// One chip, ready to draw.
export type UsageChip = {
  readonly providerId: string;
  /// The service's name, spoken by the panel and by assistive technology; never drawn beside the ring.
  readonly name: string;
  /// The declared icon name, resolved to a page URI by the host.
  readonly icon: string;
  /// The ring's value: the whole-account week when the service published one, else the governing window,
  /// else null and the ring is empty.
  readonly percent: number | null;
  /// The gauge's layers, outermost first: the week, then the five-hour window, then the first model-scoped
  /// window. Each is a concentric ring; hover says which is which (the operator's 2026-08-27 instruction).
  readonly rings: ReadonlyArray<{ readonly label: string; readonly percent: number }>;
  /// The word under the ring when there is no number, in as few characters as name a cause.
  readonly caption: string;
  /// A limit is blocking right now, which colours the ring.
  readonly reached: boolean;
  readonly state: UsageState;
  /// Where the account stands, in one short clause. Never an instruction: actions are buttons.
  readonly position: string;
  /// The plan the service named, or null.
  readonly plan: string | null;
  /// How old the last report is, or null.
  readonly age: string | null;
  readonly meters: readonly UsageMeter[];
  /// The one action the panel offers, or null when the row is only information.
  readonly action: "signIn" | "fix" | null;
  /// The service publishes a sign-in line, so the panel can offer to sign in whatever the account's state is.
  readonly canSignIn: boolean;
};

/// Rows to chips. The order is the rows' order, which is the services' order.
export function usageChips(
  rows: readonly UsageRow[],
  /// The services that publish a sign-in line of their own.
  signInAble: ReadonlySet<string> = new Set(),
): UsageChip[] {
  return rows.map((row) => {
    // The ring is the week when the service published one; otherwise the window it says governs, so a
    // five-hour limit that is blocking right now is never hidden behind "No report".
    const shown = primarySevenDayMeter(row.meters)
      ?? row.meters.find((meter) => meter.governing)
      ?? row.meters[0]
      ?? null;
    const week = primarySevenDayMeter(row.meters);
    const fiveHour = row.meters.find((meter) => meter.label === "5h" || meter.label.startsWith("5h "));
    const modelScoped = row.meters.find((meter) => meter !== week && meter !== fiveHour && meter.label.includes(" "));
    const rings = [week, fiveHour, modelScoped]
      .filter((meter): meter is UsageMeter => meter !== null && meter !== undefined)
      .map((meter) => ({ label: meter.label, percent: meter.percent }));
    return {
      providerId: row.providerId,
      name: row.name,
      icon: row.icon,
      percent: shown ? shown.percent : null,
      rings: rings.length > 0 ? rings : shown ? [{ label: shown.label, percent: shown.percent }] : [],
      caption: shown ? `${shown.percent}%` : shortCaption(row),
      reached: row.reached,
      state: row.state,
      position: row.position,
      plan: row.plan,
      age: row.age,
      meters: row.meters,
      action: chipAction(row, shown !== undefined && shown !== null),
      canSignIn: signInAble.has(row.providerId),
    };
  });
}

/// What pressing this chip does.
///
/// A chip is a button, so pressing it goes straight into the one thing worth doing for that account rather
/// than opening something to read (the operator's rule, and 2026-08-28: pressing a chip should reach that
/// provider's sign-in). A service with a figure to show has its hover panel, which is the reading surface; a
/// service that shows nothing has nothing to read, and signing in is the only lever this surface holds.
function chipAction(row: UsageRow, hasFigure: boolean): "signIn" | "fix" | null {
  if (row.state === "unavailable") return "fix";
  if (row.state === "signedOut" || row.state === "disconnected") return "signIn";
  // Still asking. Offering to sign in to an account nobody has finished checking would answer a question that
  // has not been asked yet.
  if (row.state === "checking") return null;
  return hasFigure ? null : "signIn";
}

/// The word under an empty ring. Short because the chip is narrow; the panel says the whole sentence.
///
/// A service that answered gets its own words. "No report" is true of a service nobody has heard from, and
/// measured, one service answers about the plan and the period and states no percentage because that account
/// is metered by a team: saying it made no report about the one thing it did report is the plainest way a
/// usage surface can lie.
function shortCaption(row: UsageRow): string {
  switch (row.state) {
    case "checking":
      return "Checking";
    case "unavailable":
      return "Fix";
    case "signedOut":
      return "Sign in";
    case "disconnected":
      return "Offline";
    case "available":
      return row.unmetered ?? "No report";
  }
}

/// What the page needs from the host besides the chips.
export type UsageStripAssets = {
  readonly cspSource: string;
  readonly nonce: string;
  /// Declared icon name to page URI, for every chip.
  readonly iconUris: ReadonlyMap<string, string>;
};

/// Outermost first. Three layers at most: the week, the five-hour window, a model-scoped window.
const RING_RADII = [11, 8, 5] as const;

/// The chips, as markup the sidebar page places in its usage zone.
export function usageChipsMarkup(chips: readonly UsageChip[], assets: UsageStripAssets): string {
  if (chips.length === 0) return `<p class="empty">No coding service is installed yet.</p>`;
  return `<div class="chips" role="list">${chips.map((chip, index) => chipHtml(chip, index, assets)).join("")}</div>`;
}

/// The panels behind the chips, one per chip, hidden until a chip is hovered, focused or pressed.
export function usagePanelsMarkup(chips: readonly UsageChip[]): string {
  return `<div class="panels">${chips.map((chip, index) => panelHtml(chip, index)).join("")}</div>`;
}

/// The whole strip as a standalone page, which is what the unit tests render.
export function usageStripHtml(chips: readonly UsageChip[], assets: UsageStripAssets): string {
  return `<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; img-src ${assets.cspSource}; style-src 'nonce-${assets.nonce}'; script-src 'nonce-${assets.nonce}'">
<meta name="viewport" content="width=device-width, initial-scale=1">
<style nonce="${assets.nonce}">${USAGE_STYLE}</style>
</head>
<body>
${usageChipsMarkup(chips, assets)}
${chips.length === 0 ? "" : usagePanelsMarkup(chips)}
<script nonce="${assets.nonce}">${USAGE_SCRIPT}</script>
</body>
</html>`;
}

/// A bar's fill, as a class rather than as a `style` attribute.
///
/// The page's policy allows styles from its own nonced block and from nowhere else, and a `style` attribute
/// carries no nonce, so every fill this drew was dropped without a word and each window's bar read as empty.
/// The identical mistake had already cost the project colour band days of being invisible (2026-08-28), which
/// is why the fill is a class and the rules for every whole percent are in the stylesheet.
function widthClass(percent: number): string {
  return `w${Math.max(0, Math.min(100, Math.round(percent)))}`;
}

/// One rule per whole percent, generated once. A hundred and one short rules is smaller than the code any
/// scheme for emitting only the percents in use would need, and it cannot go stale.
const WIDTH_STYLE = Array.from({ length: 101 }, (_unused, at) => `.meter .value.w${at} { width: ${at}%; }`).join(" ");

function chipHtml(chip: UsageChip, index: number, assets: UsageStripAssets): string {
  const iconUri = assets.iconUris.get(chip.icon) ?? "";
  const spoken = chip.percent === null
    ? `${chip.name}: ${chip.caption}`
    : `${chip.name}: seven day usage ${chip.percent} percent${chip.reached ? ", a limit is blocking" : ""}`;
  // No native tooltip: the hover panel is the one detail surface, and a browser tooltip floating over it was
  // read as two competing popups (operator, 2026-08-27). Screen readers keep the spoken summary.
  const direct = chip.action ? ` data-action="${chip.action}" data-provider="${escapeHtml(chip.providerId)}"` : "";
  return `<button class="chip${chip.reached ? " reached" : ""}${chip.percent === null ? " bare" : ""}" type="button" role="listitem" data-index="${index}"${direct} aria-label="${escapeHtml(spoken)}" aria-expanded="false" aria-controls="panel-${index}">
<span class="ring">
<svg viewBox="0 0 26 26" aria-hidden="true">
${chip.rings.slice(0, RING_RADII.length).map((ring, at) => {
    const radius = RING_RADII[at]!;
    const around = 2 * Math.PI * radius;
    const filled = (ring.percent / 100) * around;
    return `<circle class="track" cx="13" cy="13" r="${radius}"></circle>
<circle class="fill" cx="13" cy="13" r="${radius}" stroke-dasharray="${filled.toFixed(2)} ${around.toFixed(2)}"></circle>`;
  }).join("")}
${chip.rings.length === 0 ? `<circle class="track" cx="13" cy="13" r="${RING_RADII[0]}"></circle>` : ""}
</svg>
<img class="icon" src="${escapeHtml(iconUri)}" alt="" draggable="false">
</span>
<span class="caption">${escapeHtml(chip.caption)}</span>
</button>`;
}

function panelHtml(chip: UsageChip, index: number): string {
  // One sentence per window was read as noise (operator, 2026-08-27): a row is its bar and its percent, and
  // only the window that is actually governing keeps its words (its reset is the one actionable fact here).
  const bars = chip.meters.map((meter) => `<div class="meter${meter.governing ? " governing" : ""}">
<span class="label">${escapeHtml(meter.label)}</span>
<span class="percent">${meter.percent}%</span>
<span class="bar" role="progressbar" aria-valuemin="0" aria-valuemax="100" aria-valuenow="${meter.percent}" aria-label="${escapeHtml(`${meter.label} ${meter.percent} percent`)}"><span class="value ${widthClass(meter.percent)}"></span></span>
${meter.governing ? `<span class="detail">${escapeHtml(meter.detail)}</span>` : ""}
</div>`).join("");
  return `<section class="panel" id="panel-${index}" hidden>
<h2>${escapeHtml(chip.name)}${chip.plan ? ` <span class="plan">${escapeHtml(chip.plan)}</span>` : ""}</h2>
<p class="position${chip.reached ? " reached" : ""}">${escapeHtml(chip.position)}</p>
${bars}
${chip.age ? `<p class="age">${escapeHtml(chip.age)}</p>` : ""}
${chip.canSignIn && chip.action !== "signIn" ? `<button class="action" type="button" data-action="signIn" data-provider="${escapeHtml(chip.providerId)}">Sign in to ${escapeHtml(chip.name)}</button>` : ""}
</section>`;
}

export function escapeHtml(text: string): string {
  return text
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#39;");
}

export const USAGE_STYLE = `
body { margin: 0; padding: 6px 8px; color: var(--vscode-foreground); font: var(--vscode-font-size) var(--vscode-font-family); background: transparent; }
.empty { margin: 0; opacity: 0.8; }
.chips { display: flex; flex-wrap: wrap; gap: 2px 6px; }
.chip { display: flex; flex-direction: column; align-items: center; gap: 0; min-width: 38px; max-width: 86px; padding: 2px 3px 1px; border: 1px solid transparent; border-radius: 5px; background: transparent; color: inherit; cursor: pointer; }
.chip:hover, .chip[aria-expanded="true"] { background: var(--vscode-list-hoverBackground); }
.chip:focus-visible { outline: none; border-color: var(--vscode-focusBorder); }
.ring { position: relative; width: 26px; height: 26px; }
.ring svg { width: 26px; height: 26px; transform: rotate(-90deg); }
.ring .track { fill: none; stroke: var(--vscode-widget-border, rgba(128,128,128,0.35)); stroke-width: 2; }
.ring .fill { fill: none; stroke: var(--vscode-progressBar-background); stroke-width: 2; stroke-linecap: round; }
.chip.reached .fill { stroke: var(--vscode-errorForeground); }
.chip.bare .fill { display: none; }
.ring .icon { position: absolute; left: 8.5px; top: 8.5px; width: 9px; height: 9px; }
/* The caption belongs to its own chip. It was a fixed 38px box with no overflow rule, so a service's own
   word for its account ("team-managed") spilled across its neighbours and read as one run-on word,
   "20%team-managed" (operator's window, 2026-08-28). The chip grows for a longer word up to a cap, and past
   that the word is cut with an ellipsis: the hover panel says the whole sentence. */
.caption { max-width: 100%; overflow: hidden; text-overflow: ellipsis; font-size: 10px; line-height: 12px; opacity: 0.9; white-space: nowrap; }
.chip.reached .caption { color: var(--vscode-errorForeground); }
.panels { margin-top: 4px; }
.panel { padding: 6px 4px 2px; border-top: 1px solid var(--vscode-widget-border, rgba(128,128,128,0.35)); }
.panel h2 { margin: 0 0 2px; font-size: var(--vscode-font-size); font-weight: 600; }
.panel .plan { font-weight: 400; opacity: 0.75; }
.panel p { margin: 0 0 4px; opacity: 0.9; }
.panel .position.reached { color: var(--vscode-errorForeground); opacity: 1; }
/* The name has the line, and the bar sits under it. A model window is named by its model, and a model name
   does not fit beside a bar in a panel this wide: the column was 72px and the name was the part that
   disappeared (operator, 2026-08-28). */
.meter { display: grid; grid-template-columns: minmax(0, 1fr) auto; gap: 1px 6px; align-items: baseline; margin: 5px 0; }
.meter .label { min-width: 0; font-size: 11px; overflow-wrap: anywhere; }
.meter.governing .label { font-weight: 600; }
.meter .bar { grid-column: 1 / -1; display: block; height: 4px; border-radius: 2px; background: var(--vscode-widget-border, rgba(128,128,128,0.35)); overflow: hidden; }
.meter .value { display: block; height: 100%; border-radius: 2px; background: var(--vscode-progressBar-background); width: 0; }
${WIDTH_STYLE}
.meter .percent { font-variant-numeric: tabular-nums; text-align: right; font-size: 11px; }
.meter .detail { grid-column: 1 / -1; font-size: 11px; opacity: 0.75; }
.panel .age { font-size: 11px; opacity: 0.7; }
.action { margin: 2px 0 4px; padding: 2px 10px; border: 1px solid var(--vscode-button-border, transparent); border-radius: 2px; background: var(--vscode-button-background); color: var(--vscode-button-foreground); cursor: pointer; }
.action:hover { background: var(--vscode-button-hoverBackground); }
`;

// Hover previews, focus previews, Enter or click pins, Escape and a second press close. One panel at a time.
export const USAGE_SCRIPT = `
(function () {
  var vscode = window.__runtrolVsCodeApi || (window.__runtrolVsCodeApi = acquireVsCodeApi());
  var chips = [];
  var panels = [];
  // Which panel the person opened. It lives outside the binding so that a repaint, which is a figure ticking
  // and nothing they did, leaves their open panel open (2026-08-28).
  var pinned = null;
  // A hover preview must not move anything: scrolling the panel into view moved the chip out from under the
  // pointer, which closed the panel, which scrolled back and reopened it, and the strip flickered (2026-08-27).
  // Only a pinned panel (a click, or keyboard focus) is brought into the short view.
  function show(index, settle) {
    chips.forEach(function (chip, at) { chip.setAttribute("aria-expanded", at === index ? "true" : "false"); });
    panels.forEach(function (panel, at) { panel.hidden = at !== index; });
    if (!settle) return;
    if (index === null) { window.scrollTo(0, 0); return; }
    var open = panels[index];
    if (open) open.scrollIntoView({ block: "nearest" });
  }
  function settle() { show(pinned, true); }
  function bind() {
  chips = Array.prototype.slice.call(document.querySelectorAll(".chip"));
  panels = Array.prototype.slice.call(document.querySelectorAll(".panel"));
  if (pinned !== null && pinned >= chips.length) pinned = null;
  chips.forEach(function (chip, index) {
    chip.addEventListener("mouseenter", function () { if (pinned === null) show(index, false); });
    chip.addEventListener("focus", function () { if (pinned === null) show(index, true); });
    chip.addEventListener("click", function () {
      var direct = chip.dataset.action;
      if (direct) { vscode.postMessage({ type: "action", action: direct, providerId: chip.dataset.provider }); return; }
      pinned = pinned === index ? null : index;
      show(pinned, true);
    });
    chip.addEventListener("keydown", function (event) {
      if (event.key === "Escape") { pinned = null; show(null, true); }
    });
  });
  var strip = document.querySelector(".chips");
  if (strip) strip.addEventListener("mouseleave", settle);
  Array.prototype.slice.call(document.querySelectorAll(".action")).forEach(function (button) {
    button.addEventListener("click", function () {
      vscode.postMessage({ type: "action", action: button.dataset.action, providerId: button.dataset.provider });
    });
  });
  show(pinned, false);
  }
  window.__runtrolBindUsage = bind;
  bind();
  document.addEventListener("focusout", function (event) {
    if (!event.relatedTarget || !document.body.contains(event.relatedTarget)) settle();
  });
  document.addEventListener("keydown", function (event) {
    if (event.key === "Escape") { pinned = null; show(null, true); }
  });
})();
`;
