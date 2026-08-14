import assert from "node:assert/strict";
import { once } from "node:events";
import { mkdtemp, rm } from "node:fs/promises";
import { createServer, type Server, type Socket } from "node:net";
import { tmpdir } from "node:os";
import path from "node:path";
import { test } from "node:test";

import { connectLocalTransport } from "../src/transport.js";

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

async function open(server: Server, endpoint: string): Promise<{
  transport: Awaited<ReturnType<typeof connectLocalTransport>>;
  socket: Socket;
}> {
  const accepted = once(server, "connection") as Promise<[Socket]>;
  const transport = await connectLocalTransport(endpoint);
  const [socket] = await accepted;
  return { transport, socket };
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
