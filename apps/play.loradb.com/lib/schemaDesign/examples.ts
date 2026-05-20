/**
 * Builds illustrative Cypher snippets that demonstrate which kinds of
 * queries are accelerated by an index, or which writes a constraint
 * will start rejecting. Each builder is pure: drafts + an optional
 * sample-name hint go in, an ordered list of {@link UsageExample}s
 * comes out.
 *
 * The output is shown on the wizard's confirmation step so the user
 * can see — in plain Cypher — what their schema change is *for*.
 */

import type { ConstraintDraft, EntityKind, IndexDraft } from "./types";

export interface UsageExample {
  /** Short label rendered above the snippet (e.g. "Equality lookup"). */
  caption: string;
  /** Body of the snippet — plain Cypher, formatted on render. */
  cypher: string;
}

function quote(id: string): string {
  return "`" + id.replace(/`/g, "") + "`";
}

function patternFor(entity: EntityKind, label: string): string {
  return entity === "NODE"
    ? `(n:${quote(label)})`
    : `()-[r:${quote(label)}]-()`;
}

function varFor(entity: EntityKind): string {
  return entity === "NODE" ? "n" : "r";
}

/**
 * Build usage examples for an index draft. Returns `[]` when the draft
 * lacks the inputs needed for a meaningful snippet (no label /
 * properties on non-LOOKUP kinds).
 */
export function buildIndexUsageExamples(
  draft: IndexDraft,
  hint: { sampleLabel?: string; sampleRelType?: string } = {},
): UsageExample[] {
  if (draft.kind === "LOOKUP") {
    if (draft.entity === "NODE") {
      const label = hint.sampleLabel ?? "Label";
      return [
        {
          caption: "Label scan",
          cypher: `MATCH (n:${quote(label)}) RETURN n LIMIT 25`,
        },
      ];
    }
    const type = hint.sampleRelType ?? "TYPE";
    return [
      {
        caption: "Relationship-type scan",
        cypher: `MATCH ()-[r:${quote(type)}]-() RETURN r LIMIT 25`,
      },
    ];
  }

  if (!draft.label || draft.properties.length === 0) return [];

  const pattern = patternFor(draft.entity, draft.label);
  const v = varFor(draft.entity);
  const first = draft.properties[0]!;
  const firstRef = `${v}.${quote(first)}`;

  switch (draft.kind) {
    case "RANGE":
      return [
        {
          caption: "Equality lookup",
          cypher: `MATCH ${pattern} WHERE ${firstRef} = $value RETURN ${v}`,
        },
        {
          caption: "Range filter",
          cypher: `MATCH ${pattern} WHERE ${firstRef} >= $low AND ${firstRef} < $high RETURN ${v}`,
        },
        {
          caption: "Sorted lookup",
          cypher: `MATCH ${pattern} RETURN ${v} ORDER BY ${firstRef} LIMIT 25`,
        },
      ];
    case "TEXT":
      return [
        {
          caption: "Prefix match",
          cypher: `MATCH ${pattern} WHERE ${firstRef} STARTS WITH $prefix RETURN ${v}`,
        },
        {
          caption: "Substring match",
          cypher: `MATCH ${pattern} WHERE ${firstRef} CONTAINS $needle RETURN ${v}`,
        },
      ];
    case "POINT":
      return [
        {
          caption: "Distance filter",
          cypher: `MATCH ${pattern} WHERE point.distance(${firstRef}, $origin) < $radius RETURN ${v}`,
        },
      ];
    case "FULLTEXT": {
      const indexName = draft.name || "<index-name>";
      const fn =
        draft.entity === "NODE"
          ? "db.index.fulltext.queryNodes"
          : "db.index.fulltext.queryRelationships";
      const yieldVar = draft.entity === "NODE" ? "node" : "relationship";
      return [
        {
          caption: "Full-text search",
          cypher: `CALL ${fn}('${indexName}', $searchTerms) YIELD ${yieldVar}, score RETURN ${yieldVar}, score`,
        },
      ];
    }
    case "VECTOR":
      return [];
  }
}

/**
 * Build usage examples for a constraint draft. The output shows the
 * kind of write that will start *failing* once the constraint exists
 * (paired with a brief caption explaining the rejection).
 */
export function buildConstraintUsageExamples(
  draft: ConstraintDraft,
): UsageExample[] {
  if (!draft.label || draft.properties.length === 0) return [];

  const pattern = patternFor(draft.entity, draft.label);
  const v = varFor(draft.entity);
  const first = draft.properties[0]!;
  const firstRef = `${v}.${quote(first)}`;

  switch (draft.kind) {
    case "UNIQUE":
    case "NODE_KEY": {
      if (draft.entity !== "NODE") return [];
      const propEntries = draft.properties
        .map((p) => `${quote(p)}: $${p}`)
        .join(", ");
      return [
        {
          caption: "Safe MERGE on the key",
          cypher: `MERGE (n:${quote(draft.label)} {${propEntries}}) RETURN n`,
        },
        {
          caption: "Duplicate write that will be rejected",
          cypher: `CREATE (n:${quote(draft.label)} {${propEntries}})`,
        },
      ];
    }
    case "RELATIONSHIP_KEY": {
      const propEntries = draft.properties
        .map((p) => `${quote(p)}: $${p}`)
        .join(", ");
      return [
        {
          caption: "Safe MERGE on the key",
          cypher: `MATCH (a), (b)\nMERGE (a)-[r:${quote(draft.label)} {${propEntries}}]->(b) RETURN r`,
        },
      ];
    }
    case "NOT_NULL": {
      const createClause =
        draft.entity === "NODE"
          ? `CREATE (n:${quote(draft.label)})`
          : `MATCH (a), (b) CREATE (a)-[r:${quote(draft.label)}]->(b)`;
      return [
        {
          caption: "Valid write",
          cypher: `${createClause} SET ${firstRef} = $${first} RETURN ${v}`,
        },
        {
          caption: `Missing ${first} will be rejected`,
          cypher: `${createClause} RETURN ${v}`,
        },
      ];
    }
    case "PROPERTY_TYPE": {
      const createClause =
        draft.entity === "NODE"
          ? `CREATE (n:${quote(draft.label)})`
          : `MATCH (a), (b) CREATE (a)-[r:${quote(draft.label)}]->(b)`;
      const exampleValue = sampleValueFor(draft.propertyType);
      return [
        {
          caption: `${first} must be ${draft.propertyType}`,
          cypher: `${createClause} SET ${firstRef} = ${exampleValue} RETURN ${v}`,
        },
      ];
    }
  }
}

function sampleValueFor(type: ConstraintDraft["propertyType"]): string {
  switch (type) {
    case "STRING":
      return `'sample'`;
    case "INTEGER":
      return `42`;
    case "FLOAT":
      return `3.14`;
    case "BOOLEAN":
      return `true`;
    case "DATE":
      return `date('2026-01-01')`;
    case "LOCAL_TIME":
      return `localtime('12:00:00')`;
    case "ZONED_TIME":
      return `time('12:00:00+02:00')`;
    case "LOCAL_DATETIME":
      return `localdatetime('2026-01-01T12:00:00')`;
    case "ZONED_DATETIME":
      return `datetime('2026-01-01T12:00:00+02:00')`;
    case "DURATION":
      return `duration('P1D')`;
    case "POINT":
      return `point({x: 1.0, y: 2.0})`;
  }
}
