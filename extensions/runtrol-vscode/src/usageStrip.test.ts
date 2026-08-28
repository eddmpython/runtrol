import assert from "node:assert/strict";
import { test } from "node:test";

import type { UsageRow } from "./usageDisplay";
import { usageChips, usageStripHtml } from "./usageStrip";

function row(overrides: Partial<UsageRow>): UsageRow {
  return {
    key: "usage:codex",
    name: "Codex",
    icon: "codex",
    detail: "",
    meters: [],
    cost: null,
    reached: false,
    state: "available",
    providerId: "codex",
    tooltip: "Codex: within limits\nReported 2m ago",
    position: "Within limits",
    plan: null,
    age: "Reported 2m ago",
    unmetered: null,
    ...overrides,
  };
}

const assets = {
  cspSource: "vscode-resource:",
  nonce: "n0nce",
  iconUris: new Map([["codex", "https://icons/codex.svg"]]),
};

test("a chip's ring is the whole-account week and its caption is that number", () => {
  const [chip] = usageChips([row({
    meters: [
      { key: "5h", label: "5h", percent: 12, detail: "12% used, resets in 3h", governing: false },
      { key: "7d", label: "7d", percent: 76, detail: "76% used, resets in 6d", governing: true },
      { key: "7d:GPT", label: "7d GPT-5.3", percent: 0, detail: "0% used", governing: false },
    ],
  })]);
  assert.equal(chip!.percent, 76);
  assert.equal(chip!.caption, "76%");
  assert.equal(chip!.action, null);
  // The gauge's layers, outermost first: the week, then the five-hour window, then the model-scoped window.
  assert.deepEqual(chip!.rings, [
    { label: "7d", percent: 76 },
    { label: "5h", percent: 12 },
    { label: "7d GPT-5.3", percent: 0 },
  ]);
});

test("the chip draws one concentric ring per layer and no browser tooltip beside the panel", () => {
  const html = usageStripHtml(usageChips([row({
    meters: [
      { key: "5h", label: "5h", percent: 12, detail: "12% used", governing: false },
      { key: "7d", label: "7d", percent: 76, detail: "76% used", governing: true },
    ],
  })]), assets);
  assert.equal((html.match(/class="fill"/g) ?? []).length, 2);
  assert.ok(html.includes('r="11"'));
  assert.ok(html.includes('r="8"'));
  // The hover panel is the one detail surface: a native tooltip floating over it read as two competing
  // popups (operator, 2026-08-27), so the strip carries no title attribute anywhere.
  assert.ok(!html.includes('title="'));
});

test("a row without a number keeps an empty ring and names its cause under it", () => {
  const chips = usageChips([
    row({ state: "signedOut", position: "Not signed in", age: null }),
    row({ state: "unavailable" }),
    row({ state: "checking" }),
  ]);
  assert.deepEqual(chips.map((chip) => [chip.percent, chip.caption, chip.action]), [
    [null, "Sign in", "signIn"],
    [null, "Fix", "fix"],
    [null, "Checking", null],
  ]);
});

test("a chip with nothing to show is a way into that account", () => {
  // A chip is a button: pressing it goes into the one thing worth doing for that account. A service that
  // answered with no figure has nothing to read, so the lever is its sign-in (operator, 2026-08-28).
  const chips = usageChips([
    row({ state: "available", meters: [], position: "No report" }),
    row({ state: "disconnected", meters: [], position: "Offline" }),
  ]);
  assert.deepEqual(chips.map((chip) => chip.action), ["signIn", "signIn"]);
});

test("the page draws one bar per reported window and escapes what the service said", () => {
  const html = usageStripHtml(usageChips([row({
    name: "Codex <pro>",
    plan: "pro plan via chatgpt",
    meters: [
      { key: "7d", label: "7d", percent: 76, detail: "76% used, resets in 6d", governing: true },
      { key: "7d:GPT", label: "7d GPT-5.3", percent: 0, detail: "0% used", governing: false },
    ],
    tooltip: "Codex <pro>: within limits\n7d: 76% used\n7d GPT-5.3: 0% used\nReported 2m ago",
  })]), assets);
  assert.equal((html.match(/role="progressbar"/g) ?? []).length, 2);
  assert.ok(html.includes("Codex &lt;pro&gt;"));
  assert.ok(!html.includes("Codex <pro>"));
  assert.ok(html.includes('src="https://icons/codex.svg"'));
  assert.ok(html.includes("Reported 2m ago"));
  assert.ok(html.includes('<p class="position">Within limits</p>'));
  assert.ok(html.includes('<span class="plan">pro plan via chatgpt</span>'));
  assert.ok(!html.includes("Press Enter"));
  assert.ok(html.includes(`nonce="${assets.nonce}"`));
  assert.ok(html.includes("script-src 'nonce-n0nce'"));
});

test("a blocking limit colours the chip and the panel", () => {
  const html = usageStripHtml(usageChips([row({
    reached: true,
    position: "A limit is blocking right now",
    meters: [{ key: "5h", label: "5h", percent: 100, detail: "100% used, resets in 1h", governing: true }],
    tooltip: "Codex: a limit is blocking right now\n5h: 100% used, governing now\nReported just now",
  })]), assets);
  assert.ok(html.includes('class="chip reached"'));
  assert.ok(html.includes(">100%<"));
  assert.ok(html.includes('class="position reached"'));
});

test("a signed-out chip answers a click with the sign-in action and says so to a screen reader", () => {
  const html = usageStripHtml(usageChips([row({ state: "signedOut", position: "Not signed in", age: null })]), assets);
  assert.ok(html.includes('data-action="signIn" data-provider="codex"'));
  assert.ok(html.includes('aria-label="Codex: Sign in"'));
  assert.ok(!html.includes("Press Enter"));
});

test("no installed service says so instead of drawing nothing", () => {
  assert.ok(usageStripHtml([], assets).includes("No coding service is installed yet."));
});

test("a service that answered with no number of its own is captioned in its own words", () => {
  // Measured on a real account: one service answers about the plan and the period and states no percentage,
  // because that account is metered by a team the operator cannot see. Captioning that "No report" says the
  // service went quiet when it did the opposite.
  const [chip] = usageChips([row({
    providerId: "grok",
    name: "Grok",
    icon: "grok",
    meters: [],
    unmetered: "team-managed",
    position: "team-managed",
  })]);
  assert.equal(chip?.caption, "team-managed");
  assert.deepEqual(chip?.rings, []);
  assert.equal(chip?.percent, null);
});

test("a service nobody has heard from is still captioned as that", () => {
  const [chip] = usageChips([row({ meters: [], unmetered: null })]);
  assert.equal(chip?.caption, "No report");
});
