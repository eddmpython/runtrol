import assert from "node:assert/strict";
import { once } from "node:events";
import { mkdtemp, rm } from "node:fs/promises";
import { createServer, type Socket } from "node:net";
import { tmpdir } from "node:os";
import path from "node:path";
import test from "node:test";

import { FrameDecoder, FrameTransport, MAX_FRAME_BYTES, encodeFrame } from "./framing";

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

test("closing a private transport yields its local pipe gracefully", { timeout: 5_000 }, async () => {
  const directory = await mkdtemp(path.join(tmpdir(), "runtrol-private-transport-"));
  const endpoint = process.platform === "win32"
    ? `\\\\.\\pipe\\runtrol-private-transport-${process.pid}-${Date.now()}`
    : path.join(directory, "private.sock");
  const server = createServer();
  let accepted: Socket | undefined;
  try {
    server.listen(endpoint);
    await once(server, "listening");
    const connection = once(server, "connection") as Promise<[Socket]>;
    const transport = await FrameTransport.connect(endpoint);
    [accepted] = await connection;
    const ended = once(accepted, "end");
    transport.close();
    await ended;
    assert.equal(accepted.readableEnded, true);
  } finally {
    accepted?.destroy();
    if (server.listening) {
      const serverClosed = once(server, "close");
      server.close();
      await serverClosed;
    }
    await rm(directory, { recursive: true, force: true });
  }
});
