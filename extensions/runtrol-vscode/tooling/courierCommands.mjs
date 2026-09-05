// Real Runtime dialogue journey, called from the owned admission/handover harness.
import assert from "node:assert/strict";
import { randomBytes } from "node:crypto";

function id() {
  const bytes = randomBytes(16);
  bytes.writeUIntBE(Date.now(), 0, 6);
  bytes[6] = (bytes[6] & 15) | 112;
  bytes[8] = (bytes[8] & 63) | 128;
  const text = bytes.toString("hex");
  return `${text.slice(0, 8)}-${text.slice(8, 12)}-${text.slice(12, 16)}-${text.slice(16, 20)}-${text.slice(20)}`;
}

function envelope(source, target, body, kind = "tell", timeout = 10_000) {
  return { protocol_version: 1, message_id: id(), call_id: id(), source, target, kind,
    reply_to: null, room_id: null, deadline: Date.now() + timeout, hop_count: 0, visited: [], body };
}

export async function commandJourney(first, firstSession, open) {
  const a = { peer: first, session: firstSession };
  const b = await open(); const c = await open();
  const raw = (agent, command) => agent.peer.ask({ kind: "request", command });
  const cli = async (agent, words, body = "", status = 0) => {
    const result = await agent.peer.ask({ kind: "cli", words, body });
    assert.equal(result.status, status, `${words[0]}: ${result.stderr}`);
    return JSON.parse(result.stdout);
  };
  const receive = (agent, source = null, timeout_ms = 0) => raw(agent, { command: "receive", source, call: null, timeout_ms });
  const listed = await cli(a, ["list"]);
  assert.deepEqual(new Set(listed.sessions.map((row) => row.session)), new Set([a.session, b.session, c.session]));
  assert.ok(listed.sessions.every((row) => Object.keys(row).sort().join(",") === "pid,session"));
  const firstId = id();
  const body = "courier-probe-한국어 English\nopaque body\n";
  await cli(c, ["tell", b.session], "unrelated");
  const receipt = await cli(a, ["tell", b.session, "--message-id", firstId], body);
  assert.equal(receipt.receipt.message_id, firstId);
  assert.equal((await cli(a, ["tell", b.session, "--message-id", firstId], body, 1)).answer, "refused");
  const delivered = await cli(b, ["inbox", "--from", a.session]);
  assert.equal(delivered.envelope.body, body);
  assert.equal((await cli(b, ["inbox"])).envelope.body, "unrelated");
  assert.equal((await cli(b, ["inbox"], "", 1)).envelope, null);
  assert.equal((await raw(a, { command: "send", envelope: envelope(c.session, b.session, "spoofed") })).answer, "refused");
  for (let index = 0; index < 16; index += 1) {
    assert.equal((await raw(a, { command: "send", envelope: envelope(a.session, b.session, String(index)) })).answer, "accepted");
  }
  assert.equal((await raw(a, { command: "send", envelope: envelope(a.session, b.session, "overflow") })).answer, "refused");
  for (let index = 0; index < 16; index += 1) assert.equal((await receive(b)).envelope.body, String(index));
  const original = envelope(a.session, b.session, body, "ask");
  assert.equal((await raw(a, { command: "send", envelope: original })).answer, "accepted");
  assert.equal((await raw(a, { command: "ask", envelope: original })).answer, "refused");
  assert.equal((await receive(b)).envelope.message_id, original.message_id, "a refused duplicate cannot cancel its original");
  await cli(b, ["reply", original.message_id], "original survived");
  assert.equal((await receive(a)).envelope.body, "original survived");
  const escaped = "\0".repeat(16 * 1024);
  assert.equal((await raw(a, { command: "send", envelope: envelope(a.session, b.session, escaped) })).answer, "accepted");
  assert.equal((await receive(b)).envelope.body, escaped);

  for (const [source, target] of [[a, b], [b, a]]) {
    const ask = cli(source, ["ask", target.session, "--timeout", "10"], body);
    const request = (await cli(target, ["wait", "--from", source.session, "--timeout", "10"])).envelope;
    assert.equal(request.kind, "ask"); assert.equal(request.body, body);
    assert.equal((await cli(c, ["reply", request.message_id], "wrong role", 1)).answer, "refused");
    await cli(target, ["reply", request.message_id], "정확한 reply");
    const reply = (await ask).envelope;
    assert.equal(reply.reply_to, request.message_id); assert.equal(reply.body, "정확한 reply");
    assert.equal((await cli(target, ["reply", request.message_id], "duplicate", 1)).answer, "refused");
  }
  const cancelled = envelope(a.session, b.session, body, "ask");
  await raw(a, { command: "send", envelope: cancelled });
  await cli(a, ["cancel", cancelled.call_id]);
  assert.equal((await receive(b)).envelope.kind, "cancel");
  assert.equal((await receive(b)).envelope, null);
  const timeoutAt = Date.now();
  const expired = await cli(a, ["ask", b.session, "--timeout", "1"], body, 1);
  assert.ok(expired.answer === "refused" || (expired.answer === "received" && expired.envelope === null));
  assert.ok(Date.now() - timeoutAt >= 900 && Date.now() - timeoutAt < 4000);
  assert.equal((await receive(b)).envelope, null);

  // Fill all global wait slots across eight managed sessions; messages still have their own allowance.
  const cohort = [a, b, c];
  while (cohort.length < 8) cohort.push(await open());
  for (const agent of cohort) {
    for (let index = 0; index < 4; index += 1) {
      assert.equal((await agent.peer.ask({ kind: "hold", key: index,
        command: { command: "receive", source: c.session, call: null, timeout_ms: 60_000 } })).answer, "welcome");
    }
    assert.equal((await agent.peer.ask({ kind: "hold", key: "overflow",
      command: { command: "receive", source: null, call: null, timeout_ms: 10_000 } })).answer, "refused");
  }
  await cli(a, ["tell", b.session], "wait slots do not block send");
  assert.equal((await receive(b, a.session)).envelope.body, "wait slots do not block send");
  for (const agent of cohort) for (let index = 0; index < 4; index += 1) await agent.peer.ask({ kind: "release", key: index });
  const abandoned = envelope(a.session, b.session, body, "ask");
  await a.peer.ask({ kind: "hold", key: "ask", command: { command: "ask", envelope: abandoned } });
  await a.peer.ask({ kind: "release", key: "ask" });
  // The list round trip makes the disconnected request's cleanup observable without consuming its body.
  await cli(a, ["list"]);
  assert.equal((await receive(b)).envelope, null);
  process.stdout.write("RUNTROL_COURIER_COMMANDS list=true unicode=true duplicate=true filter=true overflow=true roundTrip=true reverse=true cancel=true expiry=true waitSaturation=true disconnect=true\n");
  return body;
}
