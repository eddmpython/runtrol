import assert from "node:assert/strict";
import path from "node:path";
import test from "node:test";

import { LegacyCleanup, legacyCleanupDue, legacyCleanupStamp } from "./legacyCleanup";

const project = path.resolve("project with spaces");

test("legacy cleanup runs the selected Core once with the project as its working directory", async () => {
  const calls: Array<{ executable: string; words: readonly string[]; workspace: string }> = [];
  const cleanup = new LegacyCleanup(async () => path.resolve("managed", "runtrol"), async (executable, words, workspace) => {
    calls.push({ executable, words, workspace });
    return {
      stdout: "legacy-mcp  agent-tools  claude  runtrolTools  -  removed\nlegacy-local  none\n",
      stderr: "",
    };
  });

  const lines = await cleanup.run(project);

  assert.deepEqual(calls, [{
    executable: path.resolve("managed", "runtrol"),
    words: ["legacy", "cleanup"],
    workspace: project,
  }]);
  assert.equal(lines.length, 2);
});

test("a cleanup line the Core did not shape as a legacy report is refused", async () => {
  const cleanup = new LegacyCleanup(
    async () => "runtrol",
    async () => ({ stdout: "done\n", stderr: "" }),
  );
  await assert.rejects(cleanup.run(project), /invalid legacy cleanup line/u);
});

test("a relative project path is refused before the Core runs", async () => {
  let ran = false;
  const cleanup = new LegacyCleanup(async () => "runtrol", async () => {
    ran = true;
    return { stdout: "", stderr: "" };
  });
  await assert.rejects(cleanup.run("relative/project"), /absolute project path/u);
  assert.equal(ran, false);
});

test("legacy cleanup is due once per Core image and once for an unmanaged Core", () => {
  assert.equal(legacyCleanupDue(undefined, "abc123"), true);
  assert.equal(legacyCleanupDue("abc123", "abc123"), false);
  assert.equal(legacyCleanupDue("abc123", "def456"), true);
  assert.equal(legacyCleanupDue(undefined, null), true);
  assert.equal(legacyCleanupDue(legacyCleanupStamp(null), null), false);
});
