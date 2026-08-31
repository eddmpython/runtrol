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
import { hasChanges, type GitChanges } from "./gitChanges";
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
  /// Whether Runtrol can end this conversation's process: it supervises it or hosts its terminal. A process
  /// alive outside both is shown, and not offered a Stop that would fail.
  readonly canStop: boolean;
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
  /// The branch this folder's repository is on, or null when it is not in one.
  readonly branch: string | null;
  /// What the repository holds uncommitted or unpushed, or null when nothing is known.
  readonly changes: GitChanges | null;
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
  /// This build's version. The host puts it in the title bar beside "Runtrol"; the page draws nothing for it.
  readonly version: string;
};

export type SidebarAssets = UsageStripAssets;

/// One rule per hue, generated from the palette so the page and the projects cannot disagree about a colour.
///
/// In the stylesheet rather than on the element: the page's CSP allows styles from this nonced block only, and a
/// nonce does not cover inline `style` attributes, so a colour written onto the element is simply dropped.
/// The first six hues are the editor's own terminal palette and follow the theme through their variables; the
/// band-only extras have no editor name, so their light value is applied by the theme kind VS Code stamps on
/// the page's body (absent in the eye harness, which therefore shows the dark pair).
const HUE_STYLE = HUES
  .map((hue) => {
    const rule = `.project-row .bar.${hue.band}, .conv.working .bar.${hue.band} { background: ${hue.dark}; }`;
    if (hue.light === hue.dark) return rule;
    return `${rule}
body[data-vscode-theme-kind="vscode-light"] .project-row .bar.${hue.band}, body[data-vscode-theme-kind="vscode-light"] .conv.working .bar.${hue.band}, body[data-vscode-theme-kind="vscode-high-contrast-light"] .project-row .bar.${hue.band}, body[data-vscode-theme-kind="vscode-high-contrast-light"] .conv.working .bar.${hue.band} { background: ${hue.light}; }`;
  })
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
<div id="page">${sidebarBody(model, assets)}</div>
<script nonce="${assets.nonce}">${SCRIPT}${USAGE_SCRIPT}</script>
</body>
</html>`;
}

/// Everything the page draws, without the document around it.
///
/// The host sends this on its own after the first paint instead of writing the document again. Replacing the
/// document rebuilds every element, and the panel a person had opened, the row they had focused and the place
/// they had scrolled to all go with it. The usage figures tick on their own clock, so a detail panel closed
/// itself while the hand was still moving towards it, which is the mouse losing its way (operator, 2026-08-28)
/// and is also the plainest kind of stutter this panel can have.
export function sidebarBody(model: SidebarModel, assets: SidebarAssets): string {
  // Every top-level part has a fixed id, present even when empty. The repaint matches elements by key, and
  // an unkeyed part that comes and goes (a notice, the service choice) shifted every part after it onto the
  // wrong element: the list was rebuilt from scratch and jumped to the top whenever a notice appeared
  // (measured 2026-08-29).
  return `<div id="notices">${model.notices.map(noticeHtml).join("")}</div>
<div id="choice">${model.serviceChoice ? serviceChoiceHtml(model.serviceChoice, assets) : ""}</div>
<div id="first-run">${model.firstRun ? firstRunHtml() : ""}</div>
${zonesHtml(model, assets)}`;
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
    : `<section class="zone usage-zone" id="usage" aria-label="Usage">
