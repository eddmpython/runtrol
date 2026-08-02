// Drives the production Tauri window over its loopback WebView2 endpoint for the real-provider
// operator gate. The journey resumes one existing native conversation and starts one fresh session
// per provider, proves both providers share one list at the same time, exercises the cancel path of
// confirmed deletion, removes every row it created, and reports content-free evidence as one JSON
// line. It never touches the composer, so the count of prompt invocations must stay zero.
import { readFile } from "node:fs/promises";
import process from "node:process";
import { chromium } from "playwright-core";

const STEP_TIMEOUT_MS = 120_000;
const TARGET_COUNT = 2;
const EXPECTED_INVOKES = Object.freeze({ start: 2, resume: 2, close: 4, prompt: 0 });

class Failed extends Error {}

function specProblems(raw) {
  if (typeof raw !== "object" || raw === null || Array.isArray(raw)) {
    return ["driver spec is not an object"];
  }
  const problems = [];
  if (raw.schema !== 1) problems.push("driver spec schema is not 1");
  if (typeof raw.binary !== "string" || !raw.binary) problems.push("driver spec names no binary");
  const targets = raw.targets;
  if (!Array.isArray(targets) || targets.length !== TARGET_COUNT) {
    problems.push(`driver spec does not carry exactly ${TARGET_COUNT} targets`);
    return problems;
  }
  const providers = new Set();
  const seeds = new Set();
  targets.forEach((target, index) => {
    const where = `target ${index + 1}`;
    for (const field of ["provider", "displayName", "native", "seedSession", "startWorkspace"]) {
      if (typeof target?.[field] !== "string" || !target[field]) {
        problems.push(`${where} has no ${field}`);
      }
    }
    if (providers.has(target?.provider)) problems.push(`${where} repeats a provider`);
    providers.add(target?.provider);
    if (seeds.has(target?.seedSession)) problems.push(`${where} repeats a seed session`);
    seeds.add(target?.seedSession);
  });
  return problems;
}

// The last honesty layer before evidence is printed. The Python gate revalidates everything, but a
// driver that can print an impossible journey as success would make that outer check unreachable.
function journeyProblems(state) {
  const problems = [];
  if (state.actualProduct !== true) problems.push("the window is not the actual Tauri product");
  if (state.mockBridge !== false) problems.push("a mock bridge owned the window");
  for (const [command, expected] of Object.entries(EXPECTED_INVOKES)) {
    if (state.invokes?.[command] !== expected) {
      problems.push(`${command} was invoked ${state.invokes?.[command]} times, not ${expected}`);
    }
  }
  if (state.simultaneousStarted !== true) problems.push("the two providers never shared one list");
  if (state.cancelKeptRow !== true) problems.push("cancelling a deletion did not keep the row");
  if (state.finalDomRows !== 0) problems.push("the final DOM list is not empty");
  if (state.finalBackendRows !== 0) problems.push("the final backend list is not empty");
  const sessions = new Set();
  for (const entry of state.providers ?? []) {
    if (entry.resumeNativeMatched !== true) {
      problems.push(`${entry.provider} resume did not carry its exact native identifier`);
    }
    if (entry.badgesMatched !== true) problems.push(`${entry.provider} rows lost their provider badge`);
    if (entry.deleted !== true) problems.push(`${entry.provider} rows were not removed`);
    for (const name of ["seedSession", "resumedSession", "startedSession"]) {
      const session = entry[name];
      if (typeof session !== "string" || !session || sessions.has(session)) {
        problems.push(`${entry.provider} ${name} is absent or reused`);
      }
      sessions.add(session);
    }
  }
  if ((state.providers ?? []).length !== TARGET_COUNT) problems.push("provider evidence is incomplete");
  return problems;
}

// Installed after the page settles, never before: predefining the `__TAURI__` global (even as an
// accessor) makes Tauri's own injection skip it and the page loses its bridge entirely (measured:
// the list never painted one row). The product looks the bridge up on every call, so wrapping
// `core.invoke` here still counts every command the journey can send from this moment on, and the
// journey clicks nothing before this hook exists.
function countingHook() {
  const bridge = window.__TAURI__;
  if (!bridge?.core?.invoke) return false;
  if (window.__RUNTROL_REAL_DRIVE__) return true;
  const counts = { start: 0, resume: 0, close: 0, prompt: 0 };
  const records = { start: [], resume: [], close: [], prompt: [] };
  const original = bridge.core.invoke.bind(bridge.core);
  const counted = async (command, args) => {
    if (command in counts) counts[command] += 1;
    const result = await original(command, args);
    if (command in records) records[command].push({ args: args ?? null, result });
    return result;
  };
  // The injected `core` namespace is frozen (measured: assigning into it is silently ignored), while
  // the global itself is a writable property and the product resolves it freshly on every call. So
  // the global is replaced with a clone that carries the counted invoke and the untouched rest.
  window.__TAURI__ = { ...bridge, core: { ...bridge.core, invoke: counted } };
  window.__RUNTROL_REAL_DRIVE__ = { counts, records };
  return window.__TAURI__.core.invoke === counted;
}

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

