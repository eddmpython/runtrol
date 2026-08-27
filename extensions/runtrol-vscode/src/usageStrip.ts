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
  /// The word under the ring when there is no number, in as few characters as name a cause.
  readonly caption: string;
  /// A limit is blocking right now, which colours the ring.
  readonly reached: boolean;
  readonly state: UsageState;
  /// The panel's lines, in order: the position sentence, one bar per window, the report age.
  readonly lines: readonly string[];
  readonly meters: readonly UsageMeter[];
  /// The one action the panel offers, or null when the row is only information.
  readonly action: "signIn" | "fix" | null;
};

/// Rows to chips. The order is the rows' order, which is the services' order.
export function usageChips(rows: readonly UsageRow[]): UsageChip[] {
  return rows.map((row) => {
    // The ring is the week when the service published one; otherwise the window it says governs, so a
    // five-hour limit that is blocking right now is never hidden behind "No report".
    const shown = primarySevenDayMeter(row.meters)
      ?? row.meters.find((meter) => meter.governing)
      ?? row.meters[0]
      ?? null;
    return {
      providerId: row.providerId,
      name: row.name,
      icon: row.icon,
      percent: shown ? shown.percent : null,
      caption: shown ? `${shown.percent}%` : shortCaption(row),
      reached: row.reached,
      state: row.state,
      lines: row.tooltip.split("\n").filter((line) => line.length > 0),
      meters: row.meters,
      action: row.state === "signedOut" ? "signIn" : row.state === "unavailable" ? "fix" : null,
    };
  });
}

/// The word under an empty ring. Short because the chip is narrow; the panel says the whole sentence.
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
      return "No report";
  }
}

/// What the page needs from the host besides the chips.
export type UsageStripAssets = {
  readonly cspSource: string;
  readonly nonce: string;
  /// Declared icon name to page URI, for every chip.
  readonly iconUris: ReadonlyMap<string, string>;
};

const RING_RADIUS = 15;
const RING_CIRCUMFERENCE = 2 * Math.PI * RING_RADIUS;

/// The whole page. Static markup for the chips and panels; the script only toggles which panel is open.
export function usageStripHtml(chips: readonly UsageChip[], assets: UsageStripAssets): string {
  const body = chips.length === 0
    ? `<p class="empty">No coding service is installed yet.</p>`
    : `<div class="chips" role="list">${chips.map((chip, index) => chipHtml(chip, index, assets)).join("")}</div>
<div class="panels">${chips.map((chip, index) => panelHtml(chip, index)).join("")}</div>`;
  return `<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; img-src ${assets.cspSource}; style-src 'nonce-${assets.nonce}'; script-src 'nonce-${assets.nonce}'">
<meta name="viewport" content="width=device-width, initial-scale=1">
<style nonce="${assets.nonce}">${STYLE}</style>
</head>
<body>
${body}
<script nonce="${assets.nonce}">${SCRIPT}</script>
</body>
</html>`;
}

function chipHtml(chip: UsageChip, index: number, assets: UsageStripAssets): string {
  const filled = chip.percent === null ? 0 : (chip.percent / 100) * RING_CIRCUMFERENCE;
  const iconUri = assets.iconUris.get(chip.icon) ?? "";
  const spoken = chip.percent === null
    ? `${chip.name}: ${chip.caption}`
    : `${chip.name}: seven day usage ${chip.percent} percent${chip.reached ? ", a limit is blocking" : ""}`;
  // The hover preview is the browser's own tooltip: it floats outside the view's box, so it is readable however
  // short the view is, and it carries every line the panel does. The panel is the same facts with bars.
  const preview = [...chip.lines.slice(0, 1), ...chip.meters.map((meter) => `${meter.label}: ${meter.detail}`), ...chip.lines.slice(1 + chip.meters.length)];
  return `<button class="chip${chip.reached ? " reached" : ""}${chip.percent === null ? " bare" : ""}" type="button" role="listitem" data-index="${index}" aria-label="${escapeHtml(spoken)}" aria-expanded="false" aria-controls="panel-${index}" title="${escapeHtml(preview.join("\n"))}">
<span class="ring">
<svg viewBox="0 0 36 36" aria-hidden="true">
<circle class="track" cx="18" cy="18" r="${RING_RADIUS}"></circle>
<circle class="fill" cx="18" cy="18" r="${RING_RADIUS}" stroke-dasharray="${filled.toFixed(2)} ${RING_CIRCUMFERENCE.toFixed(2)}"></circle>
</svg>
<img class="icon" src="${escapeHtml(iconUri)}" alt="" draggable="false">
</span>
<span class="caption">${escapeHtml(chip.caption)}</span>
</button>`;
}

