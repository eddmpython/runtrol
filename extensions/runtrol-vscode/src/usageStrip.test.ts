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
});

test("a row without a number keeps an empty ring and names its cause under it", () => {
  const chips = usageChips([
    row({ state: "signedOut", tooltip: "Grok says nobody is signed in." }),
    row({ state: "unavailable" }),
    row({ state: "checking" }),
  ]);
  assert.deepEqual(chips.map((chip) => [chip.percent, chip.caption, chip.action]), [
    [null, "Sign in", "signIn"],
    [null, "Fix", "fix"],
    [null, "Checking", null],
  ]);
});

test("the page draws one bar per reported window and escapes what the service said", () => {
  const html = usageStripHtml(usageChips([row({
    name: "Codex <pro>",
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
  assert.ok(html.includes('<p class="position">within limits</p>'));
  assert.ok(html.includes(`nonce="${assets.nonce}"`));
  assert.ok(html.includes("script-src 'nonce-n0nce'"));
});

test("a blocking limit colours the chip and the panel", () => {
  const html = usageStripHtml(usageChips([row({
    reached: true,
    meters: [{ key: "5h", label: "5h", percent: 100, detail: "100% used, resets in 1h", governing: true }],
    tooltip: "Codex: a limit is blocking right now\n5h: 100% used, governing now\nReported just now",
  })]), assets);
  assert.ok(html.includes('class="chip reached"'));
  assert.ok(html.includes(">100%<"));
  assert.ok(html.includes('class="position reached"'));
});

test("no installed service says so instead of drawing nothing", () => {
  assert.ok(usageStripHtml([], assets).includes("No coding service is installed yet."));
});
