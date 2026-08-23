import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

import { VALIDATION_SCHEMA } from "../src/generated/schema.js";

const DOCUMENT_SCHEMA = new URL("../../schema/runtime.schema.json", import.meta.url);
const MAX_VALIDATION_SCHEMA_BYTES = 40 * 1024;

test("the runtime validator carries a bounded complete definition projection", async () => {
  const document = JSON.parse(await readFile(DOCUMENT_SCHEMA, "utf8")) as {
    readonly $defs?: Readonly<Record<string, unknown>>;
  };
  assert.ok(document.$defs, "the public document must carry definitions");
  assert.deepEqual(
    Object.keys(VALIDATION_SCHEMA.$defs).sort(),
    Object.keys(document.$defs).sort(),
    "the projection must not lose a public definition",
  );

  const encoded = JSON.stringify(VALIDATION_SCHEMA);
  assert.ok(
    Buffer.byteLength(encoded, "utf8") <= MAX_VALIDATION_SCHEMA_BYTES,
    `the runtime validation projection exceeded ${MAX_VALIDATION_SCHEMA_BYTES} bytes`,
  );
  assert.equal(descriptionKeywordsIn(VALIDATION_SCHEMA), 0, "documentation must stay out of runtime memory");

  const publicSession = document.$defs.SessionDescriptor as {
    readonly properties: Readonly<Record<string, unknown>>;
  };
  const validationSession = VALIDATION_SCHEMA.$defs.SessionDescriptor as {
    readonly properties: Readonly<Record<string, unknown>>;
  };
  assert.deepEqual(
    Object.keys(validationSession.properties).sort(),
    Object.keys(publicSession.properties).sort(),
    "the projection must preserve object property names",
  );

  const definitions = VALIDATION_SCHEMA.$defs as Readonly<Record<string, unknown>>;
  for (const reference of referencesIn(VALIDATION_SCHEMA)) {
    const name = reference.match(/^#\/\$defs\/([^/]+)$/u)?.[1];
    assert.ok(name && Object.hasOwn(definitions, name), `unresolved validation reference ${reference}`);
  }
});

function referencesIn(value: unknown): string[] {
  if (Array.isArray(value)) return value.flatMap(referencesIn);
  if (!value || typeof value !== "object") return [];
  return Object.entries(value as Readonly<Record<string, unknown>>).flatMap(([key, child]) => (
    key === "$ref" && typeof child === "string" ? [child] : referencesIn(child)
  ));
}

function descriptionKeywordsIn(value: unknown): number {
  if (Array.isArray(value)) {
    return value.reduce((count, child) => count + descriptionKeywordsIn(child), 0);
  }
  if (!value || typeof value !== "object") return 0;
  const node = value as Readonly<Record<string, unknown>>;
  let count = Object.hasOwn(node, "description") ? 1 : 0;
  for (const [key, child] of Object.entries(node)) {
    if (key === "description") continue;
    if (key === "properties" && child && typeof child === "object" && !Array.isArray(child)) {
      count += Object.values(child).reduce(
        (total, property) => total + descriptionKeywordsIn(property),
        0,
      );
    } else {
      count += descriptionKeywordsIn(child);
    }
  }
  return count;
}
