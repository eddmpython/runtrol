import assert from "node:assert/strict";
import { test } from "node:test";

import {
  FINALIZED_REVISIONS,
  PUBLIC_LIMITS,
  RuntimeConnector,
  RuntimeLocator,
  RuntimeRequestError,
  TerminalClient,
  newMutationRequestId,
} from "../src/index.js";
import {
  ScriptedRuntimeTransport,
  scriptedTransportFactory,
  validatedLocator,
} from "../src/testing.js";

test("terminal control preserves output that arrives before its response", async () => {
  const instanceId = `rtm_${"8".repeat(32)}`;
  const generation = "9".repeat(64);
  const terminalId = "019c2b97-5f29-7b00-8000-000000000001";
  const viewId = "019c2b97-5f29-7b00-8000-000000000002";
  const transport = new ScriptedRuntimeTransport([
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
        runtime: { instanceId, version: "0.1.1", platform: "fixture", buildDigest: generation },
        serverCapabilities: {
          integrationEnrollment: true,
          providerInventory: true,
          managedSessionList: true,
          modelDiscovery: true,
          nativeSessionCatalogue: true,
          sessionControl: true,
          sessionEvents: true,
          terminalSurface: true,
        },
        limits: PUBLIC_LIMITS,
      },
    },
    {
      jsonrpc: "2.0",
      id: 2,
      result: {
        terminal: {
          terminalId,
          runtimeGeneration: generation,
          providerId: "example",
          workspace: "C:\\work",
          processState: "running",
          openedAtMs: Date.now(),
          terminalGeneration: 1,
          geometry: { columns: 100, rows: 30 },
        },
        viewId,
        screenBase64: Buffer.from("screen").toString("base64"),
        controlLease: {
          leaseId: "lease",
          terminalId,
          terminalGeneration: 1,
          leaseGeneration: 1,
          expiresAtMs: Date.now() + 10_000,
        },
      },
    },
    {
      jsonrpc: "2.0",
      method: "terminals/output",
      params: {
        viewId,
        sequence: 1,
        bytesBase64: Buffer.from("exact bytes").toString("base64"),
      },
    },
    { jsonrpc: "2.0", id: 3, result: {} },
  ]);
  const runtime = await new RuntimeConnector(scriptedTransportFactory(transport)).connect(
    validatedLocator(instanceId, "fixture", "0.1.1", generation),
    { name: "terminal-fixture", version: "1.0.0" },
  );
  const view = await runtime.terminals().open({
    requestId: newMutationRequestId(),
    providerId: "example",
    workspace: "C:\\work",
    target: { kind: "fresh" },
    geometry: { columns: 100, rows: 30 },
  });
  assert.equal(Buffer.from(view.initialScreen).toString(), "screen");
  const next = view.next();
  const write = view.write({
    requestId: newMutationRequestId(),
    terminalId,
    leaseId: "lease",
    leaseGeneration: 1,
    bytesBase64: Buffer.from("input").toString("base64"),
  });
  const [output] = await Promise.all([next, write]);
  assert.equal(output.kind, "output");
  if (output.kind === "output") {
    assert.equal(output.sequence, 1);
    assert.equal(Buffer.from(output.bytes).toString(), "exact bytes");
  }
  view.close();
});

test("generation-pinned attach reaches the recorded draining Runtime", async () => {
  const instanceId = `rtm_${"7".repeat(32)}`;
  const generation = "8".repeat(64);
  const terminalId = "019c2b97-5f29-7b00-8000-000000000011";
  const viewId = "019c2b97-5f29-7b00-8000-000000000012";
  const transport = new ScriptedRuntimeTransport([
    {
      jsonrpc: "2.0",
      method: "runtime/challenge",
      params: {
        instanceId,
        nonceId: `nonce_${"1".repeat(32)}`,
        nonce: Buffer.alloc(32, 1).toString("base64url"),
        expiresAtMs: Date.now() + 30_000,
      },
    },
    {
      jsonrpc: "2.0",
      id: 1,
      result: {
        selectedRevision: FINALIZED_REVISIONS[0],
        runtime: { instanceId, version: "0.1.1", platform: "fixture", buildDigest: generation },
        serverCapabilities: {
          integrationEnrollment: true,
          providerInventory: true,
          managedSessionList: true,
          modelDiscovery: true,
          nativeSessionCatalogue: true,
          sessionControl: true,
          sessionEvents: true,
          terminalSurface: true,
        },
        limits: PUBLIC_LIMITS,
      },
    },
    {
      jsonrpc: "2.0",
      id: 2,
      result: {
        terminal: {
          terminalId,
          runtimeGeneration: generation,
          providerId: "example",
          workspace: "C:\\work",
          processState: "running",
          openedAtMs: Date.now(),
          terminalGeneration: 1,
          geometry: { columns: 100, rows: 30 },
        },
        viewId,
        screenBase64: Buffer.from("restored screen").toString("base64"),
      },
    },
  ]);
  const exact = validatedLocator(instanceId, "draining-endpoint", "0.1.1", generation, true);
  const locator = {
    inspectAll: async () => [exact],
  } as unknown as RuntimeLocator;
  const connector = new RuntimeConnector(async (endpoint) => {
    assert.equal(endpoint, "draining-endpoint");
    return transport;
  });

  const view = await TerminalClient.attachInGeneration(
    connector,
    locator,
    { name: "terminal-fixture", version: "1.0.0" },
    generation,
    terminalId,
  );
  assert.equal(Buffer.from(view.initialScreen).toString(), "restored screen");
  assert.equal(view.opened.terminal.runtimeGeneration, generation);
  view.close();
});

test("generation-pinned attach never redirects a vanished terminal", async () => {
  const locator = {
    inspectAll: async () => [],
  } as unknown as RuntimeLocator;
  await assert.rejects(
    TerminalClient.attachInGeneration(
      new RuntimeConnector(),
      locator,
      { name: "terminal-fixture", version: "1.0.0" },
      "7".repeat(64),
      "019c2b97-5f29-7b00-8000-000000000021",
    ),
    (error: unknown) => error instanceof RuntimeRequestError
      && error.failure.code === "terminalGenerationUnavailable",
  );
});
