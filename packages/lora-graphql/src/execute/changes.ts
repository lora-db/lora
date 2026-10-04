// S5: what a write changed, and whether a read could have seen it.

import type { MutationOperation } from "../model/types.js";

export interface EntityRef {
  /** Node type name. */
  type: string;
  /** Its @key value. */
  key: unknown;
}

export interface RelationshipRef {
  /** Relationship type, e.g. `FOLLOWS`. */
  type: string;
  /** `Owner.field` that declares it. */
  field: string;
  from: EntityRef;
  to: EntityRef;
}

export interface WriteChange {
  /** `EXTERNAL`: from the engine's change feed (`changeFeed: true`). */
  operation: MutationOperation | "UPSERT" | "CYPHER" | "EXTERNAL";
  /** When the change was committed (ISO-8601), set when it is emitted. */
  timestamp?: string;
  /**
   * Stored properties before the write, for updated and deleted nodes of
   * types with `@subscription(previousState: true)`.
   */
  before?: Array<{
    type: string;
    key: unknown;
    properties: Record<string, unknown>;
  }>;
  /** The Mutation field that made the change. */
  field: string;
  created: EntityRef[];
  updated: EntityRef[];
  deleted: EntityRef[];
  connected: RelationshipRef[];
  disconnected: RelationshipRef[];
  /**
   * Every node whose observable state changed: created, updated and
   * deleted nodes, and both ends of every relationship written. A
   * response cache should invalidate by `types` too: lists, counts and
   * connections change without naming an entity (see the Yoga example).
   */
  entities: EntityRef[];
  /** Node types in `entities`: lists of these types may have changed. */
  types: string[];
  relationshipTypes: string[];
  /**
   * The write-set is unknown (a `@cypher` mutation): treat everything as
   * changed.
   */
  broad: boolean;
}

export interface ReadSetLike {
  labels: readonly string[];
  relationships: readonly string[];
  /** Reads nobody can name (a `@cypher` statement): every change affects it. */
  opaque?: boolean | undefined;
}

/**
 * Whether a read with this read-set may observe the change. Label-level:
 * a read of `:Festival` nodes is affected by any Festival write.
 */
export function affects(
  reads: ReadSetLike,
  change: WriteChange,
  labelsOf: (type: string) => readonly string[],
): boolean {
  if (change.broad || reads.opaque) return true;
  const labels = new Set(change.types.flatMap(labelsOf));
  return (
    reads.labels.some((l) => labels.has(l)) ||
    reads.relationships.some((r) => change.relationshipTypes.includes(r))
  );
}