<h2 class="zone-title"><i class="ci ci-gauge" aria-hidden="true"></i>Usage</h2>
${usagePanelsMarkup(model.usage)}
${usageChipsMarkup(model.usage, assets)}
</section>`;
  return `<div class="scroll" id="scroll">${projects}${loose}</div>${usage}`;
}

/// The uncommitted and unpushed work of a project, as `+120 -35 ?2 ↑3`, each part only while it is not zero.
///
/// Lines rather than files, because "three files changed" says nothing about whether it was a typo or an
/// afternoon. The chip vanishes at zero: a clean, pushed project has nothing to say and says nothing.
function changesMarkup(changes: GitChanges | null): string {
  if (!hasChanges(changes)) return "";
  const parts = [
    changes.added > 0 ? `<span class="add">+${countText(changes.added)}</span>` : "",
    changes.removed > 0 ? `<span class="del">-${countText(changes.removed)}</span>` : "",
    changes.untracked > 0 ? `<span class="new">?${countText(changes.untracked)}</span>` : "",
    changes.ahead > 0 ? `<span class="ahead">↑${countText(changes.ahead)}</span>` : "",
  ].join("");
  const said = [
    changes.added > 0 || changes.removed > 0
      ? `${countText(changes.added)} lines added, ${countText(changes.removed)} removed, not committed`
      : "",
    changes.untracked > 0 ? `${countText(changes.untracked)} new ${changes.untracked === 1 ? "file" : "files"} not in git` : "",
    changes.ahead > 0 ? `${countText(changes.ahead)} ${changes.ahead === 1 ? "commit" : "commits"} not pushed` : "",
  ].filter((line) => line !== "").join("; ");
  return `<span class="badge changes" title="${escapeHtml(said)}">${parts}</span>`;
}

/// Locale-stable grouping for compact source-control counts. The sidebar must not change shape with the host
/// locale, and every supported host understands ASCII comma-grouping.
export function countText(value: number): string {
  return Math.max(0, Math.trunc(value)).toLocaleString("en-US");
}

function projectHtml(project: SidebarProjectRow, assets: SidebarAssets): string {
  // What the project holds, not what fits: the rows are capped at five and the count is the reason a person
  // knows there is more before they reach the row that says so.
  const count = project.rows.length + project.hidden;
  const badges = [
    project.attention > 0 ? `<span class="badge attention" title="${project.attention} waiting for you">${project.attention}</span>` : "",
    project.live > 0 ? `<span class="badge live" title="${project.live} running">${project.live}</span>` : "",
    project.branch ? `<span class="badge branch" title="On branch ${escapeHtml(project.branch)}"><i class="ci ci-git-branch" aria-hidden="true"></i><span class="what">${escapeHtml(project.branch)}</span></span>` : "",
    changesMarkup(project.changes),
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
  const state = conversationStateHtml(row);
  const signal = row.signIn || row.activity === "needsYou"
    ? " needs-you"
    : row.activity === "attention"
      ? " attention"
      : row.activity === "working"
        ? " working"
        : "";
  const actions = [
    row.activity === "needsYou" ? action("runtrol.allowFromRow", "Allow", "check") + action("runtrol.declineFromRow", "Decline", "circle-slash") : "",
    row.signIn ? action("runtrol.signInFromRow", "Sign in", "key") : "",
    action(row.pinned ? "runtrol.unpinConversation" : "runtrol.pinConversation", row.pinned ? "Unpin" : "Pin to the top", row.pinned ? "pinned" : "pin"),
    action("runtrol.renameSession", "Rename", "edit"),
    row.canStop ? action("runtrol.closeSession", "Stop", "debug-stop") : "",
    row.canArchive ? action("runtrol.archiveConversation", "Archive", "archive") : "",
    row.canDelete ? action("runtrol.deleteConversation", "Delete", "trash") : "",
  ].join("");
  // A native tooltip only where it says the one thing the row cannot show: why an open would be refused.
  // Everything else the tooltip used to repeat (service, activity, model) is already on the row, and a
  // tooltip floating beside the hover actions reads as clutter (operator, 2026-08-27).
  // Running is a state of the whole row, said once on the row: the icon turns and the band flows from it.
  const visibleTitle = conversationTitle(row.title);
  const tooltip = row.blocked ?? (visibleTitle === row.title ? null : row.title);
  return `<div class="row conv${row.canOpen ? "" : " blocked"}${row.pinned ? " pinned" : ""}${state ? " stateful" : ""}${signal}" role="button" tabindex="0" data-kind="conversation" data-key="${escapeHtml(row.key)}"${tooltip ? ` title="${escapeHtml(tooltip)}"` : ""}>
