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

import { createServer } from "node:http";
import { createRequire } from "node:module";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { pathToFileURL } from "node:url";

import { build } from "esbuild";

import { extensionRoot, repositoryRoot } from "./extension-manifest.mjs";

// Resolve the public control export from this repository's exact browser-tool dependency.
const require = createRequire(path.join(repositoryRoot, "pwa", "package.json"));
const { PyProcControlClient } = await import(pathToFileURL(require.resolve("pyproc/control")).href);

const out = process.argv[2] ?? path.join(os.tmpdir(), "runtrol-sidebar-eye.png");
const temporary = await mkdtemp(path.join(os.tmpdir(), "sidebar-eye-"));
console.log(JSON.stringify({ ownedRootPid: process.pid, executable: process.execPath, temporary }));
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
const glyphBundle = path.join(temporary, "conversationGlyph.cjs");
await build({
  entryPoints: [path.join(extensionRoot, "src", "conversationGlyph.ts")],
  outfile: glyphBundle, bundle: true, platform: "node", format: "cjs", target: "node20",
});
const { accentGlyph } = await import(pathToFileURL(glyphBundle).href);

/// The editor colours the page reads. Only what the page actually asks for.
///
/// The values are the editor's own registry defaults for a dark theme, not one theme's palette. That is what a
/// page actually gets: a theme names a few dozen colours and the editor fills in every other registered colour
/// from the default for its kind. Measured 2026-08-28, the theme on the operator's machine (Visual Studio Dark)
/// defines 37 colours and none of the ones this page reads for state, so all of them come from the registry.
/// The harness was declaring one theme's palette instead and photographing colours nobody sees.
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
  --vscode-progressBar-background: #0e70c0;
  --vscode-notificationsWarningIcon-foreground: #cca700;
  --vscode-errorForeground: #f85149;
  --vscode-testing-iconPassed: #73c991;
  --vscode-terminal-ansiBlue: #2472c8;
  --vscode-terminal-ansiGreen: #0dbc79;
  --vscode-terminal-ansiMagenta: #bc3fbc;
  --vscode-terminal-ansiYellow: #e5e510;
  --vscode-terminal-ansiRed: #cd3131;
  --vscode-terminal-ansiCyan: #11a8cd;
  --vscode-gitDecoration-addedResourceForeground: #81b88b;
  --vscode-gitDecoration-deletedResourceForeground: #c74e39;
  --vscode-gitDecoration-untrackedResourceForeground: #73c991;
  --vscode-charts-blue: #59a4f9;
  --vscode-charts-green: #89d185;
  --vscode-charts-purple: #b180d7;
  --vscode-charts-yellow: #cca700;
  --vscode-charts-red: #f14c4c;
  --vscode-button-background: #0078d4;
  --vscode-button-foreground: #ffffff;
  --vscode-button-hoverBackground: #026ec1;
  --vscode-button-border: transparent;
  --vscode-button-secondaryBackground: #313131;
  --vscode-button-secondaryForeground: #cccccc;
  --vscode-menu-background: #1f1f1f;
  --vscode-menu-foreground: #cccccc;
}
/* The panel's width, declared rather than asked for.
   Measured 2026-08-28: this browser will not open a window narrower than about 500 CSS px, so a
   window size of 320 laid the page out at 500 and photographed the leftmost 320 of it. Every picture the
   harness had taken was a crop of a page that was never that narrow, which hid exactly what a narrow panel
   does to a row: the fade at the end of a long name and the percent beside a bar were both off the right
   edge, outside the picture. The width is the subject here, so the page holds it and the window merely has
   to be wider than it. */
/* Width only. The harness used to paint the background too, which is the page's own job, and painting it
   here meant the harness could never show the page failing to paint it. That is exactly what happened. */
