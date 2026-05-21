/**
 * Shared types for the Schema Design surface. Kept in their own file so
 * pure modules (ddl, validate, recommend, errorTranslate) can import
 * them without dragging in React or the Zustand store.
 */

export type IndexKind =
  | "RANGE"
  | "TEXT"
  | "POINT"
  | "LOOKUP"
  | "FULLTEXT"
  | "VECTOR";

export type ConstraintKind =
  | "UNIQUE"
  | "NODE_KEY"
  | "RELATIONSHIP_KEY"
  | "NOT_NULL"
  | "PROPERTY_TYPE";

export type EntityKind = "NODE" | "RELATIONSHIP";

export type IndexState = "online" | "populating";

/**
 * Property type predicate for `IS :: T` constraints. The grammar
 * accepts more (LIST, VECTOR), but the wizard surfaces only scalar
 * types in v1 to keep the UI simple. The constraint catalog rejects
 * MAP / ANY at create time so they're not in this list either.
 */
export type ScalarPropertyType =
  | "BOOLEAN"
  | "STRING"
  | "INTEGER"
  | "FLOAT"
  | "DATE"
  | "LOCAL_TIME"
  | "ZONED_TIME"
  | "LOCAL_DATETIME"
  | "ZONED_DATETIME"
  | "DURATION"
  | "POINT";

export interface IndexDef {
  name: string;
  kind: IndexKind;
  entity: EntityKind;
  /** Labels for NODE / relationship types for RELATIONSHIP. Empty for LOOKUP. */
  labelsOrTypes: string[];
  properties: string[];
  state: IndexState;
  populationPercent: number;
  /** True when this index was implicitly created to back a constraint. */
  owned: boolean;
  /** Constraint that owns this index, if any. */
  ownerConstraint?: string;
  /**
   * Raw `OPTIONS { indexConfig: { … } }` map surfaced by
   * `SHOW INDEXES`. Used by the edit flow to round-trip vector
   * tuning knobs (similarity, indexProvider, hnsw.*) without
   * forcing the user to re-enter them every time.
   */
  options?: Record<string, unknown>;
}

// ---------------------------------------------------------------------------
// Vector index tuning
// ---------------------------------------------------------------------------

/**
 * Similarity functions the engine accepts for VECTOR indexes. Cosine
 * is the safe default for embedding workloads; the others trade
 * different correctness / performance properties — see the wizard's
 * Tune step for the per-metric hints.
 */
export type VectorSimilarity = "cosine" | "euclidean" | "dot" | "manhattan";

/**
 * Index provider: `flat` keeps every vector in a brute-force list
 * (correct, slow at scale); `hnsw` builds a Hierarchical Navigable
 * Small World graph (approximate, sub-linear queries).
 */
export type VectorIndexProvider = "flat" | "hnsw";

/**
 * Quantization mode for HNSW. `none` keeps full f32 storage; `int8`
 * scales to signed bytes for ~4× memory reduction. The engine
 * rejects `int8` with non-cosine metrics because only cosine is
 * scale-invariant under uniform scaling.
 */
export type VectorQuantization = "none" | "int8";

/**
 * Wizard-shaped tuning knobs for a VECTOR index. Mirrors the engine
 * options validated by `validate_vector_options` in
 * `crates/lora-database/src/database/schema.rs`. Defaults are picked
 * so a user who clicks straight through the Tune step gets a
 * production-shaped index.
 */
export interface VectorIndexOptions {
  /** 1..=4096. The width of the embedding. */
  dimensions: number;
  similarity: VectorSimilarity;
  provider: VectorIndexProvider;
  /**
   * HNSW knobs — honored only when `provider === "hnsw"`. The wizard
   * still tracks them in the off state so toggling between providers
   * doesn't lose your tuning.
   */
  hnswM: number;
  hnswEfConstruction: number;
  hnswEfSearch: number;
  quantization: VectorQuantization;
  /**
   * When true the index registers in Populating state and the
   * backfill runs lazily on first query. CREATE returns instantly;
   * the first query pays the populate cost.
   */
  populateAsync: boolean;
}

export const DEFAULT_VECTOR_OPTIONS: VectorIndexOptions = {
  dimensions: 384,
  similarity: "cosine",
  provider: "hnsw",
  hnswM: 16,
  hnswEfConstruction: 200,
  hnswEfSearch: 100,
  quantization: "none",
  populateAsync: false,
};

export interface ConstraintDef {
  name: string;
  kind: ConstraintKind;
  entity: EntityKind;
  /** The single label / rel-type the constraint applies to. */
  label: string;
  properties: string[];
  /** Filled when kind === PROPERTY_TYPE. */
  propertyType?: string;
  /** Name of the auto-created backing RANGE index, when applicable. */
  ownedIndex?: string;
}

// ---------------------------------------------------------------------------
// Wizard drafts
// ---------------------------------------------------------------------------

export interface IndexDraft {
  kind: IndexKind;
  entity: EntityKind;
  /** The picked label (NODE) or rel-type (RELATIONSHIP). */
  label: string;
  properties: string[];
  name: string;
  ifNotExists: boolean;
  /**
   * Vector-specific tuning. Populated when `kind === "VECTOR"`;
   * left undefined for every other kind so the rest of the wizard
   * code doesn't have to special-case its presence.
   */
  vectorOptions?: VectorIndexOptions;
}

export interface ConstraintDraft {
  kind: ConstraintKind;
  entity: EntityKind;
  label: string;
  properties: string[];
  /** Only used when kind === PROPERTY_TYPE. */
  propertyType: ScalarPropertyType;
  name: string;
  ifNotExists: boolean;
}

// ---------------------------------------------------------------------------
// Recommendations
// ---------------------------------------------------------------------------

export type RecommendationKind =
  | "RANGE_INDEX"
  | "TEXT_INDEX"
  | "UNIQUE_CONSTRAINT"
  | "NOT_NULL_CONSTRAINT";

export interface Recommendation {
  id: string;
  kind: RecommendationKind;
  entity: EntityKind;
  label: string;
  property: string;
  /** Number of supporting observations from history. */
  evidenceCount: number;
  /** Short human-readable rationale shown on the card. */
  reason: string;
}
