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
  /// Where the account stands, in one short clause. Never an instruction: actions are buttons.
  readonly position: string;
  /// The plan the service named, or null.
  readonly plan: string | null;
  /// How old the last report is, or null.
  readonly age: string | null;
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
      position: row.position,
      plan: row.plan,
      age: row.age,
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

const RING_RADIUS = 11;
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
  // short the view is, and it carries every fact the panel does. The panel is the same facts with bars.
  const preview = [
    `${chip.name}: ${chip.position}`,
    ...(chip.plan ? [chip.plan] : []),
    ...chip.meters.map((meter) => `${meter.label}: ${meter.detail}`),
    ...(chip.age ? [chip.age] : []),
  ];
  const direct = chip.action ? ` data-action="${chip.action}" data-provider="${escapeHtml(chip.providerId)}"` : "";
  return `<button class="chip${chip.reached ? " reached" : ""}${chip.percent === null ? " bare" : ""}" type="button" role="listitem" data-index="${index}"${direct} aria-label="${escapeHtml(spoken)}" aria-expanded="false" aria-controls="panel-${index}" title="${escapeHtml(preview.join("\n"))}">
<span class="ring">
<svg viewBox="0 0 26 26" aria-hidden="true">
<circle class="track" cx="13" cy="13" r="${RING_RADIUS}"></circle>
<circle class="fill" cx="13" cy="13" r="${RING_RADIUS}" stroke-dasharray="${filled.toFixed(2)} ${RING_CIRCUMFERENCE.toFixed(2)}"></circle>
</svg>
<img class="icon" src="${escapeHtml(iconUri)}" alt="" draggable="false">
</span>
<span class="caption">${escapeHtml(chip.caption)}</span>
</button>`;
}

function panelHtml(chip: UsageChip, index: number): string {
  const bars = chip.meters.map((meter) => `<div class="meter${meter.governing ? " governing" : ""}">
<span class="label">${escapeHtml(meter.label)}</span>
<span class="bar" role="progressbar" aria-valuemin="0" aria-valuemax="100" aria-valuenow="${meter.percent}" aria-label="${escapeHtml(`${meter.label} ${meter.percent} percent`)}"><span class="value" style="width:${meter.percent}%"></span></span>
<span class="percent">${meter.percent}%</span>
<span class="detail">${escapeHtml(meter.detail)}</span>
</div>`).join("");
  return `<section class="panel" id="panel-${index}" hidden>
<h2>${escapeHtml(chip.name)}${chip.plan ? ` <span class="plan">${escapeHtml(chip.plan)}</span>` : ""}</h2>
<p class="position${chip.reached ? " reached" : ""}">${escapeHtml(chip.position)}</p>
${bars}
${chip.age ? `<p class="age">${escapeHtml(chip.age)}</p>` : ""}
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
.chips { display: flex; flex-wrap: wrap; gap: 2px 6px; }
.chip { display: flex; flex-direction: column; align-items: center; gap: 0; width: 38px; padding: 2px 0 1px; border: 1px solid transparent; border-radius: 5px; background: transparent; color: inherit; cursor: pointer; }
.chip:hover, .chip[aria-expanded="true"] { background: var(--vscode-list-hoverBackground); }
.chip:focus-visible { outline: none; border-color: var(--vscode-focusBorder); }
.ring { position: relative; width: 26px; height: 26px; }
.ring svg { width: 26px; height: 26px; transform: rotate(-90deg); }
.ring .track { fill: none; stroke: var(--vscode-widget-border, rgba(128,128,128,0.35)); stroke-width: 2.5; }
.ring .fill { fill: none; stroke: var(--vscode-progressBar-background); stroke-width: 2.5; stroke-linecap: round; }
.chip.reached .fill { stroke: var(--vscode-errorForeground); }
.chip.bare .fill { display: none; }
.ring .icon { position: absolute; left: 7px; top: 7px; width: 12px; height: 12px; }
.caption { font-size: 10px; line-height: 12px; opacity: 0.9; white-space: nowrap; }
.chip.reached .caption { color: var(--vscode-errorForeground); }
.panels { margin-top: 4px; }
.panel { padding: 6px 4px 2px; border-top: 1px solid var(--vscode-widget-border, rgba(128,128,128,0.35)); }
.panel h2 { margin: 0 0 2px; font-size: var(--vscode-font-size); font-weight: 600; }
.panel .plan { font-weight: 400; opacity: 0.75; }
.panel p { margin: 0 0 4px; opacity: 0.9; }
.panel .position.reached { color: var(--vscode-errorForeground); opacity: 1; }
.meter { display: grid; grid-template-columns: 72px minmax(0, 1fr) 34px; gap: 2px 6px; align-items: center; margin: 3px 0; }
.meter .label { white-space: nowrap; overflow: hidden; text-overflow: ellipsis; font-size: 11px; }
.meter.governing .label { font-weight: 600; }
.meter .bar { display: block; height: 4px; border-radius: 2px; background: var(--vscode-widget-border, rgba(128,128,128,0.35)); overflow: hidden; }
.meter .value { display: block; height: 100%; border-radius: 2px; background: var(--vscode-progressBar-background); }
.meter .percent { font-variant-numeric: tabular-nums; text-align: right; font-size: 11px; }
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
  document.addEventListener("focusout", function (event) {
    if (!event.relatedTarget || !document.body.contains(event.relatedTarget)) settle();
  });
  document.addEventListener("keydown", function (event) {
    if (event.key === "Escape") { pinned = null; show(null, true); }
  });
  Array.prototype.slice.call(document.querySelectorAll(".action")).forEach(function (button) {
    button.addEventListener("click", function () {
      vscode.postMessage({ type: "action", action: button.dataset.action, providerId: button.dataset.provider });
    });
  });
})();
`;
