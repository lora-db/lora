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
  operation: MutationOperation | "CYPHER";
  /** The Mutation field that made the change. */
  field: string;
  created: EntityRef[];
  updated: EntityRef[];
  deleted: EntityRef[];
  connected: RelationshipRef[];
  disconnected: RelationshipRef[];
  /**
   * Every node whose observable state changed: created, updated and
   * deleted nodes, and both ends of every relationship written. Ready for
   * a response cache's entity invalidation, e.g. GraphQL Yoga's
   * `cache.invalidate(change.entities.map(e => ({ typename: e.type, id: e.key })))`.
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
  if (change.broad) return true;
  const labels = new Set(change.types.flatMap(labelsOf));
  return (
    reads.labels.some((l) => labels.has(l)) ||
    reads.relationships.some((r) => change.relationshipTypes.includes(r))
  );
}
