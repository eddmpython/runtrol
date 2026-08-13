import { readFile, writeFile, mkdir } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";

const here = dirname(fileURLToPath(import.meta.url));
const packageRoot = resolve(here, "..");
const sourceSchema = resolve(
  packageRoot,
  "../../crates/runtrol-runtime-protocol/schema/runtime.schema.json",
);
const generatedTypes = resolve(packageRoot, "src/generated/protocol.ts");
const packagedSchema = resolve(packageRoot, "schema/runtime.schema.json");
const checking = process.argv.includes("--check");

const schemaText = await readFile(sourceSchema, "utf8");
const schema = JSON.parse(schemaText);
const definitions = schema.$defs;
if (!definitions || typeof definitions !== "object" || Array.isArray(definitions)) {
  throw new Error("public Runtime schema has no object $defs");
}

function quoted(value) {
  return JSON.stringify(value);
}

function propertyName(name) {
  return /^[A-Za-z_$][A-Za-z0-9_$]*$/.test(name) ? name : quoted(name);
}

function typeOf(node) {
  if (node === true) return "unknown";
  if (node === false) return "never";
  if (!node || typeof node !== "object" || Array.isArray(node)) return "unknown";
  if (typeof node.$ref === "string") return node.$ref.split("/").at(-1);
  if (Object.hasOwn(node, "const")) return quoted(node.const);
  if (Array.isArray(node.enum)) return node.enum.map(quoted).join(" | ") || "never";
  if (Array.isArray(node.oneOf)) return node.oneOf.map(typeOf).join(" | ");
  if (Array.isArray(node.anyOf)) return node.anyOf.map(typeOf).join(" | ");
  if (Array.isArray(node.allOf)) return node.allOf.map(typeOf).join(" & ");
  if (Array.isArray(node.type)) {
    return node.type.map((one) => typeOf({ ...node, type: one })).join(" | ");
  }
  switch (node.type) {
    case "null":
      return "null";
    case "boolean":
      return "boolean";
    case "integer":
    case "number":
      return "number";
    case "string":
      return "string";
    case "array":
      return `ReadonlyArray<${typeOf(node.items ?? true)}>`;
    case "object":
      return objectType(node);
    default:
      return "unknown";
  }
}

function objectType(node) {
  const required = new Set(Array.isArray(node.required) ? node.required : []);
  const properties = node.properties && typeof node.properties === "object"
    ? Object.entries(node.properties)
    : [];
  const lines = properties.map(([name, value]) =>
    `readonly ${propertyName(name)}${required.has(name) ? "" : "?"}: ${typeOf(value)};`,
  );
  if (node.additionalProperties === true) {
    lines.push("readonly [key: string]: unknown;");
  } else if (node.additionalProperties && typeof node.additionalProperties === "object") {
    lines.push(`readonly [key: string]: ${typeOf(node.additionalProperties)};`);
  }
  if (lines.length === 0) return "Readonly<Record<string, never>>";
  return `{ ${lines.join(" ")} }`;
}

function documentation(node) {
  if (typeof node.description !== "string" || node.description.length === 0) return "";
  return `/** ${node.description.replaceAll("*/", "* /")} */\n`;
}

const declarations = Object.entries(definitions).map(([name, node]) => {
  const docs = documentation(node);
  const plainObject = node && typeof node === "object" && !Array.isArray(node)
    && node.type === "object" && !node.oneOf && !node.anyOf && !node.allOf;
  if (!plainObject) return `${docs}export type ${name} = ${typeOf(node)};`;
  const body = objectType(node);
  if (body === "Readonly<Record<string, never>>") {
    return `${docs}export type ${name} = ${body};`;
  }
  return `${docs}export interface ${name} ${body}`;
});

const revisions = schema["x-runtrol-finalized-revisions"];
const limits = schema["x-runtrol-limits"];
if (!Array.isArray(revisions) || !limits || typeof limits !== "object") {
  throw new Error("public Runtime schema omits generated revision or limit inventory");
}

const generated = `// Generated from crates/runtrol-runtime-protocol/schema/runtime.schema.json. Do not edit.\n\n`
  + `export const FINALIZED_REVISIONS = ${JSON.stringify(revisions)} as const;\n`
  + `export const PUBLIC_LIMITS = ${JSON.stringify(limits, null, 2)} as const;\n\n`
  + `${declarations.join("\n\n")}\n`;

async function compare(path, expected, label) {
  let actual;
  try {
    actual = await readFile(path, "utf8");
  } catch {
    throw new Error(`${label} is missing; run npm run generate`);
  }
  if (actual !== expected) throw new Error(`${label} is stale; run npm run generate`);
}

if (checking) {
  await compare(generatedTypes, generated, "generated TypeScript protocol");
  await compare(packagedSchema, schemaText, "packaged Runtime schema");
} else {
  await mkdir(dirname(generatedTypes), { recursive: true });
  await mkdir(dirname(packagedSchema), { recursive: true });
  await writeFile(generatedTypes, generated, "utf8");
  await writeFile(packagedSchema, schemaText, "utf8");
}
