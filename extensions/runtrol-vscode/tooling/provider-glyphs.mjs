import { readFile, readdir, writeFile } from "node:fs/promises";
import { spawnSync } from "node:child_process";
import path from "node:path";
import { fileURLToPath } from "node:url";

const extensionRoot = fileURLToPath(new URL("../", import.meta.url));
const repositoryRoot = fileURLToPath(new URL("../../../", import.meta.url));
const accents = JSON.parse(await readFile(path.join(extensionRoot, "src/projectAccents.json"), "utf8"));

/// Manifest projection and font construction share one input, even when the manifest is imported before a build.
export const providerGlyphs = glyphProjection(await discoverIconNames(), accents);

export function glyphProjection(iconNames, accents) {
  const names = [...new Set(iconNames)].sort();
  if (names.length === 0 || names.some((name) => !/^[a-z0-9-]{1,64}$/u.test(name))) {
    throw new Error("provider glyph names must be closed manifest identifiers");
  }
  if (!Array.isArray(accents) || accents.length === 0 || new Set(accents).size !== accents.length
    || accents.some((accent) => !/^#[a-f0-9]{6}$/u.test(accent))) {
    throw new Error("provider glyphs require the unique canonical project palette");
  }
  const glyphs = [];
  const icons = {};
  const iconMap = {};
  let codepoint = 0xe000;
  for (const name of names) {
    iconMap[name] = {};
    for (const [paletteIndex, accent] of accents.entries()) {
      if (codepoint > 0xf8ff) throw new Error("provider glyphs exceed the private-use character range");
      const id = `runtrol-${name}-${accent.slice(1)}`;
      glyphs.push({ name, paletteIndex, codepoint });
      iconMap[name][accent] = id;
      icons[id] = {
        description: `Coding service glyph in project accent ${accent}.`,
        default: {
          fontPath: "./resources/provider-icons/providerIcons.woff",
          fontCharacter: String.fromCodePoint(codepoint),
        },
      };
      codepoint += 1;
    }
  }
  return { names, accents: [...accents], glyphs, icons, iconMap };
}

async function discoverIconNames() {
  const names = new Set(["sparkle"]);
  const manifests = path.join(repositoryRoot, "crates/runtrol-drivers/manifests");
  for (const entry of await readdir(manifests, { withFileTypes: true })) {
    if (!entry.isFile() || path.extname(entry.name) !== ".toml") continue;
    const manifest = await readFile(path.join(manifests, entry.name), "utf8");
    const icon = /^icon\s*=\s*"([a-z0-9-]{1,64})"\s*$/mu.exec(manifest)?.[1];
    if (icon) names.add(icon);
  }
  return [...names];
}

export async function buildProviderFont(directory) {
  const generated = spawnSync("uv", [
    "run", "--no-project", "--script", path.join(extensionRoot, "tooling/providerFont.py"),
  ], {
    input: JSON.stringify({ names: providerGlyphs.names, accents: providerGlyphs.accents,
      glyphs: providerGlyphs.glyphs, directory }),
    encoding: "utf8",
    windowsHide: true,
    maxBuffer: 1024 * 1024,
    env: { ...process.env, PYTHONDONTWRITEBYTECODE: "1" },
  });
  if (generated.error || generated.status !== 0) {
    throw new Error(`provider icon font build failed: ${generated.error?.message ?? generated.stderr}`);
  }
  await writeFile(path.join(directory, "iconMap.json"), `${JSON.stringify(providerGlyphs.iconMap)}\n`, "utf8");
}
