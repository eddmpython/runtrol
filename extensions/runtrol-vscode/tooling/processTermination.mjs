import { spawnSync } from "node:child_process";
import path from "node:path";
import { fileURLToPath } from "node:url";

// The native helper holds the handle from identity validation through the bounded exit wait.
// A second snapshot followed by process.kill(pid) would reopen the PID-reuse race.
export function terminateWindowsProcesses(identities, waitMs) {
  if (process.platform !== "win32") throw new Error("Windows process handles are required");
  if (!Number.isSafeInteger(waitMs) || waitMs < 0) throw new Error("a finite process wait is required");
  for (const identity of identities) {
    if (!Number.isSafeInteger(identity.pid) || identity.pid <= 0 || identity.pid === process.pid
      || !Number.isSafeInteger(identity.startedAt) || identity.startedAt <= 0
      || typeof identity.executable !== "string" || !path.isAbsolute(identity.executable)) {
      throw new Error("process termination requires a known PID, birth, and absolute image path");
    }
  }
  if (identities.length === 0) return [];
  const result = spawnSync("powershell.exe", ["-NoProfile", "-NonInteractive", "-File",
    fileURLToPath(new URL("./terminateProcesses.ps1", import.meta.url))], {
    input: JSON.stringify({ identities: identities.map(({ pid, startedAt, executable }) => ({ pid, startedAt, executable })),
      protectedPid: process.pid, waitMs }),
    encoding: "utf8",
    windowsHide: true,
    timeout: waitMs + 15_000,
  });
  if (result.error || result.status !== 0) {
    // The helper never echoes its input, process command lines, or executable paths.
    throw new Error(`cannot terminate exact Windows processes: ${result.error?.message ?? result.stderr.trim()}`);
  }
  const outcomes = JSON.parse(result.stdout);
  if (!Array.isArray(outcomes) || outcomes.length !== identities.length
    || outcomes.some((outcome, index) => outcome.pid !== identities[index].pid
      || !["absent", "exited", "mismatch", "pending", "failed"].includes(outcome.state))) {
    throw new Error("invalid exact Windows process termination result");
  }
  const failed = outcomes.filter((outcome) => outcome.state === "failed");
  if (failed.length > 0) {
    throw new Error(`cannot prove Windows process termination: ${failed.map((outcome) => `${outcome.pid} (${outcome.error})`).join(", ")}`);
  }
  return identities.filter((_, index) => outcomes[index].state === "pending");
}
