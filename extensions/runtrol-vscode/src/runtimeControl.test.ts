import assert from "node:assert/strict";
import { test } from "node:test";

import { RuntimeRequestError, type ControlLease } from "@runtrol/runtime-client";

import {
  cachedControlAction,
  restorableControls,
  sessionDisappearedAfterCool,
} from "./runtimeControl";

const lease: ControlLease = {
  sessionId: "session_1",
  leaseId: "lease_1",
  leaseGeneration: 7,
  sessionGeneration: 3,
  expiresAtMs: 20_000,
};

test("reuses an unexpired lease across session lifecycle generations", () => {
  assert.equal(cachedControlAction(lease, 10_000), "reuse");
});

test("renews a lease near expiry without reacquiring it", () => {
  assert.equal(cachedControlAction(lease, 16_000), "renew");
});

test("acquires control only when no live cached lease remains", () => {
  assert.equal(cachedControlAction(undefined, 10_000), "acquire");
  assert.equal(cachedControlAction(lease, 20_000), "acquire");
});

test("accepts only session disappearance after a completed cool", () => {
  assert.equal(sessionDisappearedAfterCool(new RuntimeRequestError({
    code: "sessionNotFound",
    message: "the session has no remaining pointer",
    retryable: false,
    correlationId: "correlation_1",
    operatorAction: null,
  })), true);
  assert.equal(sessionDisappearedAfterCool(new RuntimeRequestError({
    code: "runtimeUnavailable",
    message: "the Runtime is unavailable",
    retryable: true,
    correlationId: "correlation_2",
    operatorAction: null,
  })), false);
  assert.equal(sessionDisappearedAfterCool(new Error("sessionNotFound")), false);
});

test("restores only unexpired leases from the same Runtime instance", () => {
  const future = { ...lease, expiresAtMs: 20_001 };
  const expired = { ...lease, sessionId: "session_2", expiresAtMs: 20_000 };
  const state = { runtimeInstanceId: "runtime_1", leases: [future, expired] };
  assert.deepEqual(restorableControls(state, "runtime_1", 20_000), [future]);
  assert.deepEqual(restorableControls(state, "runtime_2", 20_000), []);
  assert.deepEqual(restorableControls(undefined, "runtime_1", 20_000), []);
});
