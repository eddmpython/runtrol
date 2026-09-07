import assert from "node:assert/strict";
import test from "node:test";

import { nativeTitleRefreshProviders, refreshProviderTitleBindings } from "./nativeTitleRefresh";
import { projectNativeActivity, unlistedLiveProviders } from "./nativeActivityProjection";
import { conversations } from "./conversationList";
import type { NativeChatLine, SessionLine } from "./runtimeTypes";

function session(overrides: Partial<SessionLine> = {}): SessionLine {
  return {
    sessionId: "runtime-1",
    providerId: "fixture",
    nativeSessionId: "native-1",
    label: null,
    workspace: "/workspace",
    hot: true,
    lifecycle: "hotIdle",
    waitingOn: null,
    sessionGeneration: 1,
    ...overrides,
  } as SessionLine;
}

test("a provider identity appearing asks that provider for its own conversation title", () => {
  const previous = [session({ nativeSessionId: null, lifecycle: "hotRunning" })];
  const current = [session({ nativeSessionId: "native-1", lifecycle: "hotRunning" })];
  assert.deepEqual(nativeTitleRefreshProviders(previous, current), ["fixture"]);
});

test("a completed turn refreshes one catalogue even when several sessions settle together", () => {
  const previous = [
    session({ sessionId: "runtime-1", lifecycle: "hotRunning" }),
    session({ sessionId: "runtime-2", nativeSessionId: "native-2", lifecycle: "hotRunning" }),
  ];
  const current = [
    session({ sessionId: "runtime-1", lifecycle: "hotIdle" }),
    session({ sessionId: "runtime-2", nativeSessionId: "native-2", lifecycle: "hotIdle" }),
  ];
  assert.deepEqual(nativeTitleRefreshProviders(previous, current), ["fixture"]);
});

test("ordinary index repaints do not reread provider catalogues", () => {
  const previous = [session({ waitingOn: null })];
  const current = [session({ waitingOn: "person" })];
  assert.deepEqual(nativeTitleRefreshProviders(previous, current), []);
});

test("a session without a provider-owned identity cannot have a provider-owned title", () => {
  const previous = [session({ nativeSessionId: null, lifecycle: "hotRunning" })];
  const current = [session({ nativeSessionId: null, lifecycle: "hotIdle" })];
  assert.deepEqual(nativeTitleRefreshProviders(previous, current), []);
});

test("a catalogue title refresh reaches every open surface of that provider and no other one", () => {
  const fixture = session();
  const other = session({ sessionId: "runtime-2", providerId: "other" });
  const refreshed: string[] = [];
  const binding = (value: SessionLine | null) => ({
    session: value,
    updateSession: (current: SessionLine) => refreshed.push(current.sessionId),
  });
  refreshProviderTitleBindings([
    binding(fixture),
    binding(other),
    binding(null),
  ], "fixture");
  assert.deepEqual(refreshed, ["runtime-1"]);
});

test("an already listed native gets its first title when its observed first turn settles", () => {
  const live = new Map([["fixture", new Set(["native-1"])]]);
  const running = projectNativeActivity([
    ["fixture", { providerId: "fixture", live: ["native-1"], active: ["native-1"] }],
  ], live);
  const settled = projectNativeActivity([
    ["fixture", { providerId: "fixture", live: ["native-1"], active: [] }],
  ], live, new Map(), running.activeByProvider);
  const chat = { providerId: "fixture", nativeSessionId: "native-1", cwd: "/workspace",
    additionalDirectories: [], title: null, updatedAt: null, resume: "available",
    alreadyManagedAs: null, adoptionToken: "owned-fixture" } as NativeChatLine;
  assert.equal(conversations([], [], [chat], null)[0]?.title, "Unnamed conversation");
  assert.equal(settled.refreshProviders.size, 1);
  assert.equal(unlistedLiveProviders(settled.liveByProvider, new Set(["fixture:native-1"])).size, 0);
  assert.deepEqual(nativeTitleRefreshProviders([], []), [], "there is no supervised session edge");
  const requested = [...settled.refreshProviders];
  assert.deepEqual(requested, ["fixture"]);
  const refreshed = requested.includes(chat.providerId) ? { ...chat, title: "Owned first title" } : chat;
  assert.equal(conversations([], [], [refreshed], null)[0]?.title, "Owned first title");
  const unchanged = projectNativeActivity([["fixture", { providerId: "fixture", live: ["native-1"], active: [] }]],
    live, new Map(), settled.activeByProvider);
  assert.equal(unchanged.refreshProviders.size, 0);
});

test("native title reads coalesce settled peers and do not mistake unavailable proof for completion", () => {
  const previous = new Map([["fixture", new Set(["n1", "n2"])], ["other", new Set(["o1"])]]);
  const failed = projectNativeActivity([["fixture", null]], previous, new Map(), previous);
  assert.equal(failed.refreshProviders.size, 0);
  const settled = projectNativeActivity([
    ["fixture", { providerId: "fixture", live: ["n1", "n2"], active: [] }],
    ["other", { providerId: "other", live: ["o1"], active: ["o1"] }],
  ], previous, new Map(), previous);
  assert.deepEqual([...settled.refreshProviders], ["fixture"]);
  const ended = projectNativeActivity([["fixture", { providerId: "fixture", live: [], active: [] }]], previous, new Map(), previous);
  assert.deepEqual([...ended.refreshProviders], ["fixture"]);
});
