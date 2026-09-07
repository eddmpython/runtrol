import assert from "node:assert/strict";
import test from "node:test";
import {
  FINALIZED_REVISIONS, PUBLIC_LIMITS, RuntimeConnector, RuntimeProtocolError, RuntimeRequestError, RuntimeTransportError,
  TerminalView, newMutationRequestId,
  type RuntimeTransport, type WindowInputBinding,
} from "../src/index.js";
import { ScriptedRuntimeTransport, validatedLocator } from "../src/testing.js";

const instanceId = `rtm_${"8".repeat(32)}`;
const generation = "9".repeat(64);
const subscriptionId = "input-subscription";
const binding: WindowInputBinding = {
  windowSessionId: "window", registrationGeneration: 1, hostGeneration: "host",
  terminalKey: "t1", executionId: "e1", processId: 5,
  terminalId: "019c2b97-5f29-7b00-8000-000000000001",
};
const offered = (sequence: number) => ({
  jsonrpc: "2.0", method: "windows/inputOffered", params: { subscriptionId, sequence, binding },
});

function initial(maxPendingOffers = 2): unknown[] {
  return [
    { jsonrpc: "2.0", method: "runtime/challenge", params: {
      instanceId, nonceId: `nonce_${"0".repeat(32)}`,
      nonce: Buffer.alloc(32).toString("base64url"), expiresAtMs: Date.now() + 30_000,
    } },
    { jsonrpc: "2.0", id: 1, result: {
      selectedRevision: FINALIZED_REVISIONS[0],
      runtime: { instanceId, version: "0.1.1", platform: "fixture", buildDigest: generation },
      serverCapabilities: {
        integrationEnrollment: true, providerInventory: true, managedSessionList: true, modelDiscovery: true,
        nativeSessionCatalogue: true, sessionControl: true, sessionEvents: true, terminalSurface: true,
      }, limits: PUBLIC_LIMITS,
    } },
    { jsonrpc: "2.0", id: 2, result: { subscriptionId, maxPendingOffers } },
  ];
}

async function connect(transport: RuntimeTransport) {
  const runtime = await new RuntimeConnector(async () => transport).connect(
    validatedLocator(instanceId, "fixture", "0.1.1", generation), { name: "owner-fixture", version: "1.0.0" },
  );
  return runtime.windows().watchInput({ windowSessionId: "window", registrationGeneration: 1, ownerToken: "private-owner-token" });
}

test("the owner channel matches replies while preserving bounded content-free offers", async () => {
  const transport = new ScriptedRuntimeTransport([
    ...initial(), offered(1), offered(2),
    { jsonrpc: "2.0", id: 3, result: { text: "fixture\r" } },
    { jsonrpc: "2.0", id: 4, result: {} },
    { jsonrpc: "2.0", method: "windows/inputEnded", params: { subscriptionId, reason: "authorityChanged" } },
  ]);
  const subscription = await connect(transport);
  try {
    assert.deepEqual(await subscription.next(), { kind: "offered", offered: offered(1).params });
    assert.deepEqual(await subscription.claimInput(1), { text: "fixture\r" });
    await subscription.inputReceipt({ sequence: 1, outcome: "ownerExtensionAccepted" });
    assert.deepEqual(await subscription.next(), { kind: "offered", offered: offered(2).params });
    assert.equal((await subscription.next()).kind, "ended");
    await assert.rejects(subscription.claimInput(2), RuntimeTransportError);
    const methods = transport.sent.map((bytes) => JSON.parse(Buffer.from(bytes).toString()).method);
    assert.equal(methods.filter((method) => method === "windows/claimInput").length, 1);
    assert.equal(methods.filter((method) => method === "windows/inputReceipt").length, 1);
  } finally { subscription.close(); }
});

test("a typed claim refusal preserves the receiver for the following offer", async () => {
  const transport = new ScriptedRuntimeTransport([
    ...initial(), offered(1),
    { jsonrpc: "2.0", id: 3, error: { code: "controlConflict", message: "lease changed", retryable: false, correlationId: "claim" } },
    offered(2), { jsonrpc: "2.0", id: 4, result: { text: "next input" } },
    { jsonrpc: "2.0", id: 5, result: {} },
  ]);
  const subscription = await connect(transport);
  try {
    assert.equal((await subscription.next()).kind, "offered");
    await assert.rejects(subscription.claimInput(1), RuntimeRequestError);
    assert.deepEqual(await subscription.next(), { kind: "offered", offered: offered(2).params });
    assert.deepEqual(await subscription.claimInput(2), { text: "next input" });
    await subscription.inputReceipt({ sequence: 2, outcome: "ownerExtensionAccepted" });
    const receipts = transport.sent.map((bytes) => JSON.parse(Buffer.from(bytes).toString()))
      .filter((request) => request.method === "windows/inputReceipt");
    assert.deepEqual(receipts.map((request) => request.params.sequence), [2]);
  } finally { subscription.close(); }
});

