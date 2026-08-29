import assert from "node:assert/strict";
import test from "node:test";

import { ProviderHelpCache } from "./providerHelpCache";

test("the same set of services is asked once, a changed set is asked again, and a failed ask is retried", async () => {
  const asked: string[] = [];
  let failing = false;
  const cache = new ProviderHelpCache(async (providerId) => {
    asked.push(providerId);
    if (failing) throw new Error("the daemon connection closed");
    return { signOut: providerId === "claude" ? "claude auth logout" : null };
  });
  let changed = 0;
  cache.onDidChange(() => { changed += 1; });

  await cache.refresh(["claude", "codex"]);
  assert.deepEqual(asked, ["claude", "codex"]);
  assert.equal(cache.signOutFor("claude"), "claude auth logout");
  assert.equal(cache.signOutFor("codex"), null);
  assert.equal(changed, 1);

  await cache.refresh(["codex", "claude"]);
  assert.equal(asked.length, 2, "the same set in another order is not asked again");

  failing = true;
  await cache.refresh(["claude", "codex", "grok"]);
  assert.equal(asked.length, 5);
  assert.equal(cache.signOutFor("claude"), "claude auth logout", "a failed ask keeps the earlier answer");
  failing = false;
  await cache.refresh(["claude", "codex", "grok"]);
  assert.equal(asked.length, 8, "a set whose ask failed is asked again");
  cache.dispose();
});
