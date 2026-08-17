import assert from "node:assert/strict";
import test from "node:test";

import { agentLine, NO_FACTS, NO_USAGE, usageLine } from "./statusLine";

const NOW = Date.parse("2026-08-17T12:00:00Z");

test("the agent line names only what is actually known", () => {
  assert.equal(agentLine(NO_FACTS), "");
  assert.equal(agentLine({ ...NO_FACTS, service: "Claude Code" }), "Claude Code");
  assert.equal(
    agentLine({ service: "Codex", model: "gpt-5", effort: "high", mode: "auto" }),
    "Codex · gpt-5 · high · auto",
  );
});

test("usage stays empty until a number exists", () => {
  assert.equal(usageLine(NO_USAGE, NOW), "");
  assert.equal(
    usageLine({ ...NO_USAGE, usage: { used: 0, size: 0, amount: null, currency: "" } }, NOW),
    "",
    "a zero-size context is unknown, not full",
  );
});

test("context share leads, because it is the number acted on", () => {
  assert.equal(
    usageLine({ ...NO_USAGE, usage: { used: 45_000, size: 200_000, amount: 0.1234, currency: "USD" } }, NOW),
    "Context 23% · 0.1234 USD",
  );
});

test("an untouched quota window never takes up the line", () => {
  const roomy = usageLine(
    { ...NO_USAGE, primary: { usedPercent: 12, resetsAt: null, windowMinutes: 300 } },
    NOW,
  );
  assert.equal(roomy, "");
});

test("a quota window appears once it is close enough to matter", () => {
  const tight = usageLine(
    {
      ...NO_USAGE,
      primary: { usedPercent: 82, resetsAt: NOW + 90 * 60_000, windowMinutes: 300 },
      secondary: { usedPercent: 10, resetsAt: null, windowMinutes: 10_080 },
    },
    NOW,
  );
  assert.equal(tight, "18% of 5h limit left, resets in 2h");
});

test("a reached limit is said even when the window is otherwise quiet", () => {
  const reached = usageLine(
    { ...NO_USAGE, reached: true, primary: { usedPercent: 100, resetsAt: null, windowMinutes: 60 } },
    NOW,
  );
  assert.equal(reached, "0% of 1h limit left");
});

test("the tightest window wins when two are reported", () => {
  const line = usageLine(
    {
      ...NO_USAGE,
      primary: { usedPercent: 65, resetsAt: null, windowMinutes: 300 },
      secondary: { usedPercent: 91, resetsAt: null, windowMinutes: 10_080 },
    },
    NOW,
  );
  assert.equal(line, "9% of 7d limit left");
});