test("an owner queue overflow, wrong target or mismatched response ends the channel explicitly", async () => {
  const wrongTarget = offered(1);
  wrongTarget.params.subscriptionId = "different";
  for (const frames of [
    [offered(1), offered(2), offered(3)],
    [wrongTarget],
    [{ jsonrpc: "2.0", id: 9, result: { text: "unmatched" } }],
  ]) {
    const subscription = await connect(new ScriptedRuntimeTransport([...initial(), ...frames]));
    await assert.rejects(subscription.claimInput(1), RuntimeProtocolError);
    await assert.rejects(subscription.next(), RuntimeTransportError);
  }
});

test("a pending owner read excludes another reader or command and close releases it", async () => {
  const scripted = new ScriptedRuntimeTransport(initial());
  let receives = 0;
  let reject!: (error: Error) => void;
  const transport: RuntimeTransport = {
    send: (bytes) => scripted.send(bytes),
    receive: () => ++receives <= 3 ? scripted.receive() : new Promise((_resolve, no) => { reject = no; }),
    close: () => { scripted.close(); reject?.(new RuntimeTransportError("closed")); },
  };
  const subscription = await connect(transport);
  const reading = subscription.next();
  await assert.rejects(subscription.next(), /active operation/);
  await assert.rejects(subscription.claimInput(1), /active operation/);
  subscription.close();
  await assert.rejects(reading, RuntimeTransportError);
});

test("a claim response lost in transport is never retried by the SDK", async () => {
  const scripted = new ScriptedRuntimeTransport(initial());
  let receives = 0;
  const transport: RuntimeTransport = {
    send: (bytes) => scripted.send(bytes),
    receive: async () => { if (++receives > 3) throw new RuntimeTransportError("lost claim response"); return scripted.receive(); },
    close: () => scripted.close(),
  };
  const subscription = await connect(transport);
  await assert.rejects(subscription.claimInput(1), /lost claim response/);
  await assert.rejects(subscription.claimInput(1), /closed/);
  const claims = scripted.sent.map((bytes) => JSON.parse(Buffer.from(bytes).toString()))
    .filter((request) => request.method === "windows/claimInput");
  assert.equal(claims.length, 1);
});

test("a malformed receipt response retires the owner connection before another offer", async () => {
  const transport = new ScriptedRuntimeTransport([
    ...initial(), { jsonrpc: "2.0", id: 3, result: "not an empty receipt" }, offered(2),
  ]);
  const subscription = await connect(transport);
  try {
    await assert.rejects(subscription.inputReceipt({ sequence: 1, outcome: "ownerExtensionAccepted" }), RuntimeProtocolError);
    await assert.rejects(subscription.next(), RuntimeTransportError);
  } finally { subscription.close(); }
});

test("the owner queue negotiation cannot enlarge the public receive bound", async () => {
  for (const bound of [0, 1.5, PUBLIC_LIMITS.maxTerminalViewQueueChunks + 1]) {
    await assert.rejects(connect(new ScriptedRuntimeTransport(initial(bound))), RuntimeProtocolError);
  }
});

test("the text operation returns only its own structural receipt without falling back to byte input", async () => {
  for (const differentRequest of [false, true]) {
    const requestId = newMutationRequestId();
    const receipt = {
      requestId: differentRequest ? newMutationRequestId() : requestId,
      deliverySequence: 1, ownerRegistrationGeneration: 1, outcome: "ownerExtensionAccepted" as const,
    };
    const transport = new ScriptedRuntimeTransport([
      ...initial().slice(0, 2), { jsonrpc: "2.0", id: 2, result: receipt },
    ]);
    const runtime = await new RuntimeConnector(async () => transport).connect(
      validatedLocator(instanceId, "fixture", "0.1.1", generation), { name: "viewer-fixture", version: "1.0.0" },
    );
    const view = new TerminalView(runtime, transport, {
      terminal: { terminalId: binding.terminalId, runtimeGeneration: generation, terminalGeneration: 1,
        providerId: "fixture", workspace: "C:\\work", processState: "running", openedAtMs: 1,
        origin: "observedMirror", geometry: { columns: 80, rows: 24 } },
      viewId: "019c2b97-5f29-7b00-8000-000000000002", screenBase64: "",
    });
    try {
      const sent = view.sendText({ requestId, terminalId: binding.terminalId,
        leaseId: "lease", leaseGeneration: 1, text: "fixture\r" });
      if (differentRequest) await assert.rejects(sent, RuntimeProtocolError);
      else assert.deepEqual(await sent, receipt);
      const requests = transport.sent.map((bytes) => JSON.parse(Buffer.from(bytes).toString()));
      assert.equal(requests.filter((request) => request.method === "terminals/sendText").length, 1);
      assert.equal(requests.filter((request) => request.method === "terminals/write").length, 0);
    } finally { view.close(); }
  }
});
