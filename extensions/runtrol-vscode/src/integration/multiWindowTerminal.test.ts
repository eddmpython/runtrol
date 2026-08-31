import { mkdir, readFile, rename, writeFile } from "node:fs/promises";
import path from "node:path";

import * as vscode from "vscode";

import type { RuntrolExtensionApi } from "../extension";
import type { JourneyTerminal } from "../journeyApi";
import type { JourneyInputTiming } from "../terminalTabs";
import { extensionUnderTest } from "./extensionUnderTest.test";

const OWNER_TEXT = "runtrol-owner-window-input";
const MIRROR_TEXT = "runtrol-mirror-window-input";
const DEADLINE_MS = 30_000;
const INITIALIZATION_DEADLINE_MS = 60_000;
const SAMPLE_COUNT = requiredSampleCount();
const WARM_SAMPLE_INTERVAL_MS = 50;
const NAVIGATION_MODE = process.env.RUNTROL_VSCODE_INPUT_MODE === "navigation";

type Role = "owner" | "mirror";

function requiredSampleCount(): number {
  const raw = requiredEnvironment("RUNTROL_VSCODE_LATENCY_SAMPLE_COUNT");
  if (!/^[1-9][0-9]*$/.test(raw)) {
    throw new Error("RUNTROL_VSCODE_LATENCY_SAMPLE_COUNT must be a positive integer");
  }
  const value = Number(raw);
  if (!Number.isSafeInteger(value) || value < 2) {
    throw new Error("RUNTROL_VSCODE_LATENCY_SAMPLE_COUNT must be a safe integer of at least two");
  }
  return value;
}

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
  if (NAVIGATION_MODE) {
    await delay(8_000);
    await warmNavigationPath(journey, terminal);
  }
  await publish(coordination, "owner-ready.json", terminal);

  const mirror = await readPublished<JourneyTerminal>(coordination, "mirror-armed.json", DEADLINE_MS);
  requireSameTerminal(terminal, mirror);

  const ownerSamples: number[] = [];
  const sampleStarts: number[] = [];
  const ownerInputTimings: JourneyInputTiming[] = [];
  await readPublished(coordination, "mirror-samples-armed.json", DEADLINE_MS);
  for (let index = 0; index < SAMPLE_COUNT; index += 1) {
    const marker = `${OWNER_TEXT}-${index}`;
    const ownerOutput = waitForInputOutput(journey, terminal, marker);
    if (index > 0) await delay(WARM_SAMPLE_INTERVAL_MS);
    // Start at the actual public or direct input call. Every mirror latch was armed before the loop, so no
    // coordination filesystem work competes with the Runtime, PTY, or extension host during a sample.
    const startedAtMs = Date.now();
    sampleStarts.push(startedAtMs);
    const inputTiming = await writeInput(
      journey,
      terminal,
      NAVIGATION_MODE ? "\x1b[B" : marker,
      index > 0,
    );
    if (inputTiming) ownerInputTimings.push(inputTiming);
    const ownerObservedAtMs = await ownerOutput;
    ownerSamples.push(Math.max(0, ownerObservedAtMs - startedAtMs));
    // Finish this independent sample at the second window before starting the next one. Without this barrier,
    // one host scheduling pause delays every marker queued behind it and the same pause is counted as several
    // latency samples. The timestamp is captured before this between-sample file handoff, so coordination I/O
    // never enters the measured duration.
    await readPublished(coordination, `mirror-sample-${index}.json`, DEADLINE_MS);
  }
  const mirrorObserved = await readPublished<{ observations: number[] }>(
    coordination,
    "mirror-samples-observed.json",
    DEADLINE_MS,
  );
  const mirrorSamples = requireMirrorSamples(mirrorObserved.observations, sampleStarts);
  await publish(coordination, "owner-result.json", {
    terminal,
    ownerInputSamplesMs: ownerSamples,
    ownerFirstInputMs: ownerSamples[0],
    ownerWarmInputP95Ms: p95(ownerSamples.slice(1)),
    mirrorFanoutSamplesMs: mirrorSamples,
    mirrorFirstFanoutMs: mirrorSamples[0],
    mirrorWarmFanoutP95Ms: p95(mirrorSamples.slice(1)),
    ownerInputTimings,
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

  await publish(coordination, "mirror-armed.json", terminal);

  const observations = Array.from({ length: SAMPLE_COUNT }, (_, index) =>
    waitForInputOutput(journey, terminal, `${OWNER_TEXT}-${index}`));
  await publish(coordination, "mirror-samples-armed.json", { sampleCount: SAMPLE_COUNT });
  const observed: number[] = [];
  for (let index = 0; index < observations.length; index += 1) {
    const observation = observations[index];
    if (!observation) throw new Error(`mirror sample ${index} was not armed`);
    const observedAtMs = await observation;
    observed.push(observedAtMs);
    await publish(coordination, `mirror-sample-${index}.json`, { observedAtMs });
  }
  await publish(coordination, "mirror-samples-observed.json", {
    observations: observed,
  });

  await readPublished(coordination, "owner-closed.json", DEADLINE_MS);
  const handoffSamples: number[] = [];
  const handoffInputTimings: JourneyInputTiming[] = [];
  for (let index = 0; index < SAMPLE_COUNT; index += 1) {
    const marker = `${MIRROR_TEXT}-${index}`;
    const output = waitForInputOutput(journey, terminal, marker);
    if (index > 0) await delay(WARM_SAMPLE_INTERVAL_MS);
    const started = Date.now();
    const inputTiming = await writeInput(
      journey,
      terminal,
      NAVIGATION_MODE ? "\x1b[A" : marker,
      index > 0,
    );
    if (inputTiming) handoffInputTimings.push(inputTiming);
    const observedAtMs = await output;
    handoffSamples.push(Math.max(0, observedAtMs - started));
  }
  await journey.terminalStop(terminal.runtimeGeneration, terminal.terminalId, DEADLINE_MS);
  await publish(coordination, "mirror-result.json", {
    terminal,
    handoffInputSamplesMs: handoffSamples,
    handoffFirstInputMs: handoffSamples[0],
    handoffWarmInputP95Ms: p95(handoffSamples.slice(1)),
    handoffInputTimings,
    stopAccepted: true,
    vscode: vscode.version,
  });
}

function requireMirrorSamples(observations: number[], starts: number[]): number[] {
  if (!Array.isArray(observations) || observations.length !== SAMPLE_COUNT) {
    throw new Error(`the mirror published ${observations?.length ?? "no"} observations, expected ${SAMPLE_COUNT}`);
  }
  return observations.map((observedAtMs, index) => {
    const startedAtMs = starts[index];
    if (!Number.isFinite(observedAtMs) || startedAtMs === undefined) {
      throw new Error(`mirror sample ${index} has no finite timing`);
    }
    return Math.max(0, observedAtMs - startedAtMs);
  });
}

function waitForInputOutput(
  journey: NonNullable<RuntrolExtensionApi["journey"]>,
  terminal: JourneyTerminal,
  text: string,
): Promise<number> {
  return journey.terminalWaitForOutput(
    terminal.runtimeGeneration,
    terminal.terminalId,
    NAVIGATION_MODE ? "\x1b[" : text,
    DEADLINE_MS,
  );
}

async function warmNavigationPath(
  journey: NonNullable<RuntrolExtensionApi["journey"]>,
  terminal: JourneyTerminal,
): Promise<void> {
  for (const text of ["\x1b[A", "\x1b[B"]) {
    const output = waitForInputOutput(journey, terminal, text);
    await writeInput(journey, terminal, text);
    await output;
  }
}

async function writeInput(
  journey: NonNullable<RuntrolExtensionApi["journey"]>,
  terminal: JourneyTerminal,
  text: string,
  direct = false,
): Promise<JourneyInputTiming | null> {
  if (direct) {
    return journey.terminalWriteDirect(
      terminal.runtimeGeneration,
      terminal.terminalId,
      NAVIGATION_MODE ? text : `${text}\r`,
    );
  }
  if (NAVIGATION_MODE) {
    await vscode.commands.executeCommand("workbench.action.terminal.sendSequence", { text });
    return null;
  }
  return journey.terminalWrite(terminal.runtimeGeneration, terminal.terminalId, text);
}

async function activate(): Promise<RuntrolExtensionApi> {
  const extension = extensionUnderTest<RuntrolExtensionApi>();
  const api = extension.isActive ? extension.exports : await extension.activate();
  try {
    await within(api.ready, INITIALIZATION_DEADLINE_MS, "extension readiness");
  } catch (error) {
    const detail = error instanceof Error ? error.message : String(error);
    throw new Error(
      `extension initialization failed at ${api.initializationStage ?? "unknown"}: ${detail}`,
      { cause: error },
    );
  }
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

function p95(samples: readonly number[]): number {
  if (samples.length === 0) throw new Error("the latency sample set is empty");
  const sorted = [...samples].sort((left, right) => left - right);
  const index = Math.max(0, Math.ceil(sorted.length * 0.95) - 1);
  const measured = sorted[index];
  if (measured === undefined) throw new Error("the latency p95 sample is absent");
  return measured;
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