// Step markers go to stderr, which a direct run shows and the calling gate ignores unless the last
// line becomes the failure diagnostic. Nothing session-owned is written, only driver step names.
const step = (line) => process.stderr.write(`[driver] ${line}\n`);

// Every wait polls from this driver over CDP rather than with an in-page timer or animation frame.
// A covered or unfocused WebView2 throttles both, and an unattended operator gate cannot promise the
// window stays front (measured: an in-page wait starved while a direct evaluate saw the state).
async function poll(page, fn, arg, what) {
  const deadline = Date.now() + STEP_TIMEOUT_MS;
  for (;;) {
    const value = await page.evaluate(fn, arg);
    if (value) return value;
    if (Date.now() > deadline) throw new Failed(`timed out waiting for ${what}`);
    await sleep(250);
  }
}

async function waitRow(page, session) {
  await poll(
    page,
    (id) => {
      const row = document.querySelector(`[data-testid="session-${id}"]`);
      if (!(row instanceof HTMLElement)) return false;
      const box = row.getBoundingClientRect();
      return box.width > 0 && box.height > 0;
    },
    session,
    `row session-${session}`,
  ).catch(async (error) => {
    const snapshot = await page.evaluate(() =>
      [...document.querySelectorAll('[data-testid^="session-"]')].map((row) =>
        row.getAttribute("data-testid"),
      ),
    );
    throw new Failed(`${error.message}; rows now [${snapshot.join(", ")}]`);
  });
}

async function waitRowGone(page, session) {
  await poll(
    page,
    (id) => document.querySelector(`[data-testid="session-${id}"]`) === null,
    session,
    `removal of row session-${session}`,
  );
}

// Clicks are delivered as real pointer input at the element's center. A synthetic `click()` never
// reaches the component library's press handling (measured: a clicked row started no resume), and
// real input is also what this gate claims to exercise.
async function clickAt(page, box) {
  await page.mouse.click(box.x + box.width / 2, box.y + box.height / 2);
}

async function clickRow(page, session) {
  await waitRow(page, session);
  const box = await page.evaluate((id) => {
    const rect = document
      .querySelector(`[data-testid="session-${id}"]`)
      ?.getBoundingClientRect();
    return rect ? { x: rect.x, y: rect.y, width: rect.width, height: rect.height } : null;
  }, session);
  if (!box) throw new Failed(`row session-${session} vanished before its click`);
  await clickAt(page, box);
}

// Buttons are resolved by their exact visible name, scoped to the confirmation dialog when asked.
async function clickButton(page, name, { inDialog = false } = {}) {
  const box = await poll(
    page,
    ({ name, inDialog }) => {
      const root = inDialog
        ? document.querySelector('[data-testid="remove-session-dialog"]')
        : document;
      if (!root) return false;
      const button = [...root.querySelectorAll("button")].find(
        (candidate) => candidate.textContent?.trim() === name && !candidate.disabled,
      );
      if (!button) return false;
      const rect = button.getBoundingClientRect();
      if (rect.width === 0 || rect.height === 0) return false;
      return { x: rect.x, y: rect.y, width: rect.width, height: rect.height };
    },
    { name, inDialog },
    `the ${name} button${inDialog ? " in the removal dialog" : ""}`,
  );
  await clickAt(page, box);
}

async function rowShowsBadge(page, session, provider) {
  return page.evaluate(
    ({ session, provider }) => {
      const row = document.querySelector(`[data-testid="session-${session}"]`);
      return row instanceof HTMLElement && (row.textContent ?? "").includes(provider);
    },
    { session, provider },
  );
}

