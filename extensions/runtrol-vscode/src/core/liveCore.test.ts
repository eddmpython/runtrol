import { execFile } from "node:child_process";
import { mkdtemp, rm } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import assert from "node:assert/strict";
import test from "node:test";

import { CoreClient } from "./client";
import { FrameTransport } from "./framing";
import type { CoreLocator } from "./locator";
import { WIRE_VERSION } from "../protocol";

const core = process.env.RUNTROL_TEST_CORE;

test("the extension framing greets and lists through a real core", { skip: !core }, async () => {
  const home = await mkdtemp(path.join(os.tmpdir(), "runtrol-vscode-live-"));
  let endpoint = "";
  try {
    endpoint = await discover(core as string, home);
    const transport = await FrameTransport.connect(endpoint);
    await transport.send({ ask: "hello", with: { wire: WIRE_VERSION } });
    const welcome = JSON.parse((await transport.receive()).toString("utf8"));
    assert.equal(welcome.say, "welcome");
    assert.ok(Array.isArray(welcome.with.providers));

    await transport.send({ ask: "list" });
    const listing = JSON.parse((await transport.receive()).toString("utf8"));
    assert.equal(listing.say, "sessions");
    assert.deepEqual(listing.with.sessions, []);

    const index = await FrameTransport.connect(endpoint);
    await index.send({ ask: "hello", with: { wire: WIRE_VERSION } });
    assert.equal(JSON.parse((await index.receive()).toString("utf8")).say, "welcome");
    await index.send({ ask: "watchSessions" });
    assert.equal(JSON.parse((await index.receive()).toString("utf8")).say, "watchingSessions");
    const snapshot = JSON.parse((await index.receive()).toString("utf8"));
    assert.equal(snapshot.say, "sessions");
    assert.deepEqual(snapshot.with.sessions, []);

    let locations = 0;
    const client = new CoreClient({
      locate: async () => {
        locations += 1;
        return { executable: core as string, endpoint };
      },
    } as CoreLocator);
    for (let attempt = 0; attempt < 40; attempt += 1) {
      const refreshed = await client.once({ ask: "list" });
      assert.equal(refreshed.response.say, "sessions", `refresh ${attempt} lists sessions`);
    }
    assert.equal(locations, 1, "refreshes reuse one greeted command connection");
    client.dispose();

    await transport.send({ ask: "stopEverything" });
    await assert.rejects(transport.receive(), /closed|connection/i);
    index.close();
  } finally {
    if (endpoint) {
      await stopIfRunning(endpoint);
    }
    await rm(home, { recursive: true, force: true });
  }
});

function discover(executable: string, home: string): Promise<string> {
  return new Promise((resolve, reject) => {
    execFile(
      executable,
      ["endpoint"],
      {
        encoding: "utf8",
        timeout: 15_000,
        windowsHide: true,
        env: { ...process.env, RUNTROL_HOME: home },
      },
      (error, stdout, stderr) => {
        if (error) {
          reject(new Error(stderr.trim() || error.message));
        } else {
          resolve(stdout.trim());
        }
      },
    );
  });
}

async function stopIfRunning(endpoint: string): Promise<void> {
  try {
    const transport = await FrameTransport.connect(endpoint, 250);
    await transport.send({ ask: "hello", with: { wire: WIRE_VERSION } });
    await transport.receive();
    await transport.send({ ask: "stopEverything" });
    transport.close();
  } catch {
    // The successful path already stopped it.
  }
}
