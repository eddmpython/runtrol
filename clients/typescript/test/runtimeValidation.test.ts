import assert from "node:assert/strict";
import { test } from "node:test";

import {
  FINALIZED_REVISIONS,
  IntegrationIdentity,
  PUBLIC_LIMITS,
  RuntimeConnector,
  RuntimeProtocolError,
  newMutationRequestId,
} from "../src/index.js";
import {
  ScriptedRuntimeTransport,
  scriptedTransportFactory,
  validatedLocator,
} from "../src/testing.js";
import { ValidatedLocator } from "../src/locator.js";

function challenge(instanceId: string): object {
  return {
    jsonrpc: "2.0",
    method: "runtime/challenge",
    params: {
      instanceId,
      nonceId: `nonce_${"0".repeat(32)}`,
      nonce: Buffer.alloc(32).toString("base64url"),
      expiresAtMs: Date.now() + 30_000,
    },
  };
}

function initialized(instanceId: string): object {
  return {
    jsonrpc: "2.0",
    id: 1,
    result: {
      selectedRevision: FINALIZED_REVISIONS[0],
      runtime: {
        instanceId,
        version: "0.1.1",
        platform: "fixture",
      },
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
  };
}

test("hostile Runtime results are rejected by the generated public schema", () => {
  const instanceId = `rtm_${"1".repeat(32)}`;
  assert.throws(
    () => new ValidatedLocator(
      Symbol("forged") as never,
      instanceId,
      "fixture",
      "0.1.1",
    ),
    /not validated by this SDK/,
  );
});

test("an initialized fake transport rejects an unknown provider result field", async () => {
  const instanceId = `rtm_${"2".repeat(32)}`;
  const transport = new ScriptedRuntimeTransport([
    challenge(instanceId),
    initialized(instanceId),
    {
      jsonrpc: "2.0",
      id: 2,
      result: { providers: [], authority: "invented" },
    },
  ]);
  const connector = new RuntimeConnector(scriptedTransportFactory(transport));
  const locator = validatedLocator(instanceId, "fixture", "0.1.1");
  const runtime = await connector.connect(locator, { name: "fixture", version: "1.0.0" });
  await assert.rejects(runtime.providers().list(), RuntimeProtocolError);
  runtime.close();
});

test("native catalogues reject conversation-shaped extension fields", async () => {
  const instanceId = `rtm_${"3".repeat(32)}`;
  const transport = new ScriptedRuntimeTransport([
    challenge(instanceId),
    initialized(instanceId),
    {
      jsonrpc: "2.0",
      id: 2,
      result: {
        providerId: "provider",
        coverage: { kind: "complete", source: "officialProtocol" },
        sessions: [{
          nativeSessionId: "native",
          cwd: "C:/work",
          additionalDirectories: [],
          resume: "available",
          preview: "must not cross",
        }],
      },
    },
  ]);
  const runtime = await new RuntimeConnector(scriptedTransportFactory(transport)).connect(
    validatedLocator(instanceId, "fixture", "0.1.1"),
    { name: "fixture", version: "1.0.0" },
  );
  await assert.rejects(
    runtime.providers().listNativeSessions({ providerId: "provider", root: "C:/work" }),
    RuntimeProtocolError,
  );
  runtime.close();
});

test("session open results reject conversation-shaped extension fields", async () => {
  const instanceId = `rtm_${"4".repeat(32)}`;
  const transport = new ScriptedRuntimeTransport([
    challenge(instanceId),
    initialized(instanceId),
    {
      jsonrpc: "2.0",
      id: 2,
      result: {
        session: {
          sessionId: "019c2b97-5f29-7b00-8000-000000000001",
          providerId: "provider",
          lifecycle: "hotIdle",
          sessionGeneration: 1,
          transcript: [],
        },
        control: {
          leaseId: "lease_fixture",
          sessionId: "019c2b97-5f29-7b00-8000-000000000001",
          sessionGeneration: 1,
          leaseGeneration: 1,
          expiresAtMs: Date.now() + 30_000,
        },
      },
    },
  ]);
  const runtime = await new RuntimeConnector(scriptedTransportFactory(transport)).connect(
    validatedLocator(instanceId, "fixture", "0.1.1"),
    { name: "fixture", version: "1.0.0" },
  );
  await assert.rejects(
    runtime.sessions().start({
      requestId: "019c2b97-5f29-7b00-8000-000000000000",
      providerId: "provider",
      workspace: "C:/work",
      access: "exclusive",
    }),
    RuntimeProtocolError,
  );
  runtime.close();
});

test("integration identities round trip only through explicit private bytes", () => {
  const identity = IntegrationIdentity.generate();
  const restored = IntegrationIdentity.fromPkcs8(identity.exportPkcs8());
  assert.equal(restored.publicKeyBase64(), identity.publicKeyBase64());
  assert.equal(Buffer.from(restored.signBase64(Buffer.from("fixture")), "base64url").length, 64);
});

test("mutation request identities are canonical UUIDv7 values", (context) => {
  context.mock.method(Date, "now", () => 1_999_999_999_990);
  const requestId = newMutationRequestId();
  assert.match(
    requestId,
    /^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/,
  );
  assert.equal(Number.parseInt(requestId.replaceAll("-", "").slice(0, 12), 16), Date.now());
});

test("initialization signing matches the language-neutral fixture", async (context) => {
  context.mock.method(Date, "now", () => 1_999_999_999_990);
  const instanceId = "rtm_0123456789abcdef0123456789abcdef";
  const nonceId = "nonce_0123456789abcdef0123456789abcdef";
  const nonce = Buffer.alloc(32, 3).toString("base64url");
  const seed = Buffer.alloc(32, 7);
  const identity = IntegrationIdentity.fromPkcs8(Buffer.concat([
    Buffer.from("302e020100300506032b657004220420", "hex"),
    seed,
  ]));
  const grant = {
    integrationId: "int_fixture",
    scopes: [],
    roots: [],
    keyGeneration: 2,
    grantGeneration: 3,
  } as const;
  const transport = new ScriptedRuntimeTransport([
    {
      jsonrpc: "2.0",
      method: "runtime/challenge",
      params: { instanceId, nonceId, nonce, expiresAtMs: 2_000_000_000_000 },
    },
    {
      ...initialized(instanceId),
      result: {
        ...(initialized(instanceId) as { result: object }).result,
        grant,
      },
    },
  ]);
  const runtime = await new RuntimeConnector(scriptedTransportFactory(transport)).connect(
    validatedLocator(instanceId, "fixture", "0.1.1"),
    {
      name: "fixture",
      version: "1.0.0",
      credentials: { identity, grant },
    },
  );
  const request = JSON.parse(new TextDecoder().decode(transport.sent[0])) as {
    params: { authentication: { signature: string } };
  };
  assert.equal(
    request.params.authentication.signature,
    "cBrwv1dkWz6oG-YszAimU6leDfkNriZSKxUNSGYttRiH2dD0RJQsTklzpjzW3_qSIZYwrPeSPLHnCyW5fJ5sBQ",
  );
  runtime.close();
});
