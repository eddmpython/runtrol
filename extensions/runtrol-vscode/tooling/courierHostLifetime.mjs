import { readFile } from "node:fs/promises";
import path from "node:path";

export const HOST_LIFETIME_MS = 60 * 60 * 1_000;
export const HOST_DEADLINE_ENV = "RUNTROL_TEST_HOST_DEADLINE_MS";

export function hostDeadline(value) {
  if (value === undefined) return null;
  const deadline = Number(value);
  if (!/^\d+$/u.test(value) || !Number.isSafeInteger(deadline) || deadline <= 0
    || deadline > Date.now() + HOST_LIFETIME_MS) {
    throw new Error("the provider host deadline must be a bounded absolute timestamp");
  }
  return deadline;
}

// Native work can continue without another inspection command. A shared host deadline, when supplied,
// bounds that idle wait instead of restarting a five-minute step timer. Heartbeats never extend it.
export async function readJourneyStep(coordination, name, idleTimeoutMs, deadlineAtMs = null) {
  const deadline = deadlineAtMs ?? Date.now() + idleTimeoutMs;
  while (Date.now() < deadline) {
    const stop = await optionalObject(path.join(coordination, "stop.json"));
    if (Date.now() >= deadline) break;
    if (stop) return { kind: "done" };
    const step = await optionalObject(path.join(coordination, name));
    if (Date.now() >= deadline) break;
    if (step) return step;
    await delay(25);
  }
  if (deadlineAtMs !== null) throw new Error("the provider host absolute deadline expired while waiting for a step");
  throw new Error(`${name} did not arrive within ${idleTimeoutMs} ms`);
}

export async function waitForHostStop(coordination, processes, deadlineAtMs) {
  while (Date.now() < deadlineAtMs) {
    const stop = await optionalObject(path.join(coordination, "stop.json"));
    if (Date.now() >= deadlineAtMs) break;
    if (stop) return stop.keepEvidence === true;
    if (await optionalObject(path.join(coordination, "viewer-failure.json"))) {
      throw new Error("the viewer journey failed; its structural failure receipt was retained");
    }
    const ended = processes.find(({ child }) => child.exitCode !== null || child.signalCode !== null);
    if (ended) throw new Error(`${ended.label} ended: code ${ended.child.exitCode}, signal ${ended.child.signalCode}`);
    await delay(250);
  }
  throw new Error("the provider host absolute deadline expired");
}

async function optionalObject(file) {
  try {
    const value = JSON.parse(await readFile(file, "utf8"));
    if (value && typeof value === "object" && !Array.isArray(value)) return value;
    throw new Error("a provider host coordination receipt is not a JSON object");
  } catch (error) {
    // Writers may still be publishing a receipt. Neither absence nor partial JSON grants more lifetime.
    if (error.code === "ENOENT" || error instanceof SyntaxError) return null;
    throw error;
  }
}

function delay(milliseconds) {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}
