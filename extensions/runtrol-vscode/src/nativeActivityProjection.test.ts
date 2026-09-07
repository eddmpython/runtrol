import assert from "node:assert/strict";
import test from "node:test";

import type { NativeActivity } from "@runtrol/runtime-client";

import { nativeProcessKey } from "./conversationList";
import { projectNativeActivity, unlistedLiveProviders } from "./nativeActivityProjection";

function activity(
  providerId: string,
  live: readonly string[],
  active: readonly string[],
  attachable: readonly string[] = [],
): NativeActivity {
  return { providerId, live, active, attachable };
}

test("a live conversation no row lists is asked for again, hosted or not; listed ones are not", () => {
  const live = new Map<string, ReadonlySet<string>>([
    ["claude", new Set(["n1", "n2", "n3"])],
    ["codex", new Set(["c1"])],
  ]);
  const listed = new Set([nativeProcessKey("claude", "n1"), nativeProcessKey("codex", "c1")]);
  const unlisted = unlistedLiveProviders(live, listed);

  assert.deepEqual([...unlisted], [["claude", "n2 n3"]]);
  assert.equal(
    unlistedLiveProviders(live, new Set([...listed, nativeProcessKey("claude", "n2"), nativeProcessKey("claude", "n3")])).size,
    0,
  );
});

test("a focus target is kept only while the same round proves the process live", () => {
  const projected = projectNativeActivity(
    [["claude", { ...activity("claude", ["n1"], []), focusable: ["n1", "n2"] }]],
    new Map(),
  );

  assert.deepEqual([...(projected.focusableByProvider.get("claude") ?? [])], ["n1"]);
  assert.equal(projected.focusable.size, 1);
});

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
  assert.deepEqual([...projected.refreshProviders], ["claude"]);
});

test("an authoritative empty roster removes a conversation that has stopped", () => {
  const projected = projectNativeActivity([
    ["codex", activity("codex", [], [])],
  ], new Map([["codex", new Set(["stopped"])]]));

  assert.deepEqual([...projected.live], []);
  assert.deepEqual([...projected.active], []);
  assert.deepEqual([...projected.attachable], []);
  assert.deepEqual([...projected.unconfirmed], []);
  assert.deepEqual([...projected.refreshProviders], []);
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

{
const idle = { providerId: "fixture", live: ["native"], active: [], attachable: [] };
const live = new Map([["fixture", new Set(["native"])]]);

test("fast turns and late title metadata invalidate once even while both observed states are idle", () => {
  const first = projectNativeActivity([["fixture", idle, [], "source:1"]], live);
  const same = projectNativeActivity([["fixture", idle, [], "source:1"]], live, new Map(), new Map(), first.revisions);
  assert.equal(same.refreshProviders.size, 0);
  const turn = projectNativeActivity([["fixture", idle, [], "source:2"]], live, new Map(), new Map(), same.revisions);
  assert.deepEqual([...turn.refreshProviders], ["fixture"]);
  const title = projectNativeActivity([["fixture", idle, [], "source:3"]], live, new Map(), new Map(), turn.revisions);
  assert.deepEqual([...title.refreshProviders], ["fixture"]);
});

test("unknown model activity preserves the proven owner without inventing a completed turn", () => {
  const unknown = projectNativeActivity([["fixture", idle, ["native"], "source:1"]], live,
    new Map(), live, new Map([["fixture", "source:1"]]));
  assert.deepEqual([...unknown.live], ["fixture:native"]);
  assert.deepEqual([...unknown.unknownActivity], ["fixture:native"]);
  assert.equal(unknown.unconfirmed.size, 0);
  assert.equal(unknown.active.size, 0);
  assert.equal(unknown.refreshProviders.size, 0);
});

test("lost proof retains its revision and returns as the same source without unnecessary catalogue work", () => {
  const missing = projectNativeActivity([["fixture", null]], live,
    new Map(), new Map(), new Map([["fixture", "source:5"]]));
  assert.deepEqual([...missing.unknownActivity], ["fixture:native"]);
  const restored = projectNativeActivity([["fixture", idle, [], "source:5"]], missing.liveByProvider,
    missing.unconfirmedByProvider, new Map(), missing.revisions);
  assert.equal(restored.refreshProviders.size, 0);
  assert.equal(restored.unconfirmed.size, 0);
});

}
