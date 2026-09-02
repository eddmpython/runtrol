import assert from "node:assert/strict";
import test from "node:test";

import { mirrorChunks, providerCommandNames, providerOfCommand } from "./observedMirrorState";

const names = providerCommandNames([
  { providerId: "claude", commandNames: ["claude", "claude.cmd"] },
  { providerId: "codex", commandNames: ["codex"] },
  { providerId: "fixture-acp", commandNames: ["acpFixture.exe"] },
  { providerId: "silent" },
]);

test("the program word of a command line names the provider, however the shell spelled it", () => {
  assert.equal(providerOfCommand("claude", names), "claude");
  assert.equal(providerOfCommand("  claude --resume abc  ", names), "claude");
  assert.equal(providerOfCommand("CLAUDE.CMD -p hi", names), "claude");
  assert.equal(providerOfCommand("& 'C:\\Users\\me\\AppData\\Roaming\\npm\\claude.cmd' --model x", names), "claude");
  assert.equal(providerOfCommand('"C:\\tools\\codex.exe"', names), "codex");
  assert.equal(providerOfCommand("call codex.bat", names), "codex");
  assert.equal(providerOfCommand("/usr/local/bin/codex resume", names), "codex");
  assert.equal(providerOfCommand("& 'C:\\tools\\acpFixture.exe' --tui", names), "fixture-acp", "a manifest name with its extension");
  assert.equal(providerOfCommand("acpfixture", names), "fixture-acp");
  assert.equal(providerOfCommand("claudette", names), null, "a prefix is not the command");
  assert.equal(providerOfCommand("git status", names), null);
  assert.equal(providerOfCommand("", names), null);
  assert.equal(providerOfCommand("&", names), null);
});

test("captured text is fed as bounded base64 chunks of its exact UTF-8 bytes", () => {
  assert.deepEqual(mirrorChunks(""), []);
  assert.deepEqual(mirrorChunks("\u001b[?1000h한"), [Buffer.from("\u001b[?1000h한", "utf8").toString("base64")]);
  const long = "x".repeat(70 * 1024);
  const chunks = mirrorChunks(long);
  assert.equal(chunks.length, 2);
  assert.equal(Buffer.concat(chunks.map((chunk) => Buffer.from(chunk, "base64"))).toString("utf8"), long);
  assert.equal(Buffer.from(chunks[0] ?? "", "base64").length, 64 * 1024);
});
