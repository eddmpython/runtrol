import assert from "node:assert/strict";
import test from "node:test";

import { editorPanelFor } from "./editorPanels";

test("a service with an installed editor surface gets its reveal command, others get nothing", () => {
  const installed = (extension: string) => extension === "anthropic.claude-code";
  assert.deepEqual(editorPanelFor("claude", installed), { reveal: "claude-vscode.editor.open" });
  assert.equal(editorPanelFor("codex", installed), null, "a service with no declared surface");
  assert.equal(editorPanelFor("claude", () => false), null, "declared but not installed");
});
