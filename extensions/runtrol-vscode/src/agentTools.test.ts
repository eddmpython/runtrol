import assert from "node:assert/strict";
import path from "node:path";
import test from "node:test";

import { AgentToolsController, type AgentToolsRunner } from "./agentTools";

const project = path.resolve("project with spaces");

test("enable passes one exact project word to the selected Core", async () => {
  const calls: Array<{ executable: string; words: readonly string[]; workspace: string }> = [];
  const runner: AgentToolsRunner = async (executable, words, workspace) => {
    calls.push({ executable, words, workspace });
    return {
      stdout: `Agent Tools enabled for ${workspace}\nprovider approvals still require a person in Runtrol\n`,
      stderr: "",
    };
  };
  const tools = new AgentToolsController(async () => path.resolve("managed", "runtrol"), runner);

  const result = await tools.enable(project);

  assert.deepEqual(calls, [{
    executable: path.resolve("managed", "runtrol"),
    words: ["tools", "enable", project],
    workspace: project,
  }]);
  assert.equal(result.workspace, project);
  assert.equal(result.alreadySettled, false);
});

test("disable accepts both a complete revocation and an already settled project", async () => {
  let call = 0;
  const tools = new AgentToolsController(async () => "runtrol", async () => {
    call += 1;
    return call === 1
      ? { stdout: `Agent Tools disabled and Runtime authority revoked for ${project}\n`, stderr: "" }
      : { stdout: `Agent Tools is already disabled for ${project}\n`, stderr: "" };
  });

  assert.equal((await tools.disable(project)).alreadySettled, false);
  assert.equal((await tools.disable(project)).alreadySettled, true);
});

test("a zero exit without the requested settled state is refused", async () => {
  const tools = new AgentToolsController(
    async () => "runtrol",
    async () => ({ stdout: "done\n", stderr: "" }),
  );
  await assert.rejects(tools.enable(project), /did not confirm Agent Tools enable/u);
});

test("a confirmation cannot turn a relative path into visible authority", async () => {
  const tools = new AgentToolsController(
    async () => "runtrol",
    async () => ({ stdout: "Agent Tools enabled for invented/project\n", stderr: "" }),
  );
  await assert.rejects(tools.enable(project), /invalid Agent Tools project path/u);
  assert.equal(tools.enabled(path.resolve("invented/project")), false);
});

test("relative project authority is refused before the Core runs", async () => {
  let ran = false;
  const tools = new AgentToolsController(async () => "runtrol", async () => {
    ran = true;
    return { stdout: "", stderr: "" };
  });
  await assert.rejects(tools.enable("relative/project"), /absolute project path/u);
  assert.equal(ran, false);
});

test("changes serialize so provider configuration cannot race itself", async () => {
  const order: string[] = [];
  let releaseFirst: (() => void) | null = null;
  const firstMayFinish = new Promise<void>((resolve) => {
    releaseFirst = resolve;
  });
  const tools = new AgentToolsController(async () => "runtrol", async (_executable, words, workspace) => {
    const action = words[1] as string;
    order.push(`${action}:start`);
    if (action === "enable") await firstMayFinish;
    order.push(`${action}:end`);
    return action === "enable"
      ? { stdout: `Agent Tools enabled for ${workspace}\n`, stderr: "" }
      : { stdout: `Agent Tools disabled and Runtime authority revoked for ${workspace}\n`, stderr: "" };
  });

  const enabling = tools.enable(project);
  const disabling = tools.disable(project);
  await new Promise((resolve) => setImmediate(resolve));
  assert.deepEqual(order, ["enable:start"]);
  (releaseFirst as (() => void) | null)?.();
  await Promise.all([enabling, disabling]);
  assert.deepEqual(order, ["enable:start", "enable:end", "disable:start", "disable:end"]);
});

test("one list call restores every enabled project and publishes only a changed state", async () => {
  const second = path.resolve("second project");
  let changed = 0;
  const tools = new AgentToolsController(async () => "runtrol", async (_executable, words) => {
    assert.deepEqual(words, ["tools", "list"]);
    return { stdout: `enabled  ${project}\nenabled  ${second}\n`, stderr: "" };
  });
  tools.onDidChange(() => {
    changed += 1;
  });

  await tools.refresh(project);
  assert.equal(tools.enabled(project), true);
  assert.equal(tools.enabled(second), true);
  assert.equal(changed, 1);
  await tools.refresh(project);
  assert.equal(changed, 1);
});

test("an invented list line is refused instead of becoming project authority", async () => {
  const tools = new AgentToolsController(
    async () => "runtrol",
    async () => ({ stdout: "enabled everywhere\n", stderr: "" }),
  );
  await assert.rejects(tools.refresh(project), /invalid Agent Tools project line/u);
  assert.equal(tools.enabled(project), false);
});
