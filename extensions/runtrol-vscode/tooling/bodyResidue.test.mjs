import assert from "node:assert/strict";
import { mkdir, mkdtemp, realpath, rm, symlink, writeFile } from "node:fs/promises";
import { inspect } from "node:util";
import os from "node:os";
import path from "node:path";
import test, { after, before } from "node:test";

import { assertNoBodyResidue } from "./bodyResidue.mjs";

const executionRoot = process.platform === "win32"
  ? path.join(process.env.LOCALAPPDATA, "dev-workspace")
  : path.join(os.homedir(), ".local", "share", "dev-workspace");
let ownedRoot;
let fixtureNumber = 0;
before(async () => {
  await mkdir(executionRoot, { recursive: true });
  ownedRoot = await mkdtemp(path.join(await realpath(executionRoot), "runtrolBodyResidue-"));
});
after(async () => {
  if (!ownedRoot) return;
  const resolved = await realpath(ownedRoot);
  assert.ok(path.dirname(resolved) === await realpath(executionRoot), "cleanup stays inside the execution root");
  assert.ok(path.basename(resolved).startsWith("runtrolBodyResidue-"), "cleanup owns this fixture");
  await rm(resolved, { recursive: true, force: true });
});

const sentinel = "courier-probe-";
const body = `${sentinel}한국어 English\nopaque \"body\" \\\n`;
const unicodeEscape = (text) => text.replace(/[\s\S]/gu,
  (character) => `\\u${character.charCodeAt(0).toString(16).padStart(4, "0")}`);
const representations = [
  ["raw body", body],
  ["JSON escaped body", JSON.stringify({ body })],
  ["explicit ASCII sentinel inside a JSON body", JSON.stringify({ body }), sentinel],
  ["Unicode escaped body", unicodeEscape(body)],
  ["mixed ASCII escapes", sentinel.replace(/e/gu, "\\u0065"), sentinel],
  ["uppercase hexadecimal escapes", unicodeEscape(sentinel).toUpperCase().replaceAll("\\U", "\\u"), sentinel],
];

for (const encoding of ["utf8", "utf16le"]) {
  for (const [label, text, marker = body] of representations) {
    test(`rejects ${label} in ${encoding} without exposing the marker`, async () => {
      const directory = await fixture();
      await writeFile(path.join(directory, "nested", "record.bin"), Buffer.from(text, encoding));
      await rejectedWithoutBody(directory, marker);
    });
  }
}

test("checks every marker and never includes a body-bearing filename in its error", async () => {
  const directory = await fixture();
  await writeFile(path.join(directory, "nested", sentinel), body);
  await rejectedWithoutBody(directory, ["another-unique-marker", body]);
});

test("detects a marker stored only in a filename or empty directory name", async () => {
  const directory = await fixture();
  await writeFile(path.join(directory, "nested", sentinel), "structural bytes only");
  await rejectedWithoutBody(directory, sentinel);
  const emptyDirectory = await fixture();
  await mkdir(path.join(emptyDirectory, "nested", sentinel));
  await rejectedWithoutBody(emptyDirectory, sentinel);
});

test("refuses directory links outside the audit root and a linked audit root", async (context) => {
  const directory = await fixture();
  const outside = await fixture();
  const linked = path.join(directory, "nested", "outside");
  try {
    await symlink(outside, linked, process.platform === "win32" ? "junction" : "dir");
  } catch (error) {
    // Windows may deny link creation without the required privilege. Other failures remain test failures.
    if (process.platform === "win32" && ["EPERM", "EACCES"].includes(error.code)) {
      context.skip("Windows denied the directory link privilege");
      return;
    }
    throw error;
  }
  await rejectedWithoutBody(directory, sentinel, "body residue scanning refuses symbolic links");
  await rejectedWithoutBody(linked, sentinel, "body residue scanning refuses symbolic links");
});

test("refuses file links instead of silently skipping their bytes", async (context) => {
  const directory = await fixture();
  const outside = await fixture();
  const target = path.join(outside, "record.bin");
  await writeFile(target, "structural bytes only");
  try {
    await symlink(target, path.join(directory, "nested", "outside.bin"), "file");
  } catch (error) {
    // This is a known Windows link privilege result, not evidence that the scanner handled a created link.
    if (process.platform === "win32" && ["EPERM", "EACCES"].includes(error.code)) {
      context.skip("Windows denied the file link privilege");
      return;
    }
    throw error;
  }
  await rejectedWithoutBody(directory, sentinel, "body residue scanning refuses symbolic links");
});

test("filesystem failures do not expose a body-bearing path or retain the original cause", async () => {
  const directory = await fixture();
  await rejectedWithoutBody(path.join(directory, sentinel), sentinel,
    "body residue scanning could not read an audit entry");
});

test("a body without an ASCII sentinel still has its JSON escaped representation checked", async () => {
  const directory = await fixture();
  const unicodeBody = "한국어\nopaque body\n";
  await writeFile(path.join(directory, "nested", "record.json"), JSON.stringify({ body: unicodeBody }));
  await rejectedWithoutBody(directory, unicodeBody);
});

test("structural files pass even when a fixture pipe shares the old body's ASCII prefix", async () => {
  const directory = await fixture();
  await writeFile(path.join(directory, "nested", "record.json"), '{"bodyBytes":42,"answer":"received"}');
  await writeFile(path.join(directory, "provider.toml"), 'RUNTROL_COURIER_PROBE = "runtrol-courier-probe-1234"');
  await assertNoBodyResidue(directory, [body, "body 한국어\n"]);
});

test("refuses an empty marker set instead of reporting an unaudited directory clean", async () => {
  const directory = await fixture();
  for (const markers of [[], "", [body, ""]]) {
    await assert.rejects(assertNoBodyResidue(directory, markers), {
      name: "TypeError", message: "body residue scanning requires nonempty markers",
    });
  }
});

async function fixture() {
  const directory = path.join(ownedRoot, String(++fixtureNumber));
  await mkdir(path.join(directory, "nested"), { recursive: true });
  return directory;
}

async function rejectedWithoutBody(directory, markers, message = "opaque body retained in a scanned file") {
  let failure;
  try {
    await assertNoBodyResidue(directory, markers);
  } catch (error) {
    failure = error;
  }
  assert.ok(failure instanceof Error, "injected residue must fail the scan");
  assert.ok(failure.message === message, "the failure contains only structural text");
  const rendered = inspect(failure);
  assert.ok(!rendered.includes(sentinel) && !rendered.includes("한국어"), "the scanner never prints the body");
}
