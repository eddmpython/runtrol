// Photograph the sidebar page itself, with the real markup and the real stylesheet, without a Runtime, a
// coding CLI or the operator's account.
//
// # Why this exists
//
// Every sidebar change used to be judged by installing a VSIX, opening a window, waiting for discovery and
// hoping the machine happened to hold the case being changed. States that need a running turn (a spinning
// icon) or a crowded project (six conversations under one heading) could not be reached that way at all,
// so they went unseen: the colour band was invisible for days and the two-line title survived a release
// (2026-08-28). The page is a pure function of its model, so a model can be written by hand and the result
// looked at directly.
//
// # Why a browser window rather than the editor
//
// The page runs in a webview, which is Chromium. Opening the same HTML in Chrome as a fixed-size app window
// renders the same engine against the same CSS. What it cannot bring is the editor's theme variables, so the
// harness declares them here from the Dark Modern values; anything the page reads and this file does not
// define shows up as an unstyled element, which is itself the finding.
//
// Usage: node tooling/sidebar-eye.mjs [outputPng]

import { spawn } from "node:child_process";
import { mkdtemp, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";

import { build } from "esbuild";

import { extensionRoot } from "./extension-manifest.mjs";

const out = process.argv[2] ?? path.join(os.tmpdir(), "runtrol-sidebar-eye.png");
const temporary = await mkdtemp(path.join(os.tmpdir(), "sidebar-eye-"));
const bundle = path.join(temporary, "sidebarPage.cjs");

await build({
  entryPoints: [path.join(extensionRoot, "src", "sidebarPage.ts")],
  outfile: bundle,
  bundle: true,
  platform: "node",
  format: "cjs",
  target: "node20",
});

const { sidebarHtml } = await import(`file://${bundle.replaceAll("\\", "/")}`);

/// The editor colours the page reads, at their Dark Modern values. Only what the page actually asks for.
const THEME = `
:root {
  --vscode-font-family: "Segoe UI", system-ui, sans-serif;
  --vscode-font-size: 13px;
  --vscode-foreground: #cccccc;
  --vscode-sideBar-foreground: #cccccc;
  --vscode-sideBar-background: #181818;
  --vscode-descriptionForeground: #9d9d9d;
  --vscode-widget-border: #313131;
  --vscode-sideBarSectionHeader-border: #2b2b2b;
  --vscode-list-hoverBackground: #2a2d2e;
  --vscode-toolbar-hoverBackground: #383b3d;
  --vscode-focusBorder: #0078d4;
  --vscode-editorWidget-background: #202020;
  --vscode-progressBar-background: #0078d4;
  --vscode-notificationsWarningIcon-foreground: #cca700;
  --vscode-errorForeground: #f85149;
  --vscode-testing-iconPassed: #3fb950;
  --vscode-charts-blue: #4e94ce;
  --vscode-charts-green: #89d185;
  --vscode-charts-purple: #b180d7;
  --vscode-charts-yellow: #cca700;
  --vscode-charts-red: #f14c4c;
  --vscode-button-secondaryBackground: #313131;
  --vscode-button-secondaryForeground: #cccccc;
  --vscode-menu-background: #1f1f1f;
  --vscode-menu-foreground: #cccccc;
}
html, body { background: var(--vscode-sideBar-background); }
`;

function conversation(over = {}) {
  return {
    key: `chat:${over.title ?? "x"}`,
    legacyKey: null,
    title: "A conversation",
    serviceName: "Claude Code",
    icon: "claude",
    hue: "hueGreen",
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
    workspace: "C:\\work\\cleangov",
    ...over,
  };
}

function chip(providerId, name, percent, rings) {
  return {
    providerId,
    name,
    icon: providerId,
    percent,
    rings,
    caption: "",
    reached: false,
    state: "within",
    position: "Within limits",
    plan: "Max",
    age: "2 min ago",
    meters: rings.map((ring, at) => ({
      label: ring.label,
      percent: ring.percent,
      detail: at === 0 ? "resets in 3 days" : "",
      governing: at === 0,
    })),
    action: null,
  };
}

/// The states worth looking at, in one picture: a crowded project, a running turn, a name past the width,
/// a second project in a second colour, and the usage strip that must stay at the bottom.
const model = {
  notices: [],
  projects: [
    {
      key: "project:cleangov",
      name: "cleangov",
      workspace: "C:\\work\\cleangov",
      hue: "hueGreen",
      kind: "created",
      pinned: false,
      current: true,
      collapsed: false,
      attention: 0,
      live: 1,
      agentTools: false,
      hidden: 3,
      branch: "main",
      rows: [
        conversation({ title: "돈을 벌 수 있는 구조인지 지금 상태에서 판단해라", activity: "working", live: true, memory: "306 MB" }),
        conversation({ title: "현재 이 프로젝트 수준은?", memory: "278 MB" }),
        conversation({ title: "/model" }),
        conversation({ title: "Runtrol mainPlan/localAgentRuntime 구현과 그 뒤 정리", pinned: true }),
        conversation({ title: "/logout", canOpen: false, blocked: "This coding service cannot reopen this conversation." }),
      ],
    },
    {
      key: "project:runtrol",
      name: "runtrol",
      workspace: "C:\\work\\runtrol",
      hue: "huePurple",
      kind: "created",
      pinned: false,
      current: false,
      collapsed: false,
      attention: 1,
      live: 0,
      agentTools: true,
      hidden: 0,
      branch: "feature/sidebar",
      rows: [
        conversation({ title: "Sidebar 대화삭제 및 기능 구현 미완료", hue: "huePurple", activity: "needsYou", workspace: "C:\\work\\runtrol" }),
        conversation({ title: "Runtroll 랜딩 사이트", hue: "huePurple", workspace: "C:\\work\\runtrol" }),
      ],
    },
  ],
  loose: [conversation({ title: "폴더 없이 시작한 대화", hue: null, workspace: "" })],
  usage: [
    chip("claude", "Claude Code", 32, [
      { label: "7 days", percent: 32 },
      { label: "5 hours", percent: 61 },
      { label: "claude-opus-5 weekly", percent: 74 },
    ]),
    chip("codex", "Codex", 13, [{ label: "7 days", percent: 13 }]),
  ],
  serviceChoice: null,
  firstRun: false,
};

const assets = {
  nonce: "eyen0nce",
  cspSource: "vscode-resource:",
  iconUris: new Map([
    ["claude", "data:image/svg+xml;utf8," + encodeURIComponent("<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 16 16'><path fill='%23d97757' d='M8 1l1.6 4.2L14 6.4l-3.4 2.9L11.6 14 8 11.6 4.4 14l1-4.7L2 6.4l4.4-1.2z'/></svg>")],
    ["codex", "data:image/svg+xml;utf8," + encodeURIComponent("<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 16 16'><circle cx='8' cy='8' r='6' fill='none' stroke='%23cccccc' stroke-width='1.5'/></svg>")],
  ]),
};

let html = sidebarHtml(model, assets);
// The page is written for a webview, where the editor injects its own variables. Declare them here instead,
// under the page's own nonce: its policy allows styles from that block and nothing else, and a plain <style>
// is dropped without a word, which is how the colour band came to be missing from the first picture this
// harness took.
html = html.replace("</head>", `<style nonce="${assets.nonce}">${THEME}</style></head>`);
// One panel open, because a hover panel that nothing hovers is a state this harness could never show.
html = html.replace('id="panel-0" hidden', 'id="panel-0"');
const page = path.join(temporary, "sidebar.html");
await writeFile(page, html, "utf8");

const chrome = "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe";
const profile = path.join(temporary, "chrome");
const browser = spawn(chrome, [
  `--user-data-dir=${profile}`,
  "--no-first-run",
  "--no-default-browser-check",
  "--window-size=320,900",
  "--window-position=40,40",
  `--app=file:///${page.replaceAll("\\", "/")}`,
], { detached: false, stdio: "ignore" });

// The window has to exist and paint before it can be photographed. A short wait beats a poll here: the page
// loads from disk with no network and no runtime, so it is ready as soon as Chrome has drawn once.
await new Promise((resolve) => setTimeout(resolve, 3500));

const capture = spawn("powershell.exe", [
  "-NoProfile", "-ExecutionPolicy", "Bypass",
  "-File", path.join(extensionRoot, "tooling", "capture-window.ps1"),
  "-TitleMatch", "sidebar.html",
  "-OutPath", out,
], { stdio: "inherit" });
const captured = await new Promise((resolve) => capture.on("close", resolve));

browser.kill();
if (captured !== 0) {
  throw new Error(`the sidebar page could not be photographed (capture exited ${captured})`);
}
console.log(`sidebar eye -> ${out}`);
