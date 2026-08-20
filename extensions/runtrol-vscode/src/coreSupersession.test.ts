import assert from "node:assert/strict";
import { test } from "node:test";

import type { CoreClient } from "./core/client";
import { ensureCurrentCore } from "./coreSupersession";
import type { Response } from "./protocol";

type Script = {
  digests: (string | null)[];
  retire?: () => Response;
};

/// A daemon stand-in that answers digests in order and scripts one retire outcome.
function fakeClient(script: Script): CoreClient & { retired: number; resets: number } {
  let digestCalls = 0;
  const fake = {
    retired: 0,
    resets: 0,
    ensureRuntime: async () => undefined,
    announcedBuildDigest: async () => {
      const digest = script.digests[Math.min(digestCalls, script.digests.length - 1)];
      digestCalls += 1;
      return digest;
    },
    once: async (request: { ask: string }) => {
      assert.equal(request.ask, "retire");
      fake.retired += 1;
      const retire = script.retire;
      if (!retire) throw new Error("retire was not expected");
      return { response: retire() };
    },
    reset: async () => {
      fake.resets += 1;
    },
  };
  return fake as unknown as CoreClient & { retired: number; resets: number };
}

test("a matching digest is current and nothing is asked to retire", async () => {
  const client = fakeClient({ digests: ["a".repeat(64)] });
  const outcome = await ensureCurrentCore(client, async () => "a".repeat(64));
  assert.deepEqual(outcome, { state: "current" });
  assert.equal(client.retired, 0);
});

test("no managed digest means somebody else's build and no supersession", async () => {
  const client = fakeClient({ digests: ["b".repeat(64)] });
  const outcome = await ensureCurrentCore(client, async () => null);
  assert.deepEqual(outcome, { state: "current" });
  assert.equal(client.retired, 0);
});

test("an older daemon retires and the successor's greeting proves the installed build", async () => {
  const installed = "c".repeat(64);
  const client = fakeClient({
    digests: ["old-digest", installed],
    retire: () => ({ say: "done" }) as Response,
  });
  const outcome = await ensureCurrentCore(client, async () => installed);
  assert.deepEqual(outcome, { state: "superseded" });
  assert.equal(client.retired, 1);
  assert.ok(client.resets >= 1, "the connection to the retiring daemon is dropped");
});

test("a daemon with live conversations answers busy, not legacy", async () => {
  const client = fakeClient({
    digests: ["old-digest"],
    retire: () => {
      throw new Error("2 conversation(s) still have a live process; retire waits for an idle machine");
    },
  });
  const outcome = await ensureCurrentCore(client, async () => "d".repeat(64));
  assert.equal(outcome.state, "busy");
});

test("a daemon that does not know retire is legacy", async () => {
  const client = fakeClient({
    digests: [null],
    retire: () => {
      throw new Error('no command called "retire"');
    },
  });
  const outcome = await ensureCurrentCore(client, async () => "e".repeat(64));
  assert.equal(outcome.state, "legacy");
});

test("a successor that still mismatches is reported instead of looping", async () => {
  const client = fakeClient({
    digests: ["old-digest", "still-old"],
    retire: () => ({ say: "done" }) as Response,
  });
  const outcome = await ensureCurrentCore(client, async () => "f".repeat(64));
  assert.equal(outcome.state, "legacy");
  assert.equal(client.retired, 1, "retire is asked once, never in a loop");
});
