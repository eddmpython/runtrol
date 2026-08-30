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
html, body { width: 320px; }
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
    // What the host puts under a ring that has a number. Left empty, the harness drew captionless chips and
    // hid the very collision the operator found in their own window (2026-08-28).
    caption: percent === null ? "" : `${percent}%`,
    reached: false,
    state: "available",
    canSignOut: providerId !== "grok",
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
      changes: { added: 119, removed: 4, untracked: 2, ahead: 0 },
      rows: [
        conversation({ title: "돈을 벌 수 있는 구조인지 지금 상태에서 판단해라", activity: "working", live: true, memory: "306 MB" }),
        conversation({ title: "현재 이 프로젝트 수준은?", memory: "278 MB" }),
        conversation({ title: "/model" }),
        conversation({ title: "터미널 탭이 열릴 때 서비스가 처음 그릴 때까지 마크가 도는지", pinned: true }),
        conversation({ title: "/logout", canOpen: false, blocked: "This coding service cannot reopen this conversation." }),
      ],
    },
    {
      key: "project:runtrol",
      name: "runtrol",
      workspace: "C:\\work\\runtrol",
      hue: "hueTeal",
      kind: "created",
      pinned: false,
      current: false,
      collapsed: false,
      attention: 1,
      live: 0,
      agentTools: true,
      hidden: 0,
      branch: "feature/sidebar",
      changes: { added: 0, removed: 0, untracked: 0, ahead: 3 },
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
    // A service that answered and has no number of its own: measured, one account is metered by a team and
    // that CLI publishes no percentage for it at all. This is the state the caption used to call "No report",
    // and it is only reachable in a picture from here.
    {
      ...chip("grok", "Grok", null, []),
      caption: "",
      position: "team-managed",
      plan: "SuperGrok",
      meters: [],
    },
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
    if (chip) chip.click();
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

// A Windows path in a JavaScript string: each separator is two characters or the escape eats it and the
// name becomes `C:Program Files...`, which no shell can find.
const chrome = "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe";
const profile = path.join(temporary, "chrome");

// Photographed by the browser itself rather than by capturing its window.
//
// Measured 2026-08-28: photographing an `--app` window came back, some runs, with a title bar and a client
// area of one flat colour. The page was right every time (its markup was in the file, and the same file drew
// correctly when the browser took the picture), so the blank ones said "this page draws nothing" about a page
// that drew everything. A picture that can be empty while its subject is correct is worse than no picture: it
// is a finding nobody made, about a screen nobody then looked at again.
const shot = spawn(chrome, [
  `--user-data-dir=${profile}`,
  "--no-first-run",
  "--no-default-browser-check",
  // Still the same engine and the same stylesheet; only the shutter moved.
  "--headless=new",
  "--disable-gpu",
  `--screenshot=${out}`,
  "--window-size=560,900",
  // Two device pixels per CSS pixel, so 11px text in the picture is legible enough to judge.
  "--force-device-scale-factor=2",
  `file:///${page.replaceAll("\\", "/")}`,
], { stdio: "ignore" });
const captured = await new Promise((resolve) => shot.on("close", resolve));
if (captured !== 0) {
  throw new Error(`the sidebar page could not be photographed (chrome exited ${captured})`);
}
console.log(`sidebar eye -> ${out}`);
