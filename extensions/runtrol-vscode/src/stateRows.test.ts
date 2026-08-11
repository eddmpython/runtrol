import assert from "node:assert/strict";
import test from "node:test";

import type { ProviderLine, SessionLine } from "./protocol";
import { providerRowsEqual, sessionRowsEqual } from "./stateRows";

const SESSION: SessionLine = {
  session: "session-1",
  provider: "provider-1",
  native: "native-1",
  workspace: "C:\\work",
  hot: true,
  doing: "working",
  looks_stuck: false,
};

const PROVIDER: ProviderLine = {
  id: "provider-1",
  display_name: "Provider One",
  usable: true,
  why_not: null,
};

test("equal snapshots do not require a state publication", () => {
  assert.equal(sessionRowsEqual([SESSION], [{ ...SESSION }]), true);
  assert.equal(providerRowsEqual([PROVIDER], [{ ...PROVIDER }]), true);
});

test("every visible session field invalidates the snapshot", () => {
  for (const changed of [
    { session: "session-2" },
    { provider: "provider-2" },
    { native: null },
    { workspace: "C:\\other" },
    { hot: false },
    { doing: "waiting" },
    { looks_stuck: true },
  ]) {
    assert.equal(sessionRowsEqual([SESSION], [{ ...SESSION, ...changed }]), false);
  }
  assert.equal(sessionRowsEqual([SESSION], []), false);
});

test("every visible provider field invalidates the snapshot", () => {
  for (const changed of [
    { id: "provider-2" },
    { display_name: "Provider Two" },
    { usable: false },
    { why_not: "missing" },
  ]) {
    assert.equal(providerRowsEqual([PROVIDER], [{ ...PROVIDER, ...changed }]), false);
  }
  assert.equal(providerRowsEqual([PROVIDER], []), false);
});
