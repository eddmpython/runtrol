import assert from "node:assert/strict";
import test from "node:test";

import { FrameDecoder, MAX_FRAME_BYTES, encodeFrame } from "./framing";

test("frames preserve order across split and coalesced chunks", () => {
  const expected = Array.from({ length: 500 }, (_, index) => Buffer.from(`frame-${index}`));
  const wire = Buffer.concat(expected.map(encodeFrame));
  const decoder = new FrameDecoder();
  const actual: Buffer[] = [];

  for (let offset = 0; offset < wire.length; offset += 7) {
    decoder.push(wire.subarray(offset, offset + 7));
    actual.push(...decoder.take());
  }
  actual.push(...decoder.take());

  assert.deepEqual(actual.map(String), expected.map(String));
  assert.equal(decoder.bufferedBytes, 0);
});

test("an oversized prefix is rejected before payload bytes arrive", () => {
  const decoder = new FrameDecoder();
  const header = Buffer.alloc(4);
  header.writeUInt32BE(MAX_FRAME_BYTES + 1);
  assert.throws(() => decoder.push(header), /exceeds/);
  assert.equal(decoder.bufferedBytes, 4);
});

test("a decode turn is bounded even when many frames arrive together", () => {
  const decoder = new FrameDecoder();
  decoder.push(Buffer.concat(Array.from({ length: 100 }, () => encodeFrame(Buffer.from("x")))));
  assert.equal(decoder.take().length, 64);
  assert.equal(decoder.take().length, 36);
});

