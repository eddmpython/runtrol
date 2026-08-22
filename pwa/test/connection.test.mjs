import assert from "node:assert/strict";
import test from "node:test";

import {
  attentionCount,
  consumeAttentionRequest,
  isAttentionMessage,
  nextAttentionSession,
  preferredSession,
} from "../src/attention.js";
import { base64UrlDecode, base64UrlEncode, concat, equalBytes, utf8 } from "../src/bytes.js";
import { CoreClient, readDeviceAuthority, WIRE_VERSION } from "../src/core.js";
import { validateConnection } from "../src/identityStore.js";
import { missionActions, readMissionCatalogue, readMissionSnapshot } from "../src/missions.js";
import {
  missionFlightDestination,
  missionFlightBadge,
  missionFlightLabel,
  readMissionFlightSignals,
} from "../src/missionSignals.js";
import { consumePairingFragment, parsePairingValue } from "../src/pairing.js";
import { approvalOptions, exactSubject, safeVisibleText } from "../src/presentation.js";
import { disablePush, enablePush, synchronizePush } from "../src/push.js";
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
    { say: "done" },
    {
      say: "missionFlightSignals",
      with: { signals: [], next_cursor: null, gap: false },
    },
    { say: "missions", with: [] },
    { say: "mission", with: { mission: { mission_id: "msn_example" } } },
    { say: "mission", with: { mission: { mission_id: "msn_example", state: "paused" } } },
    { say: "mission", with: { mission: { mission_id: "msn_example", state: "running" } } },
    { say: "mission", with: { mission: { mission_id: "msn_example", state: "cancelled" } } },
  ]);
  const client = new CoreClient(channel);
  const welcome = await client.exchange({ ask: "hello", with: { wire: WIRE_VERSION } });
  assert.equal(welcome.say, "welcome");
  await client.list();
  await client.stopEverything();
  await client.setPushSubscription("https://fcm.googleapis.com/fcm/send/capability");
  await client.listMissionFlightSignals(null);
  await client.listMissions();
  await client.getMission("msn_example");
  await client.pauseMission("msn_example");
  await client.resumeMission("msn_example");
  await client.cancelMission("msn_example");
  assert.deepEqual(channel.sent.map((bytes) => JSON.parse(new TextDecoder().decode(bytes))), [
    { ask: "hello", with: { wire: WIRE_VERSION } },
    { ask: "list" },
    { ask: "stopEverything" },
    { ask: "pushSubscription", with: { endpoint: "https://fcm.googleapis.com/fcm/send/capability" } },
    { ask: "missionFlightSignals", with: { after: null } },
    { ask: "missionList" },
    { ask: "missionGet", with: { mission_id: "msn_example" } },
    { ask: "missionPause", with: { mission_id: "msn_example" } },
    { ask: "missionResumeSafe", with: { mission_id: "msn_example" } },
    { ask: "missionCancel", with: { mission_id: "msn_example" } },
  ]);
});

test("Mission controls expose only current-state actions backed by exact scopes", () => {
  const every = ["mission.pause", "mission.resumeSafe", "mission.cancel"];
  assert.deepEqual(missionActions({ state: "running" }, every), {
    pause: true,
    resume: false,
    cancel: true,
  });
  assert.deepEqual(missionActions({ state: "blocked" }, ["mission.resumeSafe"]), {
    pause: false,
    resume: true,
    cancel: false,
  });
  assert.deepEqual(missionActions({ state: "completed" }, every), {
    pause: false,
    resume: false,
    cancel: false,
  });
  const mission = {
    mission_id: "msn_example",
    name: "Example",
    project: "C:\\work",
    state: "running",
    passed_tasks: 1,
    total_tasks: 2,
    awaiting_input: 1,
  };
  assert.equal(readMissionCatalogue([mission])[0].name, "Example");
  const snapshot = readMissionSnapshot({
    mission,
    mission_ref: "mission.toml",
    policy_sha256: "ab".repeat(32),
    tasks: [{
      task_id: "tsk_example",
      key: "implement",
      state: "awaitingInput",
      instruction_ref: "instructions/implement.md",
      workspace_mode: "isolatedWorktree",
      provider_selector: "operatorChoice",
      receipt_id: null,
      passed_gates: 0,
      failed_gates: 0,
    }],
  });
  assert.equal(snapshot.tasks[0].state, "awaitingInput");
  assert.throws(() => readMissionCatalogue([{ ...mission, state: "surprise" }]), /unknown/u);
  assert.throws(() => readMissionSnapshot({ mission, mission_ref: "mission.toml", policy_sha256: "no", tasks: [] }), /digest/u);
});