async function drive(page, counters) {
  const counts = () => page.evaluate(() => window.__RUNTROL_REAL_DRIVE__.counts);
  const lastRecord = (command) =>
    page.evaluate((name) => window.__RUNTROL_REAL_DRIVE__.records[name].at(-1) ?? null, command);
  const evidence = [];
  for (const [index, target] of counters.spec.targets.entries()) {
    step(`waiting for seed row of ${target.provider}`);
    await waitRow(page, target.seedSession);
    const seedBadge = await rowShowsBadge(page, target.seedSession, target.provider);
    step(`clicking seed row of ${target.provider}`);
    await clickRow(page, target.seedSession);
    // Waits on completed records rather than call entries: the count rises when the command is
    // sent, while the record lands with its answer, and reading the latest record between the two
    // returns the previous journey step's answer (measured: the codex start read the claude one).
    const resumeRecord = await poll(
      page,
      (expected) => {
        const records = window.__RUNTROL_REAL_DRIVE__.records.resume;
        return records.length === expected ? records.at(-1) : false;
      },
      index + 1,
      `the resume answer for ${target.provider}`,
    );
    const resumedSession = resumeRecord?.result?.value;
    if (typeof resumedSession !== "string" || !resumedSession) {
      throw new Failed(`${target.provider} resume returned no session identifier`);
    }
    step(`waiting for resumed row of ${target.provider}`);
    await waitRow(page, resumedSession);
    await waitRowGone(page, target.seedSession);
    evidence.push({
      provider: target.provider,
      seedSession: target.seedSession,
      resumedSession,
      resumeNativeMatched: resumeRecord?.args?.native === target.native,
      seedBadge,
    });
  }

  for (const [index, target] of counters.spec.targets.entries()) {
    const entry = evidence[index];
    // The dialog defaults to the remembered provider, which is the product's own scalar preference.
    // Setting it here chooses the provider the way the product remembers one, without driving the
    // selector widget, and the recorded start arguments prove which provider actually started.
    await page.evaluate(
      (provider) => localStorage.setItem("runtrol.lastProvider", provider),
      target.provider,
    );
    step(`opening the start dialog for ${target.provider}`);
    await clickRow(page, entry.resumedSession);
    // The previous dialog animates out after its submit; clicking through that remnant would reach
    // the dead form and resubmit its captured state (measured: the second start repeated the first
    // start's exact arguments). So the next dialog is opened only once no dialog is left at all.
    await poll(
      page,
      () => {
        const gone = (element) => {
          if (!element) return true;
          const box = element.getBoundingClientRect();
          return box.width === 0 || box.height === 0;
        };
        return (
          gone(document.querySelector(".start-fields"))
          && gone(document.querySelector('[data-testid="remove-session-dialog"]'))
        );
      },
      undefined,
      "every earlier dialog to leave",
    );
    await clickButton(page, "새 세션");
    const workspaceBox = await poll(
      page,
      () => {
        const label = [...document.querySelectorAll("label")].find((candidate) =>
          candidate.textContent?.trim().startsWith("작업 폴더"),
        );
        if (!label) return false;
        const input = label.htmlFor
          ? document.getElementById(label.htmlFor)
          : (label.querySelector("input") ?? label.parentElement?.querySelector("input"));
        if (!(input instanceof HTMLInputElement)) return false;
        const rect = input.getBoundingClientRect();
        if (rect.width === 0 || rect.height === 0) return false;
        return { x: rect.x, y: rect.y, width: rect.width, height: rect.height };
      },
      undefined,
      "the workspace field of the start dialog",
    );
    // Typed as real keyboard input: a synthetic value assignment never reaches the controlled
    // component's state (measured: the started workspace stayed the dialog's prefill).
    await clickAt(page, workspaceBox);
    await page.keyboard.press("Control+a");
    await page.keyboard.type(target.startWorkspace);
    await poll(
      page,
      (workspace) => {
        const label = [...document.querySelectorAll("label")].find((candidate) =>
          candidate.textContent?.trim().startsWith("작업 폴더"),
        );
        const input = label?.htmlFor
          ? document.getElementById(label.htmlFor)
          : (label?.querySelector("input") ?? label?.parentElement?.querySelector("input"));
        return input instanceof HTMLInputElement && input.value === workspace;
      },
      target.startWorkspace,
      "the typed workspace to land in the start dialog",
    );
    await clickButton(page, "시작");
    const startRecord = await poll(
      page,
      (expected) => {
        const records = window.__RUNTROL_REAL_DRIVE__.records.start;
        return records.length === expected ? records.at(-1) : false;
      },
      index + 1,
      `the start answer for ${target.provider}`,
    );
    const startedSession = startRecord?.result?.value;
    if (typeof startedSession !== "string" || !startedSession) {
      throw new Failed(`${target.provider} start returned no session identifier`);
    }
    if (startRecord?.args?.provider !== target.provider) {
      const dialogFacts = await page.evaluate(() => ({
        remembered: localStorage.getItem("runtrol.lastProvider"),
      }));
      throw new Failed(
        `${target.provider} start used another provider `
          + `(sent ${JSON.stringify(startRecord?.args ?? null)}, remembered ${dialogFacts.remembered})`,
      );
    }
    await waitRow(page, startedSession);
    entry.startedSession = startedSession;
    entry.startedBadge = await rowShowsBadge(page, startedSession, target.provider);
  }

  const simultaneousStarted = await page.evaluate(
    (sessions) =>
      sessions.every((session) => document.querySelector(`[data-testid="session-${session}"]`) !== null),
    evidence.flatMap((entry) => [entry.resumedSession, entry.startedSession]),
  );

  const dialogGone = () =>
    poll(
      page,
      () => {
        const dialog = document.querySelector('[data-testid="remove-session-dialog"]');
        if (!dialog) return true;
        const box = dialog.getBoundingClientRect();
        return box.width === 0 || box.height === 0;
      },
      undefined,
      "the removal dialog to close",
    );
  const closesBeforeCancel = (await counts()).close;
  await clickRow(page, evidence[0].startedSession);
  await clickButton(page, "목록에서 삭제");
  await clickButton(page, "취소", { inDialog: true });
  await dialogGone();
  const cancelKeptRow =
    (await counts()).close === closesBeforeCancel &&
    (await page.evaluate(
      (session) => document.querySelector(`[data-testid="session-${session}"]`) !== null,
      evidence[0].startedSession,
    ));

  let closes = closesBeforeCancel;
  for (const entry of evidence) {
    for (const session of [entry.startedSession, entry.resumedSession]) {
      await clickRow(page, session);
      await clickButton(page, "목록에서 삭제");
      await clickButton(page, "목록에서 삭제", { inDialog: true });
      closes += 1;
      await poll(
        page,
        (expected) => window.__RUNTROL_REAL_DRIVE__.records.close.length === expected,
        closes,
        `close answer ${closes}`,
      );
      await waitRowGone(page, session);
      await dialogGone();
    }
    entry.deleted = await page.evaluate(
      (sessions) =>
        sessions.every((session) => document.querySelector(`[data-testid="session-${session}"]`) === null),
      [entry.resumedSession, entry.startedSession],
    );
  }

  await poll(
    page,
    () => document.querySelectorAll('[data-testid^="session-"]').length === 0,
    undefined,
    "an empty final session list",
  );
  const finalDomRows = await page.evaluate(
    () => document.querySelectorAll('[data-testid^="session-"]').length,
  );
  const finalBackendRows = await page.evaluate(async () => {
    const answer = await window.__TAURI__.core.invoke("sessions");
    if (answer?.outcome !== "ok") throw new Error("the final backend listing was refused");
    return answer.value.sessions.length;
  });

  const flags = await page.evaluate(() => ({
    actualProduct: "__TAURI_INTERNALS__" in window,
    mockBridge: "__RUNTROL_PERF__" in window,
  }));

  return {
    schema: 1,
    actualProduct: flags.actualProduct,
    mockBridge: flags.mockBridge,
    simultaneousStarted,
    cancelKeptRow,
    finalDomRows,
    finalBackendRows,
    invokes: await counts(),
    providers: evidence.map((entry) => ({
      provider: entry.provider,
      seedSession: entry.seedSession,
      resumedSession: entry.resumedSession,
      startedSession: entry.startedSession,
      resumeNativeMatched: entry.resumeNativeMatched,
      badgesMatched: entry.seedBadge === true && entry.startedBadge === true,
      deleted: entry.deleted === true,
    })),
  };
}

