import assert from "node:assert/strict";
import { createInterface } from "node:readline";

import { base64UrlDecode, base64UrlEncode, text, utf8 } from "../src/bytes.js";
import { pairingInitiator, sessionInitiator } from "../src/noise.js";
import { RecordChannel, decodeRecord, encodeRecord } from "../src/records.js";

const lines = createInterface({ input: process.stdin, crlfDelay: Infinity })[Symbol.asyncIterator]();
const configuration = await receive();
const identity = await crypto.subtle.generateKey({ name: "X25519" }, false, ["deriveBits"]);
const phonePublicKey = base64UrlEncode(await crypto.subtle.exportKey("raw", identity.publicKey));

const pairing = await pairingInitiator(
  identity,
  configuration.pc_public_key,
  configuration.pairing_secret,
);
send({
  phone_public_key: phonePublicKey,
  first: base64UrlEncode(encodeRecord(await pairing.writeFirst(utf8(JSON.stringify({
    name: "Interop phone",
    platform: "WebCrypto",
  }))))),
});
const pairingAnswer = await receive();
const paired = await pairing.finish(decodeRecord(base64UrlDecode(pairingAnswer.reply)));
assert.deepEqual(JSON.parse(text(paired.payload)), {
  credential: "11".repeat(32),
  scopes: ["session.list"],
});
const pairingChannel = new RecordChannel(paired.cipher);
send({ record: base64UrlEncode((await pairingChannel.sealFrame(utf8("paired transport from WebCrypto")))[0]) });
const pairingTransportAnswer = await receive();
assert.equal(
  text(await pairingChannel.openRecord(base64UrlDecode(pairingTransportAnswer.record))),
  "paired transport from Rust",
);

const session = await sessionInitiator(
  identity,
  configuration.pc_public_key,
  configuration.relay_origin,
  base64UrlDecode(configuration.peer_id, 32),
);
send({
  phone_public_key: phonePublicKey,
  first: base64UrlEncode(encodeRecord(await session.writeFirst(new Uint8Array()))),
});
const sessionAnswer = await receive();
const connected = await session.finish(decodeRecord(base64UrlDecode(sessionAnswer.reply)));
assert.equal(connected.payload.byteLength, 0);
const sessionChannel = new RecordChannel(connected.cipher);
send({ record: base64UrlEncode((await sessionChannel.sealFrame(utf8("session transport from WebCrypto")))[0]) });
const sessionTransportAnswer = await receive();
assert.equal(
  text(await sessionChannel.openRecord(base64UrlDecode(sessionTransportAnswer.record))),
  "session transport from Rust",
);
send({ ok: true });

async function receive() {
  const next = await lines.next();
  if (next.done) throw new Error("Rust peer ended before its next command");
  return JSON.parse(next.value);
}

function send(value) {
  process.stdout.write(`${JSON.stringify(value)}\n`);
}
