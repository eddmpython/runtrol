import assert from "node:assert/strict";
import { createPublicKey, verify } from "node:crypto";
import { test } from "node:test";

import {
  FINALIZED_REVISIONS,
  type IntegrationGrant,
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

function initialized(instanceId: string, grant?: object, priority = false, digest?: string): object {
  return {
    jsonrpc: "2.0",
    id: 1,
    result: {
      selectedRevision: FINALIZED_REVISIONS[0],
      runtime: {
        instanceId,
        version: "0.1.1",
        platform: "fixture",
        ...(digest ? { buildDigest: digest } : {}),
      },
      serverCapabilities: {
        integrationEnrollment: true,
        providerInventory: true,
        managedSessionList: true,
        modelDiscovery: true,
        nativeSessionCatalogue: true,
        sessionControl: true,
        sessionEvents: true,
        ...(priority ? { terminalInputPriority: true, grantScopeProjection: true } : {}),
      },
      limits: PUBLIC_LIMITS,
      ...(grant ? { grant } : {}),
    },
  };
}

test("challenge validation tolerates bounded local clock skew and nothing beyond it", async (context) => {
  const now = 2_000_000_000_000;
  context.mock.method(Date, "now", () => now);
  const instanceId = `rtm_${"8".repeat(32)}`;
  const challengeAt = (expiresAtMs: number): object => ({
    jsonrpc: "2.0",
    method: "runtime/challenge",
    params: {
      instanceId,
      nonceId: `nonce_${"0".repeat(32)}`,
      nonce: Buffer.alloc(32).toString("base64url"),
      expiresAtMs,
    },
  });

  const acceptedTransport = new ScriptedRuntimeTransport([
    challengeAt(now + PUBLIC_LIMITS.challengeLifetimeMs + 5_000),
    initialized(instanceId),
  ]);
  const accepted = await new RuntimeConnector(scriptedTransportFactory(acceptedTransport)).connect(
    validatedLocator(instanceId, "fixture", "0.1.1"),
    { name: "fixture", version: "1.0.0" },
  );
  accepted.close();

  const rejectedTransport = new ScriptedRuntimeTransport([
    challengeAt(now + PUBLIC_LIMITS.challengeLifetimeMs + 5_001),
  ]);
  await assert.rejects(
    new RuntimeConnector(scriptedTransportFactory(rejectedTransport)).connect(
      validatedLocator(instanceId, "fixture", "0.1.1"),
      { name: "fixture", version: "1.0.0" },
    ),
    /exceeds the public lifetime and clock-skew bound/,
  );
});

test("key rotation signs the replacement key and returns replacement credentials", async () => {
  const instanceId = `rtm_${"6".repeat(32)}`;
  const original = IntegrationIdentity.generate();
  const replacement = IntegrationIdentity.fromPkcs8(Buffer.concat([
    Buffer.from("302e020100300506032b657004220420", "hex"),
    Buffer.alloc(32, 8),
  ]));
  const grant: IntegrationGrant = {
    integrationId: "int_09090909090909090909090909090909",
    scopes: ["provider.read"],
    roots: [],
    keyGeneration: 2,
    grantGeneration: 3,
  };
  const rotated = { ...grant, keyGeneration: 3 };
  const transport = new ScriptedRuntimeTransport([
    challenge(instanceId),
    initialized(instanceId, grant),
    { jsonrpc: "2.0", id: 2, result: rotated },
  ]);
  const runtime = await new RuntimeConnector(scriptedTransportFactory(transport)).connect(
    validatedLocator(instanceId, "fixture", "0.1.1"),
    { name: "fixture", version: "1.0.0", credentials: { identity: original, grant } },
  );
  const requestId = "019c2b97-5f29-7b00-8000-000000000000";
  const credentials = await runtime.integrations().rotateKey(requestId, 2, replacement);
  assert.equal(credentials.identity, replacement);
  assert.deepEqual(credentials.grant, rotated);
  const sent = JSON.parse(new TextDecoder().decode(transport.sent[2])) as {
    method: string;
    params: { requestId: string; expectedKeyGeneration: number; newPublicKey: string; newKeyProof: string };
  };
  assert.equal(sent.method, "integrations/rotateKey");
  assert.equal(sent.params.requestId, requestId);
  assert.equal(sent.params.expectedKeyGeneration, 2);
  assert.equal(sent.params.newPublicKey, "E5j2LG0aRXxRumpLXz29L2n8qTIWIY3ImX5Ba9F9k8o");
  assert.equal(
    sent.params.newKeyProof,
    "c3ZY8ElvUR3lVmFrkVrP5AnALg7q9bgcgU5DP0e0MhZZFaY_jGvRTEiesBUXnQyOjLepXGnx3xqkBmw-gZ_5CA",
  );
  runtime.close();
});

test("authenticated reconnect accepts and exposes a newer locally approved grant", async () => {
  const instanceId = `rtm_${"7".repeat(32)}`;
  const identity = IntegrationIdentity.generate();
  const previous: IntegrationGrant = {
    integrationId: "int_07070707070707070707070707070707",
    scopes: ["provider.read"],
    roots: [],
    keyGeneration: 1,
    grantGeneration: 2,
  };
  const current: IntegrationGrant = {
    ...previous,
    scopes: ["provider.read", "session.list"],
    roots: ["C:/work"],
    grantGeneration: 3,
  };
  const transport = new ScriptedRuntimeTransport([
    challenge(instanceId),
    initialized(instanceId, current),
  ]);
  const runtime = await new RuntimeConnector(scriptedTransportFactory(transport)).connect(
    validatedLocator(instanceId, "fixture", "0.1.1"),
    { name: "fixture", version: "1.0.0", credentials: { identity, grant: previous } },
  );
  assert.deepEqual(runtime.initialization.grant, current);
  runtime.close();
});

test("hostile Runtime results are rejected by the generated public schema", () => {
  const instanceId = `rtm_${"1".repeat(32)}`;
  assert.throws(
    () => new ValidatedLocator(
      Symbol("forged") as never,
      instanceId,
      "fixture",
      "0.1.1",
      "0".repeat(64),
      false,
      "fixture-control",
      "fixture-revision",
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

test("provider capabilities reject unregistered provider semantics", async () => {
  const instanceId = `rtm_${"5".repeat(32)}`;
  const observed = { availability: "available", provenance: "officialProtocol" };
  const transport = new ScriptedRuntimeTransport([
    challenge(instanceId),
    initialized(instanceId),
    {
      jsonrpc: "2.0",
      id: 2,
      result: {
        providerId: "provider",
        freshness: "current",
        freshSession: observed,
        resume: observed,
        structuredEvents: observed,
        interrupt: observed,
        approvals: observed,
        cooling: observed,
        nativeSessionCatalogue: observed,
        providerMode: "invented",
      },
    },
  ]);
  const runtime = await new RuntimeConnector(scriptedTransportFactory(transport)).connect(
    validatedLocator(instanceId, "fixture", "0.1.1"),
    { name: "fixture", version: "1.0.0" },
  );
  await assert.rejects(
    runtime.providers().getCapabilities("provider"),
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

test("initialization signing canonicalizes Runtime challenge field order", async (context) => {
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
      params: { expiresAtMs: 2_000_000_000_000, instanceId, nonce, nonceId },
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
    "76vuXq3Hzt5zh5NEBMPflZYx3f0Q5jFkkmIhQfy8DKUL5njriRErOvnMgOMSC3tap1TLT1xZZvAy4WqMjeQMBA",
  );
  runtime.close();
});

test("enrollment signing canonicalizes Runtime challenge field order", async (context) => {
  context.mock.method(Date, "now", () => 1_999_999_999_990);
  const instanceId = "rtm_0123456789abcdef0123456789abcdef";
  const nonceId = "nonce_0123456789abcdef0123456789abcdef";
  const nonce = Buffer.alloc(32, 3).toString("base64url");
  const identity = IntegrationIdentity.fromPkcs8(Buffer.concat([
    Buffer.from("302e020100300506032b657004220420", "hex"),
    Buffer.alloc(32, 7),
  ]));
  const transport = new ScriptedRuntimeTransport([
    {
      jsonrpc: "2.0",
      method: "runtime/challenge",
      params: { expiresAtMs: 2_000_000_000_000, instanceId, nonce, nonceId },
    },
    initialized(instanceId),
    {
      jsonrpc: "2.0",
      id: 2,
      result: {
        pendingId: `enr_${"1".repeat(32)}`,
        expiresAtMs: 2_000_000_030_000,
      },
    },
  ]);
  const runtime = await new RuntimeConnector(scriptedTransportFactory(transport)).connect(
    validatedLocator(instanceId, "fixture", "0.1.1"),
    { name: "fixture", version: "1.0.0", identity },
  );
  await runtime.integrations().request({
    clientInstanceId: "fixture-instance",
    manifestDigest: Buffer.alloc(32, 5),
    requestedScopes: ["provider.read"],
    requestedRoots: ["C:/work"],
  });
  const request = JSON.parse(new TextDecoder().decode(transport.sent[2])) as {
    params: {
      manifest: {
        clientInstanceId: string;
        publicKey: string;
        manifestDigest: string;
        requestedScopes: ReadonlyArray<string>;
        requestedRoots: ReadonlyArray<string>;
      };
      signature: string;
    };
  };
  const payload = Buffer.from(JSON.stringify({
    domain: "runtrol-runtime-enrollment-v1",
    challenge: { instanceId, nonceId, nonce, expiresAtMs: 2_000_000_000_000 },
    supportedRevisions: FINALIZED_REVISIONS,
    selectedRevision: FINALIZED_REVISIONS[0],
    client: { name: "fixture", version: "1.0.0" },
    clientCapabilities: { opaqueEventExtensions: false },
    manifest: request.params.manifest,
  }));
  const publicKey = createPublicKey({
    key: Buffer.concat([
      Buffer.from("302a300506032b6570032100", "hex"),
      Buffer.from(identity.publicKeyBase64(), "base64url"),
    ]),
    format: "der",
    type: "spki",
  });
  assert.equal(
    verify(null, payload, publicKey, Buffer.from(request.params.signature, "base64url")),
    true,
  );
  runtime.close();
});



test("priority vocabulary negotiates after the legacy hello and preserves canonical signatures", async (context) => {
  const now = 2_000_000_000_000;
  context.mock.method(Date, "now", () => now);
  const instanceId = `rtm_${"a".repeat(32)}`;
  const identity = IntegrationIdentity.generate();
  const grant: IntegrationGrant = {
    integrationId: "int_09090909090909090909090909090909",
    scopes: ["provider.read"], roots: [], keyGeneration: 2, grantGeneration: 3,
  };
  const full: IntegrationGrant = { ...grant, scopes: [...grant.scopes, "session.input.priority"] };
  const first = new ScriptedRuntimeTransport([challenge(instanceId), initialized(instanceId, grant, true)]);
  const second = new ScriptedRuntimeTransport([challenge(instanceId), initialized(instanceId, full, true)]);
  const transports = [first, second];
  const connector = new RuntimeConnector(async () => {
    const next = transports.shift();
    assert.ok(next, "negotiation is bounded to two connections");
    return next;
  });
  const runtime = await connector.connect(validatedLocator(instanceId, "fixture", "0.1.1"), {
    name: "fixture", version: "1.0.0", credentials: { identity, grant },
    capabilities: { opaqueEventExtensions: false, terminalInputPriority: true },
  });
  assert.deepEqual(runtime.initialization.grant, full);
  const publicKey = createPublicKey({
    key: Buffer.concat([Buffer.from("302a300506032b6570032100", "hex"),
      Buffer.from(identity.publicKeyBase64(), "base64url")]), format: "der", type: "spki",
  });
  for (const [index, transport] of [first, second].entries()) {
    const request = JSON.parse(new TextDecoder().decode(transport.sent[0]));
    const caps = { opaqueEventExtensions: false, ...(index === 1 ? { terminalInputPriority: true } : {}) };
    assert.deepEqual(request.params.clientCapabilities, caps);
    const payload = Buffer.from(JSON.stringify({
      domain: "runtrol-runtime-initialize-v1",
      challenge: { instanceId, nonceId: `nonce_${"0".repeat(32)}`, nonce: Buffer.alloc(32).toString("base64url"),
        expiresAtMs: now + 30_000 },
      supportedRevisions: FINALIZED_REVISIONS,
      client: request.params.client, clientCapabilities: caps,
      integrationId: grant.integrationId, keyGeneration: grant.keyGeneration, grantGeneration: grant.grantGeneration,
    }));
    assert.equal(verify(null, payload, publicKey, Buffer.from(request.params.authentication.signature, "base64url")), true);
  }
  assert.equal(transports.length, 0);
  runtime.close();
});


test("scope vocabulary reuse is bound to one proved generation and still validates warm replies", async () => {
  const instanceId = `rtm_${"b".repeat(32)}`;
  const digest = "1".repeat(64);
  const options = { name: "fixture", version: "1.0.0" };
  const first = new ScriptedRuntimeTransport([challenge(instanceId), initialized(instanceId, undefined, true, digest)]);
  const negotiated = new ScriptedRuntimeTransport([challenge(instanceId), initialized(instanceId, undefined, true, digest)]);
  const warm = new ScriptedRuntimeTransport([challenge(instanceId), initialized(instanceId, undefined, true, digest)]);
  const hostile = new ScriptedRuntimeTransport([challenge(instanceId), initialized(instanceId, undefined, true, "2".repeat(64))]);
  const other = new ScriptedRuntimeTransport([challenge(instanceId), initialized(instanceId)]);
  const transports = [first, negotiated, warm, hostile, other];
  const connector = new RuntimeConnector(async () => {
    const next = transports.shift(); assert.ok(next); return next;
  });
  const locator = validatedLocator(instanceId, "fixture", "0.1.1", digest);
  (await connector.connect(locator, options)).close();
  assert.equal(transports.length, 3, "cold connection performs two bounded handshakes");
  (await connector.connect(locator, options)).close();
  assert.equal(transports.length, 2, "warm connection performs one handshake");
  assert.equal(JSON.parse(new TextDecoder().decode(warm.sent[0])).params.clientCapabilities.terminalInputPriority, true);
  await assert.rejects(connector.connect(locator, options), /does not match the locator/);
  (await connector.connect(validatedLocator(instanceId, "other", "0.1.1", digest), options)).close();
  assert.deepEqual(JSON.parse(new TextDecoder().decode(other.sent[0])).params.clientCapabilities, { opaqueEventExtensions: false });
});

test("draining priority projection allows no unrelated same-generation grant change", async () => {
  const instanceId = `rtm_${"c".repeat(32)}`;
  const identity = IntegrationIdentity.generate();
  const expected: IntegrationGrant = { integrationId: "int_09090909090909090909090909090909",
    scopes: ["provider.read", "session.input.priority"], roots: ["C:/fixture"], keyGeneration: 1, grantGeneration: 2 };
  for (const [current, accepted] of [
    [{ ...expected, scopes: ["provider.read"] }, true],
    [{ ...expected, scopes: ["provider.read", "session.input.write"] }, false],
    [{ ...expected, roots: [] }, false],
  ] as const) {
    const transport = new ScriptedRuntimeTransport([challenge(instanceId), initialized(instanceId, current)]);
    const connected = new RuntimeConnector(scriptedTransportFactory(transport)).connect(
      validatedLocator(instanceId, "fixture", "0.1.1"), { name: "fixture", version: "1.0.0", credentials: { identity, grant: expected } });
    if (accepted) (await connected).close();
    else await assert.rejects(connected, /does not match the locator/);
  }
});

test("draining initialization retries stale authority but primary authentication fails immediately", async () => {
  const instanceId = `rtm_${"d".repeat(32)}`;
  const refusal = { jsonrpc: "2.0", id: 1, error: { code: "unauthenticated", message: "stale fixture authority", retryable: false, correlationId: "fixture" } };
  const failed = new ScriptedRuntimeTransport([challenge(instanceId), refusal]);
  const admitted = new ScriptedRuntimeTransport([challenge(instanceId), initialized(instanceId)]);
  const transports = [failed, admitted];
  const connector = new RuntimeConnector(async (endpoint) => {
    assert.equal(endpoint, "draining-fixture");
    const next = transports.shift(); assert.ok(next); return next;
  });
  (await connector.connect(validatedLocator(instanceId, "draining-fixture", "0.1.1", "0".repeat(64), true), { name: "fixture", version: "1" })).close();
  for (const transport of [failed, admitted]) {
    for (const frame of transport.sent) {
      assert.ok(["runtime/initialize", "runtime/initialized"].includes(JSON.parse(new TextDecoder().decode(frame)).method));
    }
  }
  let primaryAttempts = 0;
  await assert.rejects(new RuntimeConnector(async () => {
    primaryAttempts += 1; return new ScriptedRuntimeTransport([challenge(instanceId), refusal]);
  }).connect(validatedLocator(instanceId, "primary-fixture", "0.1.1"), { name: "fixture", version: "1" }),
  (error: unknown) => typeof error === "object" && error !== null && "failure" in error
    && (error.failure as { code: string }).code === "unauthenticated");
  assert.equal(primaryAttempts, 1);
});

test("draining signature refusal stays denied after bounded patience and cancellation wakes it", async () => {
  const instanceId = `rtm_${"e".repeat(32)}`;
  const denial = { jsonrpc: "2.0", id: 1, error: { code: "unauthenticated", message: "signature rejected", retryable: false, correlationId: "fixture" } };
  const locator = validatedLocator(instanceId, "draining-fixture", "0.1.1", "0".repeat(64), true);
  let attempts = 0;
  const connector = new RuntimeConnector(async () => {
    attempts += 1; return new ScriptedRuntimeTransport([challenge(instanceId), denial]);
  });
  const started = performance.now();
  await assert.rejects(connector.connect(locator, { name: "fixture", version: "1" }),
    (error: unknown) => typeof error === "object" && error !== null && "failure" in error
      && (error.failure as { code: string }).code === "unauthenticated");
  assert.ok(attempts > 1 && attempts < 40);
  assert.ok(performance.now() - started < 5_000);
  const abort = new AbortController();
  const waiting = connector.connect(locator, { name: "fixture", version: "1" }, abort.signal);
  setTimeout(() => abort.abort(new Error("cancel draining fixture")), 30);
  await assert.rejects(waiting, /cancel draining fixture/);
});