html, body { width: ${Number(process.env.RUNTROL_EYE_WIDTH ?? 320)}px; }
`;

function conversation(over = {}) {
  return {
    key: `chat:${over.title ?? "x"}`,
    legacyKey: null,
    title: "A conversation",
    serviceName: "Claude Code",
    icon: "claude",
    accent: "#48a868",
    open: false,
    activity: "saved",
    live: false,
    canOpen: true,
    blocked: null,
    stopping: false,
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
    // What the host puts under a ring that has a number. Left empty, the harness drew captionless chips and
    // hid the very collision the operator found in their own window (2026-08-28).
    caption: percent === null ? "" : `${percent}%`,
    reached: false,
    state: "available",
    canSignOut: true,
    position: "",
    plan: "Max",
    version: providerId === "claude" ? "2.1.251" : "0.63.0",
    updateTo: providerId === "claude" ? "2.1.252" : null,
    age: "2 min ago",
    meters: rings.map((ring, at) => ({
      label: ring.label,
      percent: ring.percent,
      detail: at === 0 ? "resets in 3 days" : "",
      governing: at === 0,
    })),
    action: null,
    canSignIn: true,
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
      kind: "created",
      pinned: false,
      current: true,
      collapsed: false,
      attention: 0,
      live: 1,
      hidden: 3,
      branch: "main",
      changes: { added: 119, removed: 4, untracked: 2, ahead: 0 },
      changesError: null,
      rows: [
        conversation({ title: "돈을 벌 수 있는 구조인지 지금 상태에서 판단해라", activity: "working", live: true, open: true, memory: "306 MB" }),
        conversation({ title: "현재 이 프로젝트 수준은?", open: true, memory: "278 MB" }),
        conversation({ title: "/model" }),
        conversation({ title: "터미널 탭이 열릴 때 서비스가 처음 그릴 때까지 마크가 도는지", pinned: true }),
        conversation({ title: "A live conversation with no terminal route", live: true, canOpen: false, blocked: "Live terminal unavailable." }),
        conversation({ title: "A conversation the Runtime is stopping", activity: "unknown", live: true, canOpen: false, canStop: false, stopping: true, blocked: "Runtrol asked this conversation's process to stop and is waiting for it to exit." }),
        conversation({ title: "Owner awaiting a fresh process roster", canOpen: false, blocked: "Process status unavailable." }),
      ],
    },
    {
      key: "project:runtrol",
      name: "runtrol",
      workspace: "C:\\work\\runtrol",
      kind: "created",
      pinned: false,
      current: false,
      collapsed: false,
      attention: 2,
      live: 0,
      hidden: 0,
      branch: "feature/sidebar",
      changes: { added: 0, removed: 0, untracked: 0, ahead: 3 },
      changesError: null,
      rows: [
        conversation({ title: "Sidebar 대화삭제 및 기능 구현 미완료", accent: "#b07bd8", activity: "needsYou", workspace: "C:\\work\\runtrol" }),
        conversation({ title: "Runtime generation connection failed", accent: "#b07bd8", activity: "attention", workspace: "C:\\work\\runtrol" }),
        conversation({ title: "Runtroll 랜딩 사이트", accent: "#b07bd8", workspace: "C:\\work\\runtrol" }),
      ],
    },
  ],
  loose: [conversation({ title: "폴더 없이 시작한 대화", accent: "#4e94ce", workspace: "" })],
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

const firstRun = process.env.RUNTROL_EYE_FIRST_RUN === "1";
if (firstRun) {
  model.projects = [];
  model.loose = [];
  model.usage = [];
  model.firstRun = true;
}
const sixProjects = process.env.RUNTROL_EYE_SIX_PROJECTS === "1";
if (sixProjects) {
  const projectBundle = path.join(temporary, "projects.cjs");
  await build({
    stdin: { contents: 'export { ProjectStore } from "./src/projects"; export { projectAccentColor } from "./src/projectColor";', resolveDir: extensionRoot },
    outfile: projectBundle, bundle: true, platform: "node", format: "cjs", target: "node20",
  });
  const { ProjectStore, projectAccentColor } = await import(pathToFileURL(projectBundle).href);
  const values = new Map();
  const store = new ProjectStore({ get: (key) => values.get(key), update: async (key, value) => { values.set(key, value); } });
  const candidates = Array.from({ length: 400 }, (_, index) => `C:/work/project-${index}`);
  const preferred = projectAccentColor(candidates[0]);
  const folders = candidates.filter((folder) => projectAccentColor(folder) === preferred).slice(0, 6);
  const names = ["Aurora", "Boreal", "Cinder", "Delta", "Ember", "Fjord"];
  for (const [index, folder] of folders.entries()) await store.create(folder, names[index]);
  model.projects = store.all().map((record, index) => ({
    ...model.projects[0], key: `project:${record.key}`, name: record.name, workspace: record.workspace,
    current: index === 0, attention: 0, live: 0, hidden: 0, changes: null,
    rows: [
      conversation({ key: `${record.key}:claude`, title: "Review the workspace", accent: record.accent, workspace: record.workspace, open: true }),
      conversation({ key: `${record.key}:codex`, title: "Verify the changes", serviceName: "Codex", icon: "codex", accent: record.accent, workspace: record.workspace }),
    ],
  }));
  model.loose = [];
  console.log(JSON.stringify({ projectAccents: store.all().map(({ name, accent }) => ({ name, accent })) }));
}

if (process.env.RUNTROL_EYE_GIT_FAILURE === "1") {
  model.projects[1].changes = null;
  model.projects[1].changesError = "Git read timed out";
}
if (process.env.RUNTROL_EYE_EMPTY_PICKER === "1") {
  model.serviceChoice = {
    workspace: model.projects[0].workspace,
    services: [],
    unavailable: "Checking installed services...",
  };
}
const unreadUsage = process.env.RUNTROL_EYE_UNREAD_USAGE === "1";
const projectActions = process.env.RUNTROL_EYE_PROJECT_ACTIONS === "1";
if (projectActions) {
  Object.assign(model.projects[0], { name: "alphaWork", attention: 2, live: 1,
    changes: { added: 1600, removed: 1300, untracked: 0, ahead: 0 } });
}
if (process.env.RUNTROL_EYE_PROJECT_CHANGES === "1") {
  const names = ["Alpha", "Beta", "Gamma", "Delta", "EnglishProjectWithALongName", "긴프로젝트이름에서대화제목과작업공간을구분하는프로젝트"];
  model.projects = names.map((name, index) => ({
    ...model.projects[0], key: `project:changes-${index}`, name, current: false,
    attention: index === 0 ? 2 : 0, live: 1, hidden: 0,
    branch: index === 0 ? "feature/sidebar-layout" : "main",
    changes: { added: 1600, removed: 1300, untracked: 0, ahead: 0 },
    rows: [conversation({ title: name, open: true })],
  }));
  model.loose = [];
}
const ownerInput = process.env.RUNTROL_EYE_OWNER_INPUT === "1";
if (ownerInput) {
  model.projects[0].rows = [
    conversation({ title: "Input available in this window", live: true, canStop: false, canOpenInput: true }),
    conversation({ title: "Read-only terminal in its owner window", live: true, canStop: false, canOpenInput: false }),
  ];
  model.projects[0].hidden = 0;
}
if (unreadUsage) {
  const usageBundle = path.join(temporary, "usageFixture.cjs");
  await build({
    stdin: {
      contents: 'export { usageRows } from "./src/usageDisplay"; export { usageChips } from "./src/usageStrip";',
      resolveDir: extensionRoot,
    },
    outfile: usageBundle, bundle: true, platform: "node", format: "cjs", target: "node20",
  });
  const { usageRows, usageChips } = await import(pathToFileURL(usageBundle).href);
  const now = Date.now();
  const providers = [
    { providerId: "claude", displayName: "Claude Code", icon: "claude" },
    { providerId: "codex", displayName: "Codex", icon: "codex" },
  ].map((provider) => ({
    ...provider,
    installation: { state: "usable" },
    account: { status: "unread", why: "Account request timed out.", checkedAtMs: now },
  }));
  model.usage = usageChips(usageRows([{
    providerId: "claude", reached: false, atMs: now - 120_000,
    windows: [{ id: "seven_day", usedPercent: 48, windowMinutes: 10_080 }],
  }], providers, now));
}

// Photograph the shipped provider glyphs through the same colour projection as the installed Studio.
const providerIcons = new Map([
  ["claude", await readFile(path.join(extensionRoot, "resources/provider-icons/claude.svg"), "utf8")],
  ["codex", await readFile(path.join(extensionRoot, "resources/provider-icons/openai.svg"), "utf8")],
]);
const rows = [...model.projects.flatMap((project) => project.rows), ...model.loose];
const assets = {
  nonce: "eyen0nce",
  cspSource: "vscode-resource:",
  iconUris: new Map([...providerIcons].map(([key, svg]) => [key, `data:image/svg+xml;utf8,${encodeURIComponent(svg)}`])),
  accentIconUris: new Map(rows.map((row) => [
    `${row.icon}\0${row.accent}`,
    `data:image/svg+xml;utf8,${encodeURIComponent(accentGlyph(providerIcons.get(row.icon), row.accent))}`,
  ])),
};
let html = sidebarHtml(model, assets);
// The page is written for a webview, where the editor injects its own variables. Declare them here instead,
// under the page's own nonce: its policy allows styles from that block and nothing else, and a plain <style>
// is dropped without a word, which is how the colour band came to be missing from the first picture this
// harness took.
html = html.replace("</head>", `<style nonce="${assets.nonce}">${THEME}</style></head>`);
if (process.env.RUNTROL_EYE_GRAYSCALE === "1") {
  html = html.replace("</head>", `<style nonce="${assets.nonce}">html { filter: grayscale(1); }</style></head>`);
}
// The page's own script asks the editor for its message channel before it does anything else. Outside a
// webview that call throws, the script stops on its first line, and every zone it was going to reveal stays
// hidden: the picture comes out empty and looks like a page that draws nothing. The stub answers the three
// things the script uses and nothing else, and it goes in under the page's nonce like the theme does.
html = html.replace("</head>", `<script nonce="${assets.nonce}">
  window.acquireVsCodeApi = function () {
    var state = {};
    return {
      postMessage: function () {},
      getState: function () { return state; },
      setState: function (next) { state = next; return next; },
    };
  };
