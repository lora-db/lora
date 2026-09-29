// Generated type and field names, in one place so the schema builder
// and the compiler agree.

import { lowerFirst } from "../model/build.js";
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
  singleRoot: (t: NodeType) => lowerFirst(t.name),
};

/** Whether a list relationship gets its own connection/edge types. */
export function hasOwnConnection(rel: RelationshipField): boolean {
  return rel.list && rel.properties !== undefined;
}
