import assert from "node:assert/strict";
import { test } from "node:test";

import {
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
    hue: "hueBlue",
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
    hue: "hueBlue",
    kind: "created",
    pinned: false,
    current: false,
    collapsed: false,
    attention: 0,
    live: 0,
    hidden: 0,
    branch: null,
    agentTools: false,
    rows: [conversation({})],
    ...overrides,
  };
}

function model(overrides: Partial<SidebarModel>): SidebarModel {
  return {
    notices: [],
    projects: [project({})],
    loose: [conversation({ key: "codex:loose", hue: null, serviceName: "Codex", icon: "codex" })],
    usage: [],
    serviceChoice: null,
    firstRun: false,
    ...overrides,
  };
}

const assets = {
  cspSource: "vscode-resource:",
  nonce: "n0nce",
  iconUris: new Map([["claude", "https://icons/claude.svg"], ["codex", "https://icons/codex.svg"]]),
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

test("a project's colour reaches its heading bar and every conversation under it", () => {
  const html = sidebarHtml(model({}), assets);
  // The class, not a colour written onto the element: the page's CSP allows styles only from its nonced
  // stylesheet, and a nonce never covers an inline `style` attribute, so a colour put there paints nothing.
  assert.equal((html.match(/class="bar hueBlue"/gu) ?? []).length, 2, "heading and its one row");
  assert.ok(
    html.includes(".row .bar.hueBlue { background: var(--vscode-charts-blue); }"),
    "the page carries the rule that paints the band",
  );
  assert.ok(!html.includes('style="background'), "no colour is written onto an element for the CSP to drop");
  assert.ok(html.includes('class="bar"></span>'), "a loose conversation has no project colour");
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

test("a running conversation is marked once at its icon, and action states use words instead of dots", () => {
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
  // The mark is on the slot around the icon, not on the icon itself: an image cannot carry the arc, and the
  // arc is what a person sees when the service's own icon is symmetric enough that turning it shows nothing.
  assert.equal(html.match(/class="glyph-slot working"/gu)?.length, 1);
  assert.ok(html.includes('<span class="glyph-slot"><img class="glyph"'), "an idle row carries no mark");
  assert.ok(!html.includes('class="dot'), "no unexplained status dot is drawn");
  assert.ok(html.includes('class="conv-state attention" title="Needs you">Needs you</span>'));
  assert.ok(html.includes('class="conv-state muted" title="running outside runtrol">Elsewhere</span>'));
  assert.ok(html.includes("--runtrol-running: var(--vscode-charts-blue"));
  assert.ok(html.includes("border-top-color: var(--runtrol-running)"));
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
    loose: [conversation({ key: "x", title: "<script>alert(1)</script>", hue: null })],
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
    serviceChoice: { workspace: "C:\\work\\app", services: [{ providerId: "claude", displayName: "Claude Code", icon: "claude" }] },
  }), assets);
  assert.ok(html.includes('data-command="runtrol.createProject"'));
  assert.ok(html.includes('data-command="runtrol.startSession"'));
  assert.ok(html.includes('data-command="runtrol.startSessionWith" data-kind="service" data-key="claude"'));
});
