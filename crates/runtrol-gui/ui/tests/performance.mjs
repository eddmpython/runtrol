import { access, readFile, stat } from "node:fs/promises";
import { createServer } from "node:http";
import { extname, join, normalize, resolve } from "node:path";
import process from "node:process";
import { chromium } from "playwright-core";

const ROOT = resolve(import.meta.dirname, "..");
const DIST = join(ROOT, "dist");
const MIME = new Map([
  [".css", "text/css; charset=utf-8"],
  [".html", "text/html; charset=utf-8"],
  [".js", "text/javascript; charset=utf-8"],
  [".svg", "image/svg+xml"],
]);

function percentile(values, fraction) {
  const sorted = [...values].sort((left, right) => left - right);
  return sorted[Math.round((sorted.length - 1) * fraction)] ?? 0;
}

async function executable() {
  const candidates = [
    process.env.RUNTROL_BROWSER,
    process.platform === "win32" ? "C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe" : null,
    process.platform === "win32" ? "C:/Program Files/Microsoft/Edge/Application/msedge.exe" : null,
    process.platform === "win32" ? "C:/Program Files/Google/Chrome/Application/chrome.exe" : null,
    process.platform === "win32" ? "C:/Program Files (x86)/Google/Chrome/Application/chrome.exe" : null,
    process.platform === "darwin" ? "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" : null,
    process.platform === "darwin" ? "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge" : null,
    "/usr/bin/google-chrome",
    "/usr/bin/google-chrome-stable",
    "/usr/bin/chromium",
    "/usr/bin/chromium-browser",
  ].filter(Boolean);
  for (const candidate of candidates) {
    try {
      await access(candidate);
      return candidate;
    } catch {
      // Keep looking. A missing browser is reported once, after every supported location is checked.
    }
  }
  throw new Error("no Edge or Chrome executable found; set RUNTROL_BROWSER to its absolute path");
}

function mockBridge() {
  const frameEvent = "session-frame";
  const listeners = new Map();
  const sessions = Array.from({ length: 240 }, (_, index) => ({
    session: `gate-${String(index).padStart(3, "0")}`,
    provider: index % 2 === 0 ? "provider-a" : "provider-b",
    native: `native-${index}`,
    workspace: `C:/work/project-${Math.floor(index / 12)}`,
    folder: `project-${Math.floor(index / 12)}`,
    hot: true,
    doing: "idle",
    looksStuck: false,
  }));

  const emit = (event, payload) => {
    for (const handler of listeners.get(event) ?? []) {
      handler({ payload });
    }
  };
  const frame = (session, text, messageId, delta = false) => JSON.stringify({
    body: {
      event: "agentMessageChunk",
      content: { text },
      message_id: messageId,
      delta,
    },
  });
  const replay = (session) => {
    setTimeout(() => {
      emit(frameEvent, {
        session,
        frame: frame(session, `saved tail ${session}`, `tail-${session}`),
      });
    }, 0);
  };

  window.__RUNTROL_PERF__ = {
    emit,
    frame,
    startedWith: null,
    async flood(session, seconds, perSecond) {
      const intervals = [];
      const inputLatencies = [];
      const emitDurations = [];
      const editable = document.querySelector('[contenteditable="true"]');
      if (!(editable instanceof HTMLElement)) {
        throw new Error("the real composer editor is missing");
      }

      const pctl = (values, fraction) => {
        const sorted = [...values].sort((left, right) => left - right);
        return sorted[Math.round((sorted.length - 1) * fraction)] ?? 0;
      };
      emit(frameEvent, { session, frame: frame(session, "", "flood") });
      const started = performance.now();
      let previous = started;
      let produced = 0;
      let nextInput = started + 100;
      await new Promise((done) => {
        const tick = (now) => {
          intervals.push(now - previous);
          previous = now;
          const due = Math.min(
            Math.floor(((now - started) / 1000) * perSecond) - produced,
            240,
          );
          const emitAt = performance.now();
          for (let index = 0; index < due; index += 1) {
            emit(frameEvent, {
              session,
              frame: frame(session, `line ${produced + index} 파일을 읽는 중\n`, "flood", true),
            });
          }
          emitDurations.push(performance.now() - emitAt);
          produced += Math.max(0, due);

          if (now >= nextInput) {
            const inputAt = performance.now();
            editable.textContent = `입력 ${produced}`;
            editable.dispatchEvent(new InputEvent("input", { bubbles: true, inputType: "insertText" }));
            requestAnimationFrame(() => inputLatencies.push(performance.now() - inputAt));
            nextInput += 100;
          }

          const scrollable = [...document.querySelectorAll("div")].find((element) => {
            const style = getComputedStyle(element);
            return style.overflowY === "auto" && element.scrollHeight > element.clientHeight;
          });
          if (scrollable) {
            scrollable.scrollTop = Math.max(0, scrollable.scrollHeight - scrollable.clientHeight - 30);
          }

          if (now - started < seconds * 1000) {
            requestAnimationFrame(tick);
          } else {
            done();
          }
        };
        requestAnimationFrame(tick);
      });

      const measured = intervals.slice(5);
      return {
        produced,
        frameP95Ms: pctl(measured, 0.95),
        frameMaxMs: pctl(measured, 1),
        inputP95Ms: pctl(inputLatencies, 0.95),
        emitP95Ms: pctl(emitDurations.slice(5), 0.95),
        renderedMessages: document.querySelectorAll(".verbatim").length,
        renderedCharacters: [...document.querySelectorAll(".verbatim")]
          .reduce((total, element) => total + (element.textContent?.length ?? 0), 0),
      };
    },
  };

  window.__TAURI__ = {
    core: {
      async invoke(command, args = {}) {
        if (command === "tracing") return false;
        if (command === "trace") return undefined;
        if (command === "sessions") {
          return { outcome: "ok", value: { sessions, warnings: [] } };
        }
        if (command === "watch") {
          replay(args.session);
          return { outcome: "ok", value: null };
        }
        if (command === "providers") {
          return {
            outcome: "ok",
            value: [
              { id: "provider-a", displayName: "Provider A", usable: true, whyNot: null },
              { id: "provider-b", displayName: "Provider B", usable: true, whyNot: null },
            ],
          };
        }
        if (command === "models") {
          return { outcome: "ok", value: { kind: "unknown", why: "fixture has no model catalogue" } };
        }
        if (command === "start") {
          window.__RUNTROL_PERF__.startedWith = args;
          return { outcome: "ok", value: "started-from-gui" };
        }
        if (command === "prompt" || command === "close") {
          return { outcome: "ok", value: null };
        }
        throw new Error(`performance bridge does not implement ${command}`);
      },
    },
    event: {
      async listen(event, handler) {
        const handlers = listeners.get(event) ?? [];
        handlers.push(handler);
        listeners.set(event, handlers);
        return () => listeners.set(event, (listeners.get(event) ?? []).filter((item) => item !== handler));
      },
    },
  };
}

