import { mkdir, readFile, rename, writeFile } from "node:fs/promises";
import path from "node:path";
import { performance } from "node:perf_hooks";

import * as vscode from "vscode";

import type { RuntrolExtensionApi } from "../extension";
import type { JourneyTerminal } from "../journeyApi";
import { extensionUnderTest } from "./extensionUnderTest.test";

const OWNER_TEXT = "runtrol-owner-window-input";
const MIRROR_TEXT = "runtrol-mirror-window-input";
const DEADLINE_MS = 30_000;

type Role = "owner" | "mirror";

export async function run(): Promise<void> {
  const role = requiredRole();
  const coordination = requiredEnvironment("RUNTROL_VSCODE_COORDINATION");
  await mkdir(coordination, { recursive: true });
  try {
    if (role === "owner") await ownerJourney(coordination);
    else await mirrorJourney(coordination);
  } catch (error) {
    await publish(coordination, `${role}-failure.json`, {
      role,
      failure: error instanceof Error ? error.message : String(error),
      stack: error instanceof Error ? error.stack : undefined,
    });
    throw error;
  }
}

async function ownerJourney(coordination: string): Promise<void> {
  const provider = requiredEnvironment("RUNTROL_VSCODE_PROVIDER");
  const workspace = requiredEnvironment("RUNTROL_VSCODE_WORKSPACE");
  const api = await activate();
  const journey = requireJourney(api);
  await waitForUsableProvider(journey, provider);
  const terminal = await journey.terminalStart(provider, workspace, DEADLINE_MS);
  await publish(coordination, "owner-ready.json", terminal);

  const ownerOutput = journey.terminalWaitForOutput(
    terminal.runtimeGeneration,
    terminal.terminalId,
    OWNER_TEXT,
    DEADLINE_MS,
  );
  await publish(coordination, "owner-armed.json", terminal);
  const mirror = await readPublished<JourneyTerminal>(coordination, "mirror-armed.json", DEADLINE_MS);
  requireSameTerminal(terminal, mirror);

  const started = performance.now();
  journey.terminalWrite(terminal.runtimeGeneration, terminal.terminalId, OWNER_TEXT);
  await ownerOutput;
  const ownerInputMs = performance.now() - started;
  await publish(coordination, "owner-observed.json", { ownerInputMs });
  const mirrorObserved = await readPublished<{ mirrorSawOwnerMs: number }>(
    coordination,
    "mirror-observed-owner.json",
    DEADLINE_MS,
  );
  await publish(coordination, "owner-result.json", {
    terminal,
    ownerInputMs,
    mirrorSawOwnerMs: mirrorObserved.mirrorSawOwnerMs,
    vscode: vscode.version,
  });
}

async function mirrorJourney(coordination: string): Promise<void> {
  const provider = requiredEnvironment("RUNTROL_VSCODE_PROVIDER");
  const owner = await readPublished<JourneyTerminal>(coordination, "owner-ready.json", DEADLINE_MS);
  const api = await activate();
  const journey = requireJourney(api);
  await waitForUsableProvider(journey, provider);
  const terminal = await journey.terminalAttach(
    owner.runtimeGeneration,
    owner.terminalId,
    DEADLINE_MS,
  );
  requireSameTerminal(owner, terminal);

  const ownerOutputStarted = performance.now();
  const ownerOutput = journey.terminalWaitForOutput(
    terminal.runtimeGeneration,
    terminal.terminalId,
    OWNER_TEXT,
    DEADLINE_MS,
  );
  const mirrorOutput = journey.terminalWaitForOutput(
    terminal.runtimeGeneration,
    terminal.terminalId,
    MIRROR_TEXT,
    DEADLINE_MS,
  );
  await publish(coordination, "mirror-armed.json", terminal);
  await ownerOutput;
  const mirrorSawOwnerMs = performance.now() - ownerOutputStarted;
  await publish(coordination, "mirror-observed-owner.json", { mirrorSawOwnerMs });

  await readPublished(coordination, "owner-closed.json", DEADLINE_MS);
  const handoffStarted = performance.now();
  journey.terminalWrite(terminal.runtimeGeneration, terminal.terminalId, MIRROR_TEXT);
  await mirrorOutput;
  const mirrorInputAfterHandoffMs = performance.now() - handoffStarted;
  await journey.terminalStop(terminal.runtimeGeneration, terminal.terminalId, DEADLINE_MS);
  await publish(coordination, "mirror-result.json", {
    terminal,
    mirrorSawOwnerMs,
    mirrorInputAfterHandoffMs,
    vscode: vscode.version,
  });
}

async function activate(): Promise<RuntrolExtensionApi> {
  const extension = extensionUnderTest<RuntrolExtensionApi>();
  const api = extension.isActive ? extension.exports : await extension.activate();
  await within(api.ready, DEADLINE_MS, "extension readiness");
  return api;
}

function requireJourney(api: RuntrolExtensionApi): NonNullable<RuntrolExtensionApi["journey"]> {
  if (!api.journey) throw new Error("the installed-host journey API is unavailable");
  return api.journey;
}

async function waitForUsableProvider(
  journey: NonNullable<RuntrolExtensionApi["journey"]>,
  providerId: string,
): Promise<void> {
  const deadline = Date.now() + DEADLINE_MS;
  while (Date.now() < deadline) {
    const provider = journey.providers().find((candidate) => candidate.providerId === providerId);
    if (provider?.installation.state === "usable") return;
    await delay(50);
  }
  throw new Error(`provider ${providerId} did not become usable`);
}

function requireSameTerminal(expected: JourneyTerminal, actual: JourneyTerminal): void {
  if (
    expected.runtimeGeneration !== actual.runtimeGeneration
    || expected.terminalId !== actual.terminalId
    || expected.terminalGeneration !== actual.terminalGeneration
    || expected.providerId !== actual.providerId
    || expected.workspace !== actual.workspace
  ) {
    throw new Error(`the two VS Code windows opened different terminal identities: ${JSON.stringify({ expected, actual })}`);
  }
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

function requiredRole(): Role {
  const role = requiredEnvironment("RUNTROL_VSCODE_ROLE");
  if (role !== "owner" && role !== "mirror") throw new Error(`unknown VS Code role ${role}`);
  return role;
}

function requiredEnvironment(name: string): string {
  const value = process.env[name];
  if (!value) throw new Error(`${name} is required`);
  return value;
}
