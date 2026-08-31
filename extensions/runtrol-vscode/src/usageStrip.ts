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
  /// The service publishes its own sign-out command, so a signed-in account's panel can offer it.
  readonly canSignOut: boolean;
  /// The installed release of the service's CLI, as Runtime discovered it, or null when it said none.
  readonly version: string | null;
  /// A newer release the Core confirmed it can install and roll back from, or null. Drawn as the Update button.
  readonly updateTo: string | null;
};

/// What the sidebar knows about a service beyond its usage: its CLI release and whether a newer one is ready.
export type ServiceRelease = {
  readonly version: string | null;
  readonly updateTo: string | null;
};

/// Rows to chips. The order is the rows' order, which is the services' order.
export function usageChips(
  rows: readonly UsageRow[],
  /// The services that publish a sign-in line of their own.
  signInAble: ReadonlySet<string> = new Set(),
  /// Each service's CLI release and confirmed update, by provider id.
  releases: ReadonlyMap<string, ServiceRelease> = new Map(),
  /// The services that publish a sign-out command of their own.
  signOutAble: ReadonlySet<string> = new Set(),
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
      canSignOut: signOutAble.has(row.providerId),
      version: releases.get(row.providerId)?.version ?? null,
      updateTo: releases.get(row.providerId)?.updateTo ?? null,
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
      // A service whose meter belongs to a team has no number and nothing to act on, so the chip stays bare;
      // the panel says why (operator, 2026-08-29: "team-managed" under the ring read as a broken state).
      return row.unmetered ? "" : "No report";
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
  const direct = chip.action ? ` data-action="${chip.action}"` : "";
  return `<button class="chip${chip.reached ? " reached" : ""}${chip.percent === null ? " bare" : ""}" type="button" role="listitem" data-index="${index}" data-provider="${escapeHtml(chip.providerId)}"${direct} aria-label="${escapeHtml(spoken)}" aria-expanded="false" aria-controls="panel-${escapeHtml(chip.providerId)}">
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
  //
  // Those words sit on the name's line rather than under the bar. On their own line they made the governing
  // window a three-line block among two-line ones, so the bars of one service stood at uneven distances
  // (operator, 2026-08-28: close the gap between one service's bars). Beside the name they cost no line at all.
  const bars = chip.meters.map((meter) => `<div class="meter${meter.governing ? " governing" : ""}">
<span class="label"><span class="what">${escapeHtml(meter.label)}</span>${meter.governing && meter.resets ? `<span class="when">${escapeHtml(meter.resets)}</span>` : ""}</span>
<span class="percent">${meter.percent}%</span>
<span class="bar" role="progressbar" aria-valuemin="0" aria-valuemax="100" aria-valuenow="${meter.percent}" aria-label="${escapeHtml(`${meter.label} ${meter.percent} percent`)}"><span class="value ${widthClass(meter.percent)}"></span></span>
</div>`).join("");
  return `<section class="panel" id="panel-${escapeHtml(chip.providerId)}" data-provider="${escapeHtml(chip.providerId)}" hidden>
<h2><span class="who">${escapeHtml(chip.name)}${chip.plan ? ` <span class="plan">${escapeHtml(chip.plan)}</span>` : ""}${chip.version ? ` <span class="version" title="${escapeHtml(`${chip.name} ${chip.version} is installed`)}">${escapeHtml(chip.version)}</span>` : ""}</span>${updateButton(chip)}</h2>
${chip.position ? `<p class="position${chip.reached ? " reached" : ""}">${escapeHtml(chip.position)}</p>` : ""}
${bars}
${chip.age ? `<p class="age">${escapeHtml(chip.age)}</p>` : ""}
${troubleshootButton(chip)}${signInButton(chip)}${signOutButton(chip)}
</section>`;
}

/// An unavailable service keeps its cause in the detail panel and puts the route to the provider's own repair
/// surfaces beside it. This is deliberately not an external hardcoded URL: providers declare the current doctor,
/// install, update, and sign-in routes that the host presents after this press.
function troubleshootButton(chip: UsageChip): string {
  if (chip.state !== "unavailable") return "";
  return `<button class="action" type="button" data-action="fix" data-provider="${escapeHtml(chip.providerId)}">Troubleshoot</button>`;
}

/// The Update button at the right end of the service's line, only when the Core confirmed a newer release it
/// can install and roll back from. One press updates; the version it goes to is on the button, so nothing has to
/// be asked first (operator, 2026-08-29: a new release shows an Update button, a click updates).
function updateButton(chip: UsageChip): string {
  if (!chip.updateTo) return "";
  return `<button class="action update" type="button" data-action="update" data-provider="${escapeHtml(chip.providerId)}" title="${escapeHtml(`Update ${chip.name} to ${chip.updateTo}`)}">Update to ${escapeHtml(chip.updateTo)}</button>`;
}

/// The detail panel's sign-in button, shown only when signing in is the true next step.
///
/// A signed-in account that is reporting its usage is not a sign-in situation, and offering "Sign in to
/// Claude Code" under its live figures read as a bug (operator, 2026-08-29: it was signed in and the button
/// was still there). It shows only when the account is not signed in or has dropped its connection, and only
/// when that is not already the chip's own single action (which opens sign-in on its own press).
function signInButton(chip: UsageChip): string {
  const needsSignIn = chip.state === "signedOut" || chip.state === "disconnected";
  if (!chip.canSignIn || !needsSignIn || chip.action === "signIn") {
    return "";
  }
  return `<button class="action" type="button" data-action="signIn" data-provider="${escapeHtml(chip.providerId)}">Sign in to ${escapeHtml(chip.name)}</button>`;
}

/// Sign out, for an account that is signed in and whose service publishes its own command for it.
///
/// Quiet on purpose: it ends a working login, so it dresses as a link rather than a button and sits last.
/// The service's own command runs in a terminal exactly as sign-in does (operator, 2026-08-29).
function signOutButton(chip: UsageChip): string {
  if (!chip.canSignOut || chip.state !== "available") return "";
  return `<button class="action quiet" type="button" data-action="signOut" data-provider="${escapeHtml(chip.providerId)}">Sign out of ${escapeHtml(chip.name)}</button>`;
}

export function escapeHtml(text: string): string {
  return text
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#39;");
}

/// The strip's own rules. It owns its chips and its panel and nothing outside them.
///
/// It used to open with a `body` rule, from when the strip was a second webview with a document of its own.
/// Folding it into the one page left that rule behind, last in the concatenation, where it quietly took the
/// page's margin, padding, colour and background away from the page's own rule. The background it handed
/// over was `transparent`, which is how the panel came to sit on the browser's dark canvas instead of the
/// sidebar's colour. One element, one owner.
export const USAGE_STYLE = `
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
.panel h2 { margin: 0 0 2px; font-size: var(--vscode-font-size); font-weight: 600; display: flex; align-items: center; gap: 6px; }
.panel h2 .who { min-width: 0; flex: 1 1 auto; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.panel .plan { font-weight: 400; opacity: 0.75; }
.panel .version { font-weight: 400; opacity: 0.6; font-size: 11px; font-variant-numeric: tabular-nums; }
/* The Update button holds the right end of the line and never wraps; the name yields before it does. */
.panel .action.update { flex: none; margin: 0; padding: 1px 8px; font-size: 11px; line-height: 16px; border-radius: 3px; border: 1px solid var(--vscode-button-border, transparent); background: var(--vscode-button-background); color: var(--vscode-button-foreground); cursor: pointer; }
.panel .action.update:hover { background: var(--vscode-button-hoverBackground); }
.panel p { margin: 0 0 4px; opacity: 0.9; }
.panel .position.reached { color: var(--vscode-errorForeground); opacity: 1; }
/* The name has the line, and the bar sits under it. A model window is named by its model, and a model name
   does not fit beside a bar in a panel this wide: the column was 72px and the name was the part that
   disappeared (operator, 2026-08-28). */
.meter { display: grid; grid-template-columns: minmax(0, 1fr) auto; gap: 1px 6px; align-items: baseline; margin: 3px 0; }
.meter .label { min-width: 0; display: flex; align-items: baseline; gap: 6px; font-size: 11px; }
/* The name yields before the reset does. The reset is a fixed short phrase and the name is the part that
   varies, and it arrives already shortened from its middle, so an ellipsis here is the last guard rather
   than the usual case. */
.meter .what { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.meter .when { flex: none; opacity: 0.7; }
.meter.governing .what { font-weight: 600; }
.meter .bar { grid-column: 1 / -1; display: block; height: 4px; border-radius: 2px; background: var(--vscode-widget-border, rgba(128,128,128,0.35)); overflow: hidden; }
.meter .value { display: block; height: 100%; border-radius: 2px; background: var(--vscode-progressBar-background); width: 0; }
${WIDTH_STYLE}
.meter .percent { font-variant-numeric: tabular-nums; text-align: right; font-size: 11px; }
.panel .age { font-size: 11px; opacity: 0.7; }
.action { margin: 2px 0 4px; padding: 2px 10px; border: 1px solid var(--vscode-button-border, transparent); border-radius: 2px; background: var(--vscode-button-background); color: var(--vscode-button-foreground); cursor: pointer; }
.action:hover { background: var(--vscode-button-hoverBackground); }
/* Ending a login is not the panel's main move: a quiet line, readable but never louder than the figures. */
.action.quiet { display: block; background: transparent; border-color: transparent; color: var(--vscode-descriptionForeground); padding: 0; margin: 2px 0 2px; }
.action.quiet:hover { background: transparent; color: var(--vscode-foreground); text-decoration: underline; }
`;

// Hover previews, focus previews, Enter or click pins, Escape and a second press close. One panel at a time.
export const USAGE_SCRIPT = `
(function () {
  var vscode = window.__runtrolVsCodeApi || (window.__runtrolVsCodeApi = acquireVsCodeApi());
  var chips = [];
  var panels = [];
  // Which panel the person opened, by the service it belongs to rather than by its place in the strip: a
  // service arriving or leaving must not hand the pin to its neighbour. It lives outside the binding so that
  // a repaint, which is a figure ticking and nothing they did, leaves their open panel open (2026-08-28).
  var pinned = null;
  // Which panel is showing right now: the pinned one, or the one under the pointer. A repaint writes every
  // panel back closed, and this is what reopens the one the person was reading (measured 2026-08-29: a
  // hover preview closed under the pointer on every memory tick).
  var shown = null;
  function serviceOf(node) { return node.dataset.provider || null; }
  // A hover preview must not move anything: scrolling the panel into view moved the chip out from under the
  // pointer, which closed the panel, which scrolled back and reopened it, and the strip flickered (2026-08-27).
  // Only a pinned panel (a click, or keyboard focus) is brought into the short view.
  function show(service, settle) {
    shown = service;
    chips.forEach(function (chip) { chip.setAttribute("aria-expanded", serviceOf(chip) === service ? "true" : "false"); });
    panels.forEach(function (panel) { panel.hidden = serviceOf(panel) !== service; });
    if (!settle) return;
    if (service === null) { window.scrollTo(0, 0); return; }
    var open = panels.filter(function (panel) { return serviceOf(panel) === service; })[0];
    if (open) open.scrollIntoView({ block: "nearest" });
  }
  function settle() { show(pinned, true); }
  // The preview must be reachable. The panel sits below the chips, so moving the pointer from a chip down to
  // its panel to press a button leaves the chip strip, and closing on that leave hid the panel before the
  // button could be clicked (operator, 2026-08-29: hovering a usage button, the panel vanished on the way to
  // it). Closing is deferred a beat and cancelled the moment the pointer is over the panel, so the chip and
  // its panel act as one hover region. A pinned panel (a click) is unaffected: settle keeps it open.
  var hideTimer = null;
  function keepOpen() { if (hideTimer !== null) { clearTimeout(hideTimer); hideTimer = null; } }
  function scheduleSettle() { keepOpen(); hideTimer = setTimeout(function () { hideTimer = null; settle(); }, 150); }
  // A repaint keeps the elements it can, so each one is bound once: bound twice, a chip's click would pin and
  // unpin in the same press and the Update button would ask for two updates.
  var bound = new WeakSet();
  function once(node) { if (bound.has(node)) return false; bound.add(node); return true; }
  function bind() {
  chips = Array.prototype.slice.call(document.querySelectorAll(".chip"));
  panels = Array.prototype.slice.call(document.querySelectorAll(".panel"));
  var present = chips.map(serviceOf);
  if (pinned !== null && present.indexOf(pinned) === -1) pinned = null;
  if (shown !== null && present.indexOf(shown) === -1) shown = pinned;
  chips.forEach(function (chip) {
    if (!once(chip)) return;
    chip.addEventListener("mouseenter", function () { keepOpen(); if (pinned === null) show(serviceOf(chip), false); });
    chip.addEventListener("focus", function () { if (pinned === null) show(serviceOf(chip), true); });
    chip.addEventListener("click", function () {
      var direct = chip.dataset.action;
      if (direct) { vscode.postMessage({ type: "action", action: direct, providerId: chip.dataset.provider }); return; }
      pinned = pinned === serviceOf(chip) ? null : serviceOf(chip);
      show(pinned, true);
    });
    chip.addEventListener("keydown", function (event) {
      if (event.key === "Escape") { pinned = null; show(null, true); }
    });
  });
  var strip = document.querySelector(".chips");
  if (strip && once(strip)) strip.addEventListener("mouseleave", scheduleSettle);
  // The panel is part of the same hover region: entering it cancels the pending close, leaving it schedules
  // one, so a person can travel from a chip to its panel and press the button there.
  panels.forEach(function (panel) {
    if (!once(panel)) return;
    panel.addEventListener("mouseenter", keepOpen);
    panel.addEventListener("mouseleave", scheduleSettle);
  });
  Array.prototype.slice.call(document.querySelectorAll(".action")).forEach(function (button) {
    if (!once(button)) return;
    button.addEventListener("click", function () {
      vscode.postMessage({ type: "action", action: button.dataset.action, providerId: button.dataset.provider });
    });
  });
  show(pinned !== null ? pinned : shown, false);
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
