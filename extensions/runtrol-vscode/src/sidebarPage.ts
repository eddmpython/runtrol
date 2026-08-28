/// The Runtrol sidebar, drawn by Studio itself.
///
/// One page, three zones with visible edges: projects (each a folder that collapses, its conversations under it),
/// conversations that belong to no project, and one usage chip per installed service. The title bar above the
/// page is VS Code's, and it carries the two actions that start things: add a project, start a conversation. Every
/// other action lives on the row it belongs to and appears on hover, or behind the `⋮` in the title bar the
/// editor draws above this page (`moreActions.ts`). This is the shape the operator fixed in `docs/vscodeSurface.md`; the native tree could draw
/// neither the edges between zones nor the gauges nor the row density it asks for.
///
/// Everything here is a pure function of a `SidebarModel` the host builds. The page posts back what was pressed
/// and nothing else; it never asks a provider anything and never sees a conversation.

import type { ConversationActivity } from "./conversationList";
import { HUES } from "./projectColor";
import { escapeHtml, usageChipsMarkup, usagePanelsMarkup, USAGE_SCRIPT, USAGE_STYLE, type UsageChip, type UsageStripAssets } from "./usageStrip";

export type SidebarConversationRow = {
  readonly key: string;
  readonly title: string;
  readonly serviceName: string;
  /// The declared icon name, resolved to a page URI by the host.
  readonly icon: string;
  /// The project's colour as a VS Code theme colour id, or null for a conversation outside every project.
  readonly hue: string | null;
  readonly activity: ConversationActivity;
  readonly live: boolean;
  readonly canOpen: boolean;
  readonly blocked: string | null;
  readonly pinned: boolean;
  readonly signIn: boolean;
  readonly canDelete: boolean;
  readonly canArchive: boolean;
  /// What the provider process holds right now, already formatted, or null when nothing measured it.
  readonly memory: string | null;
  /// The tool the provider says it is running, or null.
  readonly tool: string | null;
  readonly workspace: string;
};

export type SidebarProjectRow = {
  readonly key: string;
  readonly name: string;
  readonly workspace: string;
  readonly hue: string | null;
  readonly kind: "created" | "open";
  readonly pinned: boolean;
  readonly current: boolean;
  readonly collapsed: boolean;
  readonly attention: number;
  readonly live: number;
  readonly agentTools: boolean;
  readonly rows: readonly SidebarConversationRow[];
  /// How many of this project's conversations are waiting behind "Show all".
  readonly hidden: number;
};

export type SidebarNotice = {
  readonly tone: "info" | "warn" | "error";
  readonly text: string;
  readonly command: string | null;
  readonly label: string | null;
};

export type SidebarServiceChoice = {
  readonly workspace: string;
  readonly services: ReadonlyArray<{ providerId: string; displayName: string; icon: string }>;
};

export type SidebarModel = {
  readonly notices: readonly SidebarNotice[];
  readonly projects: readonly SidebarProjectRow[];
  readonly loose: readonly SidebarConversationRow[];
  readonly usage: readonly UsageChip[];
  readonly serviceChoice: SidebarServiceChoice | null;
  /// Nothing to list and a service to start with: the page offers the two first actions as rows.
  readonly firstRun: boolean;
};

export type SidebarAssets = UsageStripAssets;

/// One rule per hue, generated from the palette so the page and the projects cannot disagree about a colour.
///
/// In the stylesheet rather than on the element: the page's CSP allows styles from this nonced block only, and a
/// nonce does not cover inline `style` attributes, so a colour written onto the element is simply dropped.
const HUE_STYLE = HUES
  .map((hue) => `.row .bar.${hue.band} { background: var(--vscode-${hue.chart.replace(/\./gu, "-")}); }`)
  .join("\n");

/// Bytes as the short figure a row can carry: whole megabytes below a gigabyte, one decimal above.
export function formatMemory(bytes: number): string {
  const megabytes = bytes / (1024 * 1024);
  if (megabytes < 1) return "<1 MB";
  if (megabytes < 1024) return `${Math.round(megabytes)} MB`;
  return `${(megabytes / 1024).toFixed(1)} GB`;
}

