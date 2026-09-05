// Explicit room rounds over the production Runtime and inherited CLI, using the owned admission cohort.
import assert from "node:assert/strict";

export async function roomJourney(cohort, cli, raw, body) {
  const [owner, second, third, outsider] = cohort;
  for (const members of [[owner, second], [owner, second, third]]) {
    const opened = await cli(owner, ["room", "open", ...members.slice(1).map((peer) => peer.session)]);
    const room = opened.room.id;
    assert.equal(opened.room.owner, owner.session);
    assert.equal(opened.room.speaker, owner.session);
    assert.deepEqual(new Set(opened.room.participants), new Set(members.map((peer) => peer.session)));
    assert.equal((await cli(outsider, ["room", "inspect", room], "", 1)).answer, "refused");
    assert.equal((await cli(second, ["room", "close", room], "", 1)).answer, "refused");
    assert.equal((await cli(second, ["room", "transfer", room, second.session], "", 1)).answer, "refused");
    assert.equal((await cli(second, ["room", "ask", room, owner.session], body, 1)).answer, "refused");
    assert.equal((await cli(owner, ["room", "ask", room, owner.session], body, 1)).answer, "refused");
    for (let round = 0; round < 6; round += 1) {
      const speaker = members[round % members.length];
      const target = members[(round + 1) % members.length];
      const selected = await cli(owner, ["room", "transfer", room, speaker.session]);
      assert.equal(selected.room.speaker, speaker.session);
      assert.equal(selected.room.rounds, round);
      const [answer, message] = await Promise.all([
        cli(speaker, ["room", "ask", room, target.session, "--timeout", "10"], body),
        (async () => {
          const message = (await cli(target, ["wait", "--from", speaker.session, "--timeout", "10"])).envelope;
          assert.equal(message.room_id, room);
          assert.equal(message.body, body);
          assert.equal(message.hop_count, 1, "each explicit round makes its first hop");
          assert.deepEqual(message.visited, [speaker.session]);
          await cli(target, ["reply", message.message_id], body);
          return message;
        })(),
      ]);
      const reply = answer.envelope;
      assert.equal(reply.reply_to, message.message_id);
      assert.equal(reply.call_id, message.call_id);
      assert.equal(reply.room_id, room);
      assert.equal(reply.body, body, "the final reply remains readable at the round ceiling");
    }
    const last = (await cli(owner, ["room", "inspect", room])).room;
    assert.equal(last.rounds, 6);
    assert.equal(last.in_flight, null);
    await cli(owner, ["room", "transfer", room, owner.session]);
    assert.equal((await cli(owner, ["room", "ask", room, second.session], body, 1)).answer, "refused");
    assert.equal((await cli(owner, ["room", "close", room])).answer, "room_closed");
    assert.equal((await cli(owner, ["room", "inspect", room], "", 1)).answer, "refused");
  }
  assert.equal((await raw(owner, { command: "room_open", participants: cohort.slice(0, 4).map((peer) => peer.session),
    deadline: Date.now() + 10_000 })).answer, "refused");

  const pendingRoom = (await cli(owner, ["room", "open", second.session, third.session])).room.id;
  await owner.peer.ask({ kind: "hold", key: "room", command: { command: "room_ask", room: pendingRoom,
    target: second.session, message_id: pendingRoom, body, timeout_ms: 10_000 } });
  const pending = (await cli(second, ["wait", "--from", owner.session, "--timeout", "10"])).envelope;
  assert.equal(pending.room_id, pendingRoom);
  assert.equal((await cli(owner, ["room", "transfer", pendingRoom, third.session], "", 1)).answer, "refused");
  assert.equal((await cli(third, ["room", "ask", pendingRoom, second.session], body, 1)).answer, "refused");
  await cli(owner, ["room", "close", pendingRoom]);
  assert.equal((await cli(second, ["reply", pending.message_id], body, 1)).answer, "refused");
  await owner.peer.ask({ kind: "release", key: "room" });

  const expiredRoom = (await cli(owner, ["room", "open", second.session, "--timeout", "1"])).room.id;
  await new Promise((resolve) => setTimeout(resolve, 1100));
  assert.equal((await cli(second, ["room", "inspect", expiredRoom], "", 1)).answer, "refused");
  for (const member of [owner, second, third]) {
    assert.equal((await cli(member, ["inbox"], "", 1)).envelope, null);
  }
  process.stdout.write("RUNTROL_COURIER_ROOMS two=true three=true explicitSix=true finalReply=true seventhRefused=true owner=true speaker=true membership=true close=true expiry=true\n");
}
