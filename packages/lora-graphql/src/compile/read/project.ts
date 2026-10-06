// Projecting a node: its stored properties, guarded and masked fields,
// and the nested relationship lists read with it.

import {
  parse,
  type GraphQLObjectType,
  type OperationDefinitionNode,
  type SelectionSetNode,
} from "graphql";
import { filterDegree, perRowCost } from "../cost.js";
import type { NodeType, RelationshipField } from "../../model/types.js";
import {
  authFilter,
  authValidate,
  checkAuthentication,
  fieldReadGuard,
  maskedValue,
} from "../auth.js";
import { bind, freshVar, type CompileContext } from "../context.js";
import {
  and,
  fn,
  lit,
  prop,
  v,
  type Clause,
  type Expr,
  type ProjectionEntry,
} from "../cypher.js";
import { compileNodeWhere, relationshipPattern } from "../filter.js";
import { collectFields, fieldArgs, subSelections } from "../selection.js";
import type { Args, Where, Projection } from "./types.js";
import { collectOne } from "./common.js";
import { resolveLimit, resolveSort, sortItems } from "./sort.js";
import { projectCypher } from "./cypher-field.js";
import { projectPolymorphic } from "./abstract.js";
import { projectRelationshipConnection } from "./connection.js";

// ---------------------------------------------------------------------------
// Projection
// ---------------------------------------------------------------------------

const requiresCache = new Map<string, SelectionSetNode>();

/** A `requires` string as a selection set, parsed once. */
function requiresSelection(requires: string): SelectionSetNode {
  const cached = requiresCache.get(requires);
  if (cached) return cached;
  const op = parse(`{ ${requires} }`).definitions[0] as OperationDefinitionNode;
  requiresCache.set(requires, op.selectionSet);
  return op.selectionSet;
}

/**
 * Project `variable` (a `node`) with the selection sets. `rows` is how
 * many parents this projection runs for, for the cost estimate.
 */
export function projectNode(
  ctx: CompileContext,
  node: NodeType,
  variable: string,
  sets: SelectionSetNode[],
  rows: number,
): Projection {
  ctx.cost += rows;
  const objType = ctx.schema.getType(node.name) as GraphQLObjectType;
  const defs = objType.getFields();
  const entries: ProjectionEntry[] = [];
  const pre: Clause[] = [];
  // A @customResolver field is computed in JavaScript from its `requires`
  // selection, which is read here with the node's other fields.
  const selected = collectFields(ctx, objType, sets);
  const required: SelectionSetNode[] = [];
  for (const [, nodes] of selected) {
    const f = node.fields.get(nodes[0]!.name.value);
    if (f?.kind === "custom" && f.requires) {
      required.push(requiresSelection(f.requires));
    }
  }
  const all =
    required.length > 0
      ? collectFields(ctx, objType, [...sets, ...required])
      : selected;
  for (const [key, nodes] of all) {
    const fieldName = nodes[0]!.name.value;
    if (fieldName === "__typename") continue;
    if (node.fields.get(fieldName)?.kind === "custom") continue;
    if (fieldName === "id" && node.key.relayId) {
      entries.push({
        kind: "entry",
        key,
        value: prop(v(variable), node.key.property),
      });
      continue;
    }
    const def = defs[fieldName];
    const field = node.fields.get(fieldName);
    // Field-level READ rules and @authentication: a row failing them reads
    // the field as FORBIDDEN (UNAUTHENTICATED without a token), checked
    // per row so the statement is the same with or without a token.
    const guarded = field ?? connectionOf(node, fieldName);
    const guard = guarded
      ? fieldReadGuard(ctx, node, guarded, variable)
      : undefined;
    if (field?.kind === "scalar") {
      const masked = maskedValue(
        ctx,
        node,
        field,
        variable,
        prop(v(variable), field.property),
      );
      if (guard) {
        entries.push({
          kind: "entry",
          key,
          value: guardedValue(guard, masked),
        });
      } else if (field.authorization?.mask?.length) {
        entries.push({ kind: "entry", key, value: masked });
      } else {
        entries.push(
          key === field.property
            ? { kind: "property", key }
            : { kind: "entry", key, value: prop(v(variable), field.property) },
        );
      }
      continue;
    }
    const args = def ? fieldArgs(ctx, def, nodes[0]!) : {};
    const nested = subSelections(nodes);
    let result: Projection;
    if (
      field?.kind === "relationship" &&
      ctx.model.abstracts.has(field.target)
    ) {
      result = projectPolymorphic(
        ctx,
        variable,
        field,
        key,
        args,
        nested,
        rows,
      );
    } else if (field?.kind === "relationship") {
      result = projectRelationship(
        ctx,
        variable,
        field,
        key,
        args,
        nested,
        rows,
      );
    } else if (field?.kind === "cypher") {
      result = projectCypher(ctx, variable, field, key, args, nested, rows);
    } else {
      const connected = connectionOf(node, fieldName);
      if (!connected) continue;
      result = projectRelationshipConnection(
        ctx,
        variable,
        connected,
        key,
        args,
        nested,
        rows,
      );
    }
    pre.push(...result.pre);
    entries.push({
      kind: "entry",
      key,
      value: guard ? guardedValue(guard, result.expr) : result.expr,
    });
  }
  // Read validation: the resolvers turn `false` into a FORBIDDEN error.
  const validate = authValidate(ctx, node, variable, "READ", "BEFORE");
  if (validate)
    entries.push({ kind: "entry", key: "__authorized", value: validate });
  return { pre, expr: { kind: "mapProjection", variable, entries } };
}

