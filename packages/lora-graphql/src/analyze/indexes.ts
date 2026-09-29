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
    };

const RANGE_OPS = new Set<FilterOperator>(["LT", "LTE", "GT", "GTE"]);

/**
 * Types a RANGE index must not be inferred for. LoraDB 0.15 leaves
 * temporal values out of its sorted index but still plans range
 * predicates over them as index scans, which then return no rows. Until
 * the engine indexes temporals (or falls back to a scan), these filter
 * and sort by label scan.
 */
export const RANGE_UNINDEXABLE = new Set<ScalarType>([
  "Date",
  "Time",
  "LocalTime",
  "DateTime",
  "LocalDateTime",
  "Duration",
]);
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

/** The DDL statement that creates a requirement, idempotently. */
export function requirementDdl(r: SchemaRequirement): string {
  const label = quote(r.label);
  const property = quote(r.property);
  const name = quote(r.name);
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
