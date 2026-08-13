import assert from "node:assert/strict";
import test from "node:test";

import { base64UrlEncode, concat, equalBytes, utf8 } from "../src/bytes.js";
import { CoreClient, readDeviceAuthority, WIRE_VERSION } from "../src/core.js";
import { validateConnection } from "../src/identityStore.js";
import { consumePairingFragment, parsePairingValue } from "../src/pairing.js";
import { approvalOptions, exactSubject, safeVisibleText } from "../src/presentation.js";
import { RecordChannel, decodeRecord, encodeRecord } from "../src/records.js";
import { requestTicket } from "../src/relay.js";

test("pairing fragment accepts only the exact fresh secret-bearing contract", () => {
  const now = 1_800_000_000_000;
  const encoded = base64UrlEncode(utf8(JSON.stringify({
    version: 1,
    relay_origin: "https://relay.example.com",
    route: base64UrlEncode(new Uint8Array(32).fill(1)),
    credential: base64UrlEncode(new Uint8Array(32).fill(2)),
    pc_public_key: base64UrlEncode(new Uint8Array(32).fill(3)),
    pairing_secret: base64UrlEncode(new Uint8Array(16).fill(4)),
    expires_at_ms: now + 120_000,
  })));
  const parsed = parsePairingValue(encoded, now);
  assert.equal(parsed.relayOrigin, "https://relay.example.com");
  assert.throws(() => parsePairingValue(encoded, now + 120_000), /expired/u);
  const widened = JSON.parse(Buffer.from(encoded, "base64url").toString("utf8"));
  widened.extra = true;
  assert.throws(() => parsePairingValue(Buffer.from(JSON.stringify(widened)).toString("base64url"), now), /field set/u);

  const historyCalls = [];
  const consumed = consumePairingFragment(
    { hash: `#pair=${encoded}`, pathname: "/runtrol/app/", search: "?source=qr" },
    { replaceState: (...args) => historyCalls.push(args) },
    now,
  );
  assert.equal(consumed.route, parsed.route);
  assert.deepEqual(historyCalls, [[null, "", "/runtrol/app/?source=qr"]]);
});

test("approval presentation exposes spoofing controls and unavailable options", () => {
  assert.equal(safeVisibleText("run\u001b[31m red\u001b[0m"), "run red");
  assert.equal(safeVisibleText("safe\u202Etxt"), "safe<U+202E>txt");
  assert.match(exactSubject({ command: "cargo test", path: "C:\\work" }), /cargo test/u);
  const options = approvalOptions({
    risk: "high",
    subject_incomplete: false,
    options: [
      { id: 0, label: "Allow", kind: "allowOnce" },
      { id: 1, label: "Reject", kind: "rejectOnce" },
    ],
  }, ["approval.respond.low"]);
  assert.equal(options.length, 2, "unavailable options remain visible");
  assert.ok(options.every((option) => option.unavailable));
});

test("record framing is canonical and reassembles a bounded multi-record frame", async () => {
  const encrypted = new Uint8Array(130).fill(9);
  assert.deepEqual(decodeRecord(encodeRecord(encrypted)), encrypted);
  assert.throws(() => decodeRecord(concat(Uint8Array.of(0x90, 0x00), new Uint8Array(16))), /non-canonical/u);

  const pairs = cipherPair();
  const sender = new RecordChannel(pairs.initiator);
  const receiver = new RecordChannel(pairs.responder);
  const input = new Uint8Array(140_000).map((_, index) => index % 251);
  const records = await sender.sealFrame(input);
  assert.equal(records.length, 3);
  let output = null;
  for (const record of records) output = await receiver.openRecord(record);
  assert.deepEqual(output, input);

  const rekey = await sender.requestRekey();
  assert.equal(await receiver.openRecord(rekey), null);
  const after = await sender.sealFrame(utf8("after rekey"));
  assert.equal(new TextDecoder().decode(await receiver.openRecord(after[0])), "after rekey");
});

