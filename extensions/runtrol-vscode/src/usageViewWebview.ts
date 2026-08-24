import "./usageView.css";

import type { UsageRow } from "./usageDisplay";
import type { UsageViewSnapshot } from "./usageViewMessage";

type VsCodeApi = {
  postMessage(message: unknown): void;
};

declare function acquireVsCodeApi(): VsCodeApi;

const vscode = acquireVsCodeApi();
const usage = element<HTMLElement>("usage");
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
  replaceMissingProviderIcons();
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
  item.setAttribute("aria-label", `${row.name}, ${row.detail}`);
  if (item instanceof HTMLButtonElement) {
    item.type = "button";
    item.addEventListener("click", () => vscode.postMessage({ type: "fix", providerId: row.providerId }));
  }

  const header = document.createElement("span");
  header.className = "usage-header";
  const icon = document.createElement("span");
  icon.className = "provider-icon codicon";
  icon.classList.add(`codicon-${iconName(row.icon)}`);
  icon.setAttribute("aria-hidden", "true");
  const name = document.createElement("span");
  name.className = "provider-name";
  name.textContent = row.name;
  const status = document.createElement("span");
  status.className = "provider-status";
  status.textContent = compactStatus(row);
  header.append(icon, name, status);
  item.append(header);

  for (const meter of row.meters) {
    const block = document.createElement("span");
    block.className = "meter";
    const meta = document.createElement("span");
    meta.className = "meter-meta";
    const label = document.createElement("span");
    label.textContent = meter.label;
    const detail = document.createElement("span");
    detail.textContent = meter.detail;
    meta.append(label, detail);
    const progress = document.createElement("progress");
    progress.max = 100;
    progress.value = meter.percent;
    progress.setAttribute("aria-label", `${row.name}, ${meter.label}, ${meter.detail}`);
    block.append(meta, progress);
    item.append(block);
  }
  return item;
}

/// Provider manifests already restrict icon names. Recheck at the Webview boundary because a class name still
/// comes from a message, and fall back to the editor's neutral coding-service glyph instead of drawing text.
function iconName(value: string): string {
  return /^[a-z0-9-]{1,64}$/u.test(value) ? value : "sparkle";
}

function replaceMissingProviderIcons(): void {
  for (const icon of usage.querySelectorAll<HTMLElement>(".provider-icon")) {
    const content = getComputedStyle(icon, "::before").content;
    if (!content || content === "none" || content === "normal" || content === '""') {
      icon.className = "provider-icon codicon codicon-sparkle";
    }
  }
}

function compactStatus(row: UsageRow): string {
  if (row.state === "unavailable") return "Fix";
  if (row.state === "disconnected") return "Last report";
  if (row.reached) return "Limit reached";
  return row.meters.length > 0 ? "" : row.detail;
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
