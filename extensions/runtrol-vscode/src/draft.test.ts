import assert from "node:assert/strict";
import test from "node:test";

import {
  DEFAULT_EFFORT_LABEL,
  DEFAULT_MODEL_LABEL,
  DEFAULT_MODE_LABEL,
  draftChips,
  draftGreeting,
  newDraftId,
  NO_PROJECT_LABEL,
  NO_SERVICE_LABEL,
  readDraftState,
  type DraftState,
} from "./draft";

const ROOT = process.platform === "win32" ? "C:\\work" : "/work";
const ALPHA = [ROOT, "alpha"].join(process.platform === "win32" ? "\\" : "/");

const bare: DraftState = {
  id: "draft:1",
  workspace: null,
  providerId: null,
  alsoProviderIds: [],
  model: null,
  effort: null,
  permission: null,
};

test("a draft with nothing chosen says so on every chip instead of guessing", () => {
  const chips = draftChips(bare, null, null);
  assert.equal(chips.project, NO_PROJECT_LABEL);
  assert.equal(chips.projectPath, null);
  assert.equal(chips.branch, null);
  assert.equal(chips.service, NO_SERVICE_LABEL);
  assert.equal(chips.model, DEFAULT_MODEL_LABEL);
  assert.equal(chips.effort, DEFAULT_EFFORT_LABEL);
  assert.equal(chips.mode, DEFAULT_MODE_LABEL);
});

test("a draft on a project names the folder, its branch, and every explicit choice", () => {
  const chips = draftChips(
    { ...bare, workspace: ALPHA, providerId: "codex", model: "gpt-5", effort: "high", permission: "plan" },
    "Codex",
    "main",
  );
  assert.equal(chips.project, "alpha");
  assert.equal(chips.projectPath, ALPHA);
  assert.equal(chips.branch, "main");
  assert.equal(chips.service, "Codex");
  assert.equal(chips.model, "gpt-5");
  assert.equal(chips.effort, "high");
  assert.equal(chips.mode, "plan");
});

test("a branch is never shown for a conversation with no project", () => {
  assert.equal(draftChips(bare, "Codex", "main").branch, null);
});

test("the greeting stays conversational while the composer identifies the project", () => {
  assert.equal(draftGreeting({ project: "alpha", projectPath: ALPHA }), "What can I help with?");
  assert.equal(draftGreeting({ project: NO_PROJECT_LABEL, projectPath: null }), "What can I help with?");
});

test("draft ids are distinct and recognizable", () => {
  const first = newDraftId();
  const second = newDraftId();
  assert.notEqual(first, second);
  assert.ok(first.startsWith("draft:"));
});

test("a stamped draft reads back with only the fields that still make sense", () => {
  const stamped = {
    id: "draft:abc:3",
    workspace: ALPHA,
    providerId: "claude",
    model: "",
    effort: 42,
    permission: "plan",
    extra: "ignored",
  };
  assert.deepEqual(readDraftState(stamped), {
    id: "draft:abc:3",
    workspace: ALPHA,
    providerId: "claude",
    alsoProviderIds: [],
    model: null,
    effort: null,
    permission: "plan",
  });
});

test("anything that is not a draft record restores nothing, so a tab closes instead of guessing", () => {
  assert.equal(readDraftState(null), null);
  assert.equal(readDraftState("draft:1"), null);
  assert.equal(readDraftState({ id: "session:1" }), null);
  assert.equal(readDraftState({ id: `draft:${"x".repeat(80)}` }), null);
});