function panelHtml(chip: UsageChip, index: number): string {
  // The heading already says the name; a sentence that starts with it again says nothing new.
  const [position, ...rest] = chip.lines.map((line) => line.startsWith(`${chip.name}: `) ? line.slice(chip.name.length + 2) : line);
  const age = rest.length > 0 && rest[rest.length - 1]!.startsWith("Reported ") ? rest.pop() : null;
  const bars = chip.meters.map((meter) => `<div class="meter${meter.governing ? " governing" : ""}">
<span class="label">${escapeHtml(meter.label)}</span>
<span class="bar" role="progressbar" aria-valuemin="0" aria-valuemax="100" aria-valuenow="${meter.percent}" aria-label="${escapeHtml(`${meter.label} ${meter.percent} percent`)}"><span class="value" style="width:${meter.percent}%"></span></span>
<span class="percent">${meter.percent}%</span>
<span class="detail">${escapeHtml(meter.detail)}</span>
</div>`).join("");
  // Lines the bars already say (one per window) are not repeated as sentences; what remains is the plan and
  // any window the service named without a number.
  const spokenWindows = new Set(chip.meters.map((meter) => meter.label));
  const sentences = rest.filter((line) => !spokenWindows.has(line.split(":")[0] ?? ""));
  const action = chip.action === "signIn"
    ? `<button class="action" type="button" data-action="signIn" data-provider="${escapeHtml(chip.providerId)}">Sign in</button>`
    : chip.action === "fix"
      ? `<button class="action" type="button" data-action="fix" data-provider="${escapeHtml(chip.providerId)}">Fix</button>`
      : "";
  return `<section class="panel" id="panel-${index}" hidden>
<h2>${escapeHtml(chip.name)}</h2>
<p class="position${chip.reached ? " reached" : ""}">${escapeHtml(position ?? "")}</p>
${sentences.map((line) => `<p>${escapeHtml(line)}</p>`).join("")}
${bars}
${age ? `<p class="age">${escapeHtml(age)}</p>` : ""}
${action}
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

const STYLE = `
body { margin: 0; padding: 6px 8px; color: var(--vscode-foreground); font: var(--vscode-font-size) var(--vscode-font-family); background: transparent; }
.empty { margin: 0; opacity: 0.8; }
.chips { display: flex; flex-wrap: wrap; gap: 4px 10px; }
.chip { display: flex; flex-direction: column; align-items: center; gap: 1px; width: 52px; padding: 2px 0; border: 1px solid transparent; border-radius: 6px; background: transparent; color: inherit; cursor: pointer; }
.chip:hover, .chip[aria-expanded="true"] { background: var(--vscode-list-hoverBackground); }
.chip:focus-visible { outline: none; border-color: var(--vscode-focusBorder); }
.ring { position: relative; width: 36px; height: 36px; }
.ring svg { width: 36px; height: 36px; transform: rotate(-90deg); }
.ring .track { fill: none; stroke: var(--vscode-widget-border, rgba(128,128,128,0.35)); stroke-width: 3; }
.ring .fill { fill: none; stroke: var(--vscode-progressBar-background); stroke-width: 3; stroke-linecap: round; }
.chip.reached .fill { stroke: var(--vscode-errorForeground); }
.chip.bare .fill { display: none; }
.ring .icon { position: absolute; left: 10px; top: 10px; width: 16px; height: 16px; }
.caption { font-size: 11px; line-height: 13px; opacity: 0.9; white-space: nowrap; }
.chip.reached .caption { color: var(--vscode-errorForeground); }
.panels { margin-top: 4px; }
.panel { padding: 6px 4px 2px; border-top: 1px solid var(--vscode-widget-border, rgba(128,128,128,0.35)); }
.panel h2 { margin: 0 0 2px; font-size: var(--vscode-font-size); font-weight: 600; }
.panel p { margin: 0 0 4px; opacity: 0.9; }
.panel .position.reached { color: var(--vscode-errorForeground); opacity: 1; }
.meter { display: grid; grid-template-columns: minmax(48px, auto) 1fr auto; gap: 2px 8px; align-items: center; margin: 4px 0; }
.meter .label { white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.meter.governing .label { font-weight: 600; }
.meter .bar { display: block; height: 4px; border-radius: 2px; background: var(--vscode-widget-border, rgba(128,128,128,0.35)); overflow: hidden; }
.meter .value { display: block; height: 100%; border-radius: 2px; background: var(--vscode-progressBar-background); }
.meter .percent { font-variant-numeric: tabular-nums; }
.meter .detail { grid-column: 1 / -1; font-size: 11px; opacity: 0.75; }
.panel .age { font-size: 11px; opacity: 0.7; }
.action { margin: 2px 0 4px; padding: 2px 10px; border: 1px solid var(--vscode-button-border, transparent); border-radius: 2px; background: var(--vscode-button-background); color: var(--vscode-button-foreground); cursor: pointer; }
.action:hover { background: var(--vscode-button-hoverBackground); }
`;

// Hover previews, focus previews, Enter or click pins, Escape and a second press close. One panel at a time.
const SCRIPT = `
(function () {
  var vscode = acquireVsCodeApi();
  var chips = Array.prototype.slice.call(document.querySelectorAll(".chip"));
  var panels = Array.prototype.slice.call(document.querySelectorAll(".panel"));
  var pinned = null;
  // The view is short, so an opened panel scrolls itself into the box and a closed one gives the chips back.
  function show(index) {
    chips.forEach(function (chip, at) { chip.setAttribute("aria-expanded", at === index ? "true" : "false"); });
    panels.forEach(function (panel, at) { panel.hidden = at !== index; });
    if (index === null) { window.scrollTo(0, 0); return; }
    var open = panels[index];
    if (open) open.scrollIntoView({ block: "start" });
  }
  function settle() { show(pinned); }
  chips.forEach(function (chip, index) {
    chip.addEventListener("mouseenter", function () { if (pinned === null) show(index); });
    chip.addEventListener("focus", function () { if (pinned === null) show(index); });
    chip.addEventListener("click", function () { pinned = pinned === index ? null : index; show(pinned === null ? index : pinned); });
    chip.addEventListener("keydown", function (event) {
      if (event.key === "Escape") { pinned = null; show(null); }
    });
  });
  var strip = document.querySelector(".chips");
  if (strip) strip.addEventListener("mouseleave", settle);
  document.addEventListener("focusout", function (event) {
    if (!event.relatedTarget || !document.body.contains(event.relatedTarget)) settle();
  });
  document.addEventListener("keydown", function (event) {
    if (event.key === "Escape") { pinned = null; show(null); }
  });
  Array.prototype.slice.call(document.querySelectorAll(".action")).forEach(function (button) {
    button.addEventListener("click", function () {
      vscode.postMessage({ type: "action", action: button.dataset.action, providerId: button.dataset.provider });
    });
  });
})();
`;
