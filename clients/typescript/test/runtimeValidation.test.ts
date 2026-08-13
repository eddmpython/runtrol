import assert from "node:assert/strict";
import { test } from "node:test";

import {
  FINALIZED_REVISIONS,
  IntegrationIdentity,
  PUBLIC_LIMITS,
  RuntimeConnector,
  RuntimeProtocolError,
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

test("integration identities round trip only through explicit private bytes", () => {
  const identity = IntegrationIdentity.generate();
  const restored = IntegrationIdentity.fromPkcs8(identity.exportPkcs8());
  assert.equal(restored.publicKeyBase64(), identity.publicKeyBase64());
  assert.equal(Buffer.from(restored.signBase64(Buffer.from("fixture")), "base64url").length, 64);
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
