import assert from "node:assert/strict";
import test from "node:test";

import type { NativeActivity } from "@runtrol/runtime-client";

import { projectNativeActivity } from "./nativeActivityProjection";

function activity(
  providerId: string,
  live: readonly string[],
  active: readonly string[],
  attachable: readonly string[] = [],
): NativeActivity {
  return { providerId, live, active, attachable };
}

test("a failed roster read revokes its old live proof instead of leaving Elsewhere forever", () => {
  const previous = new Map<string, ReadonlySet<string>>([
    ["claude", new Set(["old-claude"])],
    ["codex", new Set(["old-codex"])],
  ]);

  const projected = projectNativeActivity([
    ["claude", activity("claude", ["new-claude"], ["new-claude"], ["new-claude"])],
    ["codex", null],
  ], previous);

  assert.deepEqual([...projected.live], ["claude:new-claude"]);
  assert.deepEqual([...projected.active], ["claude:new-claude"]);
  assert.deepEqual([...projected.attachable], ["claude:new-claude"]);
  assert.deepEqual([...projected.unconfirmed], ["codex:old-codex"]);
  assert.deepEqual([...projected.liveByProvider.get("codex") ?? []], []);
  assert.deepEqual([...projected.activeByProvider.get("codex") ?? []], []);
  assert.deepEqual([...projected.attachableByProvider.get("codex") ?? []], []);
  assert.deepEqual([...projected.unconfirmedByProvider.get("codex") ?? []], ["old-codex"]);
  assert.deepEqual([...projected.discoveredProviders], ["claude"]);
});

test("an authoritative empty roster removes a conversation that has stopped", () => {
  const projected = projectNativeActivity([
    ["codex", activity("codex", [], [])],
  ], new Map([["codex", new Set(["stopped"])]]));

  assert.deepEqual([...projected.live], []);
  assert.deepEqual([...projected.active], []);
  assert.deepEqual([...projected.attachable], []);
  assert.deepEqual([...projected.unconfirmed], []);
  assert.deepEqual([...projected.discoveredProviders], []);
});

test("repeated failures keep the uncertain owner blocked without calling it live", () => {
  const first = projectNativeActivity(
    [["codex", null]],
    new Map([["codex", new Set(["possibly-live"])]]),
  );
  const second = projectNativeActivity(
    [["codex", null]],
    first.liveByProvider,
    first.unconfirmedByProvider,
  );

  assert.deepEqual([...second.live], []);
  assert.deepEqual([...second.attachable], []);
  assert.deepEqual([...second.unconfirmed], ["codex:possibly-live"]);
});

test("an attachment route is accepted only for a currently live identity", () => {
  const projected = projectNativeActivity([
    ["claude", activity("claude", ["live"], [], ["live", "stale"])],
  ], new Map());

  assert.deepEqual([...projected.attachable], ["claude:live"]);
});