<span class="bar${row.hue ? ` ${row.hue}` : ""}"></span>
<span class="glyph-slot"><img class="glyph" src="${escapeHtml(iconUri)}" alt="${escapeHtml(row.serviceName)}" draggable="false"></span>
<span class="title">${escapeHtml(visibleTitle)}</span>
${state}
<span class="tail">
<span class="actions">${actions}</span>
${row.memory ? `<span class="memory" title="Memory the provider process holds now">${escapeHtml(row.memory)}</span>` : ""}
</span>
</div>`;
}

const CONVERSATION_TITLE_LIMIT = 48;

/// Provider titles remain the stored identity. Only the one-line sidebar projection is bounded, by Unicode
/// characters rather than UTF-16 units, so a provider that uses the first prompt as its title cannot consume the
/// whole row and a non-BMP character is never cut in half.
export function conversationTitle(title: string): string {
  const normalized = title.trim().replace(/\s+/gu, " ");
  const characters = Array.from(normalized);
  if (characters.length <= CONVERSATION_TITLE_LIMIT) return normalized;
  return `${characters.slice(0, CONVERSATION_TITLE_LIMIT - 1).join("").trimEnd()}…`;
}

function action(command: string, label: string, codicon: string): string {
  return `<button class="act" type="button" data-command="${command}" title="${escapeHtml(label)}" aria-label="${escapeHtml(label)}"><i class="ci ci-${codicon}" aria-hidden="true"></i></button>`;
}

function conversationStateHtml(row: SidebarConversationRow): string {
  const state = row.signIn
    ? { label: "Sign in", tone: "attention" }
    : row.activity === "needsYou"
      ? { label: "Needs you", tone: "attention" }
      : row.activity === "waitingOnQuota"
        ? { label: "Limit", tone: "error" }
        : row.activity === "attention"
          ? { label: "Error", tone: "error" }
          : !row.canOpen
            ? { label: row.live ? "Elsewhere" : "Unavailable", tone: "muted" }
            : null;
  return state
    ? `<span class="conv-state ${state.tone}" title="${escapeHtml(row.blocked ?? state.label)}">${state.label}</span>`
    : "";
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
/* The fallback colour that says a projectless conversation is running.

   Not the editor's progress colour, which is the obvious choice and is not a state colour. What it paints is
   up to whichever theme is on: measured 2026-08-28, the dark theme this build of the editor falls back to
   when nobody has chosen one paints progressBar.background #878889, and a running row drawn in it was grey,
   which is what an idle row is. The chart colours are the ones meant to be told apart at a glance, and this
   one is vivid in both light and dark. */
:root { --runtrol-running: var(--vscode-charts-blue, #4e94ce); }
/* The panel's height, taken twice: the editor gives the frame its height and the document has to claim it, or
   the page is only as tall as its rows and the usage strip stops being the bottom of the sidebar. */
html { height: 100%; }
/* The panel paints the sidebar's own background, and it is the only thing that paints one.

   Transparent is not the same as inheriting here. A view like this one is an iframe, and what shows through a
   transparent body is the frame's own backdrop, which is neither the sidebar nor the editor: measured on the
   operator's window, the editor drew #1E1E1E, the sidebar the editor itself paints drew #252526, and this
   page drew #121212. So the list sat in a near black box inside a lighter sidebar and the usage strip, the
   one element that did name a colour, stood out as a grey card glued to it (operator, 2026-08-28: why does
   Agent Usage use a black background). One owner for one fact: the strip no longer names it. */
body { margin: 0; padding: 6px 0 6px 8px; height: 100%; min-height: 100%; box-sizing: border-box; display: flex; flex-direction: column; overflow: hidden; color: var(--vscode-sideBar-foreground, var(--vscode-foreground)); background: var(--vscode-sideBar-background); font: var(--vscode-font-size) var(--vscode-font-family); user-select: none; }
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
/* The element a repaint replaces. It carries the page's column so that wrapping the content for repainting
   does not change where anything sits: without this the usage zone stopped being the bottom of the panel and
   floated up under the last conversation (measured 2026-08-28). */
#page { flex: 1 1 auto; min-height: 0; display: flex; flex-direction: column; }
/* The scrollbar sits on the panel's edge, not 8px inside it: the body gives up its right padding and the
   scroller carries it, so the bar is outside the padded content (operator, 2026-08-29: why the gap). */
.scroll { flex: 1 1 auto; min-height: 0; overflow-y: auto; overflow-x: hidden; padding-right: 8px; }
.zone { padding: 4px 0 2px; }
.zone + .zone { border-top: 1px solid var(--vscode-sideBarSectionHeader-border, var(--vscode-widget-border)); margin-top: 4px; }
.zone-title { display: flex; align-items: center; gap: 4px; margin: 4px 4px 2px; font-size: 11px; font-weight: 600; letter-spacing: 0.04em; text-transform: uppercase; opacity: 0.55; }
.usage-zone { flex: none; padding-bottom: 4px; padding-right: 8px; border-top: 1px solid var(--vscode-sideBarSectionHeader-border, var(--vscode-widget-border)); }
.row { position: relative; display: flex; align-items: center; gap: 6px; min-height: 24px; padding: 2px 4px 2px 0; border-radius: 4px; cursor: pointer; outline: none; }
.row:hover { background: var(--vscode-list-hoverBackground); }
.row:focus-visible { box-shadow: inset 0 0 0 1px var(--vscode-focusBorder); }
.row .bar { flex: none; width: 3px; align-self: stretch; border-radius: 2px; margin: 2px 5px 2px 2px; }
.project-row { font-weight: 600; }
.project-row .chevron { flex: none; width: 10px; height: 10px; margin-right: -2px; background: currentColor; opacity: 0.6; -webkit-mask: url("data:image/svg+xml;utf8,<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 16 16'><path d='M5 3l6 5-6 5z'/></svg>") center / contain no-repeat; mask: url("data:image/svg+xml;utf8,<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 16 16'><path d='M5 3l6 5-6 5z'/></svg>") center / contain no-repeat; transform: rotate(90deg); transition: transform 80ms; }
.project.collapsed .project-row .chevron { transform: rotate(0deg); }
.project.collapsed .rows { display: none; }
/* The project's name is what the row is. An item with hidden overflow may shrink to nothing, and with the
   chips beside it refusing to shrink at all, that is what it did: at a real panel width the second project
   showed its branch, its count and its chips with no name at all (measured 2026-08-28). It keeps a floor and
   takes the free space; the chips beside it give theirs up first. */
.project-row .name { flex: 0 1 auto; min-width: 3.5em; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.project-row .count { flex: none; }
.project-row .count { font-weight: 400; opacity: 0.55; font-size: 11px; }
.project-row.current .name::after { content: " · here"; font-weight: 400; opacity: 0.6; font-size: 11px; }
.badge { flex: none; font-size: 10px; line-height: 14px; padding: 0 5px; border-radius: 7px; font-weight: 600; }
.badge.attention { background: var(--vscode-notificationsWarningIcon-foreground); color: var(--vscode-sideBar-background); }
.badge.live { background: var(--vscode-progressBar-background); color: var(--vscode-sideBar-background); }
.badge.branch { flex: 0 2 auto; min-width: 2.5em; background: transparent; font-weight: 400; opacity: 0.7; display: inline-flex; align-items: center; gap: 3px; padding: 0 2px; max-width: 90px; overflow: hidden; white-space: nowrap; text-overflow: ellipsis; }
/* The icon holds its size and the name gives way with an ellipsis. Shrinking the whole chip evenly ate the
   icon first and left a bare "featu", which names nothing; a branch mark with a shortened name still says
   what it is (measured 2026-08-28). */
.badge.branch .ci { flex: none; width: 11px; height: 11px; }
.badge.branch .what { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
/* The editor's own git colours, so the numbers read the same as the explorer's file decorations beside them. */
.badge.changes { flex: none; background: transparent; font-weight: 500; display: inline-flex; align-items: center; gap: 4px; padding: 0 2px; font-variant-numeric: tabular-nums; }
.badge.changes .add { color: var(--vscode-gitDecoration-addedResourceForeground); }
.badge.changes .del { color: var(--vscode-gitDecoration-deletedResourceForeground); }
.badge.changes .new { color: var(--vscode-gitDecoration-untrackedResourceForeground); }
.badge.changes .ahead { opacity: 0.7; font-weight: 400; }
.badge.tools { background: transparent; border: 1px solid var(--vscode-widget-border); font-weight: 400; opacity: 0.8; }
.conv .glyph-slot { position: relative; flex: none; display: inline-flex; align-items: center; justify-content: center; width: 14px; height: 14px; }
.conv .glyph { flex: none; width: 14px; height: 14px; filter: grayscale(1); opacity: 0.64; }
/* The icon alone turns (operator, 2026-08-29: no ring, just the icon). Fast enough that a symmetric mark
   still reads as moving, and a full turn is a plain transform the compositor can run without a repaint. */
.conv.working .glyph { animation: spin 1.1s linear infinite; will-change: transform; filter: none; opacity: 1; }
@keyframes spin { to { transform: rotate(360deg); } }
/* Idle rows keep the bar's alignment slot but paint nothing. A working row uses its project colour, or the
   running fallback when it has no project. Needs-you and error rows use static semantic colours, so urgency
   cannot be mistaken for progress. */
.row .bar { position: relative; overflow: hidden; }
.conv:not(.working):not(.needs-you):not(.attention) .bar { background: transparent; }
.conv.working .bar { background: var(--runtrol-running); }
.conv.needs-you .bar { background: var(--vscode-editorWarning-foreground, #cca700); }
.conv.attention .bar { background: var(--vscode-errorForeground, #f85149); }
/* A compositor-only light runs down the working band in step with the icon. */
.conv.working .bar::after { content: ""; position: absolute; left: 0; right: 0; top: 0; height: 100%; background: linear-gradient(to bottom, transparent, rgba(255, 255, 255, 0.75), transparent); animation: flow 1.1s linear infinite; will-change: transform; }
@keyframes flow { from { transform: translateY(-100%); } to { transform: translateY(100%); } }
/* One line, and the tail fades out rather than ending in dots: the reader sees there is more without a
   glyph spending width to say so, and two-line rows made the list hard to scan (operator, 2026-08-28). */
.conv .title { flex: 1 1 auto; min-width: 0; white-space: nowrap; overflow: hidden; line-height: 1.4; -webkit-mask-image: linear-gradient(to right, #000 calc(100% - 20px), transparent); mask-image: linear-gradient(to right, #000 calc(100% - 20px), transparent); }
.conv.pinned .title::before { content: ""; display: inline-block; width: 9px; height: 9px; margin-right: 4px; background: currentColor; opacity: 0.55; -webkit-mask: url("data:image/svg+xml;utf8,<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 16 16'><path d='M10 1l5 5-3 1-2 2 1 4-3 1-2-4-4 4-1-1 4-4-4-2 1-3 4 1 2-2z'/></svg>") center / contain no-repeat; mask: url("data:image/svg+xml;utf8,<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 16 16'><path d='M10 1l5 5-3 1-2 2 1 4-3 1-2-4-4 4-1-1 4-4-4-2 1-3 4 1 2-2z'/></svg>") center / contain no-repeat; }
/* Working needs no word: the moving icon and band say it. Only states that change what the person can do spend
   width, and they say their meaning instead of asking the person to memorize coloured dots. */
.conv-state { flex: none; max-width: 72px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: 10px; line-height: 15px; padding: 0 5px; border-radius: 8px; }
.conv-state.attention { color: var(--vscode-notificationsWarningIcon-foreground, #cca700); background: color-mix(in srgb, currentColor 14%, transparent); }
.conv-state.error { color: var(--vscode-errorForeground, #f85149); background: color-mix(in srgb, currentColor 14%, transparent); }
.conv-state.muted { color: var(--vscode-descriptionForeground); background: var(--vscode-badge-background); }
.more .more-label { flex: 1 1 auto; font-size: 11px; opacity: 0.65; }
.more:hover .more-label { opacity: 1; }
/* One slot on the right, and nothing in the row moves when the cursor arrives. The actions hold their width
   at rest and the memory figure sits on top of them; hovering swaps which one is painted, not the layout.
   Appearing actions used to relayout the row and shift the name under the cursor (operator, 2026-08-28), and
   on hover the actions are what the person came for. */
.tail { flex: none; display: inline-flex; align-items: center; gap: 4px; justify-content: flex-end; margin-left: 2px; }
/* The figure stays in the flow. It was taken out of it so the hover buttons could sit on top of it, and in a
   box that had shrunk to nothing it broke "306 MB" across two lines and printed the running dot through it
   (measured 2026-08-28). The buttons now overlay the whole row instead, so nothing has to hide here. */
.memory { flex: none; white-space: nowrap; font-size: 10px; font-variant-numeric: tabular-nums; opacity: 0.6; }
/* Always at the right edge of the row, never packed against the name: a person reaching for delete should
   find it in the same place on every row (operator, 2026-08-28). */
/* The hover actions sit over the right end of the row rather than beside it.
   Measured 2026-08-28 at a real panel width: hidden, they still held 113px of a 304px project row, which is
   where the branch name went. Reserving the space was meant to stop the row reflowing when they appear, and
   taking them out of the flow stops it just as completely: the row's own content never moves, and what the
   buttons cover is the faded tail a name was already losing. They carry the hover colour so nothing shows
   through them. */
.actions { position: absolute; right: 3px; top: 1px; bottom: 1px; display: inline-flex; align-items: center; gap: 1px; padding-left: 8px; visibility: hidden; background: var(--vscode-list-hoverBackground); }
.row:hover .actions, .row:focus-within .actions { visibility: visible; }
.row:hover .memory, .row:focus-within .memory { visibility: hidden; }
/* A blocked row must keep saying Elsewhere or Unavailable while its actions appear. The action strip used to
   cover that state at the exact moment a person clicked, so the following notification seemed to contradict
   the row. Give the word a fixed hover slot and place the actions immediately before it. */
.conv.stateful:hover .conv-state, .conv.stateful:focus-within .conv-state { position: absolute; right: 4px; z-index: 2; box-sizing: border-box; width: 82px; text-align: center; }
.conv.stateful:hover .actions, .conv.stateful:focus-within .actions { right: 90px; }
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
.zone-title .ci { width: 11px; height: 11px; opacity: 0.8; }
.ci-gauge { --ci: url("data:image/svg+xml;utf8,<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 16 16'><path d='M8 3a7 7 0 0 0-7 7v1h3v-1a4 4 0 1 1 8 0v1h3v-1a7 7 0 0 0-7-7z'/><path d='M8.9 10.6a1.3 1.3 0 1 1-1.8-1.8l4-2.6z'/></svg>"); }
.ci-git-branch { --ci: url("data:image/svg+xml;utf8,<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 16 16'><path d='M11 2a2 2 0 0 0-1 3.7V7a2 2 0 0 1-2 2H6a3 3 0 0 0-1 .2V5.7a2 2 0 1 0-2 0v4.6a2 2 0 1 0 2 .1A2 2 0 0 1 6 10h2a4 4 0 0 0 4-4V5.7A2 2 0 0 0 11 2z'/></svg>"); }
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
  // The service choice is a question, and a question a person walks away from is withdrawn: a click anywhere
  // else, Escape, or focus leaving the panel closes it (operator, 2026-08-29: it stayed open until answered).
  function dismissChoice() {
    if (document.querySelector(".choice")) post({ type: "dismissChoice" });
  }
  window.addEventListener("blur", dismissChoice);
  document.addEventListener("click", function (event) {
    if (!event.target.closest(".choice")) dismissChoice();
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
  // The project row's fuller menu, the way an editor row has one: everything the hover icons offer plus
  // deleting the project's conversations, which is too destructive to sit beside "new conversation"
  // (operator, 2026-08-29). The host draws the menu; the page only says which project was asked.
  document.addEventListener("contextmenu", function (event) {
    var row = rowOf(event.target);
    if (!row || row.dataset.kind !== "project") return;
    event.preventDefault();
    post({ type: "command", command: "runtrol.projectMenu", target: targetOf(row) });
  });
  document.addEventListener("keydown", function (event) {
    if (event.key === "Escape") { dismissChoice(); return; }
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
  // Elements survive a repaint now, so a listener bound once must not be bound again: a second drop handler
  // would post the reorder twice.
  var boundProjects = new WeakSet();
  function bindProjects() {
  document.querySelectorAll('.project[draggable="true"]').forEach(function (project) {
    if (boundProjects.has(project)) return;
    boundProjects.add(project);
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
  }
  // Repaint by difference. Replacing the whole body rebuilt every element on every tick, and a rebuilt element
  // starts its animation from zero: with the memory figures ticking, every turning icon jumped back to its
  // start each time (operator, 2026-08-29: the spinner stutters after a while). Elements that did not change
  // are left alone, so their animations run on; elements that did are updated in place.
  function morphInto(target, html) {
    var template = document.createElement("template");
    template.innerHTML = html;
    morphChildren(target, template.content);
  }
  function keyOf(node) {
    if (node.nodeType !== 1) return null;
    return node.getAttribute("data-key") || node.getAttribute("data-project") || node.id || null;
  }
  function morphChildren(from, to) {
    var wanted = Array.prototype.slice.call(to.childNodes);
    var have = Array.prototype.slice.call(from.childNodes);
    var byKey = {};
    have.forEach(function (node) { var key = keyOf(node); if (key !== null && !byKey[key]) byKey[key] = node; });
    var matched = new Array(wanted.length);
    var used = new WeakSet();
    // Keyed parts keep their element wherever they moved to.
    wanted.forEach(function (next, index) {
      var key = keyOf(next);
      if (key === null) return;
      var match = byKey[key];
      if (match && match.tagName === next.tagName && !used.has(match)) { matched[index] = match; used.add(match); }
    });
    // Unkeyed parts pair up in order with the free unkeyed old nodes of the same kind, so one that appeared
    // or vanished shifts nothing onto the wrong element.
    var cursor = 0;
    wanted.forEach(function (next, index) {
      if (matched[index] || keyOf(next) !== null) return;
      while (cursor < have.length) {
        var candidate = have[cursor++];
        if (used.has(candidate) || keyOf(candidate) !== null) continue;
        if (candidate.nodeType === next.nodeType && candidate.tagName === next.tagName) { matched[index] = candidate; used.add(candidate); break; }
      }
    });
    // What nothing wanted leaves first, so what stays is not moved past it: moving an element is a removal
    // and a reinsertion, which restarts its animation and drops its focus (measured 2026-08-29: closing the
    // row above a working one restarted that one's icon and lost the keyboard).
    have.forEach(function (node) { if (!used.has(node)) from.removeChild(node); });
    wanted.forEach(function (next, index) {
      var current = from.childNodes[index];
      var match = matched[index];
      if (match) {
        if (match !== current) from.insertBefore(match, current || null);
        morphNode(match, next);
      } else {
        from.insertBefore(next.cloneNode(true), current || null);
      }
    });
  }
  function morphNode(node, next) {
    if (node.nodeType === 3) { if (node.nodeValue !== next.nodeValue) node.nodeValue = next.nodeValue; return; }
    if (node.nodeType !== 1) return;
    var names = {};
    Array.prototype.slice.call(next.attributes).forEach(function (attribute) {
      names[attribute.name] = true;
      if (node.getAttribute(attribute.name) !== attribute.value) node.setAttribute(attribute.name, attribute.value);
    });
    Array.prototype.slice.call(node.attributes).forEach(function (attribute) {
      if (!names[attribute.name]) node.removeAttribute(attribute.name);
    });
    morphChildren(node, next);
  }
  bindProjects();
  window.addEventListener("message", function (event) {
    var message = event.data || {};
    if (message.type === "paint") {
      var page = document.getElementById("page");
      if (page) morphInto(page, message.body);
      bindProjects();
      if (window.__runtrolBindUsage) window.__runtrolBindUsage();
      return;
    }
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
