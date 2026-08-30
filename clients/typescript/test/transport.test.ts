import assert from "node:assert/strict";
import { once } from "node:events";
import { mkdtemp, rm } from "node:fs/promises";
import { createServer, type Server, type Socket } from "node:net";
import { tmpdir } from "node:os";
import path from "node:path";
import { test } from "node:test";
import { setTimeout as delay } from "node:timers/promises";

import { RuntimeProtocolError } from "../src/errors.js";
import { PUBLIC_LIMITS } from "../src/generated/protocol.js";
import { connectLocalTransport } from "../src/transport.js";

test("receiving a frame accepts a header and body split across socket chunks", async () => {
  await withTransport(async (transport, socket) => {
    const payload = Buffer.from("split header and body", "utf8");
    const header = frameHeader(payload.byteLength);
    const received = transport.receive();

    socket.write(header.subarray(0, 2));
    await delay(10);
    socket.write(header.subarray(2));
    await delay(10);
    socket.write(payload.subarray(0, 5));
    await delay(10);
    socket.write(payload.subarray(5));

    assert.deepEqual(Buffer.from(await within(received, 2_000, "split Runtime frame")), payload);
  });
});

test("receiving coalesced frames preserves exact frame boundaries", async () => {
  await withTransport(async (transport, socket) => {
    const first = Buffer.from("first", "utf8");
    const second = Buffer.from("second", "utf8");
    socket.cork();
    socket.write(frame(first));
    socket.write(frame(second));
    socket.uncork();

    assert.deepEqual(Buffer.from(await transport.receive()), first);
    assert.deepEqual(Buffer.from(await transport.receive()), second);
  });
});

test("receiving an empty frame returns an empty payload", async () => {
  await withTransport(async (transport, socket) => {
    socket.write(frame(Buffer.alloc(0)));

    assert.equal((await transport.receive()).byteLength, 0);
  });
});

test("receiving a maximum-size frame keeps the public limit inclusive", async () => {
  await withTransport(async (transport, socket) => {
    const payload = Buffer.alloc(PUBLIC_LIMITS.maxFrameBytes, 0x5a);
    const received = transport.receive();
    socket.write(frameHeader(payload.byteLength));
    socket.write(payload);

    const answer = await within(received, 10_000, "maximum-size Runtime frame");
    const answerBuffer = Buffer.from(answer.buffer, answer.byteOffset, answer.byteLength);
    assert.equal(answerBuffer.byteLength, PUBLIC_LIMITS.maxFrameBytes);
    assert.equal(answerBuffer.equals(payload), true);
  });
});

test("receiving an oversized frame rejects its header before reading a body", async () => {
  await withTransport(async (transport, socket) => {
    socket.write(frameHeader(PUBLIC_LIMITS.maxFrameBytes + 1));

    await assert.rejects(
      transport.receive(),
      (error) => error instanceof RuntimeProtocolError
        && error.message === `Runtime announced ${PUBLIC_LIMITS.maxFrameBytes + 1} frame bytes, above ${PUBLIC_LIMITS.maxFrameBytes}`,
    );
  });
});

test("closing a local transport retires its pipe before the next connection", async () => {
  const directory = await mkdtemp(path.join(tmpdir(), "runtrol-runtime-transport-"));
  const endpoint = process.platform === "win32"
    ? `\\\\.\\pipe\\runtrol-runtime-transport-${process.pid}-${Date.now()}`
    : path.join(directory, "runtime.sock");
  const server = createServer();
  try {
    server.listen(endpoint);
    await once(server, "listening");

    const first = await open(server, endpoint);
    const firstEnded = once(first.socket, "end");
    first.transport.close();
    await within(firstEnded, 2_000, "first local transport retirement");
    assert.equal(first.socket.readableEnded, true);

    const second = await open(server, endpoint);
    const secondEnded = once(second.socket, "end");
    second.transport.close();
    await within(secondEnded, 2_000, "second local transport retirement");
    assert.equal(second.socket.readableEnded, true);
  } finally {
    if (server.listening) {
      const serverClosed = once(server, "close");
      server.close();
      await serverClosed;
    }
    await rm(directory, { recursive: true, force: true });
  }
});

test("aborting a local transport wakes a pending read without waiting for the server", async () => {
  const directory = await mkdtemp(path.join(tmpdir(), "runtrol-runtime-transport-abort-"));
  const endpoint = process.platform === "win32"
    ? `\\\\.\\pipe\\runtrol-runtime-transport-abort-${process.pid}-${Date.now()}`
    : path.join(directory, "runtime.sock");
  const server = createServer();
  let socket: Socket | null = null;
  try {
    server.listen(endpoint);
    await once(server, "listening");
    const opened = await open(server, endpoint);
    socket = opened.socket;
    const pending = opened.transport.receive();
    opened.transport.abort?.();
    await assert.rejects(
      within(pending, 2_000, "aborted local transport read"),
      /Runtime frame read failed|Runtime closed during a frame/u,
    );
  } finally {
    socket?.destroy();
    if (server.listening) {
      const serverClosed = once(server, "close");
      server.close();
      await serverClosed;
    }
    await rm(directory, { recursive: true, force: true });
  }
});

async function open(server: Server, endpoint: string): Promise<{
  transport: Awaited<ReturnType<typeof connectLocalTransport>>;
  socket: Socket;
}> {
  const accepted = once(server, "connection") as Promise<[Socket]>;
  const transport = await connectLocalTransport(endpoint);
  const [socket] = await accepted;
  return { transport, socket };
}

async function withTransport(
  run: (
    transport: Awaited<ReturnType<typeof connectLocalTransport>>,
    socket: Socket,
  ) => Promise<void>,
): Promise<void> {
  const directory = await mkdtemp(path.join(tmpdir(), "runtrol-runtime-transport-frame-"));
  const endpoint = process.platform === "win32"
    ? `\\\\.\\pipe\\runtrol-runtime-transport-frame-${process.pid}-${Date.now()}`
    : path.join(directory, "runtime.sock");
  const server = createServer();
  let transport: Awaited<ReturnType<typeof connectLocalTransport>> | null = null;
  let socket: Socket | null = null;
  try {
    server.listen(endpoint);
    await once(server, "listening");
    const opened = await open(server, endpoint);
    transport = opened.transport;
    socket = opened.socket;
    await run(transport, socket);
  } finally {
    transport?.abort?.();
    socket?.destroy();
    if (server.listening) {
      const serverClosed = once(server, "close");
      server.close();
      await serverClosed;
    }
    await rm(directory, { recursive: true, force: true });
  }
}

function frame(payload: Buffer): Buffer {
  const result = Buffer.allocUnsafe(4 + payload.byteLength);
  result.writeUInt32BE(payload.byteLength);
  payload.copy(result, 4);
  return result;
}

function frameHeader(length: number): Buffer {
  const result = Buffer.allocUnsafe(4);
  result.writeUInt32BE(length);
  return result;
}

async function within<T>(work: Promise<T>, milliseconds: number, label: string): Promise<T> {
  let timer: NodeJS.Timeout | undefined;
  try {
    return await Promise.race([
      work,
      new Promise<never>((_resolve, reject) => {
        timer = setTimeout(() => reject(new Error(`${label} exceeded ${milliseconds} ms`)), milliseconds);
      }),
    ]);
  } finally {
    if (timer) clearTimeout(timer);
  }
}
