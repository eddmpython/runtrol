import assert from "node:assert/strict";
import { test } from "node:test";

import {
  FINALIZED_REVISIONS,
  PUBLIC_LIMITS,
  RuntimeConnector,
  RuntimeProtocolError,
  RuntimeTransportError,
} from "../src/index.js";
import {
  ScriptedRuntimeTransport,
  validatedLocator,
} from "../src/testing.js";

class DisconnectingTransport extends ScriptedRuntimeTransport {
  public override async receive(): Promise<Uint8Array> {
    try {
      return await super.receive();
    } catch (error) {
      throw new RuntimeTransportError("fixture Runtime disconnected", { cause: error });
    }
  }
}

function successfulTransport(instanceId: string): ScriptedRuntimeTransport {
  return new ScriptedRuntimeTransport([
    {
      jsonrpc: "2.0",
      method: "runtime/challenge",
      params: {
        instanceId,
        nonceId: `nonce_${"0".repeat(32)}`,
        nonce: Buffer.alloc(32).toString("base64url"),
        expiresAtMs: Date.now() + 30_000,
      },
    },
    {
      jsonrpc: "2.0",
      id: 1,
      result: {
        selectedRevision: FINALIZED_REVISIONS[0],
        runtime: { instanceId, version: "0.1.1", platform: "fixture" },
        serverCapabilities: {
          integrationEnrollment: true,
          providerInventory: true,
          managedSessionList: true,
          modelDiscovery: true,
          nativeSessionCatalogue: true,
          sessionControl: true,
          sessionEvents: true,
        },
        limits: PUBLIC_LIMITS,
      },
    },
  ]);
}

test("connection retry uses bounded backoff only for transient transport failures", async () => {
  const instanceId = `rtm_${"8".repeat(32)}`;
  const transport = successfulTransport(instanceId);
  let attempts = 0;
  const connector = new RuntimeConnector(async () => {
    attempts += 1;
    if (attempts < 3) throw new RuntimeTransportError("fixture Runtime is restarting");
    return transport;
  });
  const runtime = await connector.connectWithRetry(
    validatedLocator(instanceId, "fixture", "0.1.1"),
    { name: "fixture", version: "1.0.0" },
    { initialDelayMs: 1, maximumDelayMs: 2, deadlineMs: 100 },
  );
  assert.equal(attempts, 3);
  runtime.close();
});

test("connection retry returns a protocol failure without a second attempt", async () => {
  const instanceId = `rtm_${"9".repeat(32)}`;
  let attempts = 0;
  const connector = new RuntimeConnector(async () => {
    attempts += 1;
    throw new RuntimeProtocolError("fixture contract violation");
  });
  await assert.rejects(
    connector.connectWithRetry(
      validatedLocator(instanceId, "fixture", "0.1.1"),
      { name: "fixture", version: "1.0.0" },
      { initialDelayMs: 1, maximumDelayMs: 2, deadlineMs: 100 },
    ),
    RuntimeProtocolError,
  );
  assert.equal(attempts, 1);
});

test("event reconnect resumes only from the last explicitly accepted cursor", async () => {
  const instanceId = `rtm_${"a".repeat(32)}`;
  const sessionId = "019c2b97-5f29-7b00-8000-000000000001";
  const firstCursor = {
    stream: "019c2b97-5f29-7b00-8000-000000000002",
    epoch: 1,
    seq: 4,
  };
  const nextCursor = { ...firstCursor, seq: 5 };
  const secondCursor = { ...firstCursor, epoch: 2, seq: 0 };
  const first = new DisconnectingTransport([
    ...framesForInitialization(instanceId),
    {
      jsonrpc: "2.0",
      id: 2,
      result: {
        subscriptionId: "sub_first",
        sessionId,
        startsAt: firstCursor,
        liveAt: firstCursor,
      },
    },
    {
      jsonrpc: "2.0",
      method: "sessions/event",
      params: {
        subscriptionId: "sub_first",
        sessionId,
        eventRevision: FINALIZED_REVISIONS[0],
        event: { fixture: true },
        nextExpected: nextCursor,
      },
    },
  ]);
  const second = new DisconnectingTransport([
    ...framesForInitialization(instanceId),
    {
      jsonrpc: "2.0",
      id: 2,
      result: {
        subscriptionId: "sub_second",
        sessionId,
        startsAt: secondCursor,
        liveAt: secondCursor,
        gap: { requested: nextCursor, liveAt: secondCursor },
      },
    },
  ]);
  const transports = [first, second];
  const connector = new RuntimeConnector(async () => {
    const transport = transports.shift();
    if (!transport) throw new RuntimeTransportError("fixture transports exhausted");
    return transport;
  });
  const subscription = await connector.watchEventsWithReconnect(
    validatedLocator(instanceId, "fixture", "0.1.1"),
    { name: "fixture", version: "1.0.0" },
    { sessionId, after: firstCursor },
    { initialDelayMs: 1, maximumDelayMs: 2, deadlineMs: 100 },
  );
  const event = await subscription.next();
  assert.equal(event.kind, "event");
  await assert.rejects(subscription.next(), /accept the current event/);
  if (event.kind !== "event") throw new Error("fixture event disappeared");
  subscription.accept(event.event.nextExpected);
  const reconnected = await subscription.next();
  assert.equal(reconnected.kind, "reconnected");
  if (reconnected.kind !== "reconnected") throw new Error("fixture reconnect disappeared");
  assert.deepEqual(reconnected.started.gap, { requested: nextCursor, liveAt: secondCursor });
  const watchRequest = JSON.parse(new TextDecoder().decode(second.sent[2])) as {
    params: { after: unknown };
  };
  assert.deepEqual(watchRequest.params.after, nextCursor);
  subscription.close();
});

function framesForInitialization(instanceId: string): unknown[] {
  return [
    {
      jsonrpc: "2.0",
      method: "runtime/challenge",
      params: {
        instanceId,
        nonceId: `nonce_${"0".repeat(32)}`,
        nonce: Buffer.alloc(32).toString("base64url"),
        expiresAtMs: Date.now() + 30_000,
      },
    },
    {
      jsonrpc: "2.0",
      id: 1,
      result: {
        selectedRevision: FINALIZED_REVISIONS[0],
        runtime: { instanceId, version: "0.1.1", platform: "fixture" },
        serverCapabilities: {
          integrationEnrollment: true,
          providerInventory: true,
          managedSessionList: true,
          modelDiscovery: true,
          nativeSessionCatalogue: true,
          sessionControl: true,
          sessionEvents: true,
        },
        limits: PUBLIC_LIMITS,
      },
    },
  ];
}
