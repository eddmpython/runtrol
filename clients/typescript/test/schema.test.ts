import assert from "node:assert/strict";
import { test } from "node:test";

import { RuntimeProtocolError } from "../src/errors.js";
import { validatePublic } from "../src/schema.js";
import { VALIDATION_SCHEMA } from "../src/generated/schema.js";

test("an unread account remains distinct from unsupported and signed-out responses", () => {
  for (const status of ["unread", "unpublished", "signedOut", "signedIn"]) {
    const account = { status, why: "fixture account result", checkedAtMs: 25 };
    assert.deepEqual(validatePublic("ProviderAccount", account), account);
  }
});

function pendingApproval(subjectDigest: readonly unknown[]): object {
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
  for (const invalid of [0x100, -1, 0.5, "1", null, true, {}, [], Number.NaN, Infinity]) {
    const digest = Array.from({ length: 32 }, (_unused, index) => index === 0 ? invalid : index);
    assert.throws(
      () => validatePublic("PendingApprovalList", pendingApproval(digest)),
      RuntimeProtocolError,
    );
  }
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

test("validating and then mutating one wire value cannot change the shared validation graph", () => {
  const before = JSON.stringify(VALIDATION_SCHEMA);
  const account = { status: "signedIn", why: "fixture", checkedAtMs: 25 };
  assert.equal(validatePublic("ProviderAccount", account), account);
  account.status = "not-a-status";
  assert.throws(() => validatePublic("ProviderAccount", account), RuntimeProtocolError);
  const next = Object.freeze({ status: "signedOut", why: "another fixture", checkedAtMs: 26 });
  assert.equal(validatePublic("ProviderAccount", next), next);
  assert.equal(JSON.stringify(VALIDATION_SCHEMA), before);
});
