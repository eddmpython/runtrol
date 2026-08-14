import assert from "node:assert/strict";
import { test } from "node:test";

import {
  FINALIZED_REVISIONS,
  PUBLIC_LIMITS,
  RuntimeConnector,
  RuntimeProtocolError,
  RuntimeRequestError,
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

class StalledWatchTransport extends ScriptedRuntimeTransport {
  public closed = false;
  readonly #instanceFrames: number;
  #receives = 0;
  #reject: ((reason: RuntimeTransportError) => void) | null = null;

  public constructor(instanceId: string) {
    const frames = framesForInitialization(instanceId);
    super(frames);
    this.#instanceFrames = frames.length;
  }

  public override async receive(): Promise<Uint8Array> {
    this.#receives += 1;
    if (this.#receives <= this.#instanceFrames) return super.receive();
    return new Promise<Uint8Array>((_resolve, reject) => {
      this.#reject = reject;
      if (this.closed) reject(new RuntimeTransportError("fixture watch was closed"));
    });
  }

  public override close(): void {
    this.closed = true;
    super.close();
    this.#reject?.(new RuntimeTransportError("fixture watch was closed"));
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

test("aborting an event watch closes a connection stalled during subscription", async () => {
  const instanceId = `rtm_${"7".repeat(32)}`;
  const transport = new StalledWatchTransport(instanceId);
  const connector = new RuntimeConnector(async () => transport);
  const abort = new AbortController();
  const opening = connector.watchEventsWithReconnect(
    validatedLocator(instanceId, "fixture", "0.1.1"),
    { name: "fixture", version: "1.0.0" },
    { sessionId: "session_fixture" },
    { initialDelayMs: 1, maximumDelayMs: 2, deadlineMs: 100, signal: abort.signal },
  );
  await waitFor(() => transport.sent.length === 3);
  abort.abort(new RuntimeTransportError("fixture selection changed"));
  await assert.rejects(opening, /fixture selection changed/);
  assert.equal(transport.closed, true);
});

test("aborting an event watch cancels a connection attempt before transport exists", async () => {
  const instanceId = `rtm_${"6".repeat(32)}`;
  let connecting = false;
  let cancelled = false;
  const connector = new RuntimeConnector(async (_endpoint, signal) => {
    connecting = true;
    return new Promise((_resolve, reject) => {
      signal?.addEventListener("abort", () => {
        cancelled = true;
        reject(signal.reason);
      }, { once: true });
    });
  });
  const abort = new AbortController();
  const opening = connector.watchEventsWithReconnect(
    validatedLocator(instanceId, "fixture", "0.1.1"),
    { name: "fixture", version: "1.0.0" },
    { sessionId: "session_fixture" },
    { initialDelayMs: 1, maximumDelayMs: 2, deadlineMs: 100, signal: abort.signal },
  );
  await waitFor(() => connecting);
  abort.abort(new RuntimeTransportError("fixture selection changed before connection"));
  await assert.rejects(opening, /fixture selection changed before connection/);
  assert.equal(cancelled, true);
});

test("mutation transport loss returns outcomeUnknown with the original request identity", async () => {
  const instanceId = `rtm_${"d".repeat(32)}`;
  const requestId = "019c2b97-5f29-7b00-8000-000000000004";
  const transport = new DisconnectingTransport(framesForInitialization(instanceId));
  const runtime = await new RuntimeConnector(async () => transport).connect(
    validatedLocator(instanceId, "fixture", "0.1.1"),
    { name: "fixture", version: "1.0.0" },
  );

  await assert.rejects(
    runtime.sessions().submitInput({
      requestId,
      sessionId: "session_fixture",
      leaseId: "lease_fixture",
      leaseGeneration: 4,
      input: "unchanged fixture input",
    }),
    (error: unknown) => {
      assert.ok(error instanceof RuntimeRequestError);
      assert.equal(error.failure.code, "outcomeUnknown");
      assert.equal(error.failure.correlationId, requestId);
      assert.equal(error.failure.retryable, false);
      return true;
    },
  );
  assert.equal(transport.sent.length, 3);
  const sent = JSON.parse(new TextDecoder().decode(transport.sent[2])) as {
    method: string;
    params: { requestId: string };
  };
  assert.equal(sent.method, "sessions/submitInput");
  assert.equal(sent.params.requestId, requestId);
  runtime.close();
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

test("provider snapshot reconnect publishes the replacement snapshot", async () => {
  const instanceId = `rtm_${"b".repeat(32)}`;
  const first = new DisconnectingTransport([
    ...framesForInitialization(instanceId),
    {
      jsonrpc: "2.0",
      id: 2,
      result: { subscriptionId: "providers_first", snapshot: { providers: [] } },
    },
  ]);
  const second = new DisconnectingTransport([
    ...framesForInitialization(instanceId),
    {
      jsonrpc: "2.0",
      id: 2,
      result: {
        subscriptionId: "providers_second",
        snapshot: {
          providers: [{
            providerId: "fixture",
            displayName: "Fixture",
            installation: { state: "usable", version: "1.0.0" },
          }],
        },
      },
    },
  ]);
  const transports = [first, second];
  const connector = new RuntimeConnector(async () => {
    const transport = transports.shift();
    if (!transport) throw new RuntimeTransportError("fixture transports exhausted");
    return transport;
  });
  const subscription = await connector.watchProvidersWithReconnect(
    validatedLocator(instanceId, "fixture", "0.1.1"),
    { name: "fixture", version: "1.0.0" },
    { initialDelayMs: 1, maximumDelayMs: 2, deadlineMs: 100 },
  );
  const reconnected = await subscription.next();
  assert.equal(reconnected.kind, "reconnected");
  if (reconnected.kind !== "reconnected") throw new Error("provider reconnect disappeared");
  assert.equal(reconnected.started.snapshot.providers[0]?.providerId, "fixture");
  subscription.close();
});

test("session index reconnect publishes the replacement authorized snapshot", async () => {
  const instanceId = `rtm_${"c".repeat(32)}`;
  const first = new DisconnectingTransport([
    ...framesForInitialization(instanceId),
    {
      jsonrpc: "2.0",
      id: 2,
      result: {
        subscriptionId: "sessions_first",
        snapshot: { sessions: [], warnings: [] },
      },
    },
  ]);
  const second = new DisconnectingTransport([
    ...framesForInitialization(instanceId),
    {
      jsonrpc: "2.0",
      id: 2,
      result: {
        subscriptionId: "sessions_second",
        snapshot: {
          sessions: [{
            sessionId: "session_fixture",
            providerId: "fixture",
            nativeSessionId: "native_fixture",
            workspace: "C:\\workspace",
            hot: false,
            lifecycle: "cold",
            looksStuck: false,
            sessionGeneration: 2,
          }],
          warnings: [],
        },
      },
    },
  ]);
  const transports = [first, second];
  const connector = new RuntimeConnector(async () => {
    const transport = transports.shift();
    if (!transport) throw new RuntimeTransportError("fixture transports exhausted");
    return transport;
  });
  const subscription = await connector.watchSessionIndexWithReconnect(
    validatedLocator(instanceId, "fixture", "0.1.1"),
    { name: "fixture", version: "1.0.0" },
    { initialDelayMs: 1, maximumDelayMs: 2, deadlineMs: 100 },
  );
  const reconnected = await subscription.next();
  assert.equal(reconnected.kind, "reconnected");
  if (reconnected.kind !== "reconnected") throw new Error("session reconnect disappeared");
  assert.equal(reconnected.started.snapshot.sessions[0]?.workspace, "C:\\workspace");
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

async function waitFor(condition: () => boolean): Promise<void> {
  const deadline = Date.now() + 1_000;
  while (!condition()) {
    if (Date.now() >= deadline) throw new Error("fixture condition did not settle");
    await new Promise((resolve) => setTimeout(resolve, 1));
  }
}
