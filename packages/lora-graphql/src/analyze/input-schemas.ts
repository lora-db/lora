// The validation rules as JSON Schema, for clients. Introspection shows a
// field's type and not the directives on it, so a client cannot learn
// from the API that a name takes at most 80 characters. This writes the
// rules down in a form every form library reads: one JSON Schema
// (2020-12) per generated input type, from which zod, yup or valibot
// schemas can be built.

import { jsonSchemaKeywords } from "../model/constraints.js";
import type { GraphModel, ScalarField, ScalarType } from "../model/types.js";
import {
  mutationNames,
  requiredOnCreate,
  settable,
} from "../schema/mutations.js";

export type JsonSchema = Record<string, unknown>;

const TYPES: Partial<Record<ScalarType, JsonSchema>> = {
  String: { type: "string" },
  ID: { type: "string" },
  Int: { type: "integer" },
  Float: { type: "number" },
  Boolean: { type: "boolean" },
  // Serialised as a decimal string; a small one may be sent as a number.
  BigInt: { type: ["integer", "string"] },
  Date: { type: "string", format: "date" },
  DateTime: { type: "string", format: "date-time" },
  Time: { type: "string", format: "time" },
  LocalTime: { type: "string" },
  LocalDateTime: { type: "string" },
  Duration: { type: "string", format: "duration" },
  Point: { type: "object" },
  CartesianPoint: { type: "object" },
};

function fieldSchema(
  model: GraphModel,
  f: ScalarField,
  nullable: boolean,
): JsonSchema {
  const { item, list } = jsonSchemaKeywords(f.constraints);
  const base: JsonSchema =
    f.type === "Enum"
      ? {
          enum: (model.enums.get(f.enumName ?? "")?.values ?? []).map(
            (v) => v.name,
          ),
        }
      : { ...TYPES[f.type] };
  const value = { ...base, ...item };
  let out: JsonSchema = f.list
    ? { type: "array", items: value, ...list }
    : value;
  if (nullable) out = { anyOf: [out, { type: "null" }] };
  if (f.description) out = { description: f.description, ...out };
  return out;
}

function objectSchema(
  model: GraphModel,
  title: string,
  fields: Iterable<ScalarField>,
  op: "CREATE" | "UPDATE",
): JsonSchema {
  const properties: Record<string, JsonSchema> = {};
  const required: string[] = [];
  for (const f of fields) {
    if (!settable(f, op)) continue;
    properties[f.name] = fieldSchema(model, f, op === "UPDATE" && !f.required);
    if (op === "CREATE" && requiredOnCreate(f)) required.push(f.name);
  }
  return {
    $schema: "https://json-schema.org/draft/2020-12/schema",
    title,
    type: "object",
    properties,
    ...(required.length > 0 ? { required } : {}),
    // Relationship fields (connect, create, ...) are not described here.
    additionalProperties: true,
  };
}

const scalarFields = (fields: ReadonlyMap<string, { kind: string }>) =>
  [...fields.values()].filter((f): f is ScalarField => f.kind === "scalar");

/**
 * A JSON Schema per generated create and update input, by input type
 * name (`PersonCreateInput`), describing the scalar fields a client may
 * set and the rules on them. A pattern is anchored, since here it must
 * match the whole value; a format is given both by name and, where it is
 * an expression, as a pattern, since validators treat `format` as a
 * remark by default.
 */
export function inputSchemas(model: GraphModel): Record<string, JsonSchema> {
  const out: Record<string, JsonSchema> = {};
  for (const node of model.nodes.values()) {
    const fields = scalarFields(node.fields);
    if (node.mutations.has("CREATE")) {
      const title = mutationNames.createInput(node.name);
      out[title] = objectSchema(model, title, fields, "CREATE");
    }
    if (node.mutations.has("UPDATE")) {
      const title = mutationNames.updateInput(node.name);
      out[title] = objectSchema(model, title, fields, "UPDATE");
    }
  }
  for (const props of model.relationshipProperties.values()) {
    const fields = scalarFields(props.fields);
    if (fields.length === 0) continue;
    for (const op of ["CREATE", "UPDATE"] as const) {
      if (!fields.some((f) => settable(f, op))) continue;
      const title = `${props.name}${op === "CREATE" ? "Create" : "Update"}Input`;
      out[title] = objectSchema(model, title, fields, op);
    }
  }
  return out;
}