async function exercise(endpoint, specPath) {
  const raw = JSON.parse(await readFile(specPath, "utf-8"));
  const problems = specProblems(raw);
  if (problems.length > 0) throw new Failed(`driver spec is unusable: ${problems.join("; ")}`);

  const browser = await chromium.connectOverCDP(endpoint, { timeout: STEP_TIMEOUT_MS });
  try {
    const context = browser.contexts()[0];
    if (!context) throw new Failed("the product window exposed no browser context");
    const page = context.pages()[0];
    if (!page) throw new Failed("the product window exposed no page");
    await page.reload({ waitUntil: "domcontentloaded", timeout: STEP_TIMEOUT_MS });
    await poll(
      page,
      () => window.__TAURI__?.core?.invoke !== undefined,
      undefined,
      "the product bridge after reload",
    );
    const hooked = await page.evaluate(countingHook);
    if (!hooked) throw new Failed("the invoke counting hook did not install");

    const result = await drive(page, { spec: raw });
    const failures = journeyProblems(result);
    if (failures.length > 0) throw new Failed(`the product journey did not hold: ${failures.join("; ")}`);
    process.stdout.write(`${JSON.stringify(result)}\n`);
  } finally {
    // The gate owns the window; disconnecting leaves the product process running for its own close.
    await browser.close();
  }
}

