import assert from "node:assert/strict";
import { test } from "node:test";

import type { UsageRow } from "./usageDisplay";
import type { UsageChip } from "./usageStrip";
import { usageChips, usageChipsMarkup, usagePanelsMarkup } from "./usageStrip";

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

/// The strip exactly as the sidebar page embeds it, which is the only place it is ever drawn.
///
/// These tests used to render a standalone document that production had no route to. That document carried a
/// `body` rule of its own, so a rule that was wrong on the real page was right in the tests' page and lived
/// there for weeks (2026-08-28). A test that renders what nobody ships proves what nobody sees.
function strip(chips: readonly UsageChip[]): string {
  return `${usageChipsMarkup(chips, assets)}${chips.length === 0 ? "" : usagePanelsMarkup(chips)}`;
}

const assets = {
  cspSource: "vscode-resource:",
  nonce: "n0nce",
  iconUris: new Map([["codex", "https://icons/codex.svg"]]),
};

test("a chip's ring is the whole-account week and its caption is that number", () => {
  const [chip] = usageChips([row({
    meters: [
      { key: "5h", label: "5h", percent: 12, resets: "resets in 3h", governing: false },
      { key: "7d", label: "7d", percent: 76, resets: "resets in 6d", governing: true },
      { key: "7d:GPT", label: "7d GPT-5.3", percent: 0, resets: "", governing: false },
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
  const html = strip(usageChips([row({
    meters: [
      { key: "5h", label: "5h", percent: 12, resets: "", governing: false },
      { key: "7d", label: "7d", percent: 76, resets: "", governing: true },
    ],
  })]));
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
  const html = strip(usageChips([row({
    name: "Codex <pro>",
    plan: "pro plan via chatgpt",
    meters: [
      { key: "7d", label: "7d", percent: 76, resets: "resets in 6d", governing: true },
      { key: "7d:GPT", label: "7d GPT-5.3", percent: 0, resets: "", governing: false },
    ],
    tooltip: "Codex <pro>: within limits\n7d: 76% used\n7d GPT-5.3: 0% used\nReported 2m ago",
  })]));
  assert.equal((html.match(/role="progressbar"/g) ?? []).length, 2);
  assert.ok(html.includes("Codex &lt;pro&gt;"));
  assert.ok(!html.includes("Codex <pro>"));
  assert.ok(html.includes('src="https://icons/codex.svg"'));
  assert.ok(html.includes("Reported 2m ago"));
  assert.ok(html.includes('<p class="position">Within limits</p>'));
  assert.ok(html.includes('<span class="plan">pro plan via chatgpt</span>'));
  assert.ok(!html.includes("Press Enter"));
});

test("a blocking limit colours the chip and the panel", () => {
  const html = strip(usageChips([row({
    reached: true,
    position: "A limit is blocking right now",
    meters: [{ key: "5h", label: "5h", percent: 100, resets: "resets in 1h", governing: true }],
    tooltip: "Codex: a limit is blocking right now\n5h: 100% used, governing now\nReported just now",
  })]));
  assert.ok(html.includes('class="chip reached"'));
  assert.ok(html.includes(">100%<"));
  assert.ok(html.includes('class="position reached"'));
});

test("a signed-out chip answers a click with the sign-in action and says so to a screen reader", () => {
  const html = strip(usageChips([row({ state: "signedOut", position: "Not signed in", age: null })]));
  assert.ok(html.includes('data-action="signIn" data-provider="codex"'));
  assert.ok(html.includes('aria-label="Codex: Sign in"'));
  assert.ok(!html.includes("Press Enter"));
});

test("no installed service says so instead of drawing nothing", () => {
  assert.ok(strip([]).includes("No coding service is installed yet."));
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

test("the governing window says when it resets beside its name, not on a line of its own", () => {
  const html = strip(usageChips([row({
    meters: [
      { key: "5h", label: "5h", percent: 12, resets: "resets in 3h", governing: false },
      { key: "7d", label: "7d", percent: 76, resets: "resets in 6d", governing: true },
    ],
  })]));
  // Inside the name's own cell, so the meter stays two rows tall and the bars of one service keep an even
  // distance from each other (operator, 2026-08-28: close the gap between one service's bars).
  assert.ok(
    html.includes('<span class="label"><span class="what">7d</span><span class="when">resets in 6d</span></span>'),
    html,
  );
  // The window that is not governing keeps its words to itself even when it knows its reset.
  assert.ok(html.includes('<span class="label"><span class="what">5h</span></span>'), html);
  // The number is the bar's to state. The sentence under the bar used to open by repeating it.
  assert.ok(!html.includes("76% used"), html);
  assert.ok(!html.includes('class="detail"'), "the third line is gone");
});
