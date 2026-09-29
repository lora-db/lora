// S1: the indexes and constraints an API needs, inferred from how it lets
// clients query each property. The author declares the API; the library
// derives the storage it needs to be fast.

import type {
  FilterOperator,
  GraphModel,
  IndexKind,
  ScalarType,
} from "../model/types.js";

export type SchemaRequirement =
  | {
      kind: "constraint";
      constraint: "NODE_KEY" | "UNIQUE" | "NOT_NULL";
      name: string;
      label: string;
      property: string;
      /** Why the library needs it, e.g. `Festival.key is the @key`. */
      reason: string;
    }
  | {
      kind: "index";
      index: IndexKind;
      name: string;
      label: string;
      property: string;
      reason: string;
    }
  | {
      /** Queried by name, so matched by name. */
      kind: "fulltext";
      name: string;
      label: string;
      properties: string[];
      analyzer: "STANDARD" | "SIMPLE";
      reason: string;
    }
  | {
      /** Queried by name, so matched by name. */
      kind: "vector";
      name: string;
      label: string;
      property: string;
      dimensions: number;
      similarity: "COSINE" | "EUCLIDEAN";
      reason: string;
    };

const RANGE_OPS = new Set<FilterOperator>(["LT", "LTE", "GT", "GTE"]);

/**
 * Types a RANGE index must not be inferred for. LoraDB keeps durations out
 * of its sorted index (they have no total order); other temporals are
 * indexed by the instant they denote (E17).
 */
export const RANGE_UNINDEXABLE = new Set<ScalarType>(["Duration"]);
const TEXT_OPS = new Set<FilterOperator>([
  "CONTAINS",
  "STARTS_WITH",
  "ENDS_WITH",
]);

/**
 * Derive every constraint and index the model's API needs.
 *
 * - `@key` → node key constraint (unique + present); it also serves
 *   exact-match, range and ordered scans, so no extra index.
 * - `@unique` → uniqueness constraint (backed by a RANGE index).
 * - `@sortable` on a non-null field → existence constraint: keyset
 *   pagination may then bound the scan with `>=` without losing nulls.
 * - `EQ` / `IN` → nothing: LoraDB builds exact-match indexes lazily.
 * - `LT` / `LTE` / `GT` / `GTE` or `@sortable` → RANGE.
 * - `CONTAINS` / `STARTS_WITH` / `ENDS_WITH` → TEXT.
 * - `WITHIN_BBOX` / `DISTANCE` on a Point → POINT.
 * - `@index(kind:)` → that index.
 */
export function inferRequirements(model: GraphModel): SchemaRequirement[] {
  const out: SchemaRequirement[] = [];
  for (const node of model.nodes.values()) {
    const label = node.labels[0]!;
    for (const s of node.search) {
      out.push(
        s.kind === "fulltext"
          ? {
              kind: "fulltext",
              name: s.name,
              label,
              properties: s.fields.map((f) => f.property),
              analyzer: s.analyzer,
              reason: `${node.name} has @fulltext ${s.name} (${s.queryName})`,
            }
          : {
              kind: "vector",
              name: s.name,
              label,
              property: s.field.property,
              dimensions: s.dimensions,
              similarity: s.similarity,
              reason: `${node.name}.${s.field.name} is a @vector (${s.queryName})`,
            },
      );
    }
    for (const f of node.fields.values()) {
      if (f.kind !== "scalar") continue;
      const at = `${node.name}.${f.name}`;
      if (f.key) {
        out.push({
          kind: "constraint",
          constraint: "NODE_KEY",
          name: schemaName(label, f.property, "key"),
          label,
          property: f.property,
          reason: `${at} is the @key`,
        });
        // A node key is backed by a RANGE index: nothing else to add.
        continue;
      }
      const hasUnique = f.unique;
      if (hasUnique) {
        out.push({
          kind: "constraint",
          constraint: "UNIQUE",
          name: schemaName(label, f.property, "unique"),
          label,
          property: f.property,
          reason: `${at} is @unique`,
        });
      }
      if (f.sortable && f.required && !f.list) {
        out.push({
          kind: "constraint",
          constraint: "NOT_NULL",
          name: schemaName(label, f.property, "exists"),
          label,
          property: f.property,
          reason: `${at} is non-null and @sortable`,
        });
      }
      const kinds = new Map<IndexKind, string>();
      const ops = [...f.filters];
      if (!hasUnique && !RANGE_UNINDEXABLE.has(f.type)) {
        const range = ops.filter((op) => RANGE_OPS.has(op));
        if (range.length > 0) {
          kinds.set("RANGE", `${at} is @filterable by ${range.join(", ")}`);
        } else if (f.sortable) {
          kinds.set("RANGE", `${at} is @sortable`);
        }
      }
      const spatial = ops.filter(
        (op) => op === "WITHIN_BBOX" || op === "DISTANCE",
      );
      if (spatial.length > 0) {
        kinds.set("POINT", `${at} is @filterable by ${spatial.join(", ")}`);
      }
      const text = ops.filter((op) => TEXT_OPS.has(op));
      if (text.length > 0) {
        kinds.set("TEXT", `${at} is @filterable by ${text.join(", ")}`);
      }
      for (const k of f.indexes) {
        if (!kinds.has(k) && !(k === "RANGE" && hasUnique)) {
          kinds.set(k, `${at} has @index(kind: ${k})`);
        }
      }
      for (const [index, reason] of kinds) {
        out.push({
          kind: "index",
          index,
          name: schemaName(label, f.property, index.toLowerCase()),
          label,
          property: f.property,
          reason,
        });
      }
    }
  }
  return out;
}

