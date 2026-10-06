// Pieces the mutation statements are built from: Cypher text for
// timestamps, relationship patterns and label tests, and small helpers
// over the model and the keys.

import { authFilter } from "../../compile/auth.js";
import type { CompileContext } from "../../compile/context.js";
import {
  bin,
  name,
  printExpr,
  prop,
  v,
  type Expr,
} from "../../compile/cypher.js";
import { keyOf } from "../../compile/read.js";
import type {
  Field,
  GraphModel,
  NodeType,
  RelationshipField,
  ScalarField,
} from "../../model/types.js";
import type { RelationshipRef } from "../changes.js";

const NOW: Partial<Record<string, string>> = {
  DateTime: "datetime()",
  LocalDateTime: "localdatetime()",
  Date: "date()",
};

/** `@timestamp` fields for `op`: [property, now-expression]. */
export function timestamps(
  node: { fields: ReadonlyMap<string, Field> },
  op: "CREATE" | "UPDATE",
): Array<[string, string]> {
  return [...node.fields.values()]
    .filter(
      (f): f is ScalarField => f.kind === "scalar" && !!f.timestamp?.has(op),
    )
    .map((f) => [f.property, NOW[f.type]!]);
}

/** `, r.p = <now>` for a relationship's `@timestamp(operations: [UPDATE])`s. */
export function edgeStamps(model: GraphModel, rel: RelationshipField): string {
  const props = rel.properties
    ? model.relationshipProperties.get(rel.properties)
    : undefined;
  return (props ? timestamps(props, "UPDATE") : [])
    .map(([p, now]) => `, r.${name(p)} = ${now}`)
    .join("");
}

/** `-[r:T]->(b:Target)` in the direction the owner's field declares. */
export function arrow(
  rel: RelationshipField,
  r: string,
  b: string,
  target: NodeType | undefined,
): string {
  const inner = `[${r ? name(r) : ""}:${name(rel.type)}]`;
  const node = target
    ? `(${name(b)}:${name(target.labels[0]!)})`
    : `(${name(b)})`;
  return rel.direction === "OUT" ? `-${inner}->${node}` : `<-${inner}-${node}`;
}

/**
 * `b:Label`: LoraDB 0.15 ignores the labels of every node after the first
 * in a MATCH pattern, so expansions test them explicitly.
 */
export function labelTest(variable: string, node: NodeType): string {
  return `${name(variable)}:${name(node.labels[0]!)}`;
}

/**
 * `MATCH (a:Owner) WHERE a.key = <key>` then `MATCH (a)-[r:T]->(b:Target)
 * WHERE b:Target`, for callers to extend with ` AND ...`. The key test gets
 * its own MATCH so it becomes an index seek: LoraDB 0.15 only seeks when
 * the test sits directly on a node scan, and one pattern with the test in
 * its WHERE expands every relationship of the type first.
 */
export function seekThenExpand(
  a: string,
  owner: NodeType,
  key: string,
  rel: RelationshipField,
  r: string,
  b: string,
  target: NodeType,
): string {
  return (
    `MATCH (${name(a)}:${name(owner.labels[0]!)}) WHERE ${name(a)}.${name(owner.key.property)} = ${key}\n` +
    `MATCH (${name(a)})${arrow(rel, r, b, target)}\n` +
    `WHERE ${labelTest(b, target)}`
  );
}

/** `row.fresh OR (<expr>)`: a filter that nodes this mutation creates skip. */
export const orFresh = (e: Expr | undefined): Expr | undefined =>
  e && bin("OR", prop(v("row"), "fresh"), e);

/** `AND (<expr>)`, or nothing. */
export const andText = (e: Expr | undefined) =>
  e ? ` AND (${printExpr(e)})` : "";

/** Fields on the other side of `rel`'s relationship type, pointing back. */
export function inverseFields(
  model: GraphModel,
  rel: RelationshipField,
): RelationshipField[] {
  const target = model.nodes.get(rel.target)!;
  return [...target.fields.values()].filter(
    (f): f is RelationshipField =>
      f.kind === "relationship" &&
      f.type === rel.type &&
      f.members.includes(rel.owner) &&
      f.direction !== rel.direction,
  );
}

/** Whether the caller can read `variable` (a `node`): a boolean expression. */
export function readable(
  ctx: CompileContext,
  node: NodeType,
  variable: string,
): string {
  const filter = authFilter(ctx, node, variable, "READ");
  return filter ? `coalesce(${printExpr(filter)}, false)` : "true";
}

/** `x:A OR x:B`: the field's target types (one, or an abstract's members). */
export function memberTest(
  model: GraphModel,
  variable: string,
  field: RelationshipField,
): string {
  return field.members
    .map((m) => labelTest(variable, model.nodes.get(m)!))
    .join(" OR ");
}

export function dedupe(keys: unknown[]): unknown[] {
  return [...new Map(keys.map((k) => [keyOf(k), k])).values()];
}

export function relRef(
  rel: RelationshipField,
  from: unknown,
  to: unknown,
): RelationshipRef {
  return {
    type: rel.type,
    field: `${rel.owner}.${rel.name}`,
    from: { type: rel.owner, key: from },
    to: { type: rel.target, key: to },
  };
}
