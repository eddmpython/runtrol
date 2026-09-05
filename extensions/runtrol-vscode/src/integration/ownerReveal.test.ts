import { mkdir, rename, writeFile } from "node:fs/promises";
import { monitorEventLoopDelay } from "node:perf_hooks";
import path from "node:path";

import * as vscode from "vscode";

import { HOST_DEADLINE_ENV, hostDeadline, readJourneyStep } from "../../tooling/courierHostLifetime.mjs";
import type { RuntrolExtensionApi } from "../extension";
import type { JourneyInputTiming } from "../runtimeTerminal";
import { extensionUnderTest } from "./extensionUnderTest.test";

/// The owner-reveal journey inside one isolated window (`EXT-03`, driven by `tooling/owner-reveal-eye.mjs`): the
/// harness tells this window to file projects, start a provider in an ordinary terminal, report what it lists and
/// which terminal is active, and click a sidebar row by key; the effects on the other window are read by the
/// harness from that window's own reports and from the desktop.
///
/// Coordination is files in one folder: the harness publishes `<role>-step-<n>.json`, the window publishes
/// `<role>-ready.json` and `<role>-done-<n>.json`.
const DEADLINE_MS = 60_000;

type Step =
  | { readonly kind: "done" }
  | { readonly kind: "addProject"; readonly folder: string }
  | { readonly kind: "start"; readonly label: string; readonly commandLine: string; readonly cwd?: string }
  | { readonly kind: "startTyped"; readonly label: string; readonly commandLine: string; readonly settleMs: number; readonly setupKeys?: readonly string[]; readonly setupGapMs?: number }
  | { readonly kind: "click"; readonly key: string }
  | { readonly kind: "focusTerminal"; readonly generation: string; readonly terminalId: string }
  | { readonly kind: "rowFacts"; readonly key: string }
  | { readonly kind: "showDiff"; readonly original: string; readonly modified: string }
  | { readonly kind: "rows" }
  | { readonly kind: "listing" }
  | { readonly kind: "reopenStored"; readonly provider: string }
  | { readonly kind: "closeTab"; readonly key: string }
  | { readonly kind: "stopRow"; readonly key: string }
  | { readonly kind: "setDialogue"; readonly key: string; readonly enabled: boolean }
  | { readonly kind: "inputSamples"; readonly generation: string; readonly terminalId: string; readonly text: string; readonly count: number; readonly gapMs: number }
  | { readonly kind: "listed"; readonly provider: string; readonly native: string }
  | { readonly kind: "startFresh"; readonly provider: string; readonly workspace: string }
  | { readonly kind: "showOther" }
  | { readonly kind: "report" }
  | { readonly kind: "type"; readonly label: string; readonly keys: readonly string[]; readonly gapMs: number }
  | { readonly kind: "exit"; readonly label: string; readonly keys: readonly string[]; readonly gapMs: number };

export async function run(): Promise<void> {
  const coordination = requiredEnvironment("RUNTROL_VSCODE_COORDINATION");
  const role = requiredEnvironment("RUNTROL_VSCODE_ROLE");
  await mkdir(coordination, { recursive: true });
  try {
    await journey(coordination, role);
  } catch (error) {
    await publish(coordination, `${role}-failure.json`, {
      failure: error instanceof Error ? error.message : String(error),
      stack: error instanceof Error ? error.stack ?? null : null,
    });
    throw error;
  }
}