async function serveDist() {
  const root = normalize(DIST);
  const server = createServer(async (request, response) => {
    try {
      const pathname = new URL(request.url ?? "/", "http://127.0.0.1").pathname;
      if (pathname === "/favicon.ico") {
        response.writeHead(204).end();
        return;
      }
      const relative = pathname === "/" ? "index.html" : pathname.replace(/^\/+/, "");
      const path = normalize(join(root, relative));
      if (!path.startsWith(root)) {
        response.writeHead(403).end();
        return;
      }
      const info = await stat(path);
      if (!info.isFile()) {
        response.writeHead(404).end();
        return;
      }
      const body = await readFile(path);
      response.writeHead(200, { "content-type": MIME.get(extname(path)) ?? "application/octet-stream" });
      response.end(body);
    } catch {
      response.writeHead(404).end();
    }
  });
  await new Promise((resolveListen, rejectListen) => {
    server.once("error", rejectListen);
    server.listen(0, "127.0.0.1", resolveListen);
  });
  const address = server.address();
  if (!address || typeof address === "string") {
    throw new Error("the performance server did not expose a TCP address");
  }
  return { server, url: `http://127.0.0.1:${address.port}` };
}

async function waitFor(page, predicate, timeoutMs = 5_000) {
  await page.waitForFunction(predicate, undefined, { timeout: timeoutMs });
}

async function interaction(page, url) {
  // Browser startup and product paint are different costs. Warm the renderer once, then take enough samples for
  // p95 to tolerate one scheduler outlier without turning the gate into a hosted-runner lottery.
  await page.goto(url, { waitUntil: "domcontentloaded" });
  await page.getByTestId("session-gate-002").waitFor();
  const listPaintMs = [];
  for (let run = 0; run < 20; run += 1) {
    await page.goto(url, { waitUntil: "domcontentloaded" });
    await page.getByTestId("session-gate-002").waitFor();
    listPaintMs.push(await page.evaluate(() => performance.now()));
  }

  const sessionOpenMs = await page.evaluate(async () => {
    const target = document.querySelector('[data-testid="session-gate-002"]');
    if (!(target instanceof HTMLElement)) throw new Error("the target session row is missing");
    const started = performance.now();
    target.click();
    while (!document.body.textContent?.includes("saved tail gate-002")) {
      if (performance.now() - started > 5_000) {
        throw new Error(`session open timed out: ${document.body.textContent?.slice(0, 500)}`);
      }
      await new Promise(requestAnimationFrame);
    }
    return performance.now() - started;
  });

  const inputMs = await page.evaluate(async () => {
    const editable = document.querySelector('[contenteditable="true"]');
    if (!(editable instanceof HTMLElement)) throw new Error("the composer editor is missing");
    const samples = [];
    for (let run = 0; run < 12; run += 1) {
      const started = performance.now();
      editable.textContent = `latency ${run}`;
      editable.dispatchEvent(new InputEvent("input", { bubbles: true, inputType: "insertText" }));
      await new Promise(requestAnimationFrame);
      samples.push(performance.now() - started);
    }
    return samples;
  });

  return {
    listPaintP95Ms: percentile(listPaintMs, 0.95),
    sessionOpenMs,
    inputP95Ms: percentile(inputMs, 0.95),
  };
}

