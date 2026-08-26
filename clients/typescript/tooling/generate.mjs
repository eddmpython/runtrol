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
const generatedSchema = resolve(packageRoot, "src/generated/schema.ts");
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

// The complete schema is a public documentation artifact. The SDK validator needs only the keywords it executes.
// Keeping those two representations separate avoids allocating descriptions, root catalogue aliases, release
// metadata, and other documentation on every Runtime client activation without weakening one wire check.
const validationKeywords = new Set([
  "$ref",
  "additionalProperties",
  "anyOf",
  "const",
  "enum",
  "format",
  "items",
  "maximum",
  "minimum",
  "oneOf",
  "properties",
  "required",
  "type",
]);

function compactValidationNode(node) {
  if (Array.isArray(node)) return node.map(compactValidationNode);
  if (!node || typeof node !== "object") return node;
  return Object.fromEntries(
    Object.entries(node)
      .filter(([key]) => validationKeywords.has(key))
      .filter(([key, value]) => {
        // The validator infers an object from its closed object keywords, and a constant already proves
        // its JSON type. Omitting those redundant type words keeps the complete checked projection inside
        // the existing activation memory budget as the public contract grows.
        if (key !== "type") return true;
        if (Object.hasOwn(node, "const")) return false;
        if (value !== "object") return true;
        return !Object.hasOwn(node, "properties")
          && !Object.hasOwn(node, "required")
          && !Object.hasOwn(node, "additionalProperties");
      })
      .map(([key, value]) => {
        if (key === "properties") {
          return [
            key,
            Object.fromEntries(
              Object.entries(value).map(([name, child]) => [name, compactValidationNode(child)]),
            ),
          ];
        }
        return [key, compactValidationNode(value)];
      }),
  );
}

function compactValidationSchema(sourceDefinitions) {
  return {
    $defs: Object.fromEntries(
      Object.entries(sourceDefinitions).map(([name, node]) => [name, compactValidationNode(node)]),
    ),
  };
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
const validationSchema = compactValidationSchema(definitions);
const generatedSchemaText = "// Generated validation projection. The complete public schema remains in schema/runtime.schema.json. Do not edit.\n\n"
  + `export const VALIDATION_SCHEMA = ${JSON.stringify(validationSchema)} as const;\n`;

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
  await compare(generatedSchema, generatedSchemaText, "generated TypeScript schema document");
  await compare(packagedSchema, schemaText, "packaged Runtime schema");
} else {
  await mkdir(dirname(generatedTypes), { recursive: true });
  await mkdir(dirname(packagedSchema), { recursive: true });
  await writeFile(generatedTypes, generated, "utf8");
  await writeFile(generatedSchema, generatedSchemaText, "utf8");
  await writeFile(packagedSchema, schemaText, "utf8");
}