/// Every row key in page order, which is what the eye tests compare against for duplicates.
export function rowKeys(model: SidebarModel): string[] {
  return [
    ...model.projects.flatMap((project) => [project.key, ...project.rows.map((row) => row.key)]),
    ...model.loose.map((row) => row.key),
  ];
}

export function sidebarHtml(model: SidebarModel, assets: SidebarAssets): string {
  // `img-src` also governs CSS mask images, and every glyph here is an inline SVG mask: without `data:` the
  // chevrons, action icons and the vertical dots were blank (measured 2026-08-27 on the operator window).
  return `<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; img-src ${assets.cspSource} data:; style-src 'nonce-${assets.nonce}'; script-src 'nonce-${assets.nonce}'">
<meta name="viewport" content="width=device-width, initial-scale=1">
<style nonce="${assets.nonce}">${STYLE}${HUE_STYLE}${USAGE_STYLE}</style>
</head>
<body>
${model.notices.map(noticeHtml).join("")}
${model.serviceChoice ? serviceChoiceHtml(model.serviceChoice, assets) : ""}
${model.firstRun ? firstRunHtml() : ""}
${zonesHtml(model, assets)}
<script nonce="${assets.nonce}">${SCRIPT}${USAGE_SCRIPT}</script>
</body>
</html>`;
}

function zonesHtml(model: SidebarModel, assets: SidebarAssets): string {
  const projects = model.projects.length === 0
    ? ""
    : `<section class="zone" aria-label="Projects">
<h2 class="zone-title">Projects</h2>
${model.projects.map((project) => projectHtml(project, assets)).join("")}
</section>`;
  const loose = model.loose.length === 0
    ? ""
    : `<section class="zone" aria-label="Conversations">
<h2 class="zone-title">Conversations</h2>
<div class="rows">${model.loose.map((row) => conversationHtml(row, assets)).join("")}</div>
</section>`;
  const usage = model.usage.length === 0
    ? ""
    : `<section class="zone usage-zone" aria-label="Usage">
<h2 class="zone-title">Usage</h2>
${usageChipsMarkup(model.usage, assets)}
${usagePanelsMarkup(model.usage)}
</section>`;
  return `<div class="scroll">${projects}${loose}</div>${usage}`;
}

function projectHtml(project: SidebarProjectRow, assets: SidebarAssets): string {
  // What the project holds, not what fits: the rows are capped at five and the count is the reason a person
  // knows there is more before they reach the row that says so.
  const count = project.rows.length + project.hidden;
  const badges = [
    project.attention > 0 ? `<span class="badge attention" title="${project.attention} waiting for you">${project.attention}</span>` : "",
    project.live > 0 ? `<span class="badge live" title="${project.live} running">${project.live}</span>` : "",
    project.agentTools ? `<span class="badge tools" title="Agent Tools are on for this project">tools</span>` : "",
  ].join("");
  const actions = project.kind === "created"
    ? `<span class="actions">
${action("runtrol.newConversationInProject", "New conversation here", "add")}
${action("runtrol.renameProject", "Rename", "edit")}
${action(project.agentTools ? "runtrol.disableAgentTools" : "runtrol.enableAgentTools", project.agentTools ? "Turn Agent Tools off for this project" : "Turn Agent Tools on for this project", project.agentTools ? "sparkle-filled" : "sparkle")}
${action(project.pinned ? "runtrol.unpinProject" : "runtrol.pinProject", project.pinned ? "Unpin" : "Pin to the top", project.pinned ? "pinned" : "pin")}
${action("runtrol.openProjectWorkspace", "Open this folder in a window", "link-external")}
${action("runtrol.removeProject", "Remove from the sidebar (the folder stays)", "close")}
</span>`
    : `<span class="actions">${action("runtrol.newConversationInProject", "New conversation here", "add")}${action("runtrol.createProjectHere", "Keep this folder as a project", "folder-library")}</span>`;
  return `<div class="project${project.collapsed ? " collapsed" : ""}" data-project="${escapeHtml(project.key)}"${project.kind === "created" ? ' draggable="true"' : ""}>
<div class="row project-row${project.current ? " current" : ""}" role="button" tabindex="0" data-kind="project" data-key="${escapeHtml(project.key)}" aria-expanded="${project.collapsed ? "false" : "true"}" title="${escapeHtml(project.workspace)}">
<span class="bar${project.hue ? ` ${project.hue}` : ""}"></span>
<span class="chevron" aria-hidden="true"></span>
<span class="name">${escapeHtml(project.name)}</span>
<span class="count">${count}</span>
${badges}
${actions}
</div>
<div class="rows">${project.rows.map((row) => conversationHtml(row, assets)).join("")}${moreHtml(project)}</div>
</div>`;
}