function schemaName(label: string, property: string, suffix: string): string {
  return `${snake(label)}_${snake(property)}_${suffix}`;
}

function snake(s: string): string {
  return s
    .replace(/([a-z0-9])([A-Z])/g, "$1_$2")
    .replace(/[^A-Za-z0-9]+/g, "_")
    .toLowerCase();
}

/** A stable identity: equal ids need no change between two models. */
export function requirementId(r: SchemaRequirement): string {
  switch (r.kind) {
    case "index":
      return `index:${r.index}:${r.label}:${r.property}`;
    case "constraint":
      return `constraint:${r.constraint}:${r.label}:${r.property}`;
    case "fulltext":
      return `fulltext:${r.name}:${r.label}:${r.properties.join(",")}:${r.analyzer}`;
    case "vector":
      return `vector:${r.name}:${r.label}:${r.property}:${r.dimensions}:${r.similarity}`;
  }
}

/** One line for humans, e.g. `RANGE index on :Festival(name)`. */
export function describeRequirement(r: SchemaRequirement): string {
  switch (r.kind) {
    case "index":
      return `${r.index} index on :${r.label}(${r.property})`;
    case "constraint":
      return `${r.constraint} constraint on :${r.label}(${r.property})`;
    case "fulltext":
      return `FULLTEXT index ${r.name} on :${r.label}(${r.properties.join(", ")})`;
    case "vector":
      return `VECTOR index ${r.name} on :${r.label}(${r.property}), ${r.dimensions} dimensions, ${r.similarity.toLowerCase()}`;
  }
}

/** The DDL statement that creates a requirement, idempotently. */
export function requirementDdl(r: SchemaRequirement): string {
  const label = quote(r.label);
  const name = quote(r.name);
  if (r.kind === "fulltext") {
    const props = r.properties.map((p) => `n.${quote(p)}`).join(", ");
    const options =
      r.analyzer === "SIMPLE"
        ? " OPTIONS {indexConfig: {`fulltext.analyzer`: 'simple'}}"
        : "";
    return `CREATE FULLTEXT INDEX ${name} IF NOT EXISTS FOR (n:${label}) ON EACH [${props}]${options}`;
  }
  if (r.kind === "vector") {
    return `CREATE VECTOR INDEX ${name} IF NOT EXISTS FOR (n:${label}) ON (n.${quote(r.property)}) OPTIONS {indexConfig: {\`vector.dimensions\`: ${r.dimensions}, \`vector.similarity_function\`: '${r.similarity.toLowerCase()}'}}`;
  }
  const property = quote(r.property);
  if (r.kind === "constraint") {
    const kind = {
      NODE_KEY: "IS NODE KEY",
      UNIQUE: "IS UNIQUE",
      NOT_NULL: "IS NOT NULL",
    }[r.constraint];
    return `CREATE CONSTRAINT ${name} IF NOT EXISTS FOR (n:${label}) REQUIRE n.${property} ${kind}`;
  }
  const kind = r.index === "RANGE" ? "" : `${r.index} `;
  return `CREATE ${kind}INDEX ${name} IF NOT EXISTS FOR (n:${label}) ON (n.${property})`;
}

function quote(id: string): string {
  return "`" + id.replace(/`/g, "``") + "`";
}
