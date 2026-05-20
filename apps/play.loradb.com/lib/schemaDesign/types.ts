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
}

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