/// The row that stands for the conversations this project is not showing yet.
///
/// A project with forty conversations used to push every other project off the screen, and the panel is the
/// machine's, not one project's (operator, 2026-08-28). Nothing is hidden without saying how much.
function moreHtml(project: SidebarProjectRow): string {
  if (project.hidden <= 0) return "";
  const many = project.hidden === 1 ? "1 more conversation" : `${project.hidden} more conversations`;
  return `<div class="row more" role="button" tabindex="0" data-kind="more" data-key="${escapeHtml(project.key)}">
<span class="bar"></span>
<span class="more-label">Show all (${many})</span>
</div>`;
}

function conversationHtml(row: SidebarConversationRow, assets: SidebarAssets): string {
  const iconUri = assets.iconUris.get(row.icon) ?? "";
  const dot = row.activity === "saved" && !row.live ? "" : `<span class="dot ${row.activity}" title="${escapeHtml(spokenActivity(row))}"></span>`;
  const actions = [
    row.activity === "needsYou" ? action("runtrol.allowFromRow", "Allow", "check") + action("runtrol.declineFromRow", "Decline", "circle-slash") : "",
    row.signIn ? action("runtrol.signInFromRow", "Sign in", "key") : "",
    action(row.pinned ? "runtrol.unpinConversation" : "runtrol.pinConversation", row.pinned ? "Unpin" : "Pin to the top", row.pinned ? "pinned" : "pin"),
    action("runtrol.renameSession", "Rename", "edit"),
    row.live ? action("runtrol.closeSession", "Stop", "debug-stop") : "",
    row.canArchive ? action("runtrol.archiveConversation", "Archive", "archive") : "",
    row.canDelete ? action("runtrol.deleteConversation", "Delete", "trash") : "",
  ].join("");
  // A native tooltip only where it says the one thing the row cannot show: why an open would be refused.
  // Everything else the tooltip used to repeat (service, activity, model) is already on the row, and a
  // tooltip floating beside the hover actions reads as clutter (operator, 2026-08-27).
  return `<div class="row conv${row.canOpen ? "" : " blocked"}${row.pinned ? " pinned" : ""}" role="button" tabindex="0" data-kind="conversation" data-key="${escapeHtml(row.key)}"${row.blocked ? ` title="${escapeHtml(row.blocked)}"` : ""}>
<span class="bar${row.hue ? ` ${row.hue}` : ""}"></span>
<span class="glyph-slot${row.activity === "working" ? " working" : ""}"><img class="glyph${row.activity === "working" ? " working" : ""}" src="${escapeHtml(iconUri)}" alt="${escapeHtml(row.serviceName)}" draggable="false"></span>
<span class="title">${escapeHtml(row.title)}</span>
${dot}
${row.memory ? `<span class="memory" title="Memory the provider process holds now">${escapeHtml(row.memory)}</span>` : ""}
<span class="actions">${actions}</span>
</div>`;
}

