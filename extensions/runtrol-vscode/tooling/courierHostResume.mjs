import assert from "node:assert/strict";
import { lstat, open, readFile, realpath, rm } from "node:fs/promises";
import path from "node:path";

import { normalizedExecutable } from "./process-identity.mjs";

export async function retainedHostRoot(requested, executionRoot) {
  assert.ok(typeof requested === "string" && path.isAbsolute(requested), "--resume requires an absolute retained host directory");
  const root = await realpath(executionRoot);
  const temporary = await realpath(requested);
  assert.ok(samePath(requested, temporary) && samePath(path.dirname(temporary), root)
    && /^runtrolProvider-[A-Za-z0-9]+$/u.test(path.basename(temporary)),
  "--resume must name an original provider host directory inside the execution root");
  await plainPath(temporary, "");
  return temporary;
}

// A concurrent runner must never pass the ended-owner check while another is replacing the same binary.
// A claim left by an abruptly killed launcher is refused for explicit ownership inspection, not guessed stale.
export async function claimHostRun(temporary) {
  const file = path.join(temporary, "host.claim");
  let claim;
  try { claim = await open(file, "wx"); }
  catch (error) {
    if (error.code === "EEXIST") throw new Error("the retained host is claimed; inspect its launcher before reclaiming it");
    throw error;
  }
  try { await claim.writeFile(JSON.stringify({ pid: process.pid, claimedAtMs: Date.now() })); }
  catch (error) { await claim.close(); await rm(file); throw error; }
  return async () => { await claim.close(); await rm(file); };
}

export async function readRetainedHost(temporary, rows) {
  await plainPath(temporary, "host.json");
  const previous = JSON.parse(await readFile(path.join(temporary, "host.json"), "utf8"));
  const expected = {
    temporary, home: path.join(temporary, "runtrol"), core: path.join(temporary, "bin", "runtrol.exe"),
    probe: path.join(temporary, "bin", "handoverProbe.exe"), identity: path.join(temporary, "identity.json"),
  };
  for (const [key, value] of Object.entries(expected)) {
    assert.ok(typeof previous[key] === "string" && samePath(previous[key], value), "the retained host descriptor has a different owned path");
  }
  assert.ok(typeof previous.coordination === "string" && samePath(path.dirname(previous.coordination), temporary)
    && /^coordination(?:-[A-Za-z0-9]+)?$/u.test(path.basename(previous.coordination)), "the retained coordination path is outside its host");
  for (const relative of ["runtrol", "profile", "bin/runtrol.exe", "bin/handoverProbe.exe", "extension", "identity.json",
    path.basename(previous.coordination)]) await plainPath(temporary, relative);
  // Copy destinations and profile settings may already exist. A link must not redirect a recovery write.
  for (const relative of ["extension/dist", "extension/resources", "extension/package.json", "profile/User",
    "profile/User/settings.json", "extensions"]) await plainPath(temporary, relative, true);
  assert.ok(typeof previous.workspace === "string" && path.isAbsolute(previous.workspace)
    && samePath(await realpath(previous.workspace), previous.workspace), "the retained project path changed");
  assert.ok(Array.isArray(previous.processes) && previous.processes.length === 2
    && ["runtime", "viewer"].every((label) => previous.processes.filter((entry) => entry.label === label).length === 1),
  "the retained host does not record both owned process identities");
  for (const identity of previous.processes) {
    assert.ok(Number.isSafeInteger(identity.pid) && identity.pid > 0
      && Number.isFinite(identity.startedAt) && identity.startedAt > 0
      && typeof identity.executable === "string" && identity.executable.length > 0,
    "the retained host process identity is incomplete");
    const current = rows.find((row) => row.pid === identity.pid);
    assert.ok(!current || (Number.isFinite(current.startedAt) && current.startedAt !== identity.startedAt),
      "a recorded provider host process is still alive or cannot be proven ended");
  }
  const runtime = previous.processes.find((entry) => entry.label === "runtime");
  const viewer = previous.processes.find((entry) => entry.label === "viewer");
  assert.ok(samePath(runtime.executable, expected.core), "the recorded Runtime is outside its host");
  const marker = normalize(temporary);
  const profile = normalize(path.join(temporary, "profile"));
  // The invoking shell can contain the literal --resume argument too. Only our current ancestry is exempt
  // from that marker check. Each parent birth must precede its child's birth; stale parent PIDs are not proof.
  // An actual Runtime image or retained viewer using its profile remains a conflicting owner in every case.
  const launchers = new Set();
  let launcher = rows.find((row) => row.pid === process.pid);
  while (launcher && !launchers.has(launcher.pid)) {
    launchers.add(launcher.pid);
    const parent = rows.find((row) => row.pid === launcher.ppid);
    if (!parent || !Number.isFinite(parent.startedAt) || parent.startedAt <= 0
      || !Number.isFinite(launcher.startedAt) || launcher.startedAt <= 0
      || parent.startedAt > launcher.startedAt) break;
    launcher = parent;
  }
  launchers.add(process.pid);
  assert.ok(!rows.some((row) => samePath(row.executable || "", expected.core)
    || (samePath(row.executable || "", viewer.executable) && normalize(row.command || "").includes(profile))
    || (!launchers.has(row.pid) && normalize(row.command || "").includes(marker))),
  "a process still uses the retained provider host paths");
  // Old descriptors did not record their launcher. Refuse an occupied common parent PID too: it may still
  // be unwinding cleanup even though both child roots have already ended.
  const parents = new Set(previous.processes.map((entry) => entry.ppid));
  if (parents.size === 1) {
    const parentPid = [...parents][0];
    const parent = rows.find((row) => row.pid === parentPid && row.pid !== process.pid);
    const firstChild = Math.min(...previous.processes.map((entry) => entry.startedAt));
    assert.ok(!parent || (Number.isFinite(parent.startedAt) && parent.startedAt > firstChild),
      "the previous provider host launcher has not been proven ended");
  }
  return previous;
}

async function plainPath(temporary, relative, optional = false) {
  let current = temporary;
  const parts = relative === "" ? [] : relative.split(/[\\/]/u);
  for (const part of ["", ...parts]) {
    current = path.join(current, part);
    let entry;
    try { entry = await lstat(current); }
    catch (error) {
      if (optional && error.code === "ENOENT") return;
      throw error;
    }
    assert.ok(!entry.isSymbolicLink(), "retained host paths must not contain symbolic links");
  }
}

function samePath(left, right) {
  return normalizedExecutable(left) === normalizedExecutable(right);
}

function normalize(value) {
  return process.platform === "win32" ? value.replaceAll("/", "\\").toLocaleLowerCase("en-US") : value;
}