test("relay ticket request sends the exact route credential and phone role", async () => {
  const calls = [];
  const ticket = base64UrlEncode(new Uint8Array(32).fill(12));
  const value = await requestTicket({
    relayOrigin: "https://relay.example.com",
    route: "route",
    routeCredential: "secret",
  }, async (url, init) => {
    calls.push({ url, init });
    return new Response(JSON.stringify({ ticket, expiresAt: Date.now() + 30_000 }), {
      status: 200,
      headers: { "Content-Type": "application/json" },
    });
  });
  assert.equal(value, ticket);
  assert.equal(calls[0].url, "https://relay.example.com/v1/routes/route/tickets");
  assert.equal(calls[0].init.headers.Authorization, "Bearer secret");
  assert.equal(calls[0].init.body, '{"role":"phone"}');
  assert.equal(calls[0].init.credentials, "omit");
});

test("Core client greets first and keeps every request explicit", async () => {
  const channel = new FakeChannel([
    { say: "welcome", with: { wire: WIRE_VERSION, providers: [] } },
    { say: "sessions", with: { sessions: [], warnings: [] } },
    { say: "done" },
  ]);
  const client = new CoreClient(channel);
  const welcome = await client.exchange({ ask: "hello", with: { wire: WIRE_VERSION } });
  assert.equal(welcome.say, "welcome");
  await client.list();
  await client.stopEverything();
  assert.deepEqual(channel.sent.map((bytes) => JSON.parse(new TextDecoder().decode(bytes))), [
    { ask: "hello", with: { wire: WIRE_VERSION } },
    { ask: "list" },
    { ask: "stopEverything" },
  ]);
});

test("current Core authority replaces legacy pairing hints without widening", () => {
  const legacy = validateConnection({
    deviceCredential: "credential",
    pcPublicKey: "pc",
    relayOrigin: "https://relay.example.com",
    route: "route",
    routeCredential: "ticket credential",
    scopes: ["session.list", "workspace(stale)"],
  });
  assert.deepEqual(legacy.roots, []);
  assert.deepEqual(legacy.providers, []);
  const authority = readDeviceAuthority({
    scopes: ["session.list"],
    roots: ["C:\\work"],
    providers: ["fixture"],
  });
  assert.deepEqual(authority.scopes, ["session.list"]);
  assert.throws(
    () => readDeviceAuthority({ scopes: ["session.list", "session.list"], roots: [], providers: [] }),
    /invalid/u,
  );
});

function cipherPair() {
  const initiatorSending = { generation: 0, nonce: 0 };
  const responderReceiving = { generation: 0, nonce: 0 };
  const responderSending = { generation: 0, nonce: 0 };
  const initiatorReceiving = { generation: 0, nonce: 0 };
  return {
    initiator: {
      encrypt: async (value) => tagged(value, initiatorSending),
      decrypt: async (value) => untagged(value, initiatorReceiving),
      rekeySending: async () => rekeyState(initiatorSending),
      rekeyReceiving: async () => rekeyState(initiatorReceiving),
    },
    responder: {
      encrypt: async (value) => tagged(value, responderSending),
      decrypt: async (value) => untagged(value, responderReceiving),
      rekeySending: async () => rekeyState(responderSending),
      rekeyReceiving: async () => rekeyState(responderReceiving),
    },
  };
}

function tagged(value, state) {
  const ciphertext = concat(
    Uint8Array.of(state.nonce & 0xff, state.generation),
    value,
    new Uint8Array(14),
  );
  state.nonce += 1;
  return ciphertext;
}

function untagged(value, state) {
  assert.equal(value[0], state.nonce & 0xff);
  assert.equal(value[1], state.generation);
  state.nonce += 1;
  return value.slice(2, -14);
}

function rekeyState(state) {
  state.generation += 1;
  state.nonce = 0;
}

class FakeChannel {
  constructor(responses) {
    this.responses = responses;
    this.sent = [];
  }

  async send(value) {
    this.sent.push(value);
  }

  async receive() {
    return utf8(JSON.stringify(this.responses.shift()));
  }

  close() {}
}

assert.equal(equalBytes(new Uint8Array([1]), new Uint8Array([1])), true);