function action(command: string, label: string, codicon: string): string {
  return `<button class="act" type="button" data-command="${command}" title="${escapeHtml(label)}" aria-label="${escapeHtml(label)}"><i class="ci ci-${codicon}" aria-hidden="true"></i></button>`;
}

function spokenActivity(row: SidebarConversationRow): string {
  if (!row.canOpen) return "cannot be reopened";
  switch (row.activity) {
    case "needsYou":
      return "needs you";
    case "attention":
      return "needs attention";
    case "working":
      return "working";
    case "waitingOnQuota":
      return "waiting on a limit";
    case "ready":
      return "ready";
    case "saved":
      return "saved";
  }
}

function noticeHtml(notice: SidebarNotice): string {
  const act = notice.command && notice.label
    ? `<button class="notice-act" type="button" data-command="${notice.command}">${escapeHtml(notice.label)}</button>`
    : "";
  return `<div class="notice ${notice.tone}">${escapeHtml(notice.text)}${act}</div>`;
}

function serviceChoiceHtml(choice: SidebarServiceChoice, assets: SidebarAssets): string {
  return `<div class="choice" data-workspace="${escapeHtml(choice.workspace)}">
<span class="choice-title">Start with</span>
${choice.services.map((service) => `<button class="choice-item" type="button" data-command="runtrol.startSessionWith" data-kind="service" data-key="${escapeHtml(service.providerId)}"><img src="${escapeHtml(assets.iconUris.get(service.icon) ?? "")}" alt="">${escapeHtml(service.displayName)}</button>`).join("")}
</div>`;
}

function firstRunHtml(): string {
  return `<div class="first-run">
<button class="first-act" type="button" data-command="runtrol.createProject"><i class="ci ci-folder-library"></i><span>Add a project</span><small>Bring its conversations</small></button>
<button class="first-act" type="button" data-command="runtrol.startSession"><i class="ci ci-add"></i><span>New conversation</span><small>Start without a project</small></button>
</div>`;
}

