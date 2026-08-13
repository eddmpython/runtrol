import assert from "node:assert/strict";
import test from "node:test";

import type { ProviderLine, SessionLine } from "./runtimeTypes";
import { providerRowsEqual, sessionRowsEqual } from "./stateRows";

const SESSION: SessionLine = {
  sessionId: "session-1",
  providerId: "provider-1",
  nativeSessionId: "native-1",
  label: "Release repair",
  workspace: "C:\\work",
  hot: true,
  lifecycle: "hotRunning",
  looksStuck: false,
  sessionGeneration: 1,
};

const PROVIDER: ProviderLine = {
  providerId: "provider-1",
  displayName: "Provider One",
  installation: { state: "usable", version: "1.0.0" },
};

test("equal snapshots do not require a state publication", () => {
  assert.equal(sessionRowsEqual([SESSION], [{ ...SESSION }]), true);
  assert.equal(providerRowsEqual([PROVIDER], [{ ...PROVIDER }]), true);
});

test("every visible session field invalidates the snapshot", () => {
  for (const changed of [
    { sessionId: "session-2" },
    { providerId: "provider-2" },
    { nativeSessionId: null },
    { label: null },
    { workspace: "C:\\other" },
    { hot: false },
    { lifecycle: "hotIdle" as const },
    { looksStuck: true },
    { sessionGeneration: 2 },
  ]) {
    assert.equal(sessionRowsEqual([SESSION], [{ ...SESSION, ...changed }]), false);
  }
  assert.equal(sessionRowsEqual([SESSION], []), false);
});

test("every visible provider field invalidates the snapshot", () => {
  for (const changed of [
    { providerId: "provider-2" },
    { displayName: "Provider Two" },
    { installation: { state: "missing" as const, why: "missing" } },
  ]) {
    assert.equal(providerRowsEqual([PROVIDER], [{ ...PROVIDER, ...changed }]), false);
  }
  assert.equal(providerRowsEqual([PROVIDER], []), false);
});