</script></head>`);
// One panel open, because a hover panel that nothing hovers is a state this harness could never show.
//
// Opened the way a person opens it. Stripping the `hidden` attribute used to be enough, but the page now
// restores the panel a person had open after every repaint, and that restore closed the one this harness had
// forced open before the picture was taken (2026-08-28). Pressing the chip goes through the same path the
// person's press does, which is also the only way the harness can be sure that path still works.
html = html.replace("</body>", `<script nonce="${assets.nonce}">
  window.addEventListener("load", function () {
    var chip = document.querySelectorAll(".chip")[0];
    if (chip && ${!sixProjects && !ownerInput && !projectActions && process.env.RUNTROL_EYE_PROJECT_CHANGES !== "1"}) chip.${unreadUsage ? "focus" : "click"}();
    if (${projectActions}) {
      document.querySelector('.project-row')?.focus();
      document.querySelector('.project-row .act')?.focus();
    }
    if (${ownerInput}) {
      var input = document.querySelector('[data-command="runtrol.openInputView"]');
      input?.closest('.row')?.focus();
      input?.focus();
    }
  });
</script></body>`);
// A moving state, held at one instant. The shutter opens right after load, when every animation is at its
// first frame: a light that starts outside its band is not in the picture at all, and the picture then says
// "nothing moves" about a page where something does. RUNTROL_EYE_FREEZE_MS puts every animation that many
// milliseconds into its run and holds it there, so the frame photographed is a chosen one.
const freezeMs = Number(process.env.RUNTROL_EYE_FREEZE_MS ?? "");
if (Number.isFinite(freezeMs) && freezeMs > 0) {
  html = html.replace("</head>", `<style nonce="${assets.nonce}">