test("push subscription is VAPID-bound and Core receives only the capability endpoint", async () => {
  const key = base64UrlEncode(Uint8Array.from({ length: 65 }, (_, index) => index));
  const calls = [];
  const subscription = fakeSubscription(key, calls);
  const registration = {
    pushManager: {
      getSubscription: async () => subscription,
      subscribe: async () => assert.fail("an existing matching subscription must be reused"),
    },
  };
  const client = { setPushSubscription: async (endpoint) => calls.push(["core", endpoint]) };
  assert.deepEqual(await synchronizePush(client, key, registration), {
    enabled: true,
    reason: "Notifications are on.",
  });
  await enablePush(client, key, registration);
  await disablePush(client, registration);
  assert.deepEqual(calls, [
    ["core", "https://fcm.googleapis.com/fcm/send/capability"],
    ["core", "https://fcm.googleapis.com/fcm/send/capability"],
    ["unsubscribe"],
    ["core", null],
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
  assert.equal(legacy.missionSignalCursor, null);
  const current = validateConnection({
    ...legacy,
    roots: ["C:\\work"],
    providers: ["fixture"],
    missionSignalCursor: "ab".repeat(16),
  });
  assert.equal(current.missionSignalCursor, "ab".repeat(16));
  assert.throws(
    () => validateConnection({ ...current, missionSignalCursor: "not-a-cursor" }),
    /Signal cursor/u,
  );
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

test("Mission Flight Signals are bounded and route only current structural destinations", () => {
  const page = readMissionFlightSignals({
    signals: [
      {
        signal_id: "01".repeat(16),
        mission_id: "msn_person",
        mission_sha256: "ab".repeat(32),
        kind: "person",
        session_id: "session-person",
      },
      {
        signal_id: "02".repeat(16),
        mission_id: "msn_landing",
        mission_sha256: "cd".repeat(32),
        kind: "landing",
        session_id: null,
      },
    ],
    next_cursor: "02".repeat(16),
    gap: false,
  });
  assert.equal(missionFlightDestination(page.signals, [
    { session: "session-person", waiting_on: "person" },
  ]).missionId, "msn_landing", "the newest current signal wins");
  assert.equal(missionFlightLabel("landing"), "Receipt Landing ready");
  assert.equal(missionFlightBadge("landing"), "LANDED");
  assert.equal(missionFlightDestination(page.signals.slice(0, 1), [
    { session: "session-person", waiting_on: "person" },
  ]).surface, "session");
  assert.equal(missionFlightDestination(page.signals.slice(0, 1), [
    { session: "session-person", waiting_on: "quota" },
  ]), null, "quota is never treated as a person wait");
  assert.throws(
    () => readMissionFlightSignals({
      signals: [{ ...page.signals[0], session_id: null }],
      next_cursor: "01".repeat(16),
      gap: false,
    }),
    /destination/u,
  );
  assert.throws(
    () => readMissionFlightSignals({
      signals: Array.from({ length: 65 }, () => page.signals[0]),
      next_cursor: "01".repeat(16),
      gap: false,
    }),
    /invalid/u,
  );
});

test("phone focus includes only person waits and cycles in stable catalogue order", () => {
  const sessions = [
    { session: "working", waiting_on: null },
    { session: "quota", waiting_on: "quota" },
    { session: "first", waiting_on: "person" },
    { session: "unknown", waiting_on: "future" },
    { session: "second", waiting_on: "person" },
  ];
  assert.equal(attentionCount(sessions), 2);
  assert.equal(nextAttentionSession(sessions)?.session, "first");
  assert.equal(nextAttentionSession(sessions, "first")?.session, "second");
  assert.equal(nextAttentionSession(sessions, "second")?.session, "first");
  assert.equal(preferredSession(sessions, null, null, false, true), null, "a phone opens on its bounded list");
  assert.equal(preferredSession(sessions, null, "working", true, true)?.session, "first");
  assert.equal(preferredSession(sessions, "second", "working", true, true)?.session, "second");
  assert.equal(preferredSession(sessions, null, "quota", false, true)?.session, "quota");
  assert.equal(preferredSession(sessions, null, null, false, false)?.session, "working");
});

test("a content-free attention launch is consumed without discarding other URL state", () => {
  const replacements = [];
  const requested = consumeAttentionRequest(
    { href: "https://phone.example.test/runtrol/app/?source=push&attention=1" },
    { replaceState: (...values) => replacements.push(values) },
  );
  assert.equal(requested, true);
  assert.deepEqual(replacements, [[null, "", "/runtrol/app/?source=push"]]);
  assert.equal(isAttentionMessage({ kind: "runtrolAttention" }), true);
  assert.equal(isAttentionMessage({ kind: "runtrolAttention", session: "secret" }), false);
  assert.equal(isAttentionMessage({ kind: "other" }), false);
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

function fakeSubscription(encodedKey, calls) {
  const key = base64UrlDecode(encodedKey, 65);
  return {
    endpoint: "https://fcm.googleapis.com/fcm/send/capability",
    options: { applicationServerKey: key.buffer },
    unsubscribe: async () => {
      calls.push(["unsubscribe"]);
      return true;
    },
  };
}

assert.equal(equalBytes(new Uint8Array([1]), new Uint8Array([1])), true);
