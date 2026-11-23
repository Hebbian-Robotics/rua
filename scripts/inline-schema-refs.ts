// CONSTRAINT: json-schema-to-zod does not follow references. Recursive schemas
// have no finite inline form, so only their recursive edge becomes `{}`.

import { readFileSync } from "node:fs";

type Json = null | boolean | number | string | Json[] | { [key: string]: Json };
type JsonObject = { [key: string]: Json };

const schemaPath = process.argv[2];
const component = process.argv[3];
if (!schemaPath)
  throw new Error("usage: inline-schema-refs.ts <schema.json> [component]");
const root = JSON.parse(readFileSync(schemaPath, "utf8")) as JsonObject;
// CONSTRAINT: A JSON Schema keeps its definitions in `$defs`; an OpenAPI document in
// `components.schemas`, and then the output is an object with one property per
// component so the generated zod object has a `shape.<Component>` for each.
const openApiSchemas = (root.components as JsonObject | undefined)?.schemas as
  | JsonObject
  | undefined;
const definitions = (openApiSchemas ?? root.$defs ?? {}) as JsonObject;

function inline(node: Json, referencesInProgress: readonly string[]): Json {
  if (Array.isArray(node)) {
    return node.map((item) => inline(item, referencesInProgress));
  }
  if (node === null || typeof node !== "object") return node;
  const reference = node.$ref;
  if (typeof reference === "string") {
    const name = reference.replace(/^#\/(\$defs|components\/schemas)\//, "");
    if (referencesInProgress.includes(name)) return {};
    const definition = definitions[name];
    if (definition === undefined) throw new Error(`No definition ${name}`);
    const { $ref: _reference, ...siblings } = node;
    // WHY: A tagged newtype variant is the definition plus the tag beside it: the
    // properties and required lists add up rather than replace each other.
    const inlined = inline(definition, [
      ...referencesInProgress,
      name,
    ]) as JsonObject;
    const beside = inline(siblings, referencesInProgress) as JsonObject;
    return {
      ...inlined,
      ...beside,
      ...(inlined.properties || beside.properties
        ? {
            properties: {
              ...(inlined.properties as JsonObject | undefined),
              ...(beside.properties as JsonObject | undefined),
            },
          }
        : {}),
      ...(inlined.required || beside.required
        ? {
            required: [
              ...new Set([
                ...((inlined.required as Json[] | undefined) ?? []),
                ...((beside.required as Json[] | undefined) ?? []),
              ]),
            ],
          }
        : {}),
    };
  }
  return Object.fromEntries(
    Object.entries(node)
      // WHY: The viewer needs validation rules only. The public schema owns descriptions.
      .filter(([key, value]) => !(component && key === "description" && typeof value === "string"))
      .map(([key, value]) => [
        // CONSTRAINT: json-schema-to-zod types `oneOf` as `any`, and `anyOf` as a union.
        // The variants of a tagged enum exclude each other, so the two agree.
        key === "oneOf" ? "anyOf" : key,
        inline(value, referencesInProgress),
      ]),
  );
}

function allowsNull(schema: Json): boolean {
  if (schema === null || typeof schema !== "object" || Array.isArray(schema))
    return false;
  const { type, anyOf } = schema;
  return (
    type === "null" ||
    (Array.isArray(type) && type.includes("null")) ||
    (Array.isArray(anyOf) && anyOf.some(allowsNull))
  );
}

// CONSTRAINT: Unless explicitly omitted, Serde writes absent options as null, so received
// nullable fields must be required to avoid adding undefined to their types.
function requireNullableFields(node: Json): Json {
  if (Array.isArray(node)) return node.map(requireNullableFields);
  if (node === null || typeof node !== "object") return node;
  const result = Object.fromEntries(
    Object.entries(node).map(([key, value]) => [key, requireNullableFields(value)]),
  ) as JsonObject;
  const { properties, required } = result;
  if (properties && typeof properties === "object" && !Array.isArray(properties)) {
    const alreadyRequired = Array.isArray(required) ? required : [];
    const nullable = Object.entries(properties)
      .filter(([, schema]) => {
        if (!allowsNull(schema)) return false;
        return typeof schema === "object" && schema !== null
          && !Array.isArray(schema) && schema["x-omit-when-none"] !== true;
      })
      .map(([name]) => name);
    result.required = [...new Set([...alreadyRequired, ...nullable])] as Json;
  }
  return result;
}

const SENT_BY_CONTROLLER = new Set([
  "ServerMessage",
  "ActionSequenceRecord",
  "StationState",
  "CommandRecord",
  "LiveRun",
]);

// WHY: Batch components are already expanded inside the client and server messages.
// Omitting their duplicate top-level forms keeps the generated schema manageable.
const outputNames = openApiSchemas
  ? Object.keys(openApiSchemas).filter((name) => !name.startsWith("Batch"))
  : [];

const { $defs: _definitions, ...rest } = root;
const schema = openApiSchemas
  ? {
      type: "object",
      properties: Object.fromEntries(
        outputNames.map((name) => [name, { $ref: `#/components/schemas/${name}` }]),
      ),
      required: outputNames,
    }
  : rest;
const inlined = inline(
  component ? { $ref: `#/components/schemas/${component}` } : schema,
  [],
) as JsonObject;
const properties = (inlined.properties ?? {}) as JsonObject;
for (const name of Object.keys(properties)) {
  if (SENT_BY_CONTROLLER.has(name))
    properties[name] = requireNullableFields(properties[name] as Json);
}
process.stdout.write(`${JSON.stringify(inlined, null, 2)}\n`);
