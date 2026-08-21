import assert from "node:assert/strict";
import test from "node:test";

import {
  MAX_BRANCHES,
  MIN_BRANCHES,
  branchProblem,
  fanOutName,
  instructionDigest,
  missionSpec,
} from "./fanOut";

const AT = new Date("2026-08-18T04:05:06Z");

/// Bytes as they would sit in a file the operator wrote, closing newline included.
const INSTRUCTION = "Make the parser handle trailing commas\n";

function spec(branches = 3): string {
  return missionSpec(fanOutName(INSTRUCTION, AT), instructionDigest(INSTRUCTION), {
    instruction: INSTRUCTION,
    instructionRef: "docs/attempt.md",
    branches,
    gateId: "check",
    baseRef: "main",
    outputRoots: ["src"],
    providerIds: ["runtime-a", "runtime-b"],
  });
}

test("attempts run at once because none of them depends on another", () => {
  // The whole point of the flow. A `depends_on` on any task would turn a comparison into a queue.
  const written = spec(3);
  assert.equal(written.match(/\[\[tasks\]\]/gu)?.length, 3);
  assert.equal(written.includes("depends_on"), false);
});

test("every attempt writes in its own worktree", () => {
  // Two attempts sharing a tree would overwrite each other, and the comparison would be of one thing.
  const written = spec(3);
  assert.equal(written.match(/workspace_mode = "isolated_worktree"/gu)?.length, 3);
});

test("the digest is over the bytes already on disk, not over anything reformatted", () => {
  // Mission binds a task to its instruction's bytes so an edited instruction fails validation instead of quietly
  // running something else. Trimming or re-wrapping what the operator wrote would break that immediately, which is
  // why nothing here creates or rewrites the file.
  assert.ok(spec(2).includes(`instruction_sha256 = "${instructionDigest(INSTRUCTION)}"`));
  assert.notEqual(instructionDigest(INSTRUCTION), instructionDigest(INSTRUCTION.trim()));
});

test("a fan-out is capped however many attempts were asked for", () => {
  // Each attempt owns a worktree and a provider process, so this is a memory-budget question, not a preference.
  const written = spec(99);
  assert.equal(written.match(/\[\[tasks\]\]/gu)?.length, MAX_BRANCHES);
  assert.ok(written.includes(`max_parallel_tasks = ${MAX_BRANCHES}`));
  assert.ok(written.includes(`max_hot_providers = ${MAX_BRANCHES}`));
});

test("one attempt is refused with a reason rather than silently doubled", () => {
  assert.ok(branchProblem("1")?.includes("not a comparison"));
  assert.ok(branchProblem(String(MAX_BRANCHES + 1))?.includes("worktree"));
  assert.ok(branchProblem("two")?.includes("whole number"));
  assert.equal(branchProblem(String(MIN_BRANCHES)), null);
  assert.equal(branchProblem(String(MAX_BRANCHES)), null);
});

test("a failed attempt is a result, so nothing is retried behind the operator's back", () => {
  // A fan-out compares attempts. Retrying one silently would mean the thing being compared changed while being
  // compared, and the operator would be choosing between results produced under different rules.
  const written = spec(2);
  assert.ok(written.includes("max_runs_per_task = 1"));
  assert.ok(written.includes("max_repair_cycles = 0"));
  assert.ok(written.includes("stop_on_critical_failure = false"));
  assert.ok(written.includes('completion_policy = "choose_one"'));
});

test("runtime-discovered provider choices are assigned round-robin", () => {
  const written = spec(3);
  assert.equal(written.match(/provider_selector = "runtime:runtime-a"/gu)?.length, 2);
  assert.equal(written.match(/provider_selector = "runtime:runtime-b"/gu)?.length, 1);
});

test("the generated document explains itself, because the operator reads it before starting", () => {
  // Mission means reviewed. A document generated and started without being seen would keep the machinery and lose
  // the reason it exists.
  const written = spec(2);
  assert.ok(written.startsWith("#"));
  assert.ok(written.includes("Read it before starting"));
});

test("the name says which fan-out this was", () => {
  // A directory of timestamps tells the operator nothing about which attempt was which.
  assert.equal(fanOutName(INSTRUCTION, AT), "make-the-parser-handle-20260818040506");
  assert.equal(fanOutName("!!!", AT), "fanout-20260818040506");
});

test("every attempt points at the operator's own instruction file", () => {
  // The path the operator chose, not one this code invented. Mission reads that file; nothing here writes it.
  assert.equal(spec(3).match(/instruction_ref = "docs[/]attempt[.]md"/gu)?.length, 3);
});

test("generated TOML quotes reviewed values instead of allowing structure injection", () => {
  const written = missionSpec('quoted " name', "a".repeat(64), {
    instruction: INSTRUCTION,
    instructionRef: 'docs/"attempt".md',
    branches: 2,
    gateId: 'check"gate',
    baseRef: 'main"ref',
    outputRoots: ['src/"quoted"'],
    providerIds: ['runtime"id'],
  });
  assert.ok(written.includes('name = "quoted \\" name"'));
  assert.ok(written.includes('instruction_ref = "docs/\\"attempt\\".md"'));
  assert.ok(written.includes('provider_selector = "runtime:runtime\\"id"'));
});