async function journey(coordination: string, role: string): Promise<void> {
  const deadlineAtMs = hostDeadline(process.env[HOST_DEADLINE_ENV]);
  const extension = extensionUnderTest<RuntrolExtensionApi>();
  const api = extension.isActive ? extension.exports : await extension.activate();
  await within(api.ready, DEADLINE_MS, "extension readiness");
  if (!api.journey) throw new Error("the journey API is unavailable");
  const journey = api.journey;
  await vscode.commands.executeCommand("workbench.view.extension.runtrol");
  await publish(coordination, `${role}-ready.json`, { sessionId: vscode.env.sessionId, hostPid: process.pid });

  const terminals = new Map<string, vscode.Terminal>();
  // One beat a second with the step this window is on: a harness timeout can then tell a window that is waiting from
  // a window whose Extension Host stopped running at all.
  let waitingFor = 1;
  let running: string | null = null;
  let sinceMs = Date.now();
  let inputSampling: {
    generation: string; terminalId: string; count: number; gapMs: number;
    phase: "attach" | "first" | "samples";
    first: JourneyInputTiming | null; samples: JourneyInputTiming[];
  } | null = null;
  const heartbeat = setInterval(() => {
    void writeFile(
      path.join(coordination, `${role}-alive.json`),
      JSON.stringify({
        waitingFor,
        running,
        elapsedMs: Date.now() - sinceMs,
        activeTerminal: vscode.window.activeTerminal?.name ?? null,
        terminals: vscode.window.terminals.length,
      }),
      "utf8",
    ).catch(() => undefined);
  }, 1_000);
  try {
    await steps();
  } catch (error) {
    // Capture the structural projection before the host closes. Tab count alone cannot distinguish a dead
    // provider from a disconnected index or a lost hosted-row association. No terminal output is read here.
    await publish(coordination, `${role}-failure-state.json`, {
      atMs: Date.now(), step: waitingFor, running,
      listing: journey.listing(), rows: journey.rows(),
      publishFailure: journey.windowPublishFailure(),
      inputSampling,
    });
    throw error;
  } finally {
    clearInterval(heartbeat);
  }

  async function steps(): Promise<void> {
  for (let index = 1; ; index += 1) {
    waitingFor = index;
    running = null;
    inputSampling = null;
    sinceMs = Date.now();
    const step = await readJourneyStep<Step>(coordination, `${role}-step-${index}.json`, DEADLINE_MS * 5, deadlineAtMs);
    if (step.kind === "done") break;
    running = step.kind;
    sinceMs = Date.now();
    let result: Record<string, unknown> = {};
    if (step.kind === "addProject") {
      await journey.addProject(step.folder);
    } else if (step.kind === "start") {
      const terminal = vscode.window.createTerminal({ name: `${role}-${step.label}`, cwd: step.cwd });
      terminals.set(step.label, terminal);
      terminal.show(false);
      await terminal.processId;
      if (!(await waitForShellIntegration(terminal, 30_000))) throw new Error(`${step.label}: shell integration never attached`);
      terminal.shellIntegration?.executeCommand(step.commandLine);
      const mirror = await waitForMirror(journey, 30_000);
      result = { terminalId: mirror.terminalId, refusal: mirror.refusal, terminalName: terminal.name };
    } else if (step.kind === "startTyped") {
      // Typed the way a person types it, with no shell integration to hand the command over: nothing here may
      // depend on `shellIntegration`, and no mirror is expected.
      const terminal = vscode.window.createTerminal({ name: `${role}-${step.label}` });
      terminals.set(step.label, terminal);
      terminal.show(false);
      const processId = await terminal.processId;
      const before = journey.windowMirrors().length;
      terminal.sendText(step.commandLine, true);
      // A provider opened in a fresh folder asks its first-run questions before it is in a conversation at all,
      // and a conversation is what a row is. The keys answer those questions the way a person does.
      for (const key of step.setupKeys ?? []) {
        await delay(step.setupGapMs ?? 4_000);
        terminal.sendText(key, false);
      }
      await delay(step.settleMs);
      result = {
        terminalName: terminal.name,
        shellProcessId: processId ?? null,
        shellIntegration: terminal.shellIntegration !== undefined,
        mirrorsOpenedAfterwards: journey.windowMirrors().length - before,
        mirrors: journey.windowMirrors().slice(before),
      };
    } else if (step.kind === "click") {
      // A row is a conversation the provider has to write down first; wait for it rather than click a guess.
      const waited = Date.now();
      while (!journey.rowKeys().includes(step.key)) {
        if (Date.now() - waited > 90_000) {
          throw new Error(`no sidebar row has key ${step.key} after 90 s; rows ${JSON.stringify(journey.rowKeys())}`);
        }
        await delay(250);
      }
      const facts = journey.rowFacts(step.key);
      const started = Date.now();
      // A click that explains itself with a notification carrying buttons resolves only when the notification
      // is answered, which nobody here does; the effect is read after a bounded wait instead.
      const clicked = await Promise.race([
        journey.clickRow(step.key).then(() => "done", (error: unknown) => `failed: ${error instanceof Error ? error.message : String(error)}`),
        delay(4_000).then(() => "waiting"),
      ]);
      result = {
        rowWaitMs: started - waited,
        facts,
        clickedMs: Date.now() - started,
        outcome: clicked,
        reveal: journey.lastReveal(),
        explanation: journey.lastExplanation(),
        activeTerminalName: journey.activeTerminalName(),
      };
    } else if (step.kind === "focusTerminal") {
      // A provider may publish its native identity after launch. Resolve the current row from the stable
      // Runtime terminal binding at selection time instead of keeping the provisional presentation key.
      result = await journey.terminalAttach(step.generation, step.terminalId, DEADLINE_MS);
    } else if (step.kind === "rows") {
      const publishFailure = journey.windowPublishFailure();
      result = { rows: journey.rows(), listing: journey.listing(), atMs: Date.now(), nativeChatCount: journey.nativeChatCount(), publishFailure, updatePayload: publishFailure ? journey.windowUpdatePayload() : null };
    } else if (step.kind === "closeTab") {
      // The tab's own close: the view detaches, the process is not touched.
      result = { closed: journey.closeTab(step.key) };
    } else if (step.kind === "stopRow") {
      // The row's Stop, after its confirmation: the Runtime is asked to end the process under the exact record.
      await journey.stopRow(step.key);
      result = { stopped: true };
    } else if (step.kind === "setDialogue") {
      await journey.setDialogue(step.key, step.enabled);
      result = { dialogueEnabled: step.enabled };
    } else if (step.kind === "inputSamples") {
      if (!Number.isSafeInteger(step.count) || step.count < 2 || step.count > 1024
        || !Number.isFinite(step.gapMs) || step.gapMs < 0 || step.gapMs > 1000) {
        throw new Error("input samples require bounded count and spacing");
      }
      // Keep only structural timing in memory until the step settles. A failed sample must not discard the
      // completed measurements or make a failed reattach look like a slow write. Never retain the typed bytes.
      inputSampling = { generation: step.generation, terminalId: step.terminalId,
        count: step.count, gapMs: step.gapMs, phase: "attach", first: null, samples: [] };
      await journey.terminalAttach(step.generation, step.terminalId, DEADLINE_MS);
      inputSampling.phase = "first";
      const first = await journey.terminalWriteDirect(step.generation, step.terminalId, step.text);
      inputSampling.first = first;
      inputSampling.phase = "samples";
      const samples = inputSampling.samples;
      const eventLoop = monitorEventLoopDelay({ resolution: 10 });
      eventLoop.enable();
      try {
        for (let sample = 0; sample < step.count; sample += 1) {
          await delay(step.gapMs);
          samples.push(await journey.terminalWriteDirect(step.generation, step.terminalId, step.text));
        }
      } finally {
        eventLoop.disable();
      }
      result = { first, samples, eventLoop: { p95Ms: eventLoop.percentile(95) / 1e6, maxMs: eventLoop.max / 1e6 } };
    } else if (step.kind === "reopenStored") {
      // A stored conversation of that service reopened the way a click on its row does: the resume path.
      result = { sessionId: await journey.openStoredWithTitle(step.provider) };
    } else if (step.kind === "listing") {
      // Beside the rows: whether the Core answers, the listing's own warnings and the managed session records.
      result = { ...journey.listing(), atMs: Date.now() };
    } else if (step.kind === "listed") {
      // Whether the provider's own catalogue, as this window last read it, lists that conversation.
      result = { listed: journey.nativeChatListed(step.provider, step.native), nativeChatCount: journey.nativeChatCount(), atMs: Date.now() };
    } else if (step.kind === "startFresh") {
      // The `+` button's path: a placeholder row and a tab appear at once, before the Runtime has answered.
      const started = Date.now();
      await journey.startFresh(step.provider, step.workspace);
      result = { startedMs: Date.now() - started, rows: journey.rows() };
    } else if (step.kind === "rowFacts") {
      // What the row says it can do, read without clicking: a click on a stored conversation would resume it.
      result = { facts: journey.rowFacts(step.key), present: journey.rowKeys().includes(step.key) };
    } else if (step.kind === "showDiff") {
      if (!path.isAbsolute(step.original) || !path.isAbsolute(step.modified)) {
        throw new Error("diff inspection requires exact absolute file paths");
      }
      const original = vscode.Uri.file(step.original);
      const modified = vscode.Uri.file(step.modified);
      await vscode.commands.executeCommand("vscode.diff", original, modified, "Worker change", { preview: true });
      const input = vscode.window.tabGroups.activeTabGroup.activeTab?.input;
      if (!(input instanceof vscode.TabInputTextDiff)
        || input.original.toString() !== original.toString() || input.modified.toString() !== modified.toString()) {
        throw new Error("the requested worker diff is not the active editor");
      }
      result = { original: step.original, modified: step.modified, active: true };
    } else if (step.kind === "showOther") {
      // A second terminal takes the panel, so a reveal has to bring the provider's terminal back.
      let other = terminals.get("other");
      if (!other) {
        other = vscode.window.createTerminal({ name: `${role}-other` });
        terminals.set("other", other);
      }
      other.show(false);
      await delay(500);
      result = { activeTerminalName: journey.activeTerminalName() };
    } else if (step.kind === "report") {
      result = { activeTerminalName: journey.activeTerminalName(), rowKeys: journey.rowKeys() };
    } else if (step.kind === "type") {
      // Keys into a terminal this window started, the way a person answers a provider's question in it.
      const terminal = terminals.get(step.label);
      if (!terminal) throw new Error(`${step.label}: no terminal of that label was started here`);
      terminal.show(false);
      for (const key of step.keys) {
        terminal.sendText(key, false);
        await delay(step.gapMs);
      }
    } else if (step.kind === "exit") {
      const terminal = terminals.get(step.label);
      if (terminal) {
        for (const key of step.keys) {
          terminal.sendText(key, false);
          await delay(step.gapMs);
        }
        await delay(2_000);
        terminal.dispose();
      }
    }
    await publish(coordination, `${role}-done-${index}.json`, result);
  }
  }
}

