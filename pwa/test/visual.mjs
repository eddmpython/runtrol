import { mkdir, writeFile } from "node:fs/promises";
import path from "node:path";

import { PyProcControlClient } from "pyproc/control";

const options = commandOptions(process.argv.slice(2));
const client = await PyProcControlClient.start(options.config);
let sessionRef = null;
try {
  const opened = await client.openTarget(options.url, {
    expectedRisk: "externalEffect",
    waitUntil: "load",
    timeoutMs: 30_000,
  });
  const attached = await client.attachSession(opened.output.targetRef, { timeoutMs: 10_000 });
  sessionRef = attached.output;
  if (options.fixture) {
    await client.act(sessionRef, [{
      kind: "waitFor",
      expectedRisk: "read",
      selector: "#setup",
      state: "visible",
      timeoutMs: 10_000,
    }], { timeoutMs: 15_000 });
    await client.command(sessionRef, "Runtime.evaluate", {
      expression: visualFixture(options.fixture),
      awaitPromise: true,
      returnByValue: true,
    }, { expectedRisk: "externalEffect", timeoutMs: 10_000 });
  }
  const captured = await client.act(sessionRef, [
    {
      kind: "waitFor",
      expectedRisk: "read",
      selector: options.selector,
      state: "visible",
      timeoutMs: 10_000,
    },
    {
      kind: "screenshot",
      expectedRisk: "read",
      format: "png",
      inline: true,
    },
  ], { timeoutMs: 30_000 });
  const screenshot = captured.attachments.find((attachment) => attachment.kind === "screen.capture");
  if (!screenshot || screenshot.mimeType !== "image/png") {
    throw new Error("pyproc returned no verified PNG screenshot attachment");
  }
  await mkdir(path.dirname(options.output), { recursive: true });
  await writeFile(options.output, screenshot.bytes);
  process.stdout.write(`${JSON.stringify({
    output: options.output,
    bytes: screenshot.byteLength,
    sha256: screenshot.sha256,
  })}\n`);
} finally {
  try {
    if (sessionRef) await client.detachSession(sessionRef, { timeoutMs: 10_000 });
  } finally {
    await client.close();
  }
}

function commandOptions(arguments_) {
  const values = new Map();
  for (let index = 0; index < arguments_.length; index += 2) {
    const name = arguments_[index];
    const value = arguments_[index + 1];
    if (!name?.startsWith("--") || !value) throw new Error("visual options must be --name value pairs");
    values.set(name.slice(2), value);
  }
  const config = values.get("config");
  const url = values.get("url");
  const output = values.get("output");
  const selector = values.get("selector");
  const fixture = values.get("fixture") ?? null;
  if (!config || !path.isAbsolute(config)) throw new Error("--config must be an absolute path");
  if (!output || !path.isAbsolute(output)) throw new Error("--output must be an absolute path");
  // Loopback only; the port is free because another project's preview may hold the usual one (measured
  // 2026-08-25: a sibling repository's Vite preview on 4173 would otherwise block this pass for no reason).
  if (!url || new URL(url).hostname !== "127.0.0.1" || new URL(url).protocol !== "http:") {
    throw new Error("--url must be a loopback http origin");
  }
  if (!selector || selector.length > 200) throw new Error("--selector is required and bounded");
  if (fixture !== null && !["mission-flight-list", "mission-flight-detail", "session-terminal", "session-usage"].includes(fixture)) {
    throw new Error("--fixture is not a supported visual state");
  }
  return { config, url, output, selector, fixture };
}

function visualFixture(kind) {
  if (kind === "session-terminal") return terminalFixture();
  if (kind === "session-usage") return usageFixture();
  const showDetail = kind === "mission-flight-detail";
  return `(() => {
    const byId = (id) => document.getElementById(id);
    byId("setup").hidden = true;
    byId("sessions-view").hidden = false;
    byId("session-browser").hidden = true;
    byId("mission-browser").hidden = false;
    byId("session-detail").hidden = true;
    byId("mission-detail").hidden = ${showDetail ? "false" : "true"};
    byId("show-missions").hidden = false;
    byId("show-sessions").setAttribute("aria-pressed", "false");
    byId("show-missions").setAttribute("aria-pressed", "true");
    byId("connection-status").textContent = "PC online";
    byId("connection-status").dataset.state = "online";
    byId("mission-count").textContent = "2";
    byId("mission-signal-count").hidden = false;
    byId("mission-signal-count").textContent = "1";
    byId("mission-list").innerHTML = '<button class="mission-row flight-signal selected" type="button"><span class="state-dot integrating"></span><span><strong>Release candidate</strong><small>C:\\\\work\\\\runtrol</small></span><b>LANDED</b></button><button class="mission-row" type="button"><span class="state-dot running"></span><span><strong>Documentation refresh</strong><small>C:\\\\work\\\\docs</small></span><b>2/4</b></button>';
    byId("selected-mission-state").textContent = "INTEGRATING";
    byId("selected-mission-title").textContent = "Release candidate";
    byId("selected-mission-project").textContent = "C:\\\\work\\\\runtrol";
    byId("mission-flight-signal").hidden = false;
    byId("mission-flight-signal").textContent = "Receipt Landing ready";
    byId("mission-progress").textContent = "4 of 4";
    byId("mission-awaiting").textContent = "0";
    byId("mission-source").textContent = "missions/release.toml";
    byId("mission-policy").textContent = "51".repeat(32);
    byId("mission-tasks").innerHTML = '<article class="mission-task"><h3>verify-package</h3><p>passed  isolatedWorktree  operatorChoice</p><p>instructions/verify-package.md</p><p>3 gates passed, 0 failed</p><p>Receipt rcpt_01</p></article>';
    byId("pause-mission").hidden = true;
    byId("resume-mission").hidden = true;
    byId("cancel-mission").hidden = true;
    return true;
  })()`;
}

