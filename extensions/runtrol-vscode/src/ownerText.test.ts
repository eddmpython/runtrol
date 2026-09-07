import assert from "node:assert/strict";
import test from "node:test";
import { PUBLIC_LIMITS, RuntimeRequestError, RuntimeTransportError, type WindowInputBinding } from "@runtrol/runtime-client";
import { OwnerText, type OwnerReceipt } from "./ownerText";

const binding: WindowInputBinding = {
  windowSessionId: "window", registrationGeneration: 1, hostGeneration: "host",
  terminalKey: "t1", executionId: "e1", terminalId: "terminal", processId: 5,
};
const offer = (sequence = 1) => ({ subscriptionId: "subscription", sequence, binding });

function fixture() {
  const writes: { text: string; shouldExecute: boolean }[] = [];
  const receipts: OwnerReceipt[] = [];
  const target = { sendText(text: string, shouldExecute: boolean) { writes.push({ text, shouldExecute }); } };
  const state = { current: target as typeof target | null, revoked: false, lost: false };
  const port = {
    current: () => state.current,
    authorize: async () => {
      if (state.revoked) throw new RuntimeRequestError({ code: "scopeDenied", correlationId: "claim", message: "revoked", retryable: false });
      return { text: "fixture\r" };
    },
    complete: async (receipt: OwnerReceipt) => { if (state.lost) throw new Error("receipt connection lost"); receipts.push(receipt); },
  };
  return { writes, receipts, state, port, owner: new OwnerText(port) };
}

test("a confirmed owner call executes once without adding Enter", async () => {
  const f = fixture();
  await f.owner.receive(offer());
  await assert.rejects(f.owner.receive(offer()), /sequence is stale/);
  assert.deepEqual(f.writes, [{ text: "fixture\r", shouldExecute: false }]);
  assert.deepEqual(f.receipts, [{ sequence: 1, outcome: "ownerExtensionAccepted" }]);
});

test("a lost receipt cannot authorize the same owner call again but later input works", async () => {
  const f = fixture();
  f.state.lost = true;
  await assert.rejects(f.owner.receive(offer()), /receipt connection lost/);
  f.state.lost = false;
  await assert.rejects(f.owner.receive(offer()), /sequence is stale/);
  assert.equal(f.writes.length, 1);
  await f.owner.receive(offer(2));
  assert.equal(f.writes.length, 2);
});

test("a refused claim leaves the owner ready for the next input without a second receipt", async () => {
  const f = fixture();
  f.state.revoked = true;
  const complete = f.port.complete;
  f.port.complete = async (receipt) => {
    assert.equal(f.state.revoked, false, "a refused claim already released its pending receipt");
    await complete(receipt);
  };
  await f.owner.receive(offer());
  assert.equal(f.writes.length, 0);
  assert.equal(f.receipts.length, 0);
  f.state.revoked = false;
  await f.owner.receive(offer(2));
  assert.equal(f.writes.length, 1);
  assert.equal(f.receipts[0]?.sequence, 2);
});

test("a lost claim propagates its unknown transport outcome without a false refusal receipt", async () => {
  const f = fixture();
  f.port.authorize = async () => { throw new RuntimeTransportError("claim transport lost"); };
  await assert.rejects(f.owner.receive(offer()), RuntimeTransportError);
  assert.equal(f.writes.length, 0);
  assert.equal(f.receipts.length, 0);
  await assert.rejects(f.owner.receive(offer()), /sequence is stale/);
});

test("a changed execution or owner close during authorization prevents input and bounds pending work", async () => {
  for (const close of [false, true]) {
    const f = fixture();
    let authorize!: (value: { text: string }) => void;
    f.port.authorize = () => new Promise((resolve) => { authorize = resolve; });
    const receiving = f.owner.receive(offer());
    await assert.rejects(f.owner.receive(offer(2)), /unavailable/);
    if (close) f.owner.close();
    else f.state.current = null;
    authorize({ text: "never sent" });
    await receiving;
    assert.equal(f.writes.length, 0);
    assert.equal(f.receipts[0]?.reason, close ? "receiverClosed" : "executionChanged");
  }
});

test("the byte ceiling applies to encoded Unicode text", async () => {
  const f = fixture();
  f.port.authorize = async () => ({ text: "가".repeat(Math.ceil(PUBLIC_LIMITS.maxTerminalWriteBytes / 3)) });
  await f.owner.receive(offer());
  assert.equal(f.writes.length, 0);
  assert.equal(f.receipts[0]?.reason, "textTooLarge");
});

test("a throwing input API reports unknown and never retries the invocation", async () => {
  const f = fixture();
  let calls = 0;
  f.state.current!.sendText = () => { calls += 1; throw new Error("API failed after forwarding"); };
  await f.owner.receive(offer());
  assert.equal(f.receipts[0]?.outcome, "outcomeUnknown");
  assert.equal(f.receipts[0]?.reason, "inputApiFailed");
  await assert.rejects(f.owner.receive(offer()), /sequence is stale/);
  assert.equal(calls, 1);
});
