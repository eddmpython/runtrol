import { mkdir, readFile, rename, writeFile } from "node:fs/promises";
import path from "node:path";

import * as vscode from "vscode";

import type { RuntrolExtensionApi } from "../extension";
import type { JourneyTerminal } from "../journeyApi";
import { extensionUnderTest } from "./extensionUnderTest.test";

/// The in-window half of the drag-select eye pass (`tooling/drag-select-eye.mjs`).
///
/// A provider that switches mouse reporting on is opened as a Runtrol tab and stood alone in the window. The
/// harness outside then drags across the tab exactly as a hand would, photographs it, and presses Enter. This
/// entry only reports what the window itself can prove: the text the tab selected (copied through the
/// terminal's own copy action) and that the provider echoed the line Enter submitted. Nothing here reads the
/// provider's screen for meaning.
const DEADLINE_MS = 30_000;
const INITIALIZATION_DEADLINE_MS = 60_000;

export async function run(): Promise<void> {
  const coordination = requiredEnvironment("RUNTROL_VSCODE_COORDINATION");
  await mkdir(coordination, { recursive: true });
  try {
    await journey(coordination);
  } catch (error) {
    await publish(coordination, "failure.json", {
      failure: error instanceof Error ? error.message : String(error),
      stack: error instanceof Error ? error.stack : undefined,
    });
    throw error;
  }
}

async function journey(coordination: string): Promise<void> {
  const provider = requiredEnvironment("RUNTROL_VSCODE_PROVIDER");
  const workspace = requiredEnvironment("RUNTROL_VSCODE_WORKSPACE");
  const api = await activate();
  const journeyApi = requireJourney(api);
  await waitForUsableProvider(journeyApi, provider);
  const terminal = await journeyApi.terminalStart(provider, workspace, INITIALIZATION_DEADLINE_MS);
  // The tab alone, edge to edge: no side bar, no panel, no tab strip, so the drag lands on the terminal's own
  // first row wherever the window stands. The profile disables zen mode's full screen and centring.
  await vscode.commands.executeCommand("workbench.action.closeSidebar");
  await vscode.commands.executeCommand("workbench.action.closePanel");
  await vscode.commands.executeCommand("workbench.action.toggleZenMode");
  await delay(1_500);
  await publish(coordination, "ready.json", terminal);

  await readPublished(coordination, "dragged.json", DEADLINE_MS);
  // The terminal's own copy action is the one stable window into what a VS Code terminal selected. The
  // clipboard belongs to the person at the machine, so what was there goes back afterwards.
  const clipboardBefore = await vscode.env.clipboard.readText();
  let selection = "";
  try {
    await vscode.commands.executeCommand("workbench.action.terminal.copySelection");
    await delay(300);
    selection = await vscode.env.clipboard.readText();
  } finally {
    await vscode.env.clipboard.writeText(clipboardBefore);
  }
  // Armed before the outside presses Enter: the echo follows the key within milliseconds.
  const echoed = journeyApi.terminalWaitForOutput(terminal.runtimeGeneration, terminal.terminalId, "echo:", DEADLINE_MS * 2);
  await publish(coordination, "selection.json", { selection });

  await readPublished(coordination, "entered.json", DEADLINE_MS);
  await publish(coordination, "echoed.json", { echoedAtMs: await echoed });

  await readPublished(coordination, "captured.json", DEADLINE_MS);
  await journeyApi.terminalStop(terminal.runtimeGeneration, terminal.terminalId, DEADLINE_MS);
  await publish(coordination, "result.json", { terminal, selection, vscode: vscode.version });
}

async function activate(): Promise<RuntrolExtensionApi> {
  const extension = extensionUnderTest<RuntrolExtensionApi>();
  const api = extension.isActive ? extension.exports : await extension.activate();
  await within(api.ready, INITIALIZATION_DEADLINE_MS, "extension readiness");
  return api;
}

function requireJourney(api: RuntrolExtensionApi): NonNullable<RuntrolExtensionApi["journey"]> {
  if (!api.journey) throw new Error("the installed-host journey API is unavailable");
  return api.journey;
}

async function waitForUsableProvider(
  journeyApi: NonNullable<RuntrolExtensionApi["journey"]>,
  providerId: string,
): Promise<void> {
  const deadline = Date.now() + DEADLINE_MS;
  while (Date.now() < deadline) {
    const provider = journeyApi.providers().find((candidate) => candidate.providerId === providerId);
    if (provider?.installation.state === "usable") return;
    await delay(50);
  }
  throw new Error(`provider ${providerId} did not become usable`);
}

async function publish(coordination: string, name: string, value: unknown): Promise<void> {
  const finalPath = path.join(coordination, name);
  const temporary = `${finalPath}.${process.pid}.tmp`;
  await writeFile(temporary, JSON.stringify(value), "utf8");
  await rename(temporary, finalPath);
}

async function readPublished<T = Record<string, unknown>>(
  coordination: string,
  name: string,
  deadlineMs: number,
): Promise<T> {
  const file = path.join(coordination, name);
  const deadline = Date.now() + deadlineMs;
  while (Date.now() < deadline) {
    try {
      const value: unknown = JSON.parse(await readFile(file, "utf8"));
      if (value && typeof value === "object" && !Array.isArray(value)) return value as T;
      throw new Error(`${name} is not a JSON object`);
    } catch (error) {
      if (!(error instanceof SyntaxError) && (error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
    }
    await delay(25);
  }
  throw new Error(`${name} did not arrive within ${deadlineMs} ms`);
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

export type { JourneyTerminal };
