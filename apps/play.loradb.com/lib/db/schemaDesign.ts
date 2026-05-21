"use client";

/**
 * Typed wrappers over `SHOW INDEXES` / `SHOW CONSTRAINTS` and the
 * corresponding `CREATE` / `DROP` statements. These keep the rest of
 * the playground unaware of the row shapes returned by the engine.
 *
 * Every function returns a normalised value or throws on engine
 * error — callers are expected to wrap each call in `runOrToast` /
 * a try/catch that surfaces the friendly message via
 * `errorTranslate.translateError`.
 */

import { run } from "@/lib/db/client";
import type {
  ConstraintDef,
  ConstraintKind,
  IndexDef,
  IndexKind,
} from "@/lib/schemaDesign/types";

class SchemaDesignError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "SchemaDesignError";
  }
}

function valueToString(v: unknown): string {
  return typeof v === "string" ? v : "";
}

function valueToStringList(v: unknown): string[] {
  if (!Array.isArray(v)) return [];
  return v.filter((x): x is string => typeof x === "string");
}

function valueToNumber(v: unknown, fallback = 0): number {
  return typeof v === "number" && Number.isFinite(v) ? v : fallback;
}

const INDEX_KINDS: Record<string, IndexKind> = {
  range: "RANGE",
  text: "TEXT",
  point: "POINT",
  lookup: "LOOKUP",
  fulltext: "FULLTEXT",
  vector: "VECTOR",
};

function parseIndexKind(raw: string): IndexKind | null {
  const lc = raw.toLowerCase();
  return INDEX_KINDS[lc] ?? null;
}

const CONSTRAINT_TAG_TO_KIND: Record<string, ConstraintKind> = {
  // Tags the engine emits in `type` column.
  node_uniqueness: "UNIQUE",
  relationship_uniqueness: "UNIQUE",
  node_property_existence: "NOT_NULL",
  relationship_property_existence: "NOT_NULL",
  node_key: "NODE_KEY",
  relationship_key: "RELATIONSHIP_KEY",
  node_property_type: "PROPERTY_TYPE",
  relationship_property_type: "PROPERTY_TYPE",
};

function parseConstraintKind(raw: string): ConstraintKind | null {
  const lc = raw.toLowerCase().replace(/\s+/g, "_");
  return CONSTRAINT_TAG_TO_KIND[lc] ?? null;
}

interface RowReader {
  columns: string[];
  values: unknown[];
}

function col(row: RowReader, name: string): unknown {
  const idx = row.columns.indexOf(name);
  return idx >= 0 ? row.values[idx] : undefined;
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/**
 * Fetches the current `SHOW INDEXES` snapshot and joins it with
 * `SHOW CONSTRAINTS` so the caller learns which indexes are owned.
 * Both queries run in parallel — if either fails the whole call
 * rejects so the UI can show a single banner.
 */
export async function fetchSchemaDesignSnapshot(): Promise<{
  indexes: IndexDef[];
  constraints: ConstraintDef[];
}> {
  const [idxOutcome, conOutcome] = await Promise.all([
    run("SHOW INDEXES"),
    run("SHOW CONSTRAINTS"),
  ]);
  if (idxOutcome.state !== "ok") {
    throw new SchemaDesignError(idxOutcome.message);
  }
  if (conOutcome.state !== "ok") {
    throw new SchemaDesignError(conOutcome.message);
  }

  const constraints: ConstraintDef[] = conOutcome.result.rows
    .map((r): ConstraintDef | null => {
      const reader = { columns: conOutcome.result.columns, values: r.values };
      const name = valueToString(col(reader, "name"));
      const type = valueToString(col(reader, "type"));
      const entityRaw = valueToString(col(reader, "entityType"));
      const labelsOrTypes = valueToStringList(col(reader, "labelsOrTypes"));
      const properties = valueToStringList(col(reader, "properties"));
      const propertyType = col(reader, "propertyType");
      const ownedIndex = col(reader, "ownedIndex");
      const kind = parseConstraintKind(type);
      if (!kind || name.length === 0 || labelsOrTypes.length === 0) return null;
      const def: ConstraintDef = {
        name,
        kind,
        entity: entityRaw.toUpperCase() === "NODE" ? "NODE" : "RELATIONSHIP",
        label: labelsOrTypes[0]!,
        properties,
      };
      if (typeof propertyType === "string" && propertyType.length > 0) {
        def.propertyType = propertyType;
      }
      if (typeof ownedIndex === "string" && ownedIndex.length > 0) {
        def.ownedIndex = ownedIndex;
      }
      return def;
    })
    .filter((c): c is ConstraintDef => c !== null);

  // Build a reverse map for fast owner lookup on the indexes pass.
  const ownerByIndex = new Map<string, string>();
  for (const c of constraints) {
    if (c.ownedIndex) ownerByIndex.set(c.ownedIndex, c.name);
  }

  const indexes: IndexDef[] = idxOutcome.result.rows
    .map((r): IndexDef | null => {
      const reader = { columns: idxOutcome.result.columns, values: r.values };
      const name = valueToString(col(reader, "name"));
      const typeRaw = valueToString(col(reader, "type"));
      const kind = parseIndexKind(typeRaw);
      if (!kind || name.length === 0) return null;
      const entityRaw = valueToString(col(reader, "entityType")).toUpperCase();
      const labelsOrTypes = valueToStringList(col(reader, "labelsOrTypes"));
      const properties = valueToStringList(col(reader, "properties"));
      const stateRaw = valueToString(col(reader, "state")).toLowerCase();
      const population = valueToNumber(col(reader, "populationPercent"), 0);
      const owner = ownerByIndex.get(name);
      const def: IndexDef = {
        name,
        kind,
        entity: entityRaw === "RELATIONSHIP" ? "RELATIONSHIP" : "NODE",
        labelsOrTypes,
        properties,
        state: stateRaw === "populating" ? "populating" : "online",
        populationPercent: population,
        owned: owner !== undefined,
      };
      if (owner !== undefined) def.ownerConstraint = owner;
      const rawOptions = col(reader, "options");
      if (rawOptions && typeof rawOptions === "object" && !Array.isArray(rawOptions)) {
        def.options = rawOptions as Record<string, unknown>;
      }
      return def;
    })
    .filter((i): i is IndexDef => i !== null);

  // Stable sort by entity, label, name so the UI ordering is deterministic.
  indexes.sort(
    (a, b) =>
      a.entity.localeCompare(b.entity) ||
      (a.labelsOrTypes[0] ?? "").localeCompare(b.labelsOrTypes[0] ?? "") ||
      a.name.localeCompare(b.name),
  );
  constraints.sort(
    (a, b) =>
      a.entity.localeCompare(b.entity) ||
      a.label.localeCompare(b.label) ||
      a.name.localeCompare(b.name),
  );

  return { indexes, constraints };
}

/** Run an arbitrary DDL statement and throw on engine error. */
export async function runDDL(ddl: string): Promise<void> {
  const outcome = await run(ddl);
  if (outcome.state !== "ok") {
    throw new SchemaDesignError(outcome.message);
  }
}

export { SchemaDesignError };
