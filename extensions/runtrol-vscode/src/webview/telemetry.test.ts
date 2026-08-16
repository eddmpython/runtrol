import assert from "node:assert/strict";
import test from "node:test";

import { limitTelemetry, usageTelemetry } from "./telemetry";

test("context, cost, and both limit window fields remain distinct", () => {
  assert.deepEqual(
    usageTelemetry({ used: 12_000, size: 128_000, cost: { amount: 1.25, currency: "USD" } }),
    { used: 12_000, size: 128_000, amount: 1.25, currency: "USD" },
  );
  assert.deepEqual(
    limitTelemetry({ used_percent: 42, resets_at: 1_800_000, window_minutes: 300 }),
    { usedPercent: 42, resetsAt: 1_800_000, windowMinutes: 300 },
  );
});

test("malformed provider telemetry cannot create a misleading gauge", () => {
  assert.deepEqual(
    usageTelemetry({ used: -1, size: Number.POSITIVE_INFINITY, cost: { amount: "1", currency: 2 } }),
    { used: null, size: null, amount: null, currency: "" },
  );
  assert.equal(limitTelemetry({ used_percent: "42" }), null);
  assert.equal(limitTelemetry(null), null);
});
