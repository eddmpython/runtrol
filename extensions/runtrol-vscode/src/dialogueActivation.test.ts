import assert from "node:assert/strict";
import { test } from "node:test";
import type { TerminalView } from "@runtrol/runtime-client";
import { setTerminalDialogue } from "./dialogueActivation";

function fixture(enabled = false, failWrite = false, failDisable = false) {
  const operations: unknown[] = [];
  const view = {
    opened: { terminal: { terminalId: "terminal", terminalGeneration: 1, dialogueEnabled: enabled } },
    acquireControl: async () => ({ leaseId: "lease", leaseGeneration: 2 }),
    setDialogue: async (params: { enabled: boolean }) => {
      operations.push(params.enabled);
      if (!params.enabled && failDisable) throw new Error("lease lost");
    },
    write: async (params: { bytesBase64: string }) => {
      operations.push(Buffer.from(params.bytesBase64, "base64").toString("utf8"));
      if (failWrite) throw new Error("input outcome unknown");
    },
  } as unknown as TerminalView;
  return { view, operations };
}

test("visible dialogue activation arms before paste and submits in a separate acknowledged write", async () => {
  const { view, operations } = fixture();
  await setTerminalDialogue(view, true, "ordinary instruction\n한국어");
  assert.deepEqual(operations, [true, "\x1b[200~ordinary instruction\n한국어\x1b[201~", "\x1b[F\r"]);
});

test("a failed paste is not resent or submitted and its activation is disarmed", async () => {
  const { view, operations } = fixture(false, true);
  await assert.rejects(setTerminalDialogue(view, true, "instruction"), /input outcome unknown/);
  assert.deepEqual(operations, [true, "\x1b[200~instruction\x1b[201~", false]);
  const lost = fixture(false, true, true);
  await assert.rejects(setTerminalDialogue(lost.view, true, "instruction"), AggregateError);
});

test("disabling and enabling an already armed process send no additional provider input", async () => {
  const disabled = fixture(true);
  await setTerminalDialogue(disabled.view, false, null);
  assert.deepEqual(disabled.operations, [false]);
  const repeated = fixture(true);
  await setTerminalDialogue(repeated.view, true, "instruction");
  assert.deepEqual(repeated.operations, [true]);
  const missing = fixture();
  await assert.rejects(setTerminalDialogue(missing.view, true, null), /visible instruction/);
  assert.deepEqual(missing.operations, []);
});
