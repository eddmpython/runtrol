import assert from "node:assert/strict";
import { test } from "node:test";

import {
  formatMemory,
  rowKeys,
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
    menu: [{ command: "runtrol.refresh", label: "Look again" }],
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
    projects: [project({ rows: [conversation({ live: true, activity: "needsYou", canDelete: false, canArchive: true })] })],
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

test("memory reads as a short figure and rides the row", () => {
  assert.equal(formatMemory(512 * 1024), "<1 MB");
  assert.equal(formatMemory(412 * 1024 * 1024), "412 MB");
  assert.equal(formatMemory(1.6 * 1024 * 1024 * 1024), "1.6 GB");
  const html = sidebarHtml(model({ projects: [project({ rows: [conversation({ memory: "412 MB" })] })], loose: [] }), assets);
  assert.ok(html.includes('<span class="memory" title="Memory the provider process holds now">412 MB</span>'));
});

test("row keys are unique and in page order, which is what the eye test reads", () => {
  const keys = rowKeys(model({}));
  assert.deepEqual(keys, ["project:app", "claude:one", "codex:loose"]);
});

test("the vertical-dots menu lists the rare actions and the page escapes what services said", () => {
  const html = sidebarHtml(model({
    menu: [{ command: "runtrol.pairPhone", label: "Pair a phone" }],
    loose: [conversation({ key: "x", title: "<script>alert(1)</script>", hue: null })],
  }), assets);
  assert.ok(html.includes('class="ci ci-kebab-vertical"'));
  assert.ok(html.includes('data-command="runtrol.pairPhone"'));
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
