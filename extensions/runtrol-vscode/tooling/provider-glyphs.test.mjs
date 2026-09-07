import assert from "node:assert/strict";
import test from "node:test";
import { cp, mkdir, mkdtemp, readdir, rm } from "node:fs/promises";
import { spawnSync } from "node:child_process";
import os from "node:os";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

import { glyphProjection, providerGlyphs } from "./provider-glyphs.mjs";
import { packageManifest } from "./extension-manifest.mjs";

test("font assignments and manifest registrations share provider discovery and project accents", () => {
  assert.deepEqual(packageManifest.contributes.icons, providerGlyphs.icons);
  for (const assignment of providerGlyphs.glyphs) {
    const accent = providerGlyphs.accents[assignment.paletteIndex];
    const id = providerGlyphs.iconMap[assignment.name][accent];
    assert.equal(providerGlyphs.icons[id].default.fontCharacter.codePointAt(0), assignment.codepoint);
  }
  assert.equal(new Set(providerGlyphs.glyphs.map((glyph) => glyph.codepoint)).size, providerGlyphs.glyphs.length);
});

test("provider discovery ordering and duplicates cannot change generated glyph assignments", () => {
  const first = glyphProjection(["service-b", "service-a"], ["#123456", "#abcdef"]);
  const reordered = glyphProjection(["service-a", "service-b", "service-a"], ["#123456", "#abcdef"]);
  assert.deepEqual(first, reordered);
  const extended = glyphProjection(["service-a", "service-b", "service-c"], ["#123456", "#abcdef"]);
  assert.equal(extended.glyphs.length, 6);
  assert.ok(extended.iconMap["service-c"]["#abcdef"]);
});

test("invalid glyph names and duplicate or malformed accents fail before assets are generated", () => {
  assert.throws(() => glyphProjection(["../outside"], ["#123456"]));
  assert.throws(() => glyphProjection(["service-a"], ["#123456", "#123456"]));
  assert.throws(() => glyphProjection(["service-a"], ["#ABCDEF"]));
  assert.throws(() => glyphProjection(["service-a"], []));
});

test("a clean source checkout reads its extension manifest without installed build dependencies", async () => {
  const sourceRoot = fileURLToPath(new URL("../../../", import.meta.url));
  const temporary = await mkdtemp(path.join(os.tmpdir(), "runtrol-glyph-manifest-"));
  try {
    const paths = [
      "extensions/runtrol-vscode/package.json",
      "extensions/runtrol-vscode/release-policy.json",
      "extensions/runtrol-vscode/src/projectAccents.json",
      "extensions/runtrol-vscode/tooling/extension-manifest.mjs",
      "extensions/runtrol-vscode/tooling/provider-glyphs.mjs",
    ];
    const manifests = "crates/runtrol-drivers/manifests";
    for (const name of await readdir(path.join(sourceRoot, manifests))) {
      if (name.endsWith(".toml")) paths.push(path.join(manifests, name));
    }
    for (const relative of paths) {
      const target = path.join(temporary, relative);
      await mkdir(path.dirname(target), { recursive: true });
      await cp(path.join(sourceRoot, relative), target);
    }
    const manifest = pathToFileURL(path.join(temporary, "extensions/runtrol-vscode/tooling/extension-manifest.mjs"));
    const probe = spawnSync(process.execPath, ["--input-type=module", "--eval",
      "const { packageManifest } = await import(process.argv[1]); console.log(Object.keys(packageManifest.contributes.icons).length);",
      manifest.href,
    ], { encoding: "utf8", windowsHide: true });
    assert.equal(probe.status, 0, probe.error?.message ?? probe.stderr);
    assert.equal(Number(probe.stdout.trim()), Object.keys(providerGlyphs.icons).length);
  } finally {
    const resolved = path.resolve(temporary);
    assert.equal(path.dirname(resolved), path.resolve(os.tmpdir()));
    assert.ok(path.basename(resolved).startsWith("runtrol-glyph-manifest-"));
    await rm(resolved, { recursive: true });
  }
});
