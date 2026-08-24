import "./usageView.css";

import type { UsageRow } from "./usageDisplay";
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
  const item = document.createElement(row.state === "unavailable" ? "button" : "section");
  item.className = `usage-row ${row.state}${row.reached ? " limit-reached" : ""}`;
  item.title = row.tooltip;
  const value = usageValue(row);
  // The service name and its state live in the hover, not on the row. The glyph says whose usage this is; the
  // text beside it is the usage itself and nothing else.
  item.setAttribute("aria-label", value ? `${row.name}, ${value}` : row.name);
  if (item instanceof HTMLButtonElement) {
    item.type = "button";
    item.addEventListener("click", () => vscode.postMessage({ type: "fix", providerId: row.providerId }));
  }
  const icon = providerGlyph(row.icon, "provider-icon");
  icon.setAttribute("aria-hidden", "true");
  const text = document.createElement("span");
  text.className = "usage-value";
  text.textContent = value;
  item.append(icon, text);
  return item;
}

/// The one thing beside the glyph: the service's own usage. Cost and the account percentage when reported, the
/// fix affordance when the service cannot run, and nothing at all when there is no usage to show yet.
function usageValue(row: UsageRow): string {
  if (row.state === "unavailable") return "Fix";
  if (row.state === "checking") return "";
  const parts: string[] = [];
  if (row.cost) parts.push(row.cost);
  const percentMeter = row.meters.find((meter) => Number.isFinite(meter.percent));
  if (percentMeter) parts.push(`${percentMeter.percent}%`);
  return parts.join(" · ");
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
