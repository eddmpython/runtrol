import assert from "node:assert/strict";
import { test } from "node:test";

import {
  conversationTitle,
  formatMemory,
  rowKeys,
  sidebarBody,
  sidebarHtml,
  type SidebarConversationRow,
  type SidebarModel,
  type SidebarProjectRow,
} from "./sidebarPage";

function conversation(overrides: Partial<SidebarConversationRow>): SidebarConversationRow {
  return {
    key: "claude:one",
    title: "Fix the login flow",
    serviceName: "Claude Code",
    icon: "claude",
    accent: "#4e94ce",
    open: false,
    activity: "saved",
    live: false,
    canStop: false,
    canOpen: true,
    blocked: null,
    pinned: false,
    signIn: false,
    canDelete: true,
    canArchive: false,
    memory: null,
    tool: null,
    workspace: "C:\\work\\app",
    ...overrides,
  };
}

function project(overrides: Partial<SidebarProjectRow>): SidebarProjectRow {
  return {
    key: "project:app",
    name: "app",
    workspace: "C:\\work\\app",
    kind: "created",
    pinned: false,
    current: false,
    collapsed: false,
    attention: 0,
    live: 0,
    hidden: 0,
    branch: null,
    changes: null,
    agentTools: false,
    rows: [conversation({})],
    ...overrides,
  };
}

function model(overrides: Partial<SidebarModel>): SidebarModel {
  return {
    notices: [],
    projects: [project({})],
    loose: [conversation({ key: "codex:loose", serviceName: "Codex", icon: "codex" })],
    usage: [],
    serviceChoice: null,
    firstRun: false,
    version: "0.1.34",
    ...overrides,
  };
}

const assets = {
  cspSource: "vscode-resource:",
  nonce: "n0nce",
  iconUris: new Map([["claude", "https://icons/claude.svg"], ["codex", "https://icons/codex.svg"]]),
  accentIconUris: new Map([
    ["claude\0#4e94ce", "https://icons/claude-blue.svg"],
    ["codex\0#4e94ce", "https://icons/codex-blue.svg"],
  ]),
};

test("the three zones are drawn in order with their own titles, and the project carries its conversations", () => {
  const html = sidebarHtml(model({}), assets);
  const projectsAt = html.indexOf('aria-label="Projects"');
  const conversationsAt = html.indexOf('aria-label="Conversations"');
  assert.ok(projectsAt > 0 && conversationsAt > projectsAt, "projects come before loose conversations");
  assert.ok(html.includes('data-kind="project" data-key="project:app"'));
  assert.ok(html.includes('data-kind="conversation" data-key="claude:one"'));
  assert.ok(html.includes('data-kind="conversation" data-key="codex:loose"'));
  // Usage is absent when no service is installed: no empty zone.
  assert.ok(!html.includes('aria-label="Usage"'));
});

test("what the page draws is separable from the document, so a figure ticking never rebuilds it", () => {
  // A repaint sends only this. If the document went with it, the panel a person had opened and the row they
  // had focused would go too, and the usage figures tick on a clock nobody pressed.
  const body = sidebarBody(model({}), assets);
  assert.ok(!body.includes("<script"), "the body carries no script: the document's own stays live");
  assert.ok(!body.includes("<style"), "the body carries no style: the nonced stylesheet stays live");
  assert.ok(!body.includes("<!DOCTYPE"));
  assert.ok(body.includes('data-kind="project" data-key="project:app"'));
  // The document is that same body inside the shell the page needs once.
  const html = sidebarHtml(model({}), assets);
  assert.ok(html.includes(`<div id="page">${body}</div>`), "the first paint writes the same body");
  // The page can find what to replace, and knows to rebind what it replaced.
  assert.ok(html.includes('message.type === "paint"'));
  assert.ok(html.includes("__runtrolBindUsage"));
});

test("left bars are absent and open or working conversations use the exact accented provider glyph", () => {
  const html = sidebarHtml(model({ projects: [project({ rows: [
    conversation({ key: "idle" }),
    conversation({ key: "open", open: true }),
    conversation({ key: "working", activity: "working" }),
  ] })] }), assets);
  assert.ok(!html.includes('class="bar'), "project and conversation rows carry no left colour bars");
  assert.equal((html.match(/https:\/\/icons\/claude-blue\.svg/gu) ?? []).length, 2);
  assert.equal((html.match(/https:\/\/icons\/claude\.svg/gu) ?? []).length, 1);
  assert.ok(html.includes('class="row conv open"'));
  assert.ok(html.includes('class="row conv working"'));
});

test("the build's version is not drawn in the body: the host puts it in the title bar beside Runtrol", () => {
  assert.ok(!sidebarHtml(model({ version: "0.1.35" }), assets).includes("v0.1.35"), "no version line under the header");
});

