/**
 * Pure DDL builders. Turn an {@link IndexDraft} / {@link ConstraintDraft}
 * into the exact Cypher string the playground will send to the WASM
 * engine. Keeping this pure (no React, no store) makes the generated
 * DDL trivially snapshot-testable.
 *
 * Identifier escaping uses backticks. Identifiers containing a literal
 * backtick are rejected upstream by {@link suggestIndexName} /
 * {@link suggestConstraintName} — both produce only `[A-Za-z0-9_]` —
 * but a label/rel-type returned by introspection might contain one, so
 * we strip backticks defensively before quoting.
 */

import type {
  ConstraintDef,
  ConstraintDraft,
  ConstraintKind,
  EntityKind,
  IndexDef,
  IndexDraft,
  IndexKind,
  ScalarPropertyType,
  VectorIndexOptions,
  VectorIndexProvider,
  VectorQuantization,
  VectorSimilarity,
} from "./types";
import { DEFAULT_VECTOR_OPTIONS } from "./types";

/** Wrap an identifier in backticks, stripping any embedded backticks. */
function quote(id: string): string {
  return "`" + id.replace(/`/g, "") + "`";
}

function patternFor(entity: EntityKind, label: string): string {
  return entity === "NODE"
    ? `(n:${quote(label)})`
    : `()-[r:${quote(label)}]-()`;
}

function variableFor(entity: EntityKind): string {
  return entity === "NODE" ? "n" : "r";
}

/** Comma-joined list of `n.<prop>` qualified property references. */
function qualifiedProps(
  entity: EntityKind,
  properties: readonly string[],
): string {
  const v = variableFor(entity);
  return properties.map((p) => `${v}.${quote(p)}`).join(", ");
}

/** Optional `IF NOT EXISTS` clause. */
function ifNotExistsClause(flag: boolean): string {
  return flag ? " IF NOT EXISTS" : "";
}

const INDEX_KEYWORD: Record<IndexKind, string> = {
  RANGE: "RANGE INDEX",
  TEXT: "TEXT INDEX",
  POINT: "POINT INDEX",
  LOOKUP: "LOOKUP INDEX",
  FULLTEXT: "FULLTEXT INDEX",
  VECTOR: "VECTOR INDEX",
};

/**
 * Build the `CREATE … INDEX` statement for an {@link IndexDraft}.
 *
 * LOOKUP indexes use `ON EACH labels(n)` / `ON EACH type(r)` and never
 * carry a property list. FULLTEXT indexes use a bracketed
 * `ON EACH [n.p1, n.p2]` form and can cover multiple properties. All
 * other kinds emit a parenthesised property list of qualified
 * references.
 */
export function buildCreateIndexDDL(draft: IndexDraft): string {
  const keyword = INDEX_KEYWORD[draft.kind];
  const name = draft.name ? ` ${quote(draft.name)}` : "";
  const ine = ifNotExistsClause(draft.ifNotExists);

  if (draft.kind === "LOOKUP") {
    if (draft.entity === "NODE") {
      return `CREATE ${keyword}${name}${ine} FOR (n) ON EACH labels(n)`;
    }
    return `CREATE ${keyword}${name}${ine} FOR ()-[r]-() ON EACH type(r)`;
  }

  const pattern = patternFor(draft.entity, draft.label);
  const props = qualifiedProps(draft.entity, draft.properties);

  if (draft.kind === "FULLTEXT") {
    return `CREATE ${keyword}${name}${ine} FOR ${pattern} ON EACH [${props}]`;
  }

  if (draft.kind === "VECTOR") {
    const options = buildVectorOptionsClause(
      draft.vectorOptions ?? DEFAULT_VECTOR_OPTIONS,
    );
    return `CREATE ${keyword}${name}${ine} FOR ${pattern} ON (${props}) ${options}`;
  }

  return `CREATE ${keyword}${name}${ine} FOR ${pattern} ON (${props})`;
}

/**
 * Emit the `OPTIONS { indexConfig: { … } }` clause for a vector
 * index. Only writes HNSW knobs when the provider is HNSW so the
 * `flat` DDL stays minimal. `quantization` and `populate.async`
 * are emitted only when they differ from the engine defaults so
 * generated DDL stays small for the common case.
 */
function buildVectorOptionsClause(options: VectorIndexOptions): string {
  const entries: string[] = [];
  entries.push(`\`vector.dimensions\`: ${Math.trunc(options.dimensions)}`);
  entries.push(
    `\`vector.similarity_function\`: '${quoteSimilarity(options.similarity)}'`,
  );
  entries.push(
    `\`vector.indexProvider\`: '${quoteProvider(options.provider)}'`,
  );
  if (options.provider === "hnsw") {
    entries.push(`\`vector.hnsw.m\`: ${Math.trunc(options.hnswM)}`);
    entries.push(
      `\`vector.hnsw.ef_construction\`: ${Math.trunc(options.hnswEfConstruction)}`,
    );
    entries.push(
      `\`vector.hnsw.ef_search\`: ${Math.trunc(options.hnswEfSearch)}`,
    );
    if (options.quantization !== "none") {
      entries.push(
        `\`vector.hnsw.quantization\`: '${quoteQuantization(options.quantization)}'`,
      );
    }
  }
  if (options.populateAsync) {
    entries.push("`vector.populate.async`: true");
  }
  return `OPTIONS {indexConfig: {${entries.join(", ")}}}`;
}

function quoteSimilarity(s: VectorSimilarity): string {
  return s;
}

function quoteProvider(p: VectorIndexProvider): string {
  return p;
}

function quoteQuantization(q: VectorQuantization): string {
  return q;
}

/** Build a `DROP INDEX <name> [IF EXISTS]` statement. */
export function buildDropIndexDDL(name: string, ifExists = true): string {
  return `DROP INDEX ${quote(name)}${ifExists ? " IF EXISTS" : ""}`;
}

// ---------------------------------------------------------------------------
// Constraints
// ---------------------------------------------------------------------------

const CONSTRAINT_REQUIREMENT: Record<ConstraintKind, string> = {
  UNIQUE: "IS UNIQUE",
  NODE_KEY: "IS NODE KEY",
  RELATIONSHIP_KEY: "IS RELATIONSHIP KEY",
  NOT_NULL: "IS NOT NULL",
  PROPERTY_TYPE: "", // built dynamically — `IS :: <type>`
};

/**
 * Build a `CREATE CONSTRAINT` statement for the given draft.
 *
 * Single-property constraints (NOT_NULL, PROPERTY_TYPE) emit the
 * property without parentheses to match the engine's preferred form.
 */
export function buildCreateConstraintDDL(draft: ConstraintDraft): string {
  const name = draft.name ? ` ${quote(draft.name)}` : "";
  const ine = ifNotExistsClause(draft.ifNotExists);
  const pattern = patternFor(draft.entity, draft.label);

  const v = variableFor(draft.entity);
  const propList = draft.properties.map((p) => `${v}.${quote(p)}`);
  const props =
    draft.kind === "NOT_NULL" || draft.kind === "PROPERTY_TYPE"
      ? propList[0]
      : `(${propList.join(", ")})`;

  const requirement =
    draft.kind === "PROPERTY_TYPE"
      ? `IS :: ${draft.propertyType}`
      : CONSTRAINT_REQUIREMENT[draft.kind];

  return `CREATE CONSTRAINT${name}${ine} FOR ${pattern} REQUIRE ${props} ${requirement}`;
}

/** Build `DROP CONSTRAINT <name> [IF EXISTS]`. */
export function buildDropConstraintDDL(name: string, ifExists = true): string {
  return `DROP CONSTRAINT ${quote(name)}${ifExists ? " IF EXISTS" : ""}`;
}

// ---------------------------------------------------------------------------
// Name suggestions
// ---------------------------------------------------------------------------

/**
 * Slug a free-form identifier (label, property) into something safe to
 * concatenate into a generated name. Strips non-word characters and
 * lowercases.
 */
function slug(id: string): string {
  return id
    .replace(/[^A-Za-z0-9_]+/g, "_")
    .replace(/^_+|_+$/g, "")
    .toLowerCase();
}

const INDEX_PREFIX: Record<IndexKind, string> = {
  RANGE: "idx",
  TEXT: "txt",
  POINT: "pt",
  LOOKUP: "lookup",
  FULLTEXT: "ft",
  VECTOR: "vec",
};

/**
 * Suggest a default index name like `idx_person_email` based on the
 * draft's kind, label, and properties. LOOKUP indexes don't carry a
 * label or properties so the entity-kind suffix is used instead.
 */
export function suggestIndexName(
  draft: Pick<IndexDraft, "kind" | "entity" | "label" | "properties">,
): string {
  const prefix = INDEX_PREFIX[draft.kind];
  if (draft.kind === "LOOKUP") {
    return `${prefix}_${draft.entity === "NODE" ? "labels" : "types"}`;
  }
  const labelPart = slug(draft.label || "any");
  const propsPart = draft.properties
    .map(slug)
    .filter((s) => s.length > 0)
    .join("_");
  return propsPart
    ? `${prefix}_${labelPart}_${propsPart}`
    : `${prefix}_${labelPart}`;
}

const CONSTRAINT_PREFIX: Record<ConstraintKind, string> = {
  UNIQUE: "unique",
  NODE_KEY: "nodekey",
  RELATIONSHIP_KEY: "relkey",
  NOT_NULL: "notnull",
  PROPERTY_TYPE: "ptype",
};

/** Suggest a default constraint name. */
export function suggestConstraintName(
  draft: Pick<ConstraintDraft, "kind" | "label" | "properties">,
): string {
  const prefix = CONSTRAINT_PREFIX[draft.kind];
  const labelPart = slug(draft.label || "any");
  const propsPart = draft.properties
    .map(slug)
    .filter((s) => s.length > 0)
    .join("_");
  return propsPart
    ? `${prefix}_${labelPart}_${propsPart}`
    : `${prefix}_${labelPart}`;
}

// ---------------------------------------------------------------------------
// Misc shared lookups
// ---------------------------------------------------------------------------

export const SCALAR_PROPERTY_TYPES: ReadonlyArray<ScalarPropertyType> = [
  "BOOLEAN",
  "STRING",
  "INTEGER",
  "FLOAT",
  "DATE",
  "LOCAL_TIME",
  "ZONED_TIME",
  "LOCAL_DATETIME",
  "ZONED_DATETIME",
  "DURATION",
  "POINT",
];

const SCALAR_PROPERTY_TYPE_SET: ReadonlySet<ScalarPropertyType> = new Set(
  SCALAR_PROPERTY_TYPES,
);

// ---------------------------------------------------------------------------
// Def → Draft converters
// ---------------------------------------------------------------------------

/**
 * Lift an introspected {@link IndexDef} into a wizard-shaped
 * {@link IndexDraft}. Used to seed the edit wizard and to rebuild DDL
 * for "Copy" / "Open in editor" affordances on an existing row.
 *
 * `ifNotExists` is false on the way out — when a user is reproducing
 * the DDL of an existing object they typically want the literal
 * statement, not the idempotent variant.
 */
export function indexDefToDraft(def: IndexDef): IndexDraft {
  const draft: IndexDraft = {
    kind: def.kind,
    entity: def.entity,
    label: def.labelsOrTypes[0] ?? "",
    properties: [...def.properties],
    name: def.name,
    ifNotExists: false,
  };
  if (def.kind === "VECTOR") {
    draft.vectorOptions = vectorOptionsFromMap(def.options);
  }
  return draft;
}

/**
 * Project the `OPTIONS` map surfaced by `SHOW INDEXES` back into the
 * wizard's typed shape, filling missing keys with the engine
 * defaults. Defensive: anything weird (wrong type, out-of-range)
 * falls through to the default so the edit wizard always mounts.
 */
export function vectorOptionsFromMap(
  raw: Record<string, unknown> | undefined,
): VectorIndexOptions {
  const out: VectorIndexOptions = { ...DEFAULT_VECTOR_OPTIONS };
  if (!raw) return out;
  const dim = raw["vector.dimensions"];
  if (typeof dim === "number" && dim >= 1 && dim <= 4096) {
    out.dimensions = Math.trunc(dim);
  }
  const sim = raw["vector.similarity_function"];
  if (
    sim === "cosine" ||
    sim === "euclidean" ||
    sim === "dot" ||
    sim === "manhattan"
  ) {
    out.similarity = sim;
  } else if (sim === "dot_product") {
    out.similarity = "dot";
  }
  const provider = raw["vector.indexProvider"];
  if (provider === "flat" || provider === "hnsw") {
    out.provider = provider;
  }
  const m = raw["vector.hnsw.m"];
  if (typeof m === "number" && m >= 4 && m <= 128) {
    out.hnswM = Math.trunc(m);
  }
  const efc = raw["vector.hnsw.ef_construction"];
  if (typeof efc === "number" && efc >= 16 && efc <= 2000) {
    out.hnswEfConstruction = Math.trunc(efc);
  }
  const efs = raw["vector.hnsw.ef_search"];
  if (typeof efs === "number" && efs >= 16 && efs <= 2000) {
    out.hnswEfSearch = Math.trunc(efs);
  }
  const quant = raw["vector.hnsw.quantization"];
  if (quant === "none" || quant === "int8") {
    out.quantization = quant;
  }
  const async = raw["vector.populate.async"];
  if (typeof async === "boolean") {
    out.populateAsync = async;
  }
  return out;
}

/**
 * Lift an introspected {@link ConstraintDef} into a
 * {@link ConstraintDraft}. Non-scalar `propertyType` values (LIST OF …)
 * fall back to STRING so the wizard still mounts; the user can pick
 * the right type before submitting.
 */
export function constraintDefToDraft(def: ConstraintDef): ConstraintDraft {
  const propertyType =
    def.propertyType &&
    SCALAR_PROPERTY_TYPE_SET.has(def.propertyType as ScalarPropertyType)
      ? (def.propertyType as ScalarPropertyType)
      : "STRING";
  return {
    kind: def.kind,
    entity: def.entity,
    label: def.label,
    properties: [...def.properties],
    propertyType,
    name: def.name,
    ifNotExists: false,
  };
}
