import assert from "node:assert/strict";
import path from "node:path";
import test from "node:test";
import { descendantPids, ownedProcessRoots } from "./process-identity.mjs";

test("lineage excludes unknown births and children preceding a reused parent PID", () => {
  const rows = [
    { pid: 10, ppid: 1, startedAt: 20_000 },
    { pid: 11, ppid: 10, startedAt: 20_000 },
    { pid: 12, ppid: 10, startedAt: 19_999 },
    { pid: 13, ppid: 10, startedAt: null },
    { pid: 14, ppid: 10, startedAt: 0 },
    { pid: 15, ppid: 12, startedAt: 20_001 },
    { pid: 16, ppid: 13, startedAt: 20_001 },
  ];
  assert.deepEqual([...descendantPids(rows, 10)], [11]);
  assert.deepEqual([...descendantPids([{ pid: 10, startedAt: null }, rows[1]], 10)], []);
});

test("a shared editor image cannot authorize cleanup of another profile", () => {
  const marker = path.resolve("owned", "profile");
  const image = path.resolve("installed", "Code.exe");
  const rows = [
    { pid: 1, executable: image, command: `${image} --user-data-dir=${marker}` },
    { pid: 2, executable: image, command: `${image} --user-data-dir=${marker}-other` },
    { pid: 3, executable: image, command: `${image} --user-data-dir=${path.resolve("operator", "profile")}` },
  ];
  assert.deepEqual(ownedProcessRoots(rows, marker, image).map((row) => row.pid), [1]);
});

test("an exact private Runtime image below the owned root needs no profile argument", () => {
  const marker = path.resolve("owned", "task");
  const image = path.join(marker, "core", "runtrol.exe");
  const ours = { pid: 1, executable: image, command: "runtrol daemon" };
  const other = { pid: 2, executable: path.resolve("operator", "runtrol.exe"), command: "runtrol daemon" };
  assert.deepEqual(ownedProcessRoots([ours, other], marker, image), [ours]);
});
