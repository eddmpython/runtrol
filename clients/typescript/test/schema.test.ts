import assert from "node:assert/strict";
import { test } from "node:test";

import { RuntimeProtocolError } from "../src/errors.js";
import { validatePublic } from "../src/schema.js";

function pendingApproval(subjectDigest: readonly number[]): object {
  return {
    approvals: [{
      approvalId: "approval_fixture",
      kind: "command",
      risk: "high",
      options: [{
        optionId: 1,
        label: "Reject",
        kind: "rejectOnce",
        unavailable: null,
      }],
      subject: { command: "fixture" },
      subjectIncomplete: false,
      subjectDigest,
      expiresAtMs: 2_000_000_000_000,
    }],
  };
}

test("approval digests accept the complete unsigned byte range", () => {
  const digest = Array.from({ length: 32 }, (_unused, index) => index === 0 ? 0xff : index);
  assert.deepEqual(
    validatePublic("PendingApprovalList", pendingApproval(digest)),
    pendingApproval(digest),
  );
});

test("approval digests reject values outside the unsigned byte range", () => {
  const digest = Array.from({ length: 32 }, (_unused, index) => index === 0 ? 0x100 : index);
  assert.throws(
    () => validatePublic("PendingApprovalList", pendingApproval(digest)),
    RuntimeProtocolError,
  );
});

function usage(amount: number): object {
  return {
    providers: [{
      providerId: "claude",
      reached: false,
      atMs: 1_700_000_000_000,
      cost: { amount, currency: "USD" },
    }],
  };
}

test("a reported cost keeps its fraction, which is how money is written", () => {
  // Measured: one Claude turn reported 0.4306 USD. Reading a real number as an unsigned integer rejected every
  // cost a service ever sent, and the panel showed a schema violation where the amount belonged.
  assert.deepEqual(validatePublic("ProviderUsageList", usage(0.4306)), usage(0.4306));
  assert.deepEqual(validatePublic("ProviderUsageList", usage(0)), usage(0));
  assert.deepEqual(validatePublic("ProviderUsageList", usage(1234.5)), usage(1234.5));
});

test("a cost that is not a finite number is refused", () => {
  for (const amount of [Number.NaN, Number.POSITIVE_INFINITY]) {
    assert.throws(() => validatePublic("ProviderUsageList", usage(amount)), RuntimeProtocolError);
  }
});