*, *::before, *::after { animation-delay: -${freezeMs}ms !important; animation-play-state: paused !important; }
</style></head>`);
}
const page = path.join(temporary, "sidebar.html");
await writeFile(page, html, "utf8");

const server = createServer((_request, response) => {
  response.writeHead(200, { "content-type": "text/html; charset=utf-8" });
  response.end(html);
});
let client = null;
let session = null;
try {
  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", resolve);
  });
  const origin = `http://127.0.0.1:${server.address().port}`;
  const config = path.join(temporary, "control.json");
  await writeFile(config, JSON.stringify({
    schemaVersion: 1,
    engine: { indexURL: "https://cdn.jsdelivr.net/pyodide/v314.0.2/full/" },
    timeoutMs: 180000,
    browser: {
      enabled: true, provider: "nativeCdp", allowedOrigins: [origin], maxRisk: "externalEffect",
      actions: ["navigate", "waitFor", "screenshot"], methods: [],
      viewport: { width: Number(process.env.RUNTROL_EYE_WIDTH ?? 320), height: 900, deviceScaleFactor: 2, mobile: false, touch: false },
      externalEffects: "acknowledged", purpose: "Inspect the current Studio markup and stylesheet with synthetic state",
    },
  }), "utf8");
  client = await PyProcControlClient.start(config, {
    env: { ...process.env, TEMP: temporary, TMP: temporary },
  });
  const opened = await client.openTarget(origin, { expectedRisk: "externalEffect", waitUntil: "load", timeoutMs: 30000 });
  session = (await client.attachSession(opened.output.targetRef, { timeoutMs: 10000 })).output;
  const captured = await client.act(session, [
    { kind: "waitFor", expectedRisk: "read", selector: firstRun ? '[data-command="runtrol.createProject"]'
      : `.project-row[data-key="${model.projects[0].key.replaceAll("\\", "\\\\").replaceAll('"', '\\"')}"]`, state: "visible", timeoutMs: 10000 },
    { kind: "screenshot", expectedRisk: "read", format: "png", fullPage: true, inline: true },
  ], { timeoutMs: 30000 });
  const screenshot = captured.attachments.find((attachment) => attachment.kind === "screen.capture");
  if (!screenshot || screenshot.mimeType !== "image/png") throw new Error("No verified sidebar PNG was returned");
  await writeFile(out, screenshot.bytes);
  console.log(JSON.stringify({ output: out, bytes: screenshot.byteLength, sha256: screenshot.sha256 }));
} finally {
  try {
    if (session) await client.detachSession(session, { timeoutMs: 10000 });
  } finally {
    try {
      if (client) await client.close();
    } finally {
      server.closeAllConnections();
      await new Promise((resolve, reject) => server.close((error) => error ? reject(error) : resolve()));
      await rm(temporary, { recursive: true, maxRetries: 10, retryDelay: 100 });
    }
  }
}