test("row actions are buttons that name their command, and only the actions a row can perform", () => {
  const html = sidebarHtml(model({
    projects: [project({ rows: [conversation({ live: true, canStop: true, activity: "needsYou", canDelete: false, canArchive: true })] })],
    loose: [],
  }), assets);
  for (const command of ["runtrol.allowFromRow", "runtrol.declineFromRow", "runtrol.closeSession", "runtrol.archiveConversation", "runtrol.pinConversation", "runtrol.renameSession"]) {
    assert.ok(html.includes(`data-command="${command}"`), command);
  }
  assert.ok(!html.includes('data-command="runtrol.deleteConversation"'), "a service without deletion offers none");
  for (const command of ["runtrol.newConversationInProject", "runtrol.enableAgentTools", "runtrol.pinProject", "runtrol.removeProject"]) {
    assert.ok(html.includes(`data-command="${command}"`), command);
  }
});

test("the service choice withdraws on a click elsewhere, on Escape, and when focus leaves the panel", () => {
  const html = sidebarHtml(model({
    projects: [],
    loose: [],
    serviceChoice: { workspace: "C:\\work\\app", services: [{ providerId: "claude", displayName: "Claude Code", icon: "claude" }] },
  }), assets);
  assert.ok(html.includes('class="choice"'), "the choice is on the page");
  const script = html.slice(html.indexOf("<script"));
  assert.ok(script.includes('post({ type: "dismissChoice" })'), "the page tells the host to withdraw the choice");
  assert.ok(script.includes('if (!event.target.closest(".choice")) dismissChoice();'), "a click outside withdraws it");
  assert.ok(script.includes('if (event.key === "Escape") { dismissChoice(); return; }'), "Escape withdraws it");
  assert.ok(script.includes('window.addEventListener("blur", dismissChoice);'), "focus leaving withdraws it");
});

test("a conversation alive where Runtrol cannot reach its process is not offered a Stop that would fail", () => {
  const html = sidebarHtml(model({
    projects: [project({ rows: [conversation({ live: true, canStop: false, canOpen: false, blocked: "running outside runtrol" })] })],
    loose: [],
  }), assets);
  assert.ok(!html.includes('data-command="runtrol.closeSession"'));
});

test("memory reads as a short figure and rides the row", () => {
  assert.equal(formatMemory(512 * 1024), "<1 MB");
  assert.equal(formatMemory(412 * 1024 * 1024), "412 MB");
  assert.equal(formatMemory(1.6 * 1024 * 1024 * 1024), "1.6 GB");
  const html = sidebarHtml(model({ projects: [project({ rows: [conversation({ memory: "412 MB" })] })], loose: [] }), assets);
  assert.ok(html.includes('<span class="memory" title="Memory the provider process holds now">412 MB</span>'));
});

test("only actual working state rotates while attention remains a static labelled state", () => {
  const html = sidebarHtml(model({
    projects: [project({
      rows: [
        conversation({ key: "claude:one", activity: "working" }),
        conversation({ key: "claude:two" }),
        conversation({ key: "claude:three", activity: "needsYou" }),
        conversation({ key: "claude:four", live: true, canOpen: false, blocked: "running outside runtrol" }),
      ],
    })],
    loose: [],
  }), assets);
  // Running is said once on the row, and only its provider icon animates.
  assert.equal(html.match(/class="row conv working"/gu)?.length, 1);
  assert.ok(!html.includes('class="glyph-slot working"'), "the icon slot no longer carries its own copy of the state");
  assert.ok(html.includes('<span class="glyph-slot"><img class="glyph"'), "an idle row carries no mark");
  assert.ok(!html.includes('class="dot'), "no unexplained status dot is drawn");
  assert.ok(html.includes('class="conv-state attention" title="Needs you">Needs you</span>'));
  assert.ok(html.includes('class="conv-state muted" title="running outside runtrol">Elsewhere</span>'));
  assert.ok(html.includes(".conv.open .glyph, .conv.working .glyph { filter: none; opacity: 1; }"));
  assert.ok(html.includes(".conv.working .glyph { animation: spin"));
  assert.ok(!html.includes(".glyph-slot.working::after"), "no ring is drawn around a turning icon");
  assert.ok(!html.includes(".conv.working .bar"));
  assert.ok(!html.includes("animation: flow"));
  assert.ok(!html.includes(".conv.blocked .title"), "an externally running conversation stays readable");
});

test("row keys are unique and in page order, which is what the eye test reads", () => {
  const keys = rowKeys(model({}));
  assert.deepEqual(keys, ["project:app", "claude:one", "codex:loose"]);
});

