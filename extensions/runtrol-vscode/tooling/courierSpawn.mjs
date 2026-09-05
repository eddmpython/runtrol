// Real inherited CLI commands exercise the production worker/worktree boundary.
import assert from "node:assert/strict";
import { readFile, stat, writeFile } from "node:fs/promises";
import path from "node:path";

export const spawnBody = "courier-worker-initial-한국어\nopaque delegated task\n";

export async function spawnJourney({ lead, workspace, base, accept, activate, stop, waitFor }) {
  const cli = async (peer, words, body = "", status = 0) => {
    const result = await peer.ask({ kind: "cli", words, body });
    assert.equal(result.status, status, `${words.join(" ")}: ${result.stderr}\n${result.stdout}`);
    return JSON.parse(result.stdout);
  };
  const workers = [];
  for (let index = 0; index < 2; index += 1) {
    const result = await cli(lead.peer, ["spawn", "courier-fixture", "--task", "--timeout", "120"], spawnBody);
    assert.equal(result.answer, "spawned");
    assert.equal(result.spawned_by, lead.session);
    assert.equal(result.base_commit, base);
    assert.notEqual(path.resolve(result.workspace), path.resolve(workspace));
    assert.ok(result.initial.message_id);
    const worker = await accept(result);
    assert.equal(worker.session, result.session);
    const terminal = worker.view.opened.terminal;
    assert.equal(terminal.spawnedBy, lead.session);
    assert.equal(path.resolve(terminal.projectRoot), path.resolve(workspace));
    assert.equal(path.resolve(terminal.workspace), path.resolve(result.workspace));
    assert.equal(terminal.initialMessageId, result.initial.message_id);
    assert.equal(terminal.dialogueEnabled, false);
    assert.equal((await cli(worker.peer, ["inbox"], "", 1)).answer, "refused");
    await activate(worker.view, true);
    const initial = await cli(worker.peer, ["inbox", "--from", lead.session]);
    assert.equal(initial.envelope.message_id, result.initial.message_id);
    assert.equal(initial.envelope.body, spawnBody);
    assert.equal((await cli(worker.peer, ["inbox"], "", 1)).envelope, null);
    assert.equal((await cli(worker.peer, ["spawn", "courier-fixture"], "", 1)).answer, "refused");
    workers.push({ ...worker, workspace: result.workspace });
  }
  assert.equal((await cli(lead.peer, ["spawn", "courier-fixture"], "", 1)).answer, "refused");
  assert.notEqual(workers[0].workspace, workers[1].workspace);
  const changed = path.join(workers[1].workspace, "worker-result.txt");
  await writeFile(changed, "preserve an unfinished worker result\n");
  for (const worker of workers) await stop(worker);
  await waitFor(async () => !(await exists(workers[0].workspace)), "clean worker worktree reclamation");
  assert.equal(await readFile(changed, "utf8"), "preserve an unfinished worker result\n");
  assert.equal(await exists(path.join(workspace, "worker-result.txt")), false);
  process.stdout.write("RUNTROL_COURIER_SPAWN twoWorkers=true isolated=true lineage=true initialDelivery=true activationRequired=true depthRefused=true capacityRefused=true cleanReclaimed=true dirtyPreserved=true originalUntouched=true\n");
}

async function exists(target) {
  try { await stat(target); return true; }
  catch (error) { if (error.code === "ENOENT") return false; throw error; }
}