/// The conversation as the phone shows it: the service's own screen drawn by the vendored xterm. The bytes
/// are what Codex 0.149.1 drew on a real pseudo terminal (measured 2026-08-25), so the picture is the real
/// layout with real terminal output, without a paired PC.
function terminalFixture() {
  const screen = [
    "\x1b[1;36m╭────────────────────────────────────╮\x1b[0m\r\n",
    "\x1b[1;36m│\x1b[0m \x1b[1m>_ OpenAI Codex (v0.149.1)\x1b[0m           \x1b[1;36m│\x1b[0m\r\n",
    "\x1b[1;36m│\x1b[0m                                    \x1b[1;36m│\x1b[0m\r\n",
    "\x1b[1;36m│\x1b[0m model:     gpt-5.6-sol xhigh   fast \x1b[1;36m│\x1b[0m\r\n",
    "\x1b[1;36m│\x1b[0m directory: ~\\work\\runtrol           \x1b[1;36m│\x1b[0m\r\n",
    "\x1b[1;36m╰────────────────────────────────────╯\x1b[0m\r\n",
    "  Tip: See the Codex keymap documentation for supported actions.\r\n",
    "\r\n\x1b[1m›\x1b[0m Ask Codex to do anything\r\n",
    "\x1b[2m  gpt-5.6-sol xhigh fast  ~\\work\\runtrol\x1b[0m",
  ].join("");
  return `(async () => {
    const byId = (id) => document.getElementById(id);
    byId("setup").hidden = true;
    byId("sessions-view").hidden = false;
    byId("session-browser").hidden = true;
    byId("mission-browser").hidden = true;
    byId("mission-detail").hidden = true;
    byId("session-detail").hidden = false;
    byId("connection-status").textContent = "PC online";
    byId("connection-status").dataset.state = "online";
    byId("selected-provider").textContent = "codex";
    byId("selected-title").textContent = "Review category 02 curriculum";
    byId("selected-workspace").textContent = "C:\\work\\runtrol";
    byId("terminal-note").hidden = true;
    const { Terminal } = await import("./src/vendor/xterm/xterm.mjs");
    const { FitAddon } = await import("./src/vendor/xterm/addon-fit.mjs");
    const terminal = new Terminal({ cursorBlink: false, fontSize: 13, scrollback: 0 });
    const fit = new FitAddon();
    terminal.loadAddon(fit);
    terminal.open(byId("terminal"));
    fit.fit();
    await new Promise((resolve) => terminal.write(${JSON.stringify(screen)}, resolve));
    return true;
  })()`;
}

/// The sessions browser with the usage strip the Core pushes: the same markup `renderUsage` builds, three
/// services, one bar each (icon-and-progress is the whole display).
function usageFixture() {
  return `(async () => {
    // The app's own unpaired render lands a moment after load; posing before it would be undone by it.
    await new Promise((resolve) => setTimeout(resolve, 1500));
    const byId = (id) => document.getElementById(id);
    byId("setup").hidden = true;
    byId("sessions-view").hidden = false;
    byId("session-browser").hidden = false;
    byId("mission-browser").hidden = true;
    byId("session-detail").hidden = true;
    byId("mission-detail").hidden = true;
    byId("connection-status").textContent = "PC online";
    byId("connection-status").dataset.state = "online";
    byId("session-count").textContent = "3";
    const strip = byId("usage-strip");
    strip.hidden = false;
    strip.innerHTML = [
      ["claude", 42, "42%"],
      ["codex", 69, "69% · 128k today"],
      ["grok", 0, "no limit reported"],
    ].map(([name, percent, detail]) => '<div class="usage-row"><span class="usage-name">' + name + '</span><span class="usage-meter"' + (percent ? '' : ' hidden') + '><span style="width:' + percent + '%"></span></span><span class="usage-detail">' + detail + '</span></div>').join("");
    byId("session-list").innerHTML = '<button class="session-row selected" type="button"><span class="state-dot running"></span><span><strong>Review category 02 curriculum</strong><small>codex</small></span></button><button class="session-row" type="button"><span class="state-dot"></span><span><strong>Sidebar work</strong><small>claude</small></span></button>';
    return true;
  })()`;
}