async function scroll(page, url) {
  await page.goto(url, { waitUntil: "domcontentloaded" });
  await page.getByTestId("session-gate-000").waitFor();
  await waitFor(page, () => document.body.textContent?.includes("saved tail gate-000"));
  return page.evaluate(() => window.__RUNTROL_PERF__.flood("gate-000", 3, 3_000));
}

async function startWithoutChoosingProvider(page, workspace) {
  await page.getByRole("button", { name: "새 세션" }).click();
  await page.getByLabel("작업 폴더").fill(workspace);
  await page.getByRole("button", { name: "시작", exact: true }).click();
  await waitFor(page, () => window.__RUNTROL_PERF__.startedWith !== null);
  return page.evaluate(() => window.__RUNTROL_PERF__.startedWith);
}

async function convenience(page, url) {
  await page.goto(url, { waitUntil: "domcontentloaded" });
  await page.getByTestId("session-gate-000").waitFor();
  const first = await startWithoutChoosingProvider(page, "C:/work/first");
  const defaultProvider = first.provider === "provider-a";
  const providerRememberedAfterStart = await page.evaluate(
    () => localStorage.getItem("runtrol.lastProvider") === "provider-a",
  );

  await page.evaluate(() => localStorage.setItem("runtrol.lastProvider", "provider-b"));
  await page.goto(url, { waitUntil: "domcontentloaded" });
  await page.getByTestId("session-gate-000").waitFor();
  const remembered = await startWithoutChoosingProvider(page, "C:/work/remembered");
  const rememberedProvider = remembered.provider === "provider-b";

  await page.goto(url, { waitUntil: "domcontentloaded" });
  await page.getByTestId("session-gate-000").waitFor();
  await page.evaluate(() => {
    const now = Date.now();
    window.__RUNTROL_PERF__.emit("session-frame", {
      session: "gate-000",
      frame: JSON.stringify({
        body: {
          event: "usageUpdate",
          used: 1024,
          size: 272000,
          cost: null,
          detail: null,
        },
      }),
    });
    window.__RUNTROL_PERF__.emit("session-frame", {
      session: "gate-000",
      frame: JSON.stringify({
        body: {
          event: "rateLimitUpdate",
          primary: { used_percent: 87, resets_at: now + 720000, window_minutes: 300 },
          secondary: { used_percent: 12, resets_at: null, window_minutes: 10080 },
          reached: true,
          detail: null,
        },
      }),
    });
  });
  await page.getByTestId("usage-status").waitFor();
  await page.getByTestId("rate-limit-status").waitFor();
  const usageText = await page.getByTestId("usage-status").textContent();
  const rateText = await page.getByTestId("rate-limit-status").textContent();
  return {
    defaultProvider,
    providerRememberedAfterStart,
    rememberedProvider,
    usageVisible: usageText?.includes("1,024") && usageText.includes("272,000"),
    rateLimitVisible: rateText?.includes("87%") && rateText.includes("12%"),
    quotaReachedVisible: rateText?.includes("한도 도달"),
  };
}

async function main() {
  const mode = process.argv[2];
  if (!new Set(["interaction", "scroll", "convenience"]).has(mode)) {
    throw new Error("usage: node tests/performance.mjs interaction|scroll|convenience");
  }
  await access(join(DIST, "index.html"));
  const browserPath = await executable();
  const { server, url } = await serveDist();
  let browser;
  try {
    browser = await chromium.launch({
      executablePath: browserPath,
      headless: true,
      args: ["--disable-background-timer-throttling", "--disable-renderer-backgrounding"],
    });
    const page = await browser.newPage({ viewport: { width: 1100, height: 720 } });
    page.on("console", (message) => process.stderr.write(`[browser console] ${message.text()}\n`));
    page.on("pageerror", (error) => process.stderr.write(`[browser error] ${error.message}\n`));
    await page.addInitScript(mockBridge);
    const metrics = mode === "interaction"
      ? await interaction(page, url)
      : mode === "scroll"
        ? await scroll(page, url)
        : await convenience(page, url);
    process.stdout.write(`${JSON.stringify({ mode, browserPath, ...metrics })}\n`);
  } finally {
    await browser?.close();
    await new Promise((resolveClose) => server.close(resolveClose));
  }
}

main().catch((error) => {
  process.stderr.write(`${error instanceof Error ? error.stack : String(error)}\n`);
  process.exitCode = 2;
});
