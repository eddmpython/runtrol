import assert from "node:assert/strict";
import { mkdir, mkdtemp, readFile, realpath, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test, { after, before } from "node:test";

import { HOST_LIFETIME_MS, hostDeadline, readJourneyStep, waitForHostStop } from "./courierHostLifetime.mjs";

let root;
before(async () => { root = await mkdtemp(path.join(await realpath(os.tmpdir()), "courierHostLifetime-")); });
after(async () => {
  assert.equal(path.dirname(await realpath(root)), await realpath(os.tmpdir()));
  assert.ok(path.basename(root).startsWith("courierHostLifetime-"));
  await rm(root, { recursive: true });
});
let sequence = 0;
async function fixture() {
  const directory = path.join(root, String(++sequence));
  await mkdir(directory);
  return directory;
}
const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

test("a shared host deadline accepts a command after the ordinary idle limit", async () => {
  const directory = await fixture();
  const deadline = Date.now() + 5_000;
  const waiting = readJourneyStep(directory, "viewer-step-1.json", 10, deadline);
  await delay(75);
  await writeFile(path.join(directory, "viewer-step-1.json"), JSON.stringify({ kind: "rows" }));
  assert.deepEqual(await waiting, { kind: "rows" });
  assert.equal(hostDeadline(String(deadline)), deadline);
});

test("heartbeat writes and later waits cannot renew the absolute deadline", async () => {
  const directory = await fixture();
  const deadline = Date.now() + 75;
  const waiting = assert.rejects(readJourneyStep(directory, "viewer-step-1.json", 60_000, deadline), /absolute deadline expired/u);
  await writeFile(path.join(directory, "viewer-alive.json"), '{"waitingFor":1}');
  await waiting;
  await writeFile(path.join(directory, "viewer-step-2.json"), '{"kind":"rows"}');
  await assert.rejects(readJourneyStep(directory, "viewer-step-2.json", 60_000, deadline), /absolute deadline expired/u);
  assert.deepEqual(JSON.parse(await readFile(path.join(directory, "viewer-step-2.json"), "utf8")), { kind: "rows" });
});

test("stop remains responsive while the viewer is idle and explicitly requests retention", async () => {
  const directory = await fixture();
  const deadline = Date.now() + 5_000;
  const viewer = readJourneyStep(directory, "viewer-step-1.json", 10, deadline);
  const host = waitForHostStop(directory, [], deadline);
  await writeFile(path.join(directory, "stop.json"), '{"keepEvidence":true}');
  assert.deepEqual(await viewer, { kind: "done" });
  assert.equal(await host, true);
});

test("expiry and viewer or process failure reject instead of requesting evidence deletion", async () => {
  const directory = await fixture();
  await assert.rejects(waitForHostStop(directory, [], Date.now() - 1), /absolute deadline expired/u);
  await assert.rejects(waitForHostStop(directory, [{ label: "runtime", child: { exitCode: 1, signalCode: null } }], Date.now() + 5_000), /runtime ended: code 1/u);
  await writeFile(path.join(directory, "viewer-failure.json"), '{"failure":"opaque diagnostic sentinel"}');
  await assert.rejects(waitForHostStop(directory, [], Date.now() + 5_000), {
    message: "the viewer journey failed; its structural failure receipt was retained",
  });
});

test("ordinary automated journeys retain their idle limit and malformed shared deadlines fail", async () => {
  const directory = await fixture();
  assert.equal(hostDeadline(undefined), null);
  for (const value of ["", "0", "-1", "not-a-time", "1.5", String(Date.now() + HOST_LIFETIME_MS * 2)]) {
    assert.throws(() => hostDeadline(value), /bounded absolute timestamp/u);
  }
  await assert.rejects(readJourneyStep(directory, "viewer-step-1.json", 10), /did not arrive within 10 ms/u);
});
