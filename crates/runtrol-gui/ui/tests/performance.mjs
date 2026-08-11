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

function mockBridge(measurementMode) {
  const frameEvent = "session-frame";
  const listeners = new Map();
  let nextView = 0;
  let activeWatch = null;
  let pendingReplay = null;
  let holdNextWatch = false;
  let holdNextResume = false;
  let pendingResume = null;
  let holdNextPrompt = false;
  let pendingPrompt = null;
  const pendingWatches = new Map();
  const stoppedIntents = [];
  const sessions = Array.from({ length: 240 }, (_, index) => ({
    session: `gate-${String(index).padStart(3, "0")}`,
    provider: index % 2 === 0 ? "provider-a" : "provider-b",
    native: `native-${index}`,
    workspace: `C:/work/project-${Math.floor(index / 12)}`,
    folder: `folder-${Math.floor(index / 12)}`,
    // What the daemon computes for the rail's subtitle. Short enough here that it is the path itself,
    // which is the same answer the real one gives for a path this length.
    trail: `C:/work/project-${Math.floor(index / 12)}`,
    hot: index !== 1,
    doing: "idle",
    looksStuck: false,
  }));

  const emit = (event, rawPayload) => {
    let payload = rawPayload;
    if (event === frameEvent && payload.view === undefined) {
      if (!activeWatch || payload.session !== activeWatch.session) return;
      activeWatch.cursor = { ...activeWatch.cursor, seq: activeWatch.cursor.seq + 1 };
      payload = {
        ...payload,
        view: activeWatch.view,
        nextExpected: activeWatch.cursor,
      };
    }
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
    resumeRequests: [],
    promptRequests: [],
    traceLines: [],
    closeRequests: [],
    watchRequests: [],
    currentWatch: () => activeWatch ? structuredClone(activeWatch) : null,
    holdNextWatch() {
      holdNextWatch = true;
    },
    holdNextResume() {
      holdNextResume = true;
    },
    releaseResume() {
      pendingResume?.();
    },
    holdNextPrompt() {
      holdNextPrompt = true;
    },
    releasePrompt() {
      pendingPrompt?.();
    },
    pendingIntent: () => pendingWatches.keys().next().value ?? null,
    stoppedIntents: () => [...stoppedIntents],
    emitOver(lagged = false) {
      if (!activeWatch) return;
      emit("session-over", {
        session: activeWatch.session,
        view: activeWatch.view,
        nextExpected: activeWatch.cursor,
        lagged,
      });
    },
    async flood(session, seconds, perSecond) {
      const intervals = [];
      const inputLatencies = [];
      const emitDurations = [];
      const editable = document.querySelector('[contenteditable="true"]');
      if (!(editable instanceof HTMLElement)) {
        throw new Error("the real composer editor is missing");
      }
      const scrollable = [...document.querySelectorAll("div")].find((element) => {
        const style = getComputedStyle(element);
        return style.overflowY === "auto" && element.scrollHeight > element.clientHeight;
      });

      const pctl = (values, fraction) => {
        const sorted = [...values].sort((left, right) => left - right);
        return sorted[Math.round((sorted.length - 1) * fraction)] ?? 0;
      };
      emit(frameEvent, { session, frame: frame(session, "", "flood") });
      const started = performance.now();
      let previous = started;
      let produced = 0;
      let nextInput = started + 100;
      let nextScroll = started + 100;
      let scrollToEnd = false;
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

          if (scrollable && now >= nextScroll) {
            scrollable.scrollTop = scrollToEnd ? Number.MAX_SAFE_INTEGER : 0;
            scrollToEnd = !scrollToEnd;
            nextScroll += 100;
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
        inputSamples: inputLatencies.length,
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
        // The production default is tracing off. The load gate must measure that path because tracing every
        // individual frame would turn the test recorder itself into a 3,000-line-per-second workload.
        if (command === "tracing") return measurementMode !== "scroll";
        if (command === "trace") {
          window.__RUNTROL_PERF__.traceLines.push(args.line);
          return undefined;
        }
        if (command === "sessions") {
          return { outcome: "ok", value: { sessions, warnings: [] } };
        }
        if (command === "watch") {
          window.__RUNTROL_PERF__.watchRequests.push({
            session: args.session,
            intent: args.intent,
            after: args.after ?? null,
          });
          if (holdNextWatch) {
            holdNextWatch = false;
            return new Promise((resolve) => {
              pendingWatches.set(args.intent, { resolve });
            });
          }
          nextView += 1;
          const startsAt = args.after ?? { stream: `mock-stream-${nextView}`, epoch: 0, seq: 0 };
          activeWatch = {
            session: args.session,
            intent: args.intent,
            view: nextView,
            cursor: startsAt,
          };
          pendingReplay = args.session;
          return {
            outcome: "ok",
            value: { view: nextView, startsAt, liveAt: startsAt, gap: null },
          };
        }
        if (command === "continue_watch") {
          if (activeWatch?.view !== args.view) {
            return { outcome: "broken", message: "stale view" };
          }
          const session = pendingReplay;
          pendingReplay = null;
          if (session) replay(session);
          return { outcome: "ok", value: null };
        }
        if (command === "stop_watch" || command === "unwatch") {
          const pending = pendingWatches.get(args.intent);
          if (pending) {
            pendingWatches.delete(args.intent);
            stoppedIntents.push(args.intent);
            pending.resolve({ outcome: "ok", value: null });
          }
          if (command === "unwatch" || activeWatch?.intent === args.intent) {
            if (activeWatch) stoppedIntents.push(activeWatch.intent);
            activeWatch = null;
            pendingReplay = null;
          }
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
          sessions.push({
            session: "started-from-gui",
            provider: args.provider,
            native: null,
            workspace: args.workspace,
            folder: args.workspace.split(/[\\/]/).filter(Boolean).at(-1) ?? args.workspace,
            hot: true,
            doing: "idle",
            looksStuck: false,
          });
          return { outcome: "ok", value: "started-from-gui" };
        }
        if (command === "resume") {
          window.__RUNTROL_PERF__.resumeRequests.push(args);
          const finish = () => {
            const index = sessions.findIndex((row) => row.native === args.native);
            if (index < 0) return { outcome: "broken", message: "fixture native session is missing" };
            sessions.splice(index, 1, {
              ...sessions[index],
              session: "resumed-from-gui",
              hot: true,
              doing: "idle",
            });
            return { outcome: "ok", value: "resumed-from-gui" };
          };
          if (holdNextResume) {
            holdNextResume = false;
            return new Promise((resolve) => {
              pendingResume = () => {
                pendingResume = null;
                resolve(finish());
              };
            });
          }
          return finish();
        }
        if (command === "prompt") {
          window.__RUNTROL_PERF__.promptRequests.push(args);
          if (holdNextPrompt) {
            holdNextPrompt = false;
            return new Promise((resolve) => {
              pendingPrompt = () => {
                pendingPrompt = null;
                resolve({ outcome: "ok", value: null });
              };
            });
          }
          return { outcome: "ok", value: null };
        }
        if (command === "close") {
          window.__RUNTROL_PERF__.closeRequests.push(args);
          const index = sessions.findIndex((row) => row.session === args.session);
          if (index >= 0) sessions.splice(index, 1);
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
  // Ten seconds yields about 100 independent input samples. The previous three-second run made p95 the second
  // largest of only about 30 samples, so two unrelated hosted-runner scheduler stalls could fail an otherwise
  // healthy 60 Hz journey. The latency limits stay fixed; the larger sample makes the percentile mean a rate.
  return page.evaluate(() => window.__RUNTROL_PERF__.flood("gate-000", 10, 3_000));
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

async function lifecycle(page, url) {
  await page.goto(url, { waitUntil: "domcontentloaded" });
  await page.getByTestId("session-gate-000").waitFor();
  const unifiedProviders = await page.evaluate(() => {
    const text = document.body.textContent ?? "";
    return text.includes("provider-a") && text.includes("provider-b");
  });
  const rowLabel = (testId) => page.getByTestId(testId).evaluate((row) =>
    Array.from(row.children)
      .filter((child) => child.tagName === "SPAN")
      .map((child) => child.textContent?.trim() ?? "")
      .find(Boolean) ?? "",
  );
  const nativeTitleVisible = await rowLabel("session-gate-000") === "native-0";
  const search = page.getByPlaceholder("세션 검색");
  await search.fill("native-119");
  await waitFor(page, () => document.querySelectorAll('[data-testid^="session-"]').length === 1);
  const nativeSearch = await page.getByTestId("session-gate-119").count() === 1;
  await search.fill("gate-023");
  await waitFor(page, () => document.querySelectorAll('[data-testid^="session-"]').length === 1);
  const sessionSearch = await page.getByTestId("session-gate-023").count() === 1;
  await search.fill("project-3");
  await waitFor(page, () => document.querySelectorAll('[data-testid^="session-"]').length === 12);
  const workspaceSearch = await page.getByTestId("session-gate-036").count() === 1;
  await search.fill("folder-7");
  await waitFor(page, () => document.querySelectorAll('[data-testid^="session-"]').length === 12);
  const folderSearch = await page.getByTestId("session-gate-084").count() === 1;
  await search.fill("provider-a");
  await waitFor(page, () => document.querySelectorAll('[data-testid^="session-"]').length === 120);
  const providerSearch = await page.getByTestId("session-gate-000").count() === 1
    && await page.getByTestId("session-gate-001").count() === 0;
  await search.fill("");
  await page.getByTestId("session-gate-000").waitFor();
  await waitFor(page, () => window.__RUNTROL_PERF__.currentWatch()?.session === "gate-000");
  await page.evaluate(() => window.__RUNTROL_PERF__.emit("session-frame", {
    session: "gate-000",
    frame: window.__RUNTROL_PERF__.frame("gate-000", "conversation-only-sentinel", "search-boundary"),
  }));
  await page.getByText("conversation-only-sentinel", { exact: true }).waitFor();

  // A frame runtrol relays without reading is not conversation. Twelve of them once filled the pane with
  // the word `unmapped` in front of an operator who had not sent a turn, so what is asserted here is both
  // halves: no line in the conversation, and a count where the other diagnostics are.
  await page.evaluate(() => window.__RUNTROL_PERF__.emit("session-frame", {
    session: "gate-000",
    frame: JSON.stringify({ body: { event: "unmapped", payload: { anything: "at all" } } }),
  }));
  await page.getByTestId("unread-frames").waitFor();
  const unreadCounted = (await page.getByTestId("unread-frames").textContent())?.includes("1") ?? false;
  const unreadNotDrawn = await page.evaluate(
    () => !(document.querySelector('[data-testid="conversation-pane"]')
      ?.querySelector(".verbatim, [class*='message']")?.textContent ?? "").includes("unmapped"),
  );
  const unreadFramesAreNotConversation = unreadCounted && unreadNotDrawn;

  await search.fill("conversation-only-sentinel");
  await waitFor(page, () => document.querySelectorAll('[data-testid^="session-"]').length === 0);
  const conversationExcluded = await page.getByText("검색 결과가 없다.", { exact: true }).count() === 1;
  await search.fill("");
  await page.getByTestId("session-gate-000").waitFor();
  const metadataSearch = nativeSearch
    && sessionSearch
    && workspaceSearch
    && folderSearch
    && providerSearch
    && conversationExcluded;

  await startWithoutChoosingProvider(page, "C:/work/started-project");
  await page.getByTestId("session-started-from-gui").waitFor();
  await waitFor(page, () => window.__RUNTROL_PERF__.currentWatch()?.session === "started-from-gui");
  const startOpened = await page.getByTestId("conversation-pane").isVisible();
  const startedTitle = await rowLabel("session-started-from-gui");
  const startedFolderTitle = await page.getByTestId("conversation-pane").locator("h1").textContent();
  // An identifier too long for the rail keeps its end, because both identifiers here are UUIDv7 and their
  // leading characters are a timestamp: head-truncation rendered three different sessions as `019fc4fc…`.
  // A name that already fits is untouched, which is what `native-0` checks.
  const titleFallbacks = nativeTitleVisible
    && startedTitle === "…arted-from-gui"
    && startedFolderTitle === "started-project";

  await page.evaluate(() => window.__RUNTROL_PERF__.holdNextResume());
  await page.getByTestId("session-gate-001").click();
  await page.getByText("공급자 준비 중", { exact: true }).waitFor();
  const composer = page.locator('[contenteditable="true"]');
  await composer.fill("재개 중 초안");
  await composer.press("Enter");
  const shellStayedVisible = await page.getByTestId("conversation-pane").isVisible();
  const promptBlockedWhilePreparing = await page.evaluate(
    () => window.__RUNTROL_PERF__.promptRequests.length === 0,
  );
  const preparingDraftPreserved = (await composer.textContent())?.includes("재개 중 초안") ?? false;

  await page.evaluate(() => window.__RUNTROL_PERF__.releaseResume());
  await page.getByTestId("session-resumed-from-gui").waitFor();
  await waitFor(page, () => window.__RUNTROL_PERF__.currentWatch()?.session === "resumed-from-gui");
  const resumeReplacedRow = await page.getByTestId("session-gate-001").count() === 0;

  await composer.fill("첫 요청");
  await page.evaluate(() => window.__RUNTROL_PERF__.holdNextPrompt());
  await composer.press("Enter");
  await waitFor(page, () => window.__RUNTROL_PERF__.promptRequests.length === 1);
  await composer.fill("다음 한글 초안");
  const editableWhileSending = await composer.getAttribute("contenteditable") === "true";
  const nextDraftPreserved = (await composer.textContent())?.includes("다음 한글 초안") ?? false;
  await page.evaluate(() => window.__RUNTROL_PERF__.releasePrompt());

  await page.getByRole("button", { name: "목록에서 삭제", exact: true }).click();
  const dialog = page.getByTestId("remove-session-dialog");
  await dialog.waitFor();
  await dialog.getByRole("button", { name: "취소", exact: true }).click();
  const cancelMadeNoRequest = await page.evaluate(
    () => window.__RUNTROL_PERF__.closeRequests.length === 0,
  );
  const cancelKeptRow = await page.getByTestId("session-resumed-from-gui").count() === 1;

  await page.getByRole("button", { name: "목록에서 삭제", exact: true }).click();
  await dialog.getByRole("button", { name: "목록에서 삭제", exact: true }).click();
  await waitFor(page, () => window.__RUNTROL_PERF__.closeRequests.length === 1);
  await waitFor(page, () => document.querySelector('[data-testid="session-resumed-from-gui"]') === null);
  const deleteRemovedRow = await page.getByTestId("session-resumed-from-gui").count() === 0;

  return {
    unifiedProviders,
    metadataSearch,
    titleFallbacks,
    unreadFramesAreNotConversation,
    startOpened,
    shellStayedVisible,
    promptBlockedWhilePreparing,
    preparingDraftPreserved,
    resumeReplacedRow,
    editableWhileSending,
    nextDraftPreserved,
    cancelMadeNoRequest,
    cancelKeptRow,
    deleteRemovedRow,
  };
}

async function persistence(page, url) {
  await page.goto(url, { waitUntil: "domcontentloaded" });
  await page.getByTestId("session-gate-000").waitFor();
  await page.evaluate(async () => {
    localStorage.clear();
    sessionStorage.clear();
    for (const database of await indexedDB.databases()) {
      if (database.name) indexedDB.deleteDatabase(database.name);
    }
    for (const key of await caches.keys()) await caches.delete(key);
  });
  await page.reload({ waitUntil: "domcontentloaded" });
  await page.getByTestId("session-gate-000").waitFor();
  await startWithoutChoosingProvider(page, "C:/work/persistence-check");
  await page.evaluate(() => {
    window.__RUNTROL_PERF__.emit("session-frame", {
      session: "started-from-gui",
      frame: window.__RUNTROL_PERF__.frame(
        "started-from-gui",
        "conversation sentinel must disappear on reload",
        "persistence-sentinel",
      ),
    });
  });
  await waitFor(page, () => document.body.textContent?.includes("conversation sentinel must disappear on reload"));
  await page.reload({ waitUntil: "domcontentloaded" });
  await page.getByTestId("session-gate-000").waitFor();
  return page.evaluate(async () => {
    const localKeys = Object.keys(localStorage).sort();
    return {
      frameGoneAfterReload: !document.body.textContent?.includes("conversation sentinel must disappear on reload"),
      onlyScalarPreferences: JSON.stringify(localKeys) === JSON.stringify([
        "runtrol.lastProvider",
        "runtrol.theme",
      ]),
      sessionStorageEmpty: sessionStorage.length === 0,
      indexedDbEmpty: (await indexedDB.databases()).length === 0,
      cacheStorageEmpty: (await caches.keys()).length === 0,
    };
  });
}

async function textInput(page, url) {
  await page.goto(url, { waitUntil: "domcontentloaded" });
  await page.getByTestId("session-gate-000").waitFor();
  const composer = page.locator('[contenteditable="true"]');
  await composer.waitFor();
  const initial = await page.evaluate(() => {
    const editable = document.querySelector('[contenteditable="true"]');
    if (!(editable instanceof HTMLElement)) throw new Error("the Astryx composer is missing");
    window.__RUNTROL_TEXT_COPY_EVENTS__ = 0;
    editable.addEventListener("copy", () => { window.__RUNTROL_TEXT_COPY_EVENTS__ += 1; });
    editable.focus();
    editable.textContent = "안녕하세요";
    editable.dispatchEvent(new InputEvent("input", {
      bubbles: true,
      inputType: "insertCompositionText",
      data: "안녕하세요",
      isComposing: true,
    }));
    editable.dispatchEvent(new CompositionEvent("compositionstart", { bubbles: true, data: "ㅇ" }));
    editable.dispatchEvent(new CompositionEvent("compositionupdate", { bubbles: true, data: "안녕하세요" }));
    editable.dispatchEvent(new KeyboardEvent("keydown", {
      bubbles: true,
      cancelable: true,
      key: "Enter",
      // WebView builds have reported false here while a compositionstart is still active. The product tracks
      // both signals, so this fixture proves the event-field-only regression cannot submit a partial syllable.
      isComposing: false,
    }));
    const preserved = editable.textContent === "안녕하세요";
    const requestsAfterComposingEnter = window.__RUNTROL_PERF__.promptRequests.length;
    editable.dispatchEvent(new CompositionEvent("compositionend", { bubbles: true, data: "안녕하세요" }));

    const token = document.createElement("span");
    token.dataset.tokenSentinel = "true";
    token.contentEditable = "false";
    editable.append(token);
    window.__RUNTROL_COMMIT_KEYDOWN_DEFAULT_PREVENTED__ = null;
    window.addEventListener("keydown", (event) => {
      queueMicrotask(() => { window.__RUNTROL_COMMIT_KEYDOWN_DEFAULT_PREVENTED__ = event.defaultPrevented; });
    }, { capture: true, once: true });
    return { preserved, requestsAfterComposingEnter };
  });

  await composer.press("Enter");
  const result = await page.evaluate(async ({ preserved, requestsAfterComposingEnter }) => {
    const editable = document.querySelector('[contenteditable="true"]');
    if (!(editable instanceof HTMLElement)) throw new Error("the Astryx composer is missing");
    const token = editable.querySelector('[data-token-sentinel="true"]');
    const paragraphBlocked = window.__RUNTROL_PERF__.traceLines
      .includes("composer composition commit break blocked");
    const secondParagraph = new InputEvent("beforeinput", {
      bubbles: true,
      cancelable: true,
      inputType: "insertParagraph",
      data: null,
    });
    editable.dispatchEvent(secondParagraph);

    const armCommitGuard = () => {
      editable.dispatchEvent(new CompositionEvent("compositionend", { bubbles: true, data: "안녕하세요" }));
      const keydown = new KeyboardEvent("keydown", {
        bubbles: true,
        cancelable: true,
        key: "Enter",
        isComposing: false,
      });
      editable.dispatchEvent(keydown);
      return keydown;
    };
    const lineCommitEnter = armCommitGuard();
    const lineBreak = new InputEvent("beforeinput", {
      bubbles: true,
      cancelable: true,
      inputType: "insertLineBreak",
      data: null,
    });
    editable.dispatchEvent(lineBreak);

    armCommitGuard();
    const tracesBeforeNonCancelable = window.__RUNTROL_PERF__.traceLines
      .filter((line) => line === "composer composition commit break blocked").length;
    const nonCancelableBreak = new InputEvent("beforeinput", {
      bubbles: true,
      cancelable: false,
      inputType: "insertParagraph",
      data: null,
    });
    editable.dispatchEvent(nonCancelableBreak);
    const tracesAfterNonCancelable = window.__RUNTROL_PERF__.traceLines
      .filter((line) => line === "composer composition commit break blocked").length;
    const afterNonCancelable = new InputEvent("beforeinput", {
      bubbles: true,
      cancelable: true,
      inputType: "insertParagraph",
      data: null,
    });
    editable.dispatchEvent(afterNonCancelable);

    armCommitGuard();
    const tracesBeforeUnmatched = window.__RUNTROL_PERF__.traceLines
      .filter((line) => line === "composer composition commit break blocked").length;
    const unmatchedInput = new InputEvent("beforeinput", {
      bubbles: true,
      cancelable: true,
      inputType: "insertText",
      data: "x",
    });
    editable.dispatchEvent(unmatchedInput);
    const tracesAfterUnmatched = window.__RUNTROL_PERF__.traceLines
      .filter((line) => line === "composer composition commit break blocked").length;
    const afterUnmatched = new InputEvent("beforeinput", {
      bubbles: true,
      cancelable: true,
      inputType: "insertParagraph",
      data: null,
    });
    editable.dispatchEvent(afterUnmatched);

    armCommitGuard();
    const foreignTargetBreak = new InputEvent("beforeinput", {
      bubbles: true,
      cancelable: true,
      inputType: "insertParagraph",
      data: null,
    });
    token.dispatchEvent(foreignTargetBreak);
    const afterForeignTarget = new InputEvent("beforeinput", {
      bubbles: true,
      cancelable: true,
      inputType: "insertParagraph",
      data: null,
    });
    editable.dispatchEvent(afterForeignTarget);

    const originalSetTimeout = window.setTimeout;
    const staleCallbacks = [];
    window.setTimeout = (callback, delay, ...args) => {
      if (delay === 0) {
        staleCallbacks.push(callback);
        return 2_146_000_000 + staleCallbacks.length;
      }
      return originalSetTimeout(callback, delay, ...args);
    };
    let staleTimerBreak;
    try {
      armCommitGuard();
      armCommitGuard();
      staleCallbacks[0]?.();
      staleTimerBreak = new InputEvent("beforeinput", {
        bubbles: true,
        cancelable: true,
        inputType: "insertParagraph",
        data: null,
      });
      editable.dispatchEvent(staleTimerBreak);
    } finally {
      window.setTimeout = originalSetTimeout;
    }

    armCommitGuard();
    await new Promise((resolve) => setTimeout(resolve, 10));
    const expiredBreak = new InputEvent("beforeinput", {
      bubbles: true,
      cancelable: true,
      inputType: "insertParagraph",
      data: null,
    });
    editable.dispatchEvent(expiredBreak);

    const shiftedEnter = new KeyboardEvent("keydown", {
      bubbles: true,
      cancelable: true,
      key: "Enter",
      shiftKey: true,
      isComposing: false,
    });
    editable.dispatchEvent(shiftedEnter);
    const shiftedBreak = new InputEvent("beforeinput", {
      bubbles: true,
      cancelable: true,
      inputType: "insertLineBreak",
      data: null,
    });
    editable.dispatchEvent(shiftedBreak);
    const ordinaryBreak = new InputEvent("beforeinput", {
      bubbles: true,
      cancelable: true,
      inputType: "insertParagraph",
      data: null,
    });
    editable.dispatchEvent(ordinaryBreak);
    const selection = window.getSelection();
    const range = document.createRange();
    range.selectNodeContents(editable);
    selection?.removeAllRanges();
    selection?.addRange(range);
    editable.dispatchEvent(new ClipboardEvent("copy", { bubbles: true, cancelable: true }));
    const selectedText = selection?.toString() ?? "";
    return {
      draftPreservedDuringComposition: preserved,
      composingEnterBlocked: requestsAfterComposingEnter === 0,
      commitEndedEnterBlocked: window.__RUNTROL_PERF__.promptRequests.length === 0,
      commitNativeDefaultAllowed: window.__RUNTROL_COMMIT_KEYDOWN_DEFAULT_PREVENTED__ === false
        && !lineCommitEnter.defaultPrevented,
      commitParagraphBreakBlocked: paragraphBlocked,
      commitLineBreakBlocked: lineBreak.defaultPrevented,
      commitBreakOneShot: !secondParagraph.defaultPrevented,
      nonCancelableBreakIgnored: !nonCancelableBreak.defaultPrevented
        && tracesAfterNonCancelable === tracesBeforeNonCancelable
        && afterNonCancelable.defaultPrevented,
      unmatchedInputIgnored: !unmatchedInput.defaultPrevented
        && tracesAfterUnmatched === tracesBeforeUnmatched
        && afterUnmatched.defaultPrevented,
      foreignTargetIgnored: !foreignTargetBreak.defaultPrevented && afterForeignTarget.defaultPrevented,
      staleTimerPreservesNewGuard: staleTimerBreak?.defaultPrevented === true,
      expiredGuardIgnored: !expiredBreak.defaultPrevented,
      commitBreaksLeaveExactText: editable.textContent === "안녕하세요"
        && !editable.textContent.includes("\r") && !editable.textContent.includes("\n"),
      commitFallbackTraceRecorded: window.__RUNTROL_PERF__.traceLines
        .includes("composer composition commit enter blocked"),
      commitBreakTraceRecorded: window.__RUNTROL_PERF__.traceLines
        .includes("composer composition commit break blocked"),
      shiftedBreakAllowed: !shiftedEnter.defaultPrevented && !shiftedBreak.defaultPrevented,
      ordinaryBreakAllowed: !ordinaryBreak.defaultPrevented,
      tokenNodePreserved: token instanceof HTMLElement && token.isConnected && token.parentElement === editable,
      selectionCreated: selectedText === "안녕하세요",
      copyEventReached: window.__RUNTROL_TEXT_COPY_EVENTS__ === 1,
    };
  }, initial);
  await page.getByTestId("session-gate-002").click();
  await waitFor(page, () => window.__RUNTROL_PERF__.currentWatch()?.session === "gate-002");
  await page.getByTestId("session-gate-000").click();
  await waitFor(page, () => window.__RUNTROL_PERF__.currentWatch()?.session === "gate-000");
  const listenerSingleAfterSessionSwitch = await page.evaluate(() => {
    const editable = document.querySelector('[contenteditable="true"]');
    if (!(editable instanceof HTMLElement)) throw new Error("the switched composer is missing");
    const before = window.__RUNTROL_PERF__.traceLines
      .filter((line) => line === "composer composition commit break blocked").length;
    editable.dispatchEvent(new CompositionEvent("compositionend", { bubbles: true, data: "안녕하세요" }));
    editable.dispatchEvent(new KeyboardEvent("keydown", {
      bubbles: true,
      cancelable: true,
      key: "Enter",
      isComposing: false,
    }));
    const paragraph = new InputEvent("beforeinput", {
      bubbles: true,
      cancelable: true,
      inputType: "insertParagraph",
      data: null,
    });
    editable.dispatchEvent(paragraph);
    const after = window.__RUNTROL_PERF__.traceLines
      .filter((line) => line === "composer composition commit break blocked").length;
    return paragraph.defaultPrevented && after - before === 1;
  });

  await page.waitForTimeout(120);
  await composer.press("Enter");
  await waitFor(page, () => window.__RUNTROL_PERF__.promptRequests.length === 1);
  await page.evaluate(() => {
    const editable = document.querySelector('[contenteditable="true"]');
    if (!(editable instanceof HTMLElement)) throw new Error("the composer to unmount is missing");
    window.__RUNTROL_OLD_EDITABLE__ = editable;
    const originalSetTimeout = window.setTimeout;
    window.setTimeout = (callback, delay, ...args) => {
      if (delay === 0) {
        window.__RUNTROL_STALE_COMMIT_TIMER__ = callback;
        return 2_147_000_000;
      }
      return originalSetTimeout(callback, delay, ...args);
    };
    try {
      editable.dispatchEvent(new CompositionEvent("compositionend", { bubbles: true, data: "" }));
      editable.dispatchEvent(new KeyboardEvent("keydown", {
        bubbles: true,
        cancelable: true,
        key: "Enter",
        isComposing: false,
      }));
    } finally {
      window.setTimeout = originalSetTimeout;
    }
    window.__RUNTROL_UNMOUNT_BREAK_TRACES__ = window.__RUNTROL_PERF__.traceLines
      .filter((line) => line === "composer composition commit break blocked").length;
  });
  await page.getByRole("button", { name: "목록에서 삭제", exact: true }).click();
  const dialog = page.getByTestId("remove-session-dialog");
  await dialog.waitFor();
  await dialog.getByRole("button", { name: "목록에서 삭제", exact: true }).click();
  await waitFor(page, () => document.querySelector('[contenteditable="true"]') === null);
  const unmountCleanup = await page.evaluate(() => {
    const editable = window.__RUNTROL_OLD_EDITABLE__;
    if (!(editable instanceof HTMLElement)) return false;
    const paragraph = new InputEvent("beforeinput", {
      bubbles: true,
      cancelable: true,
      inputType: "insertParagraph",
      data: null,
    });
    editable.dispatchEvent(paragraph);
    const after = window.__RUNTROL_PERF__.traceLines
      .filter((line) => line === "composer composition commit break blocked").length;
    return !paragraph.defaultPrevented && after === window.__RUNTROL_UNMOUNT_BREAK_TRACES__;
  });

  await page.goto(url, { waitUntil: "domcontentloaded" });
  await page.getByTestId("session-gate-000").waitFor();
  await page.locator('[contenteditable="true"]').fill("조합 시작 전환");
  await page.evaluate(() => {
    const editable = document.querySelector('[contenteditable="true"]');
    if (!(editable instanceof HTMLElement)) throw new Error("the composition-start composer is missing");
    editable.dispatchEvent(new CompositionEvent("compositionstart", { bubbles: true, data: "조" }));
  });
  await page.getByTestId("session-gate-002").click();
  await waitFor(page, () => window.__RUNTROL_PERF__.currentWatch()?.session === "gate-002");
  const startSwitchBlockedBefore = await page.evaluate(() => window.__RUNTROL_PERF__.traceLines
    .filter((line) => line === "composer composing enter blocked").length);
  await page.locator('[contenteditable="true"]').press("Enter");
  await waitFor(page, () => window.__RUNTROL_PERF__.promptRequests.length === 1);
  const compositionStartSessionSwitchReset = await page.evaluate((before) => window.__RUNTROL_PERF__.traceLines
    .filter((line) => line === "composer composing enter blocked").length === before, startSwitchBlockedBefore);

  await page.goto(url, { waitUntil: "domcontentloaded" });
  await page.getByTestId("session-gate-000").waitFor();
  await page.locator('[contenteditable="true"]').fill("조합 종료 전환");
  await page.evaluate(() => {
    const editable = document.querySelector('[contenteditable="true"]');
    if (!(editable instanceof HTMLElement)) throw new Error("the composition-end composer is missing");
    editable.dispatchEvent(new CompositionEvent("compositionstart", { bubbles: true, data: "조" }));
    editable.dispatchEvent(new CompositionEvent("compositionend", { bubbles: true, data: "조합 종료 전환" }));
  });
  await page.getByTestId("session-gate-002").click();
  await waitFor(page, () => window.__RUNTROL_PERF__.currentWatch()?.session === "gate-002");
  const endSwitchMarkerBefore = await page.evaluate(() => window.__RUNTROL_PERF__.traceLines
    .filter((line) => line === "composer composition commit enter blocked").length);
  await page.locator('[contenteditable="true"]').press("Enter");
  await waitFor(page, () => window.__RUNTROL_PERF__.promptRequests.length === 1);
  const compositionEndSessionSwitchReset = await page.evaluate((before) => window.__RUNTROL_PERF__.traceLines
    .filter((line) => line === "composer composition commit enter blocked").length === before, endSwitchMarkerBefore);

  await page.goto(url, { waitUntil: "domcontentloaded" });
  await page.getByTestId("session-gate-000").waitFor();
  const editableRemountCompositionReset = await page.evaluate(async () => {
    const editable = document.querySelector('[contenteditable="true"]');
    if (!(editable instanceof HTMLElement)) throw new Error("the remount composer is missing");
    editable.dispatchEvent(new CompositionEvent("compositionstart", { bubbles: true, data: "조" }));
    const replacement = editable.cloneNode(true);
    if (!(replacement instanceof HTMLElement)) return false;
    editable.replaceWith(replacement);
    await new Promise(requestAnimationFrame);
    const blockedBefore = window.__RUNTROL_PERF__.traceLines
      .filter((line) => line === "composer composing enter blocked").length;
    let staleEnterReachedTarget = false;
    replacement.addEventListener("keydown", () => { staleEnterReachedTarget = true; }, { once: true });
    replacement.dispatchEvent(new KeyboardEvent("keydown", {
      bubbles: true,
      cancelable: true,
      key: "Enter",
      isComposing: false,
    }));
    const staleWasNotBlocked = staleEnterReachedTarget && window.__RUNTROL_PERF__.traceLines
      .filter((line) => line === "composer composing enter blocked").length === blockedBefore;
    replacement.dispatchEvent(new CompositionEvent("compositionstart", { bubbles: true, data: "새" }));
    replacement.dispatchEvent(new KeyboardEvent("keydown", {
      bubbles: true,
      cancelable: true,
      key: "Enter",
      isComposing: false,
    }));
    const replacementStillUsesLifecycle = window.__RUNTROL_PERF__.traceLines
      .filter((line) => line === "composer composing enter blocked").length === blockedBefore + 1;
    replacement.dispatchEvent(new CompositionEvent("compositionend", { bubbles: true, data: "새" }));
    return staleWasNotBlocked && replacementStillUsesLifecycle;
  });
  return {
    ...result,
    listenerSingleAfterSessionSwitch,
    normalEnterSubmitted: true,
    unmountCleanup,
    compositionStartSessionSwitchReset,
    compositionEndSessionSwitchReset,
    editableRemountCompositionReset,
  };
}

async function reconnect(page, url) {
  await page.goto(url, { waitUntil: "domcontentloaded" });
  await page.getByTestId("session-gate-000").waitFor();
  await waitFor(page, () => document.body.textContent?.includes("saved tail gate-000"));
  const expected = await page.evaluate(() => {
    const before = window.__RUNTROL_PERF__.currentWatch();
    window.__RUNTROL_PERF__.emit("session-frame", {
      session: "gate-000",
      frame: window.__RUNTROL_PERF__.frame("gate-000", "drain one", "drain-one"),
    });
    window.__RUNTROL_PERF__.emit("session-frame", {
      session: "gate-000",
      frame: window.__RUNTROL_PERF__.frame("gate-000", "drain two", "drain-two"),
    });
    const target = window.__RUNTROL_PERF__.currentWatch().cursor;
    window.__RUNTROL_PERF__.emitOver(true);
    return { oldView: before.view, target };
  });
  await waitFor(page, () => (
    document.body.textContent?.includes("drain one")
    && document.body.textContent?.includes("drain two")
    && window.__RUNTROL_PERF__.watchRequests.length >= 2
  ));
  const result = await page.evaluate((oldView) => {
    const requestsBefore = window.__RUNTROL_PERF__.watchRequests.length;
    const current = window.__RUNTROL_PERF__.currentWatch();
    window.__RUNTROL_PERF__.emit("session-over", {
      session: "gate-000",
      view: oldView,
      nextExpected: current.cursor,
      lagged: true,
    });
    return { requestsBefore, request: window.__RUNTROL_PERF__.watchRequests.at(-1) };
  }, expected.oldView);
  await new Promise((resolveWait) => setTimeout(resolveWait, 400));
  const requestsAfter = await page.evaluate(() => window.__RUNTROL_PERF__.watchRequests.length);
  await page.evaluate(() => {
    window.__RUNTROL_PERF__.holdNextWatch();
    document.querySelector('[data-testid="session-gate-002"]')?.click();
  });
  await waitFor(page, () => window.__RUNTROL_PERF__.pendingIntent() !== null);
  const pendingIntent = await page.evaluate(() => window.__RUNTROL_PERF__.pendingIntent());
  await page.evaluate(() => {
    document.querySelector('[data-testid="session-gate-003"]')?.click();
  });
  await waitFor(page, () => window.__RUNTROL_PERF__.currentWatch()?.session === "gate-003");
  const pendingWatchCancelled = await page.evaluate(
    (intent) => window.__RUNTROL_PERF__.stoppedIntents().includes(intent),
    pendingIntent,
  );
  return {
    drainedBeforeReconnect: true,
    reconnectCursorExact: JSON.stringify(result.request.after) === JSON.stringify(expected.target),
    staleOverIgnored: requestsAfter === result.requestsBefore,
    pendingWatchCancelled,
  };
}

async function retention(page, url) {
  const limit = 256 * 1024;
  const open = async () => {
    await page.goto(url, { waitUntil: "domcontentloaded" });
    await page.getByTestId("session-gate-000").waitFor();
    await waitFor(page, () => document.body.textContent?.includes("saved tail gate-000"));
  };
  const emitText = (text, messageId) => page.evaluate(({ text, messageId }) => {
    window.__RUNTROL_PERF__.emit("session-frame", {
      session: "gate-000",
      frame: window.__RUNTROL_PERF__.frame("gate-000", text, messageId),
    });
  }, { text, messageId });
  const settleFrames = () => page.evaluate(async () => {
    await new Promise(requestAnimationFrame);
    await new Promise(requestAnimationFrame);
    await new Promise(requestAnimationFrame);
  });
  const domMetrics = () => page.evaluate(() => {
    const nodes = [...document.querySelectorAll(".verbatim")];
    return {
      items: nodes.length,
      lengths: nodes.map((node) => node.textContent?.length ?? 0),
      characters: nodes.reduce((total, node) => total + (node.textContent?.length ?? 0), 0),
      first: nodes[0]?.textContent?.at(0) ?? "",
      last: nodes.at(-1)?.textContent?.at(-1) ?? "",
    };
  });

  await open();
  await page.evaluate((characters) => {
    window.__RUNTROL_PERF__.emit("session-frame", {
      session: "gate-000",
      frame: window.__RUNTROL_PERF__.frame(
        "gate-000",
        "A".repeat(characters + 100),
        "huge-retained",
      ),
    });
    window.__RUNTROL_PERF__.emit("session-frame", {
      session: "gate-000",
      frame: JSON.stringify({ body: { event: "turn", step: "ended", stop: "done" } }),
    });
  }, limit);
  await settleFrames();
  const hugeStatus = await domMetrics();

  await open();
  await emitText("E".repeat(limit), "exact-bound");
  await settleFrames();
  const exact = await domMetrics();

  await open();
  await emitText("A".repeat(100 * 1024), "multiple-a");
  await emitText("B".repeat(100 * 1024), "multiple-b");
  await emitText("C".repeat(100 * 1024), "multiple-c");
  await settleFrames();
  const multiple = await domMetrics();

  await open();
  await page.evaluate(() => { window.__RUNTROL_PERF__.traceLines.length = 0; });
  for (let batch = 0; batch < 20; batch += 1) {
    await page.evaluate((start) => {
      for (let offset = 0; offset < 25; offset += 1) {
        const index = start + offset;
        window.__RUNTROL_PERF__.emit("session-frame", {
          session: "gate-000",
          frame: window.__RUNTROL_PERF__.frame(
            "gate-000",
            `${String(index).padStart(4, "0")}${"x".repeat(1020)}`,
            `bounded-${index}`,
          ),
        });
      }
    }, batch * 25);
    await settleFrames();
  }
  const growth = await page.evaluate(() => {
    const applied = window.__RUNTROL_PERF__.traceLines
      .filter((line) => line.startsWith("frame applied checkpoint="))
      .map((line) => {
        const match = /items=(\d+) characters=(\d+)$/.exec(line);
        return match ? { items: Number(match[1]), characters: Number(match[2]) } : null;
      })
      .filter(Boolean);
    const latest = applied.at(-1) ?? { items: 0, characters: 0 };
    const nodes = [...document.querySelectorAll(".verbatim")];
    return {
      latest,
      maximumCharacters: Math.max(0, ...applied.map((entry) => entry.characters)),
      renderedItems: nodes.length,
      newestVisible: nodes.at(-1)?.textContent?.startsWith("0499") ?? false,
    };
  });

  return {
    hugeItemAndStatus: hugeStatus.characters === limit
      && hugeStatus.items === 2
      && hugeStatus.first === "A",
    exactBound: exact.characters === limit && exact.items === 1,
    multipleItems: multiple.characters === limit
      && multiple.items === 3
      && multiple.lengths[0] === 56 * 1024
      && multiple.first === "A"
      && multiple.last === "C",
    boundedGrowth: growth.latest.characters === limit
      && growth.latest.items <= 400
      && growth.maximumCharacters <= limit
      && growth.renderedItems <= 48
      && growth.newestVisible,
  };
}

async function main() {
  const mode = process.argv[2];
  if (!new Set(["interaction", "scroll", "convenience", "lifecycle", "persistence", "text-input", "reconnect", "retention"]).has(mode)) {
    throw new Error(
      "usage: node tests/performance.mjs interaction|scroll|convenience|lifecycle|persistence|text-input|reconnect|retention",
    );
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
    await page.addInitScript(mockBridge, mode);
    const metrics = mode === "interaction"
      ? await interaction(page, url)
      : mode === "scroll"
        ? await scroll(page, url)
        : mode === "convenience"
          ? await convenience(page, url)
          : mode === "lifecycle"
            ? await lifecycle(page, url)
            : mode === "persistence"
              ? await persistence(page, url)
            : mode === "text-input"
              ? await textInput(page, url)
              : mode === "retention"
                ? await retention(page, url)
              : await reconnect(page, url);
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
