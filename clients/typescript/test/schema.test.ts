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
