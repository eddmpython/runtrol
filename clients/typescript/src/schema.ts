import { RuntimeProtocolError } from "./errors.js";
import { VALIDATION_SCHEMA } from "./generated/schema.js";

const rootSchema = VALIDATION_SCHEMA as JsonSchema;

type JsonSchema = boolean | {
  readonly $defs?: Readonly<Record<string, JsonSchema>>;
  readonly $ref?: string;
  readonly additionalProperties?: boolean;
  readonly anyOf?: ReadonlyArray<JsonSchema>;
  readonly const?: unknown;
  readonly enum?: ReadonlyArray<unknown>;
  readonly format?: string;
  readonly items?: JsonSchema;
  readonly maximum?: number;
  readonly minimum?: number;
  readonly oneOf?: ReadonlyArray<JsonSchema>;
  readonly properties?: Readonly<Record<string, JsonSchema>>;
  readonly required?: ReadonlyArray<string>;
  readonly type?: string | ReadonlyArray<string>;
};

export function validatePublic<T>(name: string, value: unknown): T {
  const definition = typeof rootSchema === "object" ? rootSchema.$defs?.[name] : undefined;
  if (definition === undefined) {
    throw new RuntimeProtocolError(`public schema definition ${name} does not exist`);
  }
  const failure = validationFailure(definition, value, `$defs.${name}`);
  if (failure) {
    throw new RuntimeProtocolError(`${name} violates the selected public schema: ${failure}`);
  }
  return value as T;
}

function validationFailure(schema: JsonSchema, value: unknown, path: string): string | undefined {
  if (schema === true) return undefined;
  if (schema === false) return `${path} is forbidden`;
  if (schema.$ref) {
    const definitionName = schema.$ref.match(/^#\/\$defs\/([^/]+)$/)?.[1];
    const definition = definitionName && typeof rootSchema === "object"
      ? rootSchema.$defs?.[definitionName]
      : undefined;
    return definition
      ? validationFailure(definition, value, path)
      : `${path} has an unresolved schema reference`;
  }
  if (schema.const !== undefined && !jsonEqual(value, schema.const)) {
    return `${path} does not equal its required constant`;
  }
  if (schema.enum && !schema.enum.some((candidate) => jsonEqual(value, candidate))) {
    return `${path} is not an allowed value`;
  }
  if (schema.oneOf) {
    const matches = schema.oneOf.filter((candidate) => !validationFailure(candidate, value, path));
    if (matches.length !== 1) return `${path} does not match exactly one allowed shape`;
  }
  if (schema.anyOf
    && !schema.anyOf.some((candidate) => !validationFailure(candidate, value, path))) {
    return `${path} does not match any allowed shape`;
  }
  const declaredTypes = typeof schema.type === "string" ? [schema.type] : schema.type;
  const objectIsImplied = schema.properties !== undefined
    || schema.required !== undefined
    || schema.additionalProperties !== undefined;
  const expectedTypes = declaredTypes ?? (objectIsImplied ? ["object"] : undefined);
  if (expectedTypes && !expectedTypes.some((type) => matchesType(value, type))) {
    return `${path} has the wrong JSON type`;
  }
  if (typeof value === "number") {
    if (schema.minimum !== undefined && value < schema.minimum) {
      return `${path} is below its minimum`;
    }
    if (schema.maximum !== undefined && value > schema.maximum) {
      return `${path} is above its maximum`;
    }
    if (schema.format && !matchesNumberFormat(value, schema.format)) {
      return `${path} does not match ${schema.format}`;
    }
  }
  if (Array.isArray(value) && schema.items) {
    for (let index = 0; index < value.length; index += 1) {
      const failure = validationFailure(schema.items, value[index], `${path}[${index}]`);
      if (failure) return failure;
    }
  }
  if (isJsonObject(value)) {
    const properties = schema.properties ?? {};
    for (const required of schema.required ?? []) {
      if (!Object.hasOwn(value, required)) return `${path}.${required} is required`;
    }
    for (const [key, child] of Object.entries(value)) {
      const propertySchema = properties[key];
      if (propertySchema) {
        const failure = validationFailure(propertySchema, child, `${path}.${key}`);
        if (failure) return failure;
      } else if (schema.additionalProperties === false) {
        return `${path}.${key} is not allowed`;
      }
    }
  }
  return undefined;
}

function matchesType(value: unknown, type: string): boolean {
  switch (type) {
    case "array": return Array.isArray(value);
    case "boolean": return typeof value === "boolean";
    case "integer": return Number.isSafeInteger(value);
    // A JSON number with a fraction. The public schema had only integers until a provider reported money, and
    // a type this validator does not know is refused, which is how every cost report came to be rejected.
    case "number": return typeof value === "number" && Number.isFinite(value);
    case "null": return value === null;
    case "object": return isJsonObject(value);
    case "string": return typeof value === "string";
    default: return false;
  }
}

function matchesNumberFormat(value: number, format: string): boolean {
  // A real number: any finite value, sign and fraction included. A money amount is written this way, and
  // reading one as an unsigned integer is what rejected every cost report the services sent.
  if (format === "double" || format === "float") return Number.isFinite(value);
  if (format === "int32") {
    return Number.isSafeInteger(value) && value >= -0x8000_0000 && value <= 0x7fff_ffff;
  }
  if (!Number.isSafeInteger(value) || value < 0) return false;
  if (format === "uint8") return value <= 0xff;
  if (format === "uint16") return value <= 0xffff;
  if (format === "uint32") return value <= 0xffff_ffff;
  return format === "uint" || format === "uint64";
}

function isJsonObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function jsonEqual(left: unknown, right: unknown): boolean {
  return JSON.stringify(left) === JSON.stringify(right);
}

export function requireEmpty(value: unknown): void {
  if (!isJsonObject(value) || Object.keys(value).length !== 0) {
    throw new RuntimeProtocolError("Runtime returned a non-empty result for an empty operation");
  }
}