test("a project says which branch its folder is on, and says nothing when it is not in a repository", () => {
  const on = sidebarHtml(model({ projects: [project({ branch: "release/1.2" })] }), assets);
  assert.ok(on.includes('class="badge branch"'));
  assert.ok(on.includes("release/1.2"));
  const off = sidebarHtml(model({ projects: [project({ branch: null })] }), assets);
  assert.ok(!off.includes('class="badge branch"'), "a folder outside a repository carries no chip");
});

test("a project counts what it holds, and says how many wait behind the row that shows them", () => {
  const html = sidebarHtml(model({
    projects: [project({ rows: [conversation({ key: "a" })], hidden: 7 })],
  }), assets);
  // Eight conversations, one drawn: the heading counts all of them, because the count is what tells a reader
  // the list is capped before they read the row that offers the rest.
  assert.ok(html.includes('<span class="count">8</span>'), html.slice(html.indexOf("count")));
  assert.ok(html.includes("Show all (7 more conversations)"));
  const one = sidebarHtml(model({ projects: [project({ rows: [conversation({ key: "a" })], hidden: 1 })] }), assets);
  assert.ok(one.includes("Show all (1 more conversation)"), "one is not one conversations");
});

test("the page spends no row on a menu, and escapes what services said", () => {
  const html = sidebarHtml(model({
    loose: [conversation({ key: "x", title: "<script>alert(1)</script>" })],
  }), assets);
  // The rare actions live behind the title bar's own `⋮` now. A strip inside the page spent a whole row of a
  // 200px panel on one button, which is the row the operator asked about on 2026-08-28.
  assert.ok(!html.includes("menu-bar"));
  assert.ok(!html.includes('class="ci ci-kebab-vertical"'));
  assert.ok(!html.includes("<script>alert(1)</script>"));
  assert.ok(html.includes("&lt;script&gt;alert(1)&lt;/script&gt;"));
  assert.ok(html.includes("script-src 'nonce-n0nce'"));
});

test("first run offers the two starting actions and a chosen service offers its buttons", () => {
  const html = sidebarHtml(model({
    projects: [],
    loose: [],
    firstRun: true,
    version: "0.1.34",
    serviceChoice: { workspace: "C:\\work\\app", services: [{ providerId: "claude", displayName: "Claude Code", icon: "claude" }] },
  }), assets);
  assert.ok(html.includes('data-command="runtrol.createProject"'));
  assert.ok(html.includes('data-command="runtrol.startSession"'));
  assert.ok(html.includes('data-command="runtrol.startSessionWith" data-kind="service" data-key="claude"'));
});

test("a project shows its uncommitted lines, new files and unpushed commits, and nothing when it is clean", () => {
  const dirty = sidebarHtml(model({ projects: [project({ changes: { added: 120, removed: 35, untracked: 2, ahead: 3 } })] }), assets);
  assert.ok(dirty.includes('<span class="add">+120</span>'));
  assert.ok(dirty.includes('<span class="del">-35</span>'));
  assert.ok(dirty.includes('<span class="new">?2</span>'));
  assert.ok(dirty.includes('<span class="ahead">↑3</span>'));
  assert.ok(dirty.includes("120 lines added, 35 removed, not committed; 2 new files not in git; 3 commits not pushed"));
  const partial = sidebarHtml(model({ projects: [project({ changes: { added: 0, removed: 0, untracked: 0, ahead: 1 } })] }), assets);
  assert.ok(partial.includes('<span class="ahead">↑1</span>'));
  assert.ok(!partial.includes('class="add"'), "a zero is not drawn");
  const clean = sidebarHtml(model({ projects: [project({ changes: { added: 0, removed: 0, untracked: 0, ahead: 0 } })] }), assets);
  assert.ok(!clean.includes('class="badge changes"'), "a clean, pushed project has no chip");
  const unknown = sidebarHtml(model({ projects: [project({ changes: null })] }), assets);
  assert.ok(!unknown.includes('class="badge changes"'));

  const large = sidebarHtml(model({ projects: [project({ changes: { added: 9120, removed: 8305, untracked: 1234, ahead: 1000 } })] }), assets);
  assert.ok(large.includes('<span class="add">+9,120</span>'));
  assert.ok(large.includes('<span class="del">-8,305</span>'));
  assert.ok(large.includes('<span class="new">?1,234</span>'));
  assert.ok(large.includes('<span class="ahead">↑1,000</span>'));
});

test("long provider titles are bounded only in the sidebar projection", () => {
  const title = "Claude uses the first prompt as a provider-owned title and it can be much wider than one row";
  const visible = conversationTitle(title);
  const html = sidebarHtml(model({ projects: [project({ rows: [conversation({ title })] })] }), assets);
  assert.equal(Array.from(visible).length, 48);
  assert.ok(visible.endsWith("…"));
  assert.ok(html.includes(visible));
  assert.ok(html.includes(`title="${title}"`), "the full provider title remains available without replacing it");
});