function selftest() {
  const spec = {
    schema: 1,
    binary: "C:/product/runtrol.exe",
    targets: [
      {
        provider: "provider-a",
        displayName: "Provider A",
        native: "native-a",
        seedSession: "11111111-1111-1111-1111-111111111111",
        startWorkspace: "C:/work/a",
      },
      {
        provider: "provider-b",
        displayName: "Provider B",
        native: "native-b",
        seedSession: "22222222-2222-2222-2222-222222222222",
        startWorkspace: "C:/work/b",
      },
    ],
  };
  if (specProblems(spec).length > 0) throw new Failed("selftest defect: a complete spec was rejected");
  const specInjections = [
    { ...spec, schema: 2 },
    { ...spec, targets: spec.targets.slice(0, 1) },
    { ...spec, targets: [spec.targets[0], spec.targets[0]] },
    { ...spec, targets: [spec.targets[0], { ...spec.targets[1], native: "" }] },
  ];
  for (const [index, broken] of specInjections.entries()) {
    if (specProblems(broken).length === 0) {
      throw new Failed(`selftest defect: spec injection ${index + 1} escaped`);
    }
  }

  const green = {
    actualProduct: true,
    mockBridge: false,
    simultaneousStarted: true,
    cancelKeptRow: true,
    finalDomRows: 0,
    finalBackendRows: 0,
    invokes: { start: 2, resume: 2, close: 4, prompt: 0 },
    providers: [
      {
        provider: "provider-a",
        seedSession: "11111111-1111-1111-1111-111111111111",
        resumedSession: "33333333-3333-3333-3333-333333333333",
        startedSession: "44444444-4444-4444-4444-444444444444",
        resumeNativeMatched: true,
        badgesMatched: true,
        deleted: true,
      },
      {
        provider: "provider-b",
        seedSession: "22222222-2222-2222-2222-222222222222",
        resumedSession: "55555555-5555-5555-5555-555555555555",
        startedSession: "66666666-6666-6666-6666-666666666666",
        resumeNativeMatched: true,
        badgesMatched: true,
        deleted: true,
      },
    ],
  };
  if (journeyProblems(green).length > 0) {
    throw new Failed("selftest defect: a complete journey was rejected");
  }
  const journeyInjections = [
    { ...green, actualProduct: false },
    { ...green, mockBridge: true },
    { ...green, simultaneousStarted: false },
    { ...green, cancelKeptRow: false },
    { ...green, finalDomRows: 1 },
    { ...green, finalBackendRows: 3 },
    { ...green, invokes: { ...green.invokes, prompt: 1 } },
    { ...green, invokes: { ...green.invokes, close: 3 } },
    { ...green, providers: [{ ...green.providers[0], resumeNativeMatched: false }, green.providers[1]] },
    { ...green, providers: [{ ...green.providers[0], badgesMatched: false }, green.providers[1]] },
    { ...green, providers: [{ ...green.providers[0], deleted: false }, green.providers[1]] },
    {
      ...green,
      providers: [
        { ...green.providers[0], resumedSession: green.providers[0].seedSession },
        green.providers[1],
      ],
    },
    { ...green, providers: green.providers.slice(0, 1) },
  ];
  for (const [index, broken] of journeyInjections.entries()) {
    if (journeyProblems(broken).length === 0) {
      throw new Failed(`selftest defect: journey injection ${index + 1} escaped`);
    }
  }
  process.stdout.write(
    `driver selftest ok: ${specInjections.length + journeyInjections.length} injected defects detected\n`,
  );
}

async function main() {
  const argv = process.argv.slice(2);
  if (argv.includes("--selftest")) {
    if (argv.length !== 1) throw new Failed("--selftest takes no other argument");
    selftest();
    return;
  }
  const endpointAt = argv.indexOf("--endpoint");
  const specAt = argv.indexOf("--spec");
  const endpoint = endpointAt >= 0 ? argv[endpointAt + 1] : undefined;
  const spec = specAt >= 0 ? argv[specAt + 1] : undefined;
  if (!endpoint || !spec || argv.length !== 4) {
    throw new Failed("usage: node tests/actualProductLifecycle.mjs --endpoint <url> --spec <path> | --selftest");
  }
  await exercise(endpoint, spec);
}

main().catch((error) => {
  process.stderr.write(`${error instanceof Error ? error.message : String(error)}\n`);
  process.exitCode = 2;
});
