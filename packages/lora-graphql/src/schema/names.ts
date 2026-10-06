// Generated type and field names, in one place so the schema builder
// and the compiler agree.

import { lowerFirst } from "../model/build/shapes.js";
import type { NodeType, RelationshipField } from "../model/types.js";

export const upperFirst = (s: string) => s.charAt(0).toUpperCase() + s.slice(1);

export const names = {
  where: (type: string) => `${type}Where`,
  fieldFilter: (type: string, field: string) =>
    `${type}${upperFirst(field)}Filter`,
  sort: (type: string) => `${type}Sort`,
  connection: (type: string) => `${type}Connection`,
  edge: (type: string) => `${type}Edge`,
  aggregate: (type: string) => `${type}Aggregate`,
  match: (type: string) => `${type}Match`,
  searchConnection: (type: string) => `${type}SearchConnection`,
  searchEdge: (type: string) => `${type}SearchEdge`,
  relationFilter: (type: string, field: string) =>
    `${type}${upperFirst(field)}Filter`,
  relConnection: (type: string, field: string) =>
    `${type}${upperFirst(field)}Connection`,
  relEdge: (type: string, field: string) => `${type}${upperFirst(field)}Edge`,
  relConnectionWhere: (type: string, field: string) =>
    `${type}${upperFirst(field)}ConnectionWhere`,
  connectionField: (field: string) => `${field}Connection`,

  listRoot: (t: NodeType) => t.plural,
  connectionRoot: (t: NodeType) => `${t.plural}Connection`,
  aggregateRoot: (t: NodeType) => `${t.plural}Aggregate`,
  groupedRoot: (t: NodeType) => `${t.plural}Grouped`,
  singleRoot: (t: NodeType) => lowerFirst(t.name),
};

/**
 * Whether a list relationship gets its own connection/edge types (and a
 * `{ node, edge }` where): it has relationship properties.
 */
export function hasOwnConnection(rel: RelationshipField): boolean {
  return rel.list && rel.properties !== undefined;
}

/**
 * The connection and edge type names of a list relationship's connection:
 * its own when it has properties or aggregates (whose count covers
 * relationships too) or opts out of them, the target's otherwise.
 */
export function connectionTypeNames(
  rel: RelationshipField,
  target: NodeType,
): {
  connection: string;
  edge: string;
} {
  return hasOwnConnection(rel) || !rel.aggregate || target.aggregate
    ? {
        connection: names.relConnection(rel.owner, rel.name),
        edge: names.relEdge(rel.owner, rel.name),
      }
    : {
        connection: names.connection(rel.target),
        edge: names.edge(rel.target),
      };
}
