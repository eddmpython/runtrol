import assert from "node:assert/strict";
import test from "node:test";

import type { ProviderLine, ProviderUsageGauge } from "./runtimeTypes";
import { usageDetail, usageRows } from "./usageDisplay";

const NOW = Date.parse("2026-08-18T12:00:00Z");

const PROVIDERS = [
  { providerId: "claude", displayName: "Claude Code", icon: "claude", installation: { state: "usable" } },
  { providerId: "codex", displayName: "Codex", icon: "openai", installation: { state: "usable" } },
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
  assert.equal(rows.length, 2);
  const row = rows[0];
  assert.equal(row?.name, "Claude Code");
  assert.equal(row?.icon, "claude");
  assert.ok(row?.tooltip.includes("Reported"), "the hover says how old the report is");
});

test("every connected CLI stays visible before it reports usage", () => {
  const rows = usageRows([], PROVIDERS, NOW);
  assert.deepEqual(rows.map((row) => [row.name, row.detail]), [
    ["Claude Code", "No report yet"],
    ["Codex", "No report yet"],
  ]);
});

test("the fixed area includes checking and broken installed CLIs but omits missing ones", () => {
  const providers = [
    ...PROVIDERS.slice(0, 1),
    {
      providerId: "codex",
      displayName: "Codex",
      icon: "openai",
      installation: {
        state: "unavailable",
        why: "the installed executable has not completed a verified probe",
      },
    },
    {
      providerId: "grok",
      displayName: "Grok",
      icon: "sparkle",
      installation: { state: "unavailable", why: "the installed CLI exited during its probe" },
    },
    {
      providerId: "cline",
      displayName: "Cline",
      icon: "robot",
      installation: { state: "missing" },
    },
  ] as unknown as ProviderLine[];

  const rows = usageRows([], providers, NOW);
  assert.deepEqual(rows.map((row) => [row.name, row.detail, row.state]), [
    ["Claude Code", "No report yet", "available"],
    ["Codex", "Checking", "checking"],
    ["Grok", "Unavailable · Fix", "unavailable"],
  ]);
  assert.equal(rows[2]?.provider?.providerId, "grok");
  assert.match(rows[2]?.tooltip ?? "", /Press Enter/);
});

test("a last report never disguises a disconnected CLI as available", () => {
  const missing = [{
    providerId: "codex",
    displayName: "Codex",
    installation: { state: "missing" },
  }] as unknown as ProviderLine[];
  const rows = usageRows([gauge({ primary: { usedPercent: 48 } })], missing, NOW);
  assert.equal(rows.length, 1);
  assert.equal(rows[0]?.detail, "Disconnected · 48%");
  assert.equal(rows[0]?.state, "disconnected");
  assert.equal(rows[0]?.provider, null);
});
