import assert from "node:assert/strict";
import { mkdir, mkdtemp, readFile, realpath, rm, symlink, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test, { after, before } from "node:test";

import { claimHostRun, readRetainedHost, retainedHostRoot } from "./courierHostResume.mjs";

let root;
before(async () => { root = await mkdtemp(path.join(await realpath(os.tmpdir()), "courierHostResume-")); });
after(async () => {
  assert.equal(path.dirname(await realpath(root)), await realpath(os.tmpdir()));
  assert.ok(path.basename(root).startsWith("courierHostResume-"));
  await rm(root, { recursive: true });
});

async function fixture() {
  const temporary = await mkdtemp(path.join(root, "runtrolProvider-"));
  for (const directory of ["runtrol", "profile/User", "project", "bin", "coordination", "extension"]) {
    await mkdir(path.join(temporary, directory), { recursive: true });
  }
  for (const file of ["bin/runtrol.exe", "bin/handoverProbe.exe", "identity.json", "runtrol/registry.json", "profile/User/settings.json"]) {
    await writeFile(path.join(temporary, file), '{"structural":"retained"}');
  }
  const descriptor = {
    temporary, workspace: path.join(temporary, "project"), coordination: path.join(temporary, "coordination"),
    home: path.join(temporary, "runtrol"), core: path.join(temporary, "bin", "runtrol.exe"),
    probe: path.join(temporary, "bin", "handoverProbe.exe"), identity: path.join(temporary, "identity.json"),
    processes: [
      { label: "runtime", pid: 1111, ppid: 1000, startedAt: 101, executable: path.join(temporary, "bin", "runtrol.exe") },
      { label: "viewer", pid: 2222, ppid: 1000, startedAt: 102, executable: path.join(root, "Code.exe") },
    ],
  };
  await writeFile(path.join(temporary, "host.json"), JSON.stringify(descriptor));
  return descriptor;
}

test("same-home recovery preserves the recorded project, profile, identity and worktree registry", async () => {
  const expected = await fixture();
  const temporary = await retainedHostRoot(expected.temporary, root);
  const release = await claimHostRun(temporary);
  try {
    assert.deepEqual(await readRetainedHost(temporary, []), expected);
    assert.deepEqual(JSON.parse(await readFile(path.join(temporary, "host.json"), "utf8")), expected);
    for (const relative of ["runtrol/registry.json", "profile/User/settings.json", "identity.json"]) {
      assert.equal(await readFile(path.join(temporary, relative), "utf8"), '{"structural":"retained"}');
    }
  } finally { await release(); }
});

test("an exact old root, uncertain start time, old launcher or another profile owner refuses recovery", async () => {
  const previous = await fixture();
  const runtime = previous.processes[0];
  for (const rows of [
    [runtime], [{ ...runtime, startedAt: null }],
    [{ pid: 1000, startedAt: 99, executable: "node.exe", command: "node providerHost" }],
    [{ pid: 3333, startedAt: 150, executable: "Code.exe", command: `Code --user-data-dir=${path.join(previous.temporary, "profile")}` }],
  ]) await assert.rejects(readRetainedHost(previous.temporary, rows), /still alive|still uses|not been proven ended/u);
  const reused = { ...runtime, startedAt: 200, executable: path.join(root, "other.exe"), command: "other" };
  assert.deepEqual(await readRetainedHost(previous.temporary, [reused]), previous);
});

test("a conflicting runner claim refuses and releases without deleting retained files", async () => {
  const previous = await fixture();
  const release = await claimHostRun(previous.temporary);
  try { await assert.rejects(claimHostRun(previous.temporary), /host is claimed/u); }
  finally { await release(); }
  const nextRelease = await claimHostRun(previous.temporary);
  await nextRelease();
  assert.deepEqual(await readRetainedHost(previous.temporary, []), previous);
});

test("the invoking shell's resume argument is not mistaken for another profile owner", async () => {
  const previous = await fixture();
  const rows = [
    { pid: process.pid, ppid: 4444, startedAt: 300, executable: "node.exe", command: `node host --resume ${previous.temporary}` },
    { pid: 4444, ppid: 5555, startedAt: 200, executable: "powershell.exe", command: `node host --resume ${previous.temporary}` },
  ];
  assert.deepEqual(await readRetainedHost(previous.temporary, rows), previous);
  rows.push({ pid: 6666, ppid: 4444, executable: "Code.exe", command: `Code --user-data-dir=${path.join(previous.temporary, "profile")}` });
  await assert.rejects(readRetainedHost(previous.temporary, rows), /still uses/u);
});

test("an unknown or recycled parent cannot inherit the launcher's path exemption", async () => {
  const previous = await fixture();
  for (const [childBirth, parentBirth] of [[300, null], [300, 400], [null, 200]]) {
    const rows = [
      { pid: process.pid, ppid: 4444, startedAt: childBirth, executable: "node.exe", command: "node host" },
      { pid: 4444, ppid: 5555, startedAt: parentBirth, executable: "powershell.exe", command: `other --resume ${previous.temporary}` },
    ];
    await assert.rejects(readRetainedHost(previous.temporary, rows), /still uses/u);
  }
});

test("the retained viewer image using its profile refuses even as a proven ancestor", async () => {
  const previous = await fixture();
  const rows = [
    { pid: process.pid, ppid: 4444, startedAt: 300, executable: "node.exe", command: "node host" },
    { pid: 4444, ppid: 5555, startedAt: 200, executable: previous.processes[1].executable,
      command: `Code --user-data-dir=${path.join(previous.temporary, "profile")}` },
  ];
  await assert.rejects(readRetainedHost(previous.temporary, rows), /still uses/u);
});

test("descriptor path substitution and incomplete process identity fail closed", async () => {
  const previous = await fixture();
  for (const changed of [
    { ...previous, home: root },
    { ...previous, coordination: root },
    { ...previous, processes: [{ ...previous.processes[0], startedAt: null }, previous.processes[1]] },
  ]) {
    await writeFile(path.join(previous.temporary, "host.json"), JSON.stringify(changed));
    await assert.rejects(readRetainedHost(previous.temporary, []));
  }
  await assert.rejects(retainedHostRoot(root, root), /original provider host directory/u);
});

test("a linked write destination cannot redirect recovery outside its owned root", async (context) => {
  const previous = await fixture();
  const outside = await mkdtemp(path.join(root, "outside-"));
  try { await symlink(outside, path.join(previous.temporary, "extension", "dist"), process.platform === "win32" ? "junction" : "dir"); }
  catch (error) {
    if (process.platform === "win32" && ["EPERM", "EACCES"].includes(error.code)) {
      context.skip("Windows denied the directory link privilege");
      return;
    }
    throw error;
  }
  await assert.rejects(readRetainedHost(previous.temporary, []), /symbolic links/u);
});