// Codicon glyphs the page uses, as inline masks over the theme foreground: the webview cannot load the editor's
// icon font, and an <img> would not follow the theme colour. Each is the codicon outline in a 16-unit box.
const STYLE = `
:root { color-scheme: light dark; }
/* The panel's height, taken twice: the editor gives the frame its height and the document has to claim it, or
   the page is only as tall as its rows and the usage strip stops being the bottom of the sidebar. */
html { height: 100%; }
body { margin: 0; padding: 0; height: 100%; min-height: 100%; box-sizing: border-box; display: flex; flex-direction: column; overflow: hidden; color: var(--vscode-sideBar-foreground, var(--vscode-foreground)); background: transparent; font: var(--vscode-font-size) var(--vscode-font-family); user-select: none; }
button { font: inherit; color: inherit; }
.notice { margin: 4px 4px 0; padding: 4px 6px; border-radius: 4px; font-size: 12px; background: var(--vscode-editorWidget-background); border-left: 3px solid var(--vscode-widget-border); }
.notice.warn { border-left-color: var(--vscode-editorWarning-foreground); }
.notice.error { border-left-color: var(--vscode-errorForeground); }
.notice-act { margin-left: 8px; border: 1px solid var(--vscode-button-border, transparent); border-radius: 3px; background: var(--vscode-button-secondaryBackground); color: var(--vscode-button-secondaryForeground); padding: 1px 8px; cursor: pointer; }
.choice { margin: 4px 4px 0; padding: 6px; border-radius: 6px; background: var(--vscode-editorWidget-background); border: 1px solid var(--vscode-widget-border); }
.choice-title { display: block; font-size: 11px; opacity: 0.7; margin: 0 0 4px 2px; }
.choice-item { display: flex; align-items: center; gap: 6px; width: 100%; border: 0; border-radius: 4px; background: transparent; padding: 4px 6px; text-align: left; cursor: pointer; }
.choice-item:hover, .choice-item:focus-visible { background: var(--vscode-list-hoverBackground); outline: none; }
.choice-item img { width: 14px; height: 14px; }
.first-run { display: grid; gap: 6px; padding: 8px 4px 4px; }
.first-act { display: grid; grid-template-columns: 20px 1fr; grid-template-rows: auto auto; column-gap: 8px; align-items: center; text-align: left; border: 1px solid var(--vscode-widget-border); border-radius: 6px; background: var(--vscode-editorWidget-background); padding: 8px 10px; cursor: pointer; }
.first-act:hover, .first-act:focus-visible { border-color: var(--vscode-focusBorder); outline: none; }
.first-act i { grid-row: 1 / span 2; }
.first-act span { font-weight: 600; }
.first-act small { opacity: 0.7; }
/* The list scrolls and the usage strip does not: a person looking for how much is left should not have to
   scroll a list of conversations to find it (operator, 2026-08-28). */
.scroll { flex: 1 1 auto; min-height: 0; overflow-y: auto; overflow-x: hidden; }
.zone { padding: 4px 0 2px; }
.zone + .zone { border-top: 1px solid var(--vscode-sideBarSectionHeader-border, var(--vscode-widget-border)); margin-top: 4px; }
.zone-title { margin: 4px 4px 2px; font-size: 11px; font-weight: 600; letter-spacing: 0.04em; text-transform: uppercase; opacity: 0.55; }
.usage-zone { flex: none; background: var(--vscode-sideBar-background); padding-bottom: 4px; border-top: 1px solid var(--vscode-sideBarSectionHeader-border, var(--vscode-widget-border)); }
.row { position: relative; display: flex; align-items: center; gap: 6px; min-height: 24px; padding: 2px 4px 2px 0; border-radius: 4px; cursor: pointer; outline: none; }
.row:hover { background: var(--vscode-list-hoverBackground); }
.row:focus-visible { box-shadow: inset 0 0 0 1px var(--vscode-focusBorder); }
.row .bar { flex: none; width: 3px; align-self: stretch; border-radius: 2px; margin: 2px 5px 2px 2px; }
.project-row { font-weight: 600; }
.project-row .chevron { flex: none; width: 10px; height: 10px; margin-right: -2px; background: currentColor; opacity: 0.6; -webkit-mask: url("data:image/svg+xml;utf8,<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 16 16'><path d='M5 3l6 5-6 5z'/></svg>") center / contain no-repeat; mask: url("data:image/svg+xml;utf8,<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 16 16'><path d='M5 3l6 5-6 5z'/></svg>") center / contain no-repeat; transform: rotate(90deg); transition: transform 80ms; }
.project.collapsed .project-row .chevron { transform: rotate(0deg); }
.project.collapsed .rows { display: none; }
.project-row .name { white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.project-row .count { font-weight: 400; opacity: 0.55; font-size: 11px; }
.project-row.current .name::after { content: " · here"; font-weight: 400; opacity: 0.6; font-size: 11px; }
.badge { flex: none; font-size: 10px; line-height: 14px; padding: 0 5px; border-radius: 7px; font-weight: 600; }
.badge.attention { background: var(--vscode-notificationsWarningIcon-foreground); color: var(--vscode-sideBar-background); }
.badge.live { background: var(--vscode-progressBar-background); color: var(--vscode-sideBar-background); }
.badge.tools { background: transparent; border: 1px solid var(--vscode-widget-border); font-weight: 400; opacity: 0.8; }
.conv .glyph-slot { flex: none; position: relative; display: inline-flex; width: 14px; height: 14px; }
.conv .glyph { width: 14px; height: 14px; }
/* A turn is running. The icon turns, and a ring turns around it: the icon alone is 14px of slow rotation that
   a reader scanning the list does not catch (operator, 2026-08-28: make it unmistakable). */
.conv .glyph.working { animation: spin 1.1s linear infinite; }
.conv .glyph-slot.working::after { content: ""; position: absolute; inset: -3px; border-radius: 50%; border: 1.5px solid transparent; border-top-color: var(--vscode-progressBar-background); border-right-color: var(--vscode-progressBar-background); animation: spin 0.9s linear infinite; }
@keyframes spin { to { transform: rotate(360deg); } }
/* One line, and the tail fades out rather than ending in dots: the reader sees there is more without a
   glyph spending width to say so, and two-line rows made the list hard to scan (operator, 2026-08-28). */
.conv .title { flex: 1 1 auto; min-width: 0; white-space: nowrap; overflow: hidden; line-height: 1.4; -webkit-mask-image: linear-gradient(to right, #000 calc(100% - 20px), transparent); mask-image: linear-gradient(to right, #000 calc(100% - 20px), transparent); }
.conv.blocked .title { opacity: 0.5; }
.conv.pinned .title::before { content: ""; display: inline-block; width: 9px; height: 9px; margin-right: 4px; background: currentColor; opacity: 0.55; -webkit-mask: url("data:image/svg+xml;utf8,<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 16 16'><path d='M10 1l5 5-3 1-2 2 1 4-3 1-2-4-4 4-1-1 4-4-4-2 1-3 4 1 2-2z'/></svg>") center / contain no-repeat; mask: url("data:image/svg+xml;utf8,<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 16 16'><path d='M10 1l5 5-3 1-2 2 1 4-3 1-2-4-4 4-1-1 4-4-4-2 1-3 4 1 2-2z'/></svg>") center / contain no-repeat; }
.dot { flex: none; width: 7px; height: 7px; border-radius: 50%; background: var(--vscode-descriptionForeground); opacity: 0.6; }
.dot.working { background: var(--vscode-progressBar-background); opacity: 1; }
.dot.needsYou { background: var(--vscode-notificationsWarningIcon-foreground); opacity: 1; box-shadow: 0 0 0 2px color-mix(in srgb, var(--vscode-notificationsWarningIcon-foreground) 30%, transparent); }
.dot.attention { background: var(--vscode-notificationsWarningIcon-foreground); opacity: 1; }
.dot.waitingOnQuota { background: var(--vscode-errorForeground); opacity: 1; }
.dot.ready { background: var(--vscode-testing-iconPassed, var(--vscode-charts-green)); opacity: 0.9; }
.more .more-label { flex: 1 1 auto; font-size: 11px; opacity: 0.65; }
.more:hover .more-label { opacity: 1; }
.memory { flex: none; font-size: 10px; font-variant-numeric: tabular-nums; opacity: 0.6; }
.actions { flex: none; display: none; gap: 1px; margin-left: 2px; }
.row:hover .actions, .row:focus-within .actions { display: inline-flex; }
.act { border: 0; background: transparent; padding: 2px; border-radius: 3px; cursor: pointer; opacity: 0.75; line-height: 0; }
.act:hover, .act:focus-visible { background: var(--vscode-toolbar-hoverBackground); opacity: 1; outline: none; }
.project[draggable="true"] .project-row { cursor: grab; }
.project.dragging { opacity: 0.4; }
.project.drop-before { box-shadow: inset 0 2px 0 var(--vscode-focusBorder); }
.project.drop-after { box-shadow: inset 0 -2px 0 var(--vscode-focusBorder); }
.ci { display: inline-block; width: 14px; height: 14px; background: currentColor; -webkit-mask: var(--ci) center / contain no-repeat; mask: var(--ci) center / contain no-repeat; }
.ci-add { --ci: url("data:image/svg+xml;utf8,<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 16 16'><path d='M7 2h2v5h5v2H9v5H7V9H2V7h5z'/></svg>"); }
.ci-close { --ci: url("data:image/svg+xml;utf8,<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 16 16'><path d='M3.3 2l4.7 4.7L12.7 2 14 3.3 9.3 8l4.7 4.7-1.3 1.3L8 9.3 3.3 14 2 12.7 6.7 8 2 3.3z'/></svg>"); }
.ci-pin { --ci: url("data:image/svg+xml;utf8,<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 16 16'><path d='M10 1l5 5-3 1-2 2 1 4-3 1-2-4-4 4-1-1 4-4-4-2 1-3 4 1 2-2z' fill='none' stroke='currentColor' stroke-width='1.5'/></svg>"); }
.ci-pinned { --ci: url("data:image/svg+xml;utf8,<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 16 16'><path d='M10 1l5 5-3 1-2 2 1 4-3 1-2-4-4 4-1-1 4-4-4-2 1-3 4 1 2-2z'/></svg>"); }
.ci-edit { --ci: url("data:image/svg+xml;utf8,<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 16 16'><path d='M12 1l3 3-8 8H4V9zM2 14h12v1H2z'/></svg>"); }
.ci-trash { --ci: url("data:image/svg+xml;utf8,<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 16 16'><path d='M6 1h4l1 1h3v2H2V2h3zM3 5h10l-1 10H4z'/></svg>"); }
.ci-archive { --ci: url("data:image/svg+xml;utf8,<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 16 16'><path d='M1 2h14v4H1zM2 7h12v8H2zm4 2v1h4V9z'/></svg>"); }
.ci-debug-stop { --ci: url("data:image/svg+xml;utf8,<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 16 16'><rect x='3' y='3' width='10' height='10' rx='1'/></svg>"); }
.ci-check { --ci: url("data:image/svg+xml;utf8,<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 16 16'><path d='M6 11L2.5 7.5l1.4-1.4L6 8.2l6.1-6.1 1.4 1.4z'/></svg>"); }
.ci-circle-slash { --ci: url("data:image/svg+xml;utf8,<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 16 16'><path d='M8 1a7 7 0 110 14A7 7 0 018 1zm0 2a5 5 0 00-4 8l7-7a5 5 0 00-3-1zm4 2l-7 7a5 5 0 007-7z'/></svg>"); }
.ci-key { --ci: url("data:image/svg+xml;utf8,<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 16 16'><path d='M10 1a5 5 0 00-4.7 6.7L1 12v3h3v-2h2v-2h2l.3-.3A5 5 0 1010 1zm1 3a1.5 1.5 0 110 3 1.5 1.5 0 010-3z'/></svg>"); }
.ci-sparkle { --ci: url("data:image/svg+xml;utf8,<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 16 16'><path d='M8 1l1.6 4.4L14 7l-4.4 1.6L8 13l-1.6-4.4L2 7l4.4-1.6z' fill='none' stroke='currentColor' stroke-width='1.4'/></svg>"); }
.ci-sparkle-filled { --ci: url("data:image/svg+xml;utf8,<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 16 16'><path d='M8 1l1.6 4.4L14 7l-4.4 1.6L8 13l-1.6-4.4L2 7l4.4-1.6z'/></svg>"); }
.ci-link-external { --ci: url("data:image/svg+xml;utf8,<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 16 16'><path d='M9 1h6v6h-2V4.4L7.4 10 6 8.6 11.6 3H9zM2 3h5v2H4v7h7V9h2v5H2z'/></svg>"); }
.ci-folder-library { --ci: url("data:image/svg+xml;utf8,<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 16 16'><path d='M1 3h5l1 1h8v10H1zm2 2v7h10V6H6.6L5.6 5z'/></svg>"); }
.ci-kebab-vertical { --ci: url("data:image/svg+xml;utf8,<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 16 16'><circle cx='8' cy='3' r='1.5'/><circle cx='8' cy='8' r='1.5'/><circle cx='8' cy='13' r='1.5'/></svg>"); }
`;