/** `value` where the guard holds, else the marker the resolvers raise. */
export function guardedValue(
  guard: NonNullable<ReturnType<typeof fieldReadGuard>>,
  value: Expr,
): Expr {
  return {
    kind: "case",
    when: fn("coalesce", guard.when, lit(false)),
    then: value,
    else: {
      kind: "map",
      entries: [
        { key: "__forbidden", value: lit(true) },
        ...(guard.code === "UNAUTHENTICATED"
          ? [{ key: "__unauthenticated", value: lit(true) }]
          : []),
      ],
    },
  };
}

function connectionOf(
  node: NodeType,
  fieldName: string,
): RelationshipField | undefined {
  if (!fieldName.endsWith("Connection")) return undefined;
  const rel = node.fields.get(fieldName.slice(0, -"Connection".length));
  return rel?.kind === "relationship" && rel.list ? rel : undefined;
}

/** Rows a list relationship yields per parent, for the cost estimate. */
export function fanOut(
  ctx: CompileContext,
  rel: RelationshipField,
  limit: number,
): number {
  const degree = ctx.degrees.get(`${rel.owner}.${rel.name}`) ?? rel.cardinality;
  return degree === undefined ? limit : Math.min(limit, Math.max(degree, 1));
}

function projectRelationship(
  ctx: CompileContext,
  parent: string,
  rel: RelationshipField,
  key: string,
  args: Args,
  sets: SelectionSetNode[],
  rows: number,
): Projection {
  const target = ctx.model.nodes.get(rel.target)!;
  checkAuthentication(ctx, target, "READ");
  const label = target.labels[0]!;
  ctx.reads.labels.add(label);
  ctx.reads.relationships.add(rel.type);
  const x = freshVar(ctx, `${parent}_${key}`);
  const pattern = relationshipPattern(parent, rel, label, x);
  const where = and(
    compileNodeWhere(ctx, target, x, args["where"] as Where),
    authFilter(ctx, target, x, "READ"),
  );

  if (!rel.list) {
    const nested = projectNode(ctx, target, x, sets, rows);
    if (nested.pre.length === 0) {
      // head([(this)-[:T]->(x:Target) | x { ... }])
      return {
        pre: [],
        expr: fn("head", {
          kind: "comprehension",
          pattern,
          where,
          projection: nested.expr,
        }),
      };
    }
    const out = freshVar(ctx, `${x}_one`);
    return {
      pre: [
        {
          kind: "call",
          imports: [parent],
          body: [
            { kind: "match", pattern, where },
            { kind: "with", items: [{ expr: v(x) }], limit: lit(1) },
            ...nested.pre,
            ...collectOne(nested.expr, out),
          ],
        },
      ],
      expr: v(out),
    };
  }

  const limit = resolveLimit(args["limit"], rel.limit ?? target.limit, "limit");
  const userSort = args["sort"] as unknown[] | null | undefined;
  const limitParam = bind(ctx, limit);
  // A filter examines every related node, not only the page it returns.
  ctx.cost +=
    rows *
    filterDegree(ctx, rel, true) *
    perRowCost(ctx, target, args["where"] as Where | undefined);
  const nested = projectNode(
    ctx,
    target,
    x,
    sets,
    rows * fanOut(ctx, rel, limit),
  );
  // Without a sort, related nodes come back in @key order: deterministic,
  // and limited before they are projected.
  const sort = resolveSort(ctx, target, userSort, false);
  const out = freshVar(ctx, `${x}_list`);
  return {
    pre: [
      {
        kind: "call",
        imports: [parent],
        body: [
          { kind: "match", pattern, where },
          {
            kind: "with",
            items: [{ expr: v(x) }],
            orderBy: sortItems(x, sort),
            limit: limitParam,
          },
          ...nested.pre,
          {
            kind: "return",
            items: [{ expr: fn("collect", nested.expr), alias: out }],
          },
        ],
      },
    ],
    expr: v(out),
  };
}
