import assert from "node:assert/strict";
import test from "node:test";
import { captureMirrorView } from "./observedMirrorCapture.mjs";

function view(events) {
  const state = { closed: 0 };
  return { state, opened: { terminal: { origin: "observedMirror", ownerWindowSessionId: "owner", ownerTerminalKey: "t1" } },
    initialScreen: new Uint8Array(4), next: async () => events.shift(), close: () => { state.closed += 1; } };
}

test("real-provider observation counts all bytes without retaining or projecting payload", async () => {
  const fixture = view([{ kind: "output", bytes: Buffer.alloc(300_000, 65) }, { kind: "output", bytes: Buffer.from("provider text") },
    { kind: "lagged" }, { kind: "exited", exitCode: 0 }]);
  const result = await captureMirrorView(fixture, 1_000);
  assert.equal(result.liveBytes, 300_013);
  assert.equal(result.chunks, 2);
  assert.equal(result.lagged, 1);
  assert.equal(result.exited, 0);
  assert.equal("fixtureHex" in result, false);
  assert.equal("headHex" in result, false);
  assert.equal(JSON.stringify(result).includes("provider text"), false);
  assert.equal(fixture.state.closed, 1);
});

test("only a synthetic fixture permits finite content comparison", async () => {
  const fixture = view([{ kind: "output", bytes: Buffer.from("synthetic") }, { kind: "exited", exitCode: 0 }]);
  const result = await captureMirrorView(fixture, 1_000, true);
  assert.equal(result.fixtureHex, Buffer.from("synthetic").toString("hex"));
  const excessive = view([{ kind: "output", bytes: Buffer.alloc(300_000) }]);
  await assert.rejects(captureMirrorView(excessive, 1_000, true), /exceeded its capture bound/);
  assert.equal(excessive.state.closed, 1);
});
