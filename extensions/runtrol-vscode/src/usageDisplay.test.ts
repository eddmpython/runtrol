import assert from "node:assert/strict";
import test from "node:test";

import type { ProviderLine, ProviderUsageGauge, ProviderUsageWindow } from "./runtimeTypes";
import {
  primarySevenDayMeter,
  setupRows,
  shortened,
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

/// One window, in the shape the Runtime publishes them.
function window(
  id: string,
  overrides: Partial<ProviderUsageWindow> = {},
): ProviderUsageWindow {
  return { id, ...overrides } as ProviderUsageWindow;
}

/// The three windows a real Claude account reported on 2026-08-26, as the Runtime published them.
///
/// Kept as measured because the shape is the whole point: two of the three were comfortable while the third,
/// scoped to one model, was the one refusing work.
const MEASURED_CLAUDE = [
  window("five_hour", { usedPercent: 13, windowMinutes: 300, resetsAtMs: NOW + 23 * 60_000 }),
  window("seven_day", { usedPercent: 95, windowMinutes: 10_080, resetsAtMs: NOW + 2 * 86_400_000 }),
  window("seven_day:Fable", {
    usedPercent: 100,
    windowMinutes: 10_080,
    scope: "Fable",
    governing: true,
    resetsAtMs: NOW + 2 * 86_400_000,
  }),
];

test("every window a service described becomes its own labelled bar", () => {
  // The defect this replaced: two slots, so a driver picked two of the three and the one actually blocking
  // was routinely the one dropped.
  const meters = usageMeters(gauge({ windows: MEASURED_CLAUDE }), NOW);
  assert.deepEqual(meters.map((meter) => meter.label), ["5h", "7d", "7d Fable"]);
  assert.deepEqual(meters.map((meter) => meter.percent), [13, 95, 100]);
  assert.deepEqual(meters.map((meter) => meter.key), [
    "five_hour",
    "seven_day",
    "seven_day:Fable",
  ]);
});

test("the compact sidebar prefers the whole-account week over an earlier model-scoped week", () => {
  const meters = usageMeters(
    gauge({ windows: [MEASURED_CLAUDE[2], MEASURED_CLAUDE[1], MEASURED_CLAUDE[0]] }),
    NOW,
  );

  assert.deepEqual(primarySevenDayMeter(meters), meters[1]);
  assert.equal(primarySevenDayMeter(meters)?.label, "7d");
  assert.equal(primarySevenDayMeter(meters)?.percent, 95);
});

test("a bar is named by what the service scoped it to, never by a phrase composed here", () => {
  const [spark] = usageMeters(
    gauge({
      windows: [window("codex_bengalfox.primary", {
        usedPercent: 4,
        windowMinutes: 300,
        label: "GPT-5.3-Codex-Spark",
      })],
    }),
    NOW,
  );
  // The length leads because it is what tells this bar from the same model's weekly one. The name is cut
  // from the middle rather than the end: `GPT-5.3-Co...` is the half every bucket of that service shares,
  // and `GPT...Spark` is the vendor's word and the distinguishing word both.
  assert.equal(spark?.label, "5h GPT…Spark");
});

test("a short name the service gave is left exactly as it gave it", () => {
  // Nothing is renamed and nothing is shortened that fits. `Fable` is five characters and stays five.
  assert.equal(shortened("Fable"), "Fable");
  assert.equal(shortened("sonnet"), "sonnet");
  assert.equal(shortened(null), null);
});

test("the line names the window the service says is governing, not the shortest one", () => {
  // Reading the first window showed 13% on an account that was already refusing work on a window the same
  // report described. The service marks which one binds; that is the one a person acts on.
  const detail = usageDetail(gauge({ reached: true, windows: MEASURED_CLAUDE }), NOW);
  assert.ok(detail.startsWith("limit reached"), detail);
  assert.ok(detail.includes("7d Fable 100%"), detail);
});

test("with no window marked governing the fullest one speaks for the row", () => {
  const detail = usageDetail(
    gauge({
      windows: [
        window("codex_bengalfox.primary", { usedPercent: 0, windowMinutes: 300 }),
        window("codex.primary", { usedPercent: 30, windowMinutes: 10_080 }),
      ],
    }),
    NOW,
  );
  assert.ok(detail.includes("7d 30%"), detail);
});

test("a provider that reports a percentage shows it, in its own number", () => {
  const detail = usageDetail(
    gauge({
      windows: [window("five_hour", {
        usedPercent: 87,
        resetsAtMs: NOW + 23 * 60_000,
        windowMinutes: 300,
      })],
    }),
    NOW,
  );
  assert.equal(detail, "5h 87% · resets in 23m");
});

test("a percentage past the top of the track is drawn at the top of the track", () => {
  const meters = usageMeters(
    gauge({ windows: [window("seven_day", { usedPercent: 142, windowMinutes: 10_080 })] }),
    NOW,
  );
  assert.equal(meters[0]?.percent, 100);
});

test("the line, the bar and the hover never disagree about one number", () => {
  // A service can report past a full window. The bar cannot draw past its track, so a line that printed the
  // raw number sat beside a full bar saying something else.
  const over = gauge({
    providerId: "codex",
    windows: [window("codex.primary", { usedPercent: 250, windowMinutes: 10_080 })],
  });
  assert.equal(usageMeters(over, NOW)[0]?.percent, 100);
  assert.equal(usageDetail(over, NOW), "7d 100%");
  const row = usageRows([over], PROVIDERS, NOW).find((line) => line.providerId === "codex");
  assert.ok(row?.tooltip.includes("100% used"), row?.tooltip);
  assert.ok(!row?.tooltip.includes("250"), row?.tooltip);
});

test("a reset without a reported percentage does not invent an empty progress bar", () => {
  assert.deepEqual(
    usageMeters(gauge({ windows: [window("billing_period", { resetsAtMs: NOW + 60_000 })] }), NOW),
    [],
  );
  assert.deepEqual(
    usageMeters(gauge({ windows: [window("five_hour", { usedPercent: Number.NaN })] }), NOW),
    [],
  );
});

test("a provider that reports only a reset still has something true to say", () => {
  // Measured: one service publishes its usage period and no percentage for it at all. Showing nothing would
  // read as "no limit exists" for an account that has one.
  const detail = usageDetail(
    gauge({ windows: [window("billing_period", { resetsAtMs: NOW + 2 * 3_600_000 })] }),
    NOW,
  );
  assert.equal(detail, "resets in 2h");
});

test("a blocking limit is the first thing the line says", () => {
  const detail = usageDetail(
    gauge({
      reached: true,
      windows: [window("five_hour", { usedPercent: 100, resetsAtMs: NOW + 90 * 60_000 })],
    }),
    NOW,
  );
  assert.ok(detail.startsWith("limit reached"), detail);
});

test("a service that described no limit is said as that, never as silence and never as room it did not claim", () => {
  assert.equal(usageDetail(gauge({}), NOW), "no limit reported");
});

test("a reset already in the past is not offered as a wait", () => {
  assert.equal(
    usageDetail(gauge({ windows: [window("five_hour", { resetsAtMs: NOW - 1_000 })] }), NOW),
    "within limits",
  );
});

test("the hover spells out every window, including the one that is governing", () => {
  const [row] = usageRows(
    [gauge({ providerId: "claude", reached: true, windows: MEASURED_CLAUDE })],
    PROVIDERS,
    NOW,
  );
  assert.ok(row?.tooltip.includes("7d Fable: 100% used"), row?.tooltip);
  assert.ok(row?.tooltip.includes("governing now"), row?.tooltip);
  assert.ok(row?.tooltip.includes("5h: 13% used"), row?.tooltip);
});

test("a service that reported a period and no number leads with its own reason", () => {
  // Measured: one service answers about the plan and the period and states no percentage, because that
  // account is metered by a team. A bar-less row saying only when it resets reads as a failure.
  const providers = [
    {
      providerId: "grok",
      displayName: "Grok",
      icon: "grok",
      installation: { state: "usable" },
      account: {
        status: "signedIn",
        plan: "SuperGrok",
        limitsAbsent: { kind: "unmetered", why: "team-managed" },
        checkedAtMs: NOW,
      },
    },
  ] as unknown as ProviderLine[];
  const [row] = usageRows(
    [gauge({
      providerId: "grok",
      windows: [window("billing_period", {
        windowMinutes: 10_080,
        resetsAtMs: NOW + 4 * 86_400_000,
      })],
    })],
    providers,
    NOW,
  );
  assert.equal(row?.detail, "team-managed · 7d · resets in 4d");
  assert.deepEqual(row?.meters, [], "no number was stated, so no bar is drawn");
});

test("a service that did state a number keeps its number and not the reason", () => {
  const providers = [
    {
      providerId: "grok",
      displayName: "Grok",
      icon: "grok",
      installation: { state: "usable" },
      account: {
        status: "signedIn",
        limitsAbsent: { kind: "unmetered", why: "team-managed" },
        checkedAtMs: NOW,
      },
    },
  ] as unknown as ProviderLine[];
  const [row] = usageRows(
    [gauge({
      providerId: "grok",
      windows: [window("billing_period", { usedPercent: 42, windowMinutes: 10_080 })],
    })],
    providers,
    NOW,
  );
  assert.equal(row?.detail, "7d 42%");
});

test("rows carry the service's declared mark and name", () => {
  const rows = usageRows(
    [gauge({ providerId: "claude", windows: [window("five_hour", { resetsAtMs: NOW + 60_000 })] })],
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
    ["Claude Code", "Checking usage"],
    ["Codex", "Checking usage"],
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
      providerId: "acp-fixture",
      displayName: "Cline",
      icon: "robot",
      installation: { state: "missing" },
    },
  ] as unknown as ProviderLine[];

  const rows = usageRows([], providers, NOW);
  assert.deepEqual(rows.map((row) => [row.name, row.detail, row.state]), [
    ["Claude Code", "Checking usage", "available"],
    ["Codex", "Checking", "checking"],
    ["Grok", "Unavailable · Fix", "unavailable"],
  ]);
  assert.equal(rows[2]?.providerId, "grok");
  // The cause is the position; the fix is a button, never a sentence telling somebody to press a key.
  assert.equal(rows[2]?.position, "the installed CLI exited during its probe");
  assert.doesNotMatch(rows[2]?.tooltip ?? "", /Press Enter/);
});

