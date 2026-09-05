// Body retirement across actual Runtime generations. This client owns no provider transcript.
import assert from "node:assert/strict";
import { randomBytes } from "node:crypto";

export async function lifecycleJourney(context, first) {
  const sentinel = `courier-lifecycle-${randomBytes(16).toString("hex")}`;
  context.retainMarker(sentinel);
  const body = `${sentinel}\nopaque 한국어 and English\n`;
  const cli = async (agent, words, text = "", status = 0) => {
    const result = await agent.peer.ask({ kind: "cli", words, body: text });
    assert.ok(result.status === status, `lifecycle ${words[0]} returned an unexpected status`);
    return JSON.parse(result.stdout);
  };
  const sendAsk = async (source, target, phase) => {
    const envelope = { protocol_version: 1, message_id: context.newId(), call_id: context.newId(),
      source: source.session, target: target.session, kind: "ask", reply_to: null, room_id: null,
      deadline: Date.now() + 60_000, hop_count: 0, visited: [], body: `${body}${phase}` };
    const answer = await source.peer.ask({ kind: "request", command: { command: "send", envelope } });
    assert.ok(answer.answer === "accepted", "the lifecycle ask was not admitted");
    assert.ok(answer.receipt.message_id === envelope.message_id, "the lifecycle receipt names another message");
    return envelope;
  };
  const finishAsk = async (source, target, pending) => {
    const received = (await cli(target, ["inbox", "--from", source.session])).envelope;
    assert.ok(received.message_id === pending.message_id && received.call_id === pending.call_id,
      "handover changed the pending request identity");
    assert.ok(received.body === pending.body, "handover changed the opaque request");
    await cli(target, ["reply", received.message_id], body);
    const reply = (await cli(source, ["inbox", "--from", target.session])).envelope;
    assert.ok(reply.reply_to === pending.message_id && reply.call_id === pending.call_id,
      "handover changed the reply correlation");
    assert.ok(reply.body === body, "handover changed the opaque reply");
    assert.ok((await cli(source, ["inbox"], "", 1)).envelope === null, "a reply was delivered twice");
    assert.ok((await cli(target, ["inbox"], "", 1)).envelope === null, "a request was delivered twice");
  };
  const second = await context.open();
  await finishAsk(first, second, await sendAsk(first, second, "success"));
  await cli(first, ["ask", second.session, "--timeout", "1"], body, 1);
  assert.ok((await cli(second, ["inbox"], "", 1)).envelope === null, "an expired request was retained");

  const pendingUpgrade = await sendAsk(first, second, "upgrade");
  const upgraded = await context.startNext();
  assert.ok(upgraded.generation !== first.terminal.generation, "upgrade did not replace the Runtime image");
  await context.draining(first.terminal.generation);
  await finishAsk(first, second, pendingUpgrade);
  await context.stop(first); await context.stop(second);
  await context.endedGeneration(first.terminal.generation);

  const third = await context.open();
  const fourth = await context.open();
  const pendingRollback = await sendAsk(third, fourth, "rollback");
  const restored = await context.startOriginal();
  assert.ok(restored.identity.pid !== first.runtimePid, "rollback did not create a new Runtime process");
  assert.ok(restored.generation === first.terminal.generation, "rollback did not restore the original Runtime image");
  await context.draining(third.terminal.generation);
  await finishAsk(third, fourth, pendingRollback);

  const abandoned = await sendAsk(third, fourth, "crash");
  await context.crash(upgraded);
  await context.waitFor(() => third.peer.ended() && fourth.peer.ended(), "crashed generation peer closure");
  const replacement = await context.open();
  const list = await cli(replacement, ["list"]);
  assert.ok(!list.sessions.some((row) => [third.session, fourth.session].includes(row.session)),
    "the replacement Runtime replayed old session authority");
  assert.ok((await cli(replacement, ["inbox"], "", 1)).envelope === null,
    "the replacement Runtime replayed an abandoned body");
  const staleReply = await cli(replacement, ["reply", abandoned.message_id], body, 1);
  assert.ok(staleReply.answer === "refused", "the replacement Runtime revived an abandoned call");

  for (const shape of ["invalidJson", "bodyType", "oversizedPrefix"]) {
    const answer = await replacement.peer.ask({ kind: "malformed", shape, marker: sentinel });
    assert.ok(answer.closed || answer.refused, "a malformed invocation remained admitted");
    assert.ok(answer.bodyAbsent, "a malformed invocation echoed its opaque marker");
  }
  await context.stop(replacement);
  process.stdout.write("RUNTROL_COURIER_LIFECYCLE success=true timeout=true upgrade=true rollback=true crash=true peerClosure=true noReplay=true malformed=true\n");
}
