#!/usr/bin/env node

import { createHash } from "node:crypto";
import { mkdir, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const REGISTRY_URL = "https://cdn.agentclientprotocol.com/registry/v1/latest/registry.json";
const REGISTRY_SCHEMA = "1.0.0";
const MAX_REGISTRY_BYTES = 4 * 1024 * 1024;
const MAX_AGENTS = 256;
const HANDWRITTEN = ["claude", "codex", "cline", "opencode", "grok"];
const OFFICIAL_REPLACEMENTS = new Map([
  ["cline", "cline"],
  ["opencode", "opencode"],
  ["grok-build", "grok"],
]);
const OUTPUT = fileURLToPath(new URL("../src/generated_acp_registry.rs", import.meta.url));

function fail(message) {
  throw new Error(`ACP registry sync refused: ${message}`);
}

function plain(value, what, maximum = 4096) {
  if (typeof value !== "string" || value.length === 0 || value.length > maximum || /[\u0000-\u001f\u007f]/u.test(value)) {
    fail(`${what} is not bounded plain text`);
  }
  return value;
}

function words(value, what) {
  if (value === undefined) return [];
  if (!Array.isArray(value) || value.length > 64) fail(`${what} is not a bounded argument list`);
  return value.map((word, index) => plain(word, `${what}[${index}]`, 4096));
}

function packageCoordinate(value) {
  const coordinate = plain(value, "npx package", 512);
  const split = coordinate.lastIndexOf("@");
  if (split <= 0 || split === coordinate.length - 1) fail(`npx package ${coordinate} is not exact`);
  const name = coordinate.slice(0, split);
  const version = coordinate.slice(split + 1);
  if (!/^(@[a-z0-9._-]+\/)?[a-z0-9._-]+$/u.test(name) || !/^[0-9]+\.[0-9]+\.[0-9]+(?:[-+][0-9A-Za-z.-]+)?$/u.test(version)) {
    fail(`npx package ${coordinate} is not an exact safe package coordinate`);
  }
  return { coordinate, name, version };
}

function commandName(command) {
  const normalized = plain(command, "binary command", 1024).replaceAll("\\", "/");
  const name = normalized.slice(normalized.lastIndexOf("/") + 1);
  if (!name || !/^[A-Za-z0-9._+-]+$/u.test(name) || name.startsWith("-")) {
    fail(`binary command ${command} has no safe executable name`);
  }
  return name;
}

function executableCandidates(names) {
  const candidates = new Set();
  for (const value of names) {
    const name = commandName(value);
    candidates.add(name);
    if (name.endsWith(".exe") || name.endsWith(".cmd")) {
      candidates.add(name.replace(/\.(exe|cmd)$/u, ""));
    } else {
      candidates.add(`${name}.cmd`);
      candidates.add(`${name}.exe`);
    }
  }
  return [...candidates];
}

function toml(value) {
  return JSON.stringify(value).replaceAll("\u2028", "\\u2028").replaceAll("\u2029", "\\u2029");
}

function sameWords(left, right) {
  return left.length === right.length && left.every((word, index) => word === right[index]);
}

function binaryLaunch(agent) {
  const targets = Object.values(agent.distribution?.binary ?? {});
  if (targets.length === 0) return null;
  let args = null;
  const commands = [];
  for (const target of targets) {
    if (!target || typeof target !== "object" || Object.keys(target.env ?? {}).length > 0) return null;
    const current = words(target.args, `${agent.id} binary args`);
    if (args === null) args = current;
    if (!sameWords(args, current)) return null;
    commands.push(target.cmd);
  }
  return { names: executableCandidates(commands), args: args ?? [], install: null };
}

async function npmLaunch(agent) {
  const npx = agent.distribution?.npx;
  if (!npx || typeof npx !== "object" || Object.keys(npx.env ?? {}).length > 0) return null;
  const coordinate = packageCoordinate(npx.package);
  const url = `https://registry.npmjs.org/${encodeURIComponent(coordinate.name)}/${encodeURIComponent(coordinate.version)}`;
  const response = await fetch(url, { headers: { accept: "application/json" } });
  if (!response.ok) fail(`npm metadata for ${coordinate.coordinate} answered ${response.status}`);
  const metadata = await response.json();
  if (metadata.name !== coordinate.name || metadata.version !== coordinate.version) {
    fail(`npm metadata identity changed for ${coordinate.coordinate}`);
  }
  const bin = metadata.bin;
  const names = typeof bin === "string"
    ? [coordinate.name.slice(coordinate.name.lastIndexOf("/") + 1)]
    : bin && typeof bin === "object"
      ? Object.keys(bin)
      : [];
  if (names.length === 0 || names.length > 32) return null;
  for (const name of names) commandName(name);
  return {
    names: executableCandidates(names),
    args: words(npx.args, `${agent.id} npx args`),
    install: `npm install --global ${coordinate.coordinate}`,
  };
}

async function launch(agent) {
  const binary = binaryLaunch(agent);
  if (binary) return binary;
  return npmLaunch(agent);
}

function manifest(agent, selected) {
  const displayName = agent.id.endsWith("-acp") && !/\bacp\b/iu.test(agent.name)
    ? `${agent.name} ACP`
    : agent.name;
  const lines = [
    `# Generated from the official ACP Registry ${REGISTRY_SCHEMA}. Do not edit by hand.`,
    "schema = 1",
    `id = ${toml(agent.id)}`,
    `display_name = ${toml(displayName)}`,
    'icon = "hubot"',
    'kind = "acp"',
    "",
    "[bin]",
    `names = [${selected.names.map(toml).join(", ")}]`,
    "",
    "[transport]",
    `argv = [${selected.args.map(toml).join(", ")}]`,
    'listen = "stdio"',
  ];
  if (selected.install) lines.push("", "[help]", `install = ${toml(selected.install)}`);
  return `${lines.join("\n")}\n`;
}

async function main() {
  const response = await fetch(REGISTRY_URL, { headers: { accept: "application/json" }, redirect: "error" });
  if (!response.ok) fail(`registry answered ${response.status}`);
  const bytes = Buffer.from(await response.arrayBuffer());
  if (bytes.length === 0 || bytes.length > MAX_REGISTRY_BYTES) fail(`registry size ${bytes.length} is outside the bound`);
  const registry = JSON.parse(bytes.toString("utf8"));
  if (registry.version !== REGISTRY_SCHEMA) fail(`registry schema ${registry.version} is not ${REGISTRY_SCHEMA}`);
  if (!Array.isArray(registry.agents) || registry.agents.length === 0 || registry.agents.length > MAX_AGENTS) {
    fail("agent count is outside the bound");
  }
  const seen = new Set();
  const generated = [];
  const skipped = [];
  for (const raw of registry.agents) {
    const id = plain(raw?.id, "agent id", 40);
    if (!/^[a-z0-9]+(?:-[a-z0-9]+)*$/u.test(id) || seen.has(id)) fail(`agent id ${id} is invalid or repeated`);
    seen.add(id);
    if (OFFICIAL_REPLACEMENTS.has(id)) continue;
    const agent = { ...raw, id, name: plain(raw.name, `${id} name`, 160) };
    const selected = await launch(agent);
    if (!selected) {
      skipped.push(id);
      continue;
    }
    generated.push({ id, text: manifest(agent, selected) });
  }
  generated.sort((left, right) => left.id.localeCompare(right.id, "en"));
  const sha256 = createHash("sha256").update(bytes).digest("hex");
  const prelude = [
    "// Generated by tooling/sync-acp-registry.mjs from the official ACP Registry.",
    "// Runtime performs no registry request and never invokes npx or another downloader.",
    "/// ACP Registry format consumed by the maintainer-only synchronizer.",
    `pub const ACP_REGISTRY_SCHEMA: &str = ${toml(REGISTRY_SCHEMA)};`,
    "/// SHA-256 of the exact official registry snapshot used for this build.",
    "pub const ACP_REGISTRY_SHA256: &str =",
    `    ${toml(sha256)};`,
    "/// Agent declarations in that official snapshot.",
    `pub const ACP_REGISTRY_AGENT_COUNT: usize = ${registry.agents.length};`,
    "/// Snapshot agents safely expressible as local executable ACP manifests.",
    `pub const ACP_REGISTRY_ADAPTER_COUNT: usize = ${generated.length};`,
    "/// Official entries served by richer handwritten manifests, including equivalent launch identities.",
    `pub const ACP_REGISTRY_REPLACED_COUNT: usize = ${OFFICIAL_REPLACEMENTS.size};`,
    "/// Snapshot agents skipped because their launch requires unsupported environment or distribution semantics.",
    `pub const ACP_REGISTRY_SKIPPED_COUNT: usize = ${skipped.length};`,
    "",
    "/// Handwritten providers followed by generated official ACP Registry adapters.",
    "pub const MANIFESTS: &[&str] = &[",
    ...HANDWRITTEN.map((id) => `    include_str!(\"../manifests/${id}.toml\"),`),
    ...generated.map(({ text }) => `    r#\"${text}\"#,`),
    "];",
    "",
  ].join("\n");
  await mkdir(path.dirname(OUTPUT), { recursive: true });
  await writeFile(OUTPUT, prelude, { encoding: "utf8" });
  process.stdout.write(`ACP Registry ${REGISTRY_SCHEMA}: ${generated.length} safe local adapters, ${skipped.length} skipped, sha256 ${sha256}\n`);
}

await main();
