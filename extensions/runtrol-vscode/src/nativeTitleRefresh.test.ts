import assert from "node:assert/strict";
import test from "node:test";

import { nativeTitleRefreshProviders, refreshProviderTitleBindings } from "./nativeTitleRefresh";
import type { SessionLine } from "./runtimeTypes";

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
