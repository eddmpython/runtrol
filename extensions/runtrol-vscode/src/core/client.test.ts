import { mkdtemp, rm } from "node:fs/promises";
import net, { type Socket } from "node:net";
import os from "node:os";
import path from "node:path";
import assert from "node:assert/strict";
import { test } from "node:test";

import { CoreClient } from "./client";
import { encodeFrame, FrameDecoder } from "./framing";
import type { CoreLocator } from "./locator";
import { WIRE_VERSION } from "../protocol";

test("a read-only command reconnects once when the Core closes before greeting", async () => {
  const directory = await mkdtemp(path.join(os.tmpdir(), "runtrol-core-client-"));
  const endpoint = process.platform === "win32"
    ? `\\\\.\\pipe\\runtrol-core-client-${process.pid}-${Date.now()}`
    : path.join(directory, "core.sock");
  const server = net.createServer();
  const sockets = new Set<Socket>();
  let connections = 0;
  server.on("connection", (socket) => {
    connections += 1;
    const connection = connections;
    const decoder = new FrameDecoder();
    sockets.add(socket);
    socket.on("close", () => sockets.delete(socket));
    socket.on("data", (chunk) => {
      decoder.push(chunk);
      for (const frame of decoder.take()) {
        const request = JSON.parse(frame.toString("utf8")) as { ask?: string };
        if (request.ask === "hello") {
          if (connection === 1) {
            socket.end();
            continue;
          }
          socket.write(encodeFrame(Buffer.from(JSON.stringify({
            say: "welcome",
            with: {
              wire: WIRE_VERSION,
              providers: [],
              device: null,
              push_public_key: null,
            },
          }))));
        } else if (request.ask === "missionList" && connection === 2) {
          socket.write(encodeFrame(Buffer.from(JSON.stringify({ say: "missions", with: [] }))));
        }
      }
    });
  });

  try {
    await new Promise<void>((resolve, reject) => {
      server.once("error", reject);
      server.listen(endpoint, resolve);
    });
    const client = new CoreClient({
      locate: async () => ({ executable: "runtrol", endpoint }),
    } as CoreLocator);
    const { response } = await client.read({ ask: "missionList" });
    assert.equal(response.say, "missions");
    assert.equal(connections, 2);
    client.dispose();
  } finally {
    for (const socket of sockets) socket.destroy();
    await new Promise<void>((resolve) => server.close(() => resolve()));
    await rm(directory, { recursive: true, force: true });
  }
});
