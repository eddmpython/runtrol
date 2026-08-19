import assert from "node:assert/strict";
import test from "node:test";

import type { ProviderLine, ProviderUsageGauge } from "./runtimeTypes";
import { usageDetail, usageRows } from "./usageDisplay";

const NOW = Date.parse("2026-08-18T12:00:00Z");

const PROVIDERS = [
  { providerId: "claude", displayName: "Claude Code", icon: "claude" },
  { providerId: "codex", displayName: "Codex", icon: "openai" },
] as unknown as ProviderLine[];

function gauge(overrides: Partial<ProviderUsageGauge>): ProviderUsageGauge {
  return {
    providerId: "codex",
    reached: false,
    atMs: NOW - 30_000,
    ...overrides,
  } as ProviderUsageGauge;
}

test("a provider that reports a percentage shows it, in its own number", () => {
  const detail = usageDetail(
    gauge({ primary: { usedPercent: 87, resetsAtMs: NOW + 23 * 60_000, windowMinutes: 300 } }),
    NOW,
  );
  assert.equal(detail, "87% · resets in 23m");
});

test("a provider that reports only a reset still has something true to say", () => {
  // Measured on a real turn: one CLI reports which window governs and when it resets, never how full it is.
  // Showing nothing for it would read as "no limit exists" about the account most in use.
  const detail = usageDetail(gauge({ primary: { resetsAtMs: NOW + 2 * 3_600_000 } }), NOW);
  assert.equal(detail, "resets in 2h");
});

test("a blocking limit is the first thing the line says", () => {
  const detail = usageDetail(
    gauge({ reached: true, primary: { usedPercent: 100, resetsAtMs: NOW + 90 * 60_000 } }),
    NOW,
  );
  assert.ok(detail.startsWith("limit reached"), detail);
});

test("no window at all is said as within limits, never as silence", () => {
  assert.equal(usageDetail(gauge({}), NOW), "within limits");
});

test("a reset already in the past is not offered as a wait", () => {
  assert.equal(usageDetail(gauge({ primary: { resetsAtMs: NOW - 1_000 } }), NOW), "within limits");
});

test("rows carry the service's declared mark and name", () => {
  const rows = usageRows(
    [gauge({ providerId: "claude", primary: { resetsAtMs: NOW + 60_000 } })],
    PROVIDERS,
    NOW,
  );
  assert.equal(rows.length, 1);
  const row = rows[0];
  assert.equal(row?.name, "Claude Code");
  assert.equal(row?.icon, "claude");
  assert.ok(row?.tooltip.includes("Reported"), "the hover says how old the report is");
});

test("an account nobody has heard from is absent, not green", () => {
  // The strip lists reports, and the view's empty text says "no report yet" in words. Listing every installed
  // provider with an invented "ok" would be the strip lying exactly when it knows least.
  assert.deepEqual(usageRows([], PROVIDERS, NOW), []);
});