// The page's own behaviour: presses go to the host as a command and a target, projects collapse and reorder
// locally, and the keyboard walks rows. Nothing here computes anything about a conversation.
const SCRIPT = `
(function () {
  var vscode = window.__runtrolVsCodeApi || (window.__runtrolVsCodeApi = acquireVsCodeApi());
  var restored = vscode.getState() || {};
  function post(message) { vscode.postMessage(message); }
  function rowOf(element) { return element.closest(".row"); }
  function targetOf(element) {
    var row = rowOf(element);
    if (!row) return null;
    return { kind: row.dataset.kind, key: row.dataset.key };
  }
  document.addEventListener("click", function (event) {
    var button = event.target.closest("[data-command]");
    if (button) {
      event.stopPropagation();
      var kind = button.dataset.kind;
      var key = button.dataset.key;
      var choice = button.closest(".choice");
      var target = kind ? { kind: kind, key: key, workspace: choice ? choice.dataset.workspace : undefined } : targetOf(button);
      post({ type: "command", command: button.dataset.command, target: target });
      return;
    }
    var row = rowOf(event.target);
    if (!row) return;
    if (row.dataset.kind === "more") {
      post({ type: "expand", key: row.dataset.key });
      return;
    }
    if (row.dataset.kind === "project") {
      var project = row.closest(".project");
      project.classList.toggle("collapsed");
      var collapsed = project.classList.contains("collapsed");
      row.setAttribute("aria-expanded", collapsed ? "false" : "true");
      post({ type: "collapse", key: row.dataset.key, collapsed: collapsed });
      return;
    }
    post({ type: "command", command: "runtrol.selectSession", target: targetOf(row) });
  });
  document.addEventListener("keydown", function (event) {
    var row = rowOf(document.activeElement);
    if (!row) return;
    if (event.key === "Enter" || event.key === " ") { event.preventDefault(); row.click(); return; }
    if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
    event.preventDefault();
    var rows = Array.prototype.slice.call(document.querySelectorAll(".row")).filter(function (candidate) { return candidate.offsetParent !== null; });
    var at = rows.indexOf(row);
    var next = rows[at + (event.key === "ArrowDown" ? 1 : -1)];
    if (next) next.focus();
  });
  var dragging = null;
  document.querySelectorAll('.project[draggable="true"]').forEach(function (project) {
    project.addEventListener("dragstart", function (event) {
      dragging = project;
      project.classList.add("dragging");
      event.dataTransfer.effectAllowed = "move";
      event.dataTransfer.setData("text/plain", project.dataset.project);
    });
    project.addEventListener("dragend", function () {
      project.classList.remove("dragging");
      document.querySelectorAll(".drop-before, .drop-after").forEach(function (marked) { marked.classList.remove("drop-before", "drop-after"); });
      dragging = null;
    });
    project.addEventListener("dragover", function (event) {
      if (!dragging || dragging === project) return;
      event.preventDefault();
      var box = project.getBoundingClientRect();
      var before = event.clientY < box.top + box.height / 2;
      project.classList.toggle("drop-before", before);
      project.classList.toggle("drop-after", !before);
    });
    project.addEventListener("dragleave", function () { project.classList.remove("drop-before", "drop-after"); });
    project.addEventListener("drop", function (event) {
      if (!dragging || dragging === project) return;
      event.preventDefault();
      var box = project.getBoundingClientRect();
      var before = event.clientY < box.top + box.height / 2;
      project.parentNode.insertBefore(dragging, before ? project : project.nextSibling);
      var keys = Array.prototype.slice.call(document.querySelectorAll('.project[draggable="true"]')).map(function (node) { return node.dataset.project; });
      post({ type: "reorder", keys: keys });
    });
  });
  window.addEventListener("message", function (event) {
    var message = event.data || {};
    if (message.type === "reveal") {
      var row = document.querySelector('.row[data-key="' + CSS.escape(message.key) + '"]');
      if (!row) return;
      var project = row.closest(".project");
      if (project && project.classList.contains("collapsed") && row.dataset.kind !== "project") {
        project.classList.remove("collapsed");
      }
      row.scrollIntoView({ block: "nearest" });
      row.focus({ preventScroll: true });
    }
  });
  var scroller = document.querySelector(".scroll");
  if (scroller) {
    if (typeof restored.scrollTop === "number") scroller.scrollTop = restored.scrollTop;
    scroller.addEventListener("scroll", function () { vscode.setState({ scrollTop: scroller.scrollTop }); });
  }
  post({ type: "ready" });
})();
`;
