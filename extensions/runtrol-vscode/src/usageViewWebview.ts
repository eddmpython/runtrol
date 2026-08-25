import "./usageView.css";

import type { UsageMeter, UsageRow } from "./usageDisplay";
import type { UsageViewSnapshot } from "./usageViewMessage";

type VsCodeApi = {
  postMessage(message: unknown): void;
};

declare function acquireVsCodeApi(): VsCodeApi;

const vscode = acquireVsCodeApi();
const usage = element<HTMLElement>("usage");
/// The folder the service SVGs live in, handed in by the host so no icon path is built here.
const iconBase = usage.dataset.iconBase ?? "";
const empty = element<HTMLParagraphElement>("empty");
const error = element<HTMLParagraphElement>("error");
const discover = element<HTMLButtonElement>("discover");

window.addEventListener("message", (event: MessageEvent<unknown>) => {
  const snapshot = usageSnapshot(event.data);
  if (snapshot) render(snapshot);
});

discover.addEventListener("click", () => vscode.postMessage({ type: "discover" }));
vscode.postMessage({ type: "ready" });

function render(snapshot: UsageViewSnapshot): void {
  usage.replaceChildren(...snapshot.rows.map(usageRow));
  empty.hidden = snapshot.rows.length > 0 || snapshot.installableCount > 0;
  error.textContent = snapshot.error ?? "";
  error.hidden = snapshot.error === null;
  discover.textContent = snapshot.installableCount === 1
    ? "Add coding service · 1 available"
    : `Add coding services · ${snapshot.installableCount} available`;
  discover.hidden = snapshot.installableCount === 0;
}

function usageRow(row: UsageRow): HTMLElement {
  const actionable = row.state === "unavailable" || row.state === "signedOut";
  const item = document.createElement(actionable ? "button" : "section");
  item.className = `usage-row ${row.state}${row.reached ? " limit-reached" : ""}`;
  item.title = row.tooltip;
  // The service name and its state live in the hover, not on the row. The glyph says whose usage this is; what
  // sits beside it is that service's own usage and nothing else.
  item.setAttribute("aria-label", `${row.name}, ${row.detail}`);
  if (item instanceof HTMLButtonElement) {
    item.type = "button";
    const type = row.state === "signedOut" ? "signIn" : "fix";
    item.addEventListener("click", () => vscode.postMessage({ type, providerId: row.providerId }));
  }
  const icon = providerGlyph(row.icon, "provider-icon");
  icon.setAttribute("aria-hidden", "true");
  const body = document.createElement("div");
  body.className = "usage-body";
  body.append(...usageBody(row));
  item.append(icon, body);
  return item;
}

/// What the service itself reported, drawn the way that service reports it.
///
/// A service that states how much of a window it has used gets a real bar per window, because that is a
/// proportion and a bar is how a proportion is read. One that states only a running spend, or only when its
/// window resets, gets that sentence instead: an empty bar beside a service that never sent a percentage would
/// be an invention, and this panel only ever shows a number some service actually said.
function usageBody(row: UsageRow): HTMLElement[] {
  if (row.state === "unavailable") return [textLine("Fix")];
  if (row.state === "signedOut") return [textLine("Not signed in · Sign in")];
  if (row.state === "checking") return [textLine("")];
  if (row.meters.length === 0) return [textLine(row.cost ?? row.detail)];
  // The cells go straight into the body's own grid rather than into a box per window, so every bar of a service
  // that reported two windows ends at the same place instead of being shortened by the spend beside the first.
  return row.meters.flatMap((meter, index) => meterCells(meter, index === 0 ? row.cost : null));
}

/// One reported window, as three cells: the proportion as a bar, the same proportion as digits, and the spend
/// when the service also reported one.
function meterCells(meter: UsageMeter, cost: string | null): HTMLElement[] {
  const bar = document.createElement("progress");
  bar.max = 100;
  bar.value = meter.percent;
  // The window this bar is of, and its reset, spoken rather than crowded onto the row. A bar with no name is
  // just a number to a reader who cannot see which window it belongs to.
  bar.title = `${meter.label}: ${meter.detail}`;
  bar.setAttribute("aria-label", `${meter.label}: ${meter.detail}`);
  const percent = document.createElement("span");
  percent.className = "usage-percent";
  percent.textContent = `${meter.percent}%`;
  const spend = document.createElement("span");
  spend.className = "usage-cost";
  spend.textContent = cost ?? "";
  return [bar, percent, spend];
}

function textLine(value: string): HTMLElement {
  const text = document.createElement("span");
  text.className = "usage-value";
  text.textContent = value;
  return text;
}

/// Provider manifests already restrict icon names. Recheck at the Webview boundary because the name still comes
/// from a message and is about to be put in a URL, and fall back to the neutral coding-service name.
function iconName(value: string): string {
  return /^[a-z0-9-]{1,64}$/u.test(value) ? value : "sparkle";
}

/// One service glyph as an `<img>` to the theme-aware SVG the host's icon folder carries. A name that names no
/// bundled SVG loads nothing and swaps once to the neutral coding-service mark, so a row never shows a broken
/// image and never a wrong service's glyph.
function providerGlyph(declared: string, className: string): HTMLImageElement {
  const image = document.createElement("img");
  image.className = className;
  image.alt = "";
  const fallback = `${iconBase}/sparkle.svg`;
  image.src = `${iconBase}/${iconName(declared)}.svg`;
  image.addEventListener(
    "error",
    () => {
      image.src = fallback;
    },
    { once: true },
  );
  return image;
}

function usageSnapshot(value: unknown): UsageViewSnapshot | null {
  if (!value || typeof value !== "object") return null;
  const record = value as Record<string, unknown>;
  if (
    record.type !== "snapshot"
    || !Array.isArray(record.rows)
    || typeof record.installableCount !== "number"
    || !(typeof record.error === "string" || record.error === null)
  ) return null;
  return value as UsageViewSnapshot;
}

function element<T extends HTMLElement>(id: string): T {
  const found = document.getElementById(id);
  if (!found) throw new Error(`missing usage view element ${id}`);
  return found as T;
}