async function waitForMirror(
  journey: NonNullable<RuntrolExtensionApi["journey"]>,
  deadlineMs: number,
): Promise<{ terminalId: string | null; refusal: string | null }> {
  const before = journey.windowMirrors().length;
  const deadline = Date.now() + deadlineMs;
  while (Date.now() < deadline) {
    const mirrors = journey.windowMirrors();
    const latest = mirrors[mirrors.length - 1];
    if (mirrors.length >= before && latest && (latest.terminalId !== null || latest.refusal !== null)) {
      return { terminalId: latest.terminalId, refusal: latest.refusal };
    }
    await delay(25);
  }
  throw new Error("no mirror was opened or refused for the command");
}

async function waitForShellIntegration(terminal: vscode.Terminal, deadlineMs: number): Promise<boolean> {
  const started = Date.now();
  while (Date.now() - started < deadlineMs) {
    if (terminal.shellIntegration?.cwd) return true;
    await delay(50);
  }
  return false;
}

async function publish(coordination: string, name: string, value: unknown): Promise<void> {
  const finalPath = path.join(coordination, name);
  const temporary = `${finalPath}.${process.pid}.tmp`;
  await writeFile(temporary, JSON.stringify(value), "utf8");
  await rename(temporary, finalPath);
}

function within<T>(work: Promise<T>, milliseconds: number, label: string): Promise<T> {
  let timer: NodeJS.Timeout | undefined;
  return Promise.race([
    work,
    new Promise<never>((_resolve, reject) => {
      timer = setTimeout(() => reject(new Error(`${label} exceeded ${milliseconds} ms`)), milliseconds);
    }),
  ]).finally(() => {
    if (timer) clearTimeout(timer);
  });
}

function delay(milliseconds: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}

function requiredEnvironment(name: string): string {
  const value = process.env[name];
  if (!value) throw new Error(`${name} is required`);
  return value;
}