test("a last report never disguises a disconnected CLI as available", () => {
  const missing = [{
    providerId: "codex",
    displayName: "Codex",
    installation: { state: "missing" },
  }] as unknown as ProviderLine[];
  const rows = usageRows([gauge({ windows: [window("five_hour", { usedPercent: 48 })] })], missing, NOW);
  assert.equal(rows.length, 1);
  assert.equal(rows[0]?.detail, "Disconnected · 48%");
  assert.equal(rows[0]?.state, "disconnected");
  assert.equal(rows[0]?.providerId, "codex");
  assert.equal(rows[0]?.meters[0]?.percent, 48);
});

test("equivalent status snapshots do not demand another view render", () => {
  const rows = usageRows([gauge({ windows: [window("five_hour", { usedPercent: 48 })] })], PROVIDERS, NOW);
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

test("the set-up list names every shipped service and what each one still needs", () => {
  const providers = [
    { providerId: "claude", displayName: "Claude Code", icon: "claude", installation: { state: "usable" },
      account: { status: "signedIn", plan: "max", checkedAtMs: NOW } },
    { providerId: "codex", displayName: "Codex", icon: "openai", installation: { state: "usable" },
      account: { status: "signedOut", checkedAtMs: NOW } },
    { providerId: "grok", displayName: "Grok", icon: "grok", installation: { state: "missing" },
      help: { install: "npm install --global @vibe-kit/grok-cli" } },
    { providerId: "manual-only", displayName: "Manual Only", icon: "hubot", installation: { state: "missing" } },
    { providerId: "sick", displayName: "Sick", icon: "hubot",
      installation: { state: "unavailable", why: "its executable answered nothing" } },
  ] as unknown as ProviderLine[];
  assert.deepEqual(
    setupRows(providers).map((row) => [row.name, row.state, row.actionable]),
    [
      // A service that is ready is still listed, because "which services do I have" is the question this list
      // is opened with, and it is answered with a row rather than an absence.
      ["Claude Code", "ready", false],
      ["Codex", "signedOut", true],
      ["Grok", "missing", true],
      // Nothing to press: this build has no command to offer, and a button that reports its own emptiness is
      // worse than a line that says the state.
      ["Manual Only", "missing", false],
      ["Sick", "unavailable", true],
    ],
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
  const rows = usageRows([gauge({ providerId: "claude", windows: [window("five_hour", { usedPercent: 40 })] })], providers, NOW);
  assert.deepEqual(rows.map((row) => [row.name, row.detail, row.state]), [
    // The bar row is the number and nothing else; the plan the service named is in the hover.
    ["Claude Code", "40%", "available"],
    ["Codex", "Not signed in · Sign in", "signedOut"],
    // No bar, so the row names the cause and the service's own sentence moves to the hover.
    ["Grok", "No usage published", "available"],
  ]);
  assert.equal(rows[0]?.meters.length, 1, "a reported window is still a bar");
  assert.match(rows[2]?.tooltip ?? "", /publishes no usage or sign-in status/);
  assert.equal(rows[1]?.position, "Not signed in");
  assert.doesNotMatch(rows[1]?.tooltip ?? "", /Press Enter/);
  assert.equal(rows[0]?.plan, "max plan via claude.ai");
  assert.equal(rows[0]?.position, "Within limits");
  assert.match(rows[0]?.tooltip ?? "", /max plan via claude\.ai/);
});

test("a bar-less row names the cause that matches what the service actually said", () => {
  // Three different absences, three different next steps. Collapsing them once told a signed-in service that
  // nothing would ever arrive, when its number rides on the next turn it takes.
  const providers = [
    { providerId: "unprobed", displayName: "Unprobed", icon: "hubot", installation: { state: "usable" } },
    { providerId: "out", displayName: "Out", icon: "hubot", installation: { state: "usable" },
      account: { status: "signedOut", checkedAtMs: NOW } },
    { providerId: "silent", displayName: "Silent", icon: "hubot", installation: { state: "usable" },
      account: { status: "unpublished", why: "no status command", checkedAtMs: NOW } },
    { providerId: "waiting", displayName: "Waiting", icon: "hubot", installation: { state: "usable" },
      account: { status: "signedIn", plan: "max", checkedAtMs: NOW } },
  ] as unknown as ProviderLine[];
  assert.deepEqual(usageRows([], providers, NOW).map((row) => row.detail), [
    "Checking usage",
    "Not signed in · Sign in",
    "No usage published",
    "Usage arrives with the first turn",
  ]);
});

test("a service that publishes its own daily token count shows today's tokens beside the window", () => {
  const detail = usageDetail(
    gauge({
      windows: [window("codex.primary", {
        usedPercent: 65,
        resetsAtMs: NOW + 5 * 24 * 60 * 60_000,
        windowMinutes: 10_080,
      })],
      tokensToday: 12_345_678,
    }),
    NOW,
  );
  assert.equal(detail, "7d 65% · resets in 5d · 12.3M tokens today");
});
