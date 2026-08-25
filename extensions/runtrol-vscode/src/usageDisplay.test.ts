import assert from "node:assert/strict";
import test from "node:test";

import type { ProviderLine, ProviderUsageGauge } from "./runtimeTypes";
import {
  installableProviders,
  usageDetail,
  usageMeters,
  usageRows,
  usageRowsEqual,
} from "./usageDisplay";

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

test("reported account windows become bounded progress meters", () => {
  const meters = usageMeters(gauge({
    primary: { usedPercent: 87, resetsAtMs: NOW + 23 * 60_000, windowMinutes: 300 },
    secondary: { usedPercent: 142, resetsAtMs: NOW + 2 * 86_400_000, windowMinutes: 10_080 },
  }), NOW);
  assert.deepEqual(meters, [
    { key: "primary", label: "5h", percent: 87, detail: "87% used, resets in 23m" },
    { key: "secondary", label: "7d", percent: 100, detail: "100% used, resets in 2d" },
  ]);
});

test("a reset without a reported percentage does not invent an empty progress bar", () => {
  assert.deepEqual(usageMeters(gauge({ primary: { resetsAtMs: NOW + 60_000 } }), NOW), []);
  assert.deepEqual(usageMeters(gauge({ primary: { usedPercent: Number.NaN } }), NOW), []);
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

test("a service that described no limit is said as that, never as silence and never as room it did not claim", () => {
  assert.equal(usageDetail(gauge({}), NOW), "no limit reported");
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
  assert.deepEqual(row?.meters, []);
  assert.ok(row?.tooltip.includes("Reported"), "the hover says how old the report is");
});

test("every connected CLI stays visible before it reports usage", () => {
  const rows = usageRows([], PROVIDERS, NOW);
  // Nobody has asked the service anything yet, and the line says exactly that instead of "Ready".
  assert.deepEqual(rows.map((row) => [row.name, row.detail]), [
    ["Claude Code", "Not checked yet"],
    ["Codex", "Not checked yet"],
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
    ["Claude Code", "Not checked yet", "available"],
    ["Codex", "Checking", "checking"],
    ["Grok", "Unavailable · Fix", "unavailable"],
  ]);
  assert.equal(rows[2]?.providerId, "grok");
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
  assert.equal(rows[0]?.providerId, "codex");
  assert.equal(rows[0]?.meters[0]?.percent, 48);
});

test("equivalent status snapshots do not demand another view render", () => {
  const rows = usageRows([gauge({ primary: { usedPercent: 48 } })], PROVIDERS, NOW);
  assert.equal(usageRowsEqual(rows, structuredClone(rows)), true);
});

test("a visible or actionable status change demands another view render", () => {
  const rows = usageRows([], PROVIDERS, NOW);
  assert.equal(usageRowsEqual(rows, rows.map((row, index) => (
    index === 0 ? { ...row, detail: "Checking" } : row
  ))), false);
  assert.equal(usageRowsEqual(rows, rows.map((row, index) => (
    index === 0
      ? { ...row, providerId: "replacement" }
      : row
  ))), false);
});

test("the service catalogue offers only missing CLIs with an exact install line", () => {
  const providers = [
    ...PROVIDERS,
    {
      providerId: "available-later",
      displayName: "Available Later",
      installation: { state: "missing" },
      help: { install: "npm install --global available-later@1.0.0" },
    },
    {
      providerId: "manual-only",
      displayName: "Manual Only",
      installation: { state: "missing" },
    },
  ] as unknown as ProviderLine[];
  assert.deepEqual(
    installableProviders(providers).map((provider) => provider.providerId),
    ["available-later"],
  );
});

test("the account line says what the service said: signed out is an action, a plan is a word, no surface is named", () => {
  const providers = [
    { providerId: "claude", displayName: "Claude Code", icon: "claude", installation: { state: "usable" },
      account: { status: "signedIn", plan: "max", method: "claude.ai", checkedAtMs: NOW } },
    { providerId: "codex", displayName: "Codex", icon: "openai", installation: { state: "usable" },
      account: { status: "signedOut", checkedAtMs: NOW } },
    { providerId: "grok", displayName: "Grok", icon: "grok", installation: { state: "usable" },
      account: { status: "unpublished", why: "no status command", checkedAtMs: NOW } },
  ] as unknown as ProviderLine[];
  const rows = usageRows([gauge({ providerId: "claude", primary: { usedPercent: 40 } })], providers, NOW);
  assert.deepEqual(rows.map((row) => [row.name, row.detail, row.state]), [
    ["Claude Code", "max plan via claude.ai · 40%", "available"],
    ["Codex", "Not signed in · Sign in", "signedOut"],
    ["Grok", "Grok publishes no usage or sign-in status", "available"],
  ]);
  assert.equal(rows[0]?.meters.length, 1, "a reported window is still a bar");
  assert.match(rows[1]?.tooltip ?? "", /Press Enter to sign in/);
});
