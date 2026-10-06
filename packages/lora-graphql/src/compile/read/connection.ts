// Nested relationship connections: the page, its counts and aggregates,
// the relationship properties, and what a connection selection asks for.

import {
  getNamedType,
  type GraphQLObjectType,
  type SelectionSetNode,
} from "graphql";
import { filterDegree, perRowCost } from "../cost.js";
import {
  distinctCount,
  listAggregate,
  type ListAggregate,
} from "../aggregate.js";
import type {
  NodeType,
  RelationshipPropertiesType,
  RelationshipField,
  ScalarField,
} from "../../model/types.js";
import { connectionTypeNames, hasOwnConnection } from "../../schema/names.js";
import {
  authFilter,
  authValidate,
  checkAuthentication,
  checkFieldAuthentication,
  checkPropertyAccess,
  propertyReadAccess,
  propertyReadGuard,
  refusedEdgeRead,
  relationshipRules,
} from "../auth.js";
import { bind, freshVar, type CompileContext } from "../context.js";
import {
  and,
  fn,
  lit,
  not,
  prop,
  v,
  type Expr,
  type ProjectionEntry,
} from "../cypher.js";
import {
  compileNodeWhere,
  compilePropsWhere,
  relationshipPattern,
} from "../filter.js";
import { collectFields, subSelections } from "../selection.js";
import type { Args, Where, Projection } from "./types.js";
import { refuseRowRules } from "./common.js";
import {
  resolvePage,
  sortItems,
  cursorValues,
  keysetPredicate,
} from "./sort.js";
import { AGGREGATES } from "./aggregates.js";
import { projectNode, guardedValue, fanOut } from "./project.js";

export function projectRelationshipConnection(
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
  const props = rel.properties
    ? ctx.model.relationshipProperties.get(rel.properties)
    : undefined;

  const { first, sort, signature, cursor, backward } = resolvePage(
    ctx,
    target,
    args,
    rel.limit ?? target.limit,
    hasOwnConnection(rel) ? props : undefined,
  );

  const own = hasOwnConnection(rel);
  const typeNames = connectionTypeNames(rel, target);
  const connType = ctx.schema.getType(
    typeNames.connection,
  ) as GraphQLObjectType;
  const edgeType = ctx.schema.getType(typeNames.edge) as GraphQLObjectType;
  const sel = connectionSelections(
    ctx,
    connType,
    edgeType,
    sets,
    target,
    props,
  );

  const x = freshVar(ctx, `${parent}_${key}`);
  const r = freshVar(ctx, `${x}_rel`);
  const pattern = relationshipPattern(parent, rel, label, x, r);
  // READ_EDGE rules decide per relationship: its properties read as
  // FORBIDDEN where they fail, and nothing may filter, sort or aggregate
  // by them (that would reveal the values the rule hides).
  const edgeRead = props
    ? relationshipRules(
        ctx,
        rel,
        "READ_EDGE",
        { owner: parent, target: x, rel: r },
        "read",
      )
    : undefined;

  const whereArg = args["where"] as Where | null | undefined;
  const nodeWhere = own ? (whereArg?.["node"] as Where | undefined) : whereArg;
  const edgeWhere = own ? (whereArg?.["edge"] as Where | undefined) : undefined;
  const filterFor = (nv: string, rv: string) =>
    and(
      compileNodeWhere(ctx, target, nv, nodeWhere),
      props ? compilePropsWhere(ctx, props, rv, edgeWhere) : undefined,
      authFilter(ctx, target, nv, "READ"),
    );
  if (
    edgeRead !== undefined &&
    (edgeWhere != null ||
      sort.some((k) => k.edge) ||
      (sel.aggregate?.edge.length ?? 0) > 0)
  ) {
    throw refusedEdgeRead(rel);
  }
  const filter = filterFor(x, r);
  ctx.cost +=
    rows * filterDegree(ctx, rel, true) * perRowCost(ctx, target, nodeWhere);
  const keyset = cursor ? keysetPredicate(ctx, x, sort, cursor, r) : undefined;

  const nested = projectNode(
    ctx,
    target,
    x,
    sel.node,
    rows * fanOut(ctx, rel, first + 1),
  );
  const edgeEntries = [
    { key: "node", value: nested.expr },
    { key: "__cursor", value: cursorValues(x, sort, r) },
  ];
  if (props && sel.properties.length > 0) {
    edgeEntries.push({
      key: "properties",
      value: projectProperties(ctx, props.name, r, sel.properties, edgeRead, {
        rel,
        owner: parent,
        target: x,
      }),
    });
  }

  const rowsVar = freshVar(ctx, `${x}_rows`);
  const connection: Array<{ key: string; value: Expr }> = [
    { key: "__rows", value: v(rowsVar) },
    { key: "__first", value: bind(ctx, first) },
    { key: "__after", value: bind(ctx, cursor !== undefined) },
    { key: "__backward", value: bind(ctx, backward) },
    { key: "__sort", value: bind(ctx, signature) },
  ];
  // All matching pairs, for totalCount and aggregates.
  const matching = (projection: (nv: string, rv: string) => Expr): Expr => {
    const cx = freshVar(ctx, `${x}_all`);
    const cr = freshVar(ctx, `${cx}_rel`);
    return {
      kind: "comprehension",
      pattern: relationshipPattern(parent, rel, label, cx, cr),
      where: filterFor(cx, cr),
      projection: projection(cx, cr),
    };
  };
  if (sel.aggregate) {
    const sides: Array<{ key: string; value: Expr }> = [];
    for (const side of ["node", "edge"] as const) {
      const fields = sel.aggregate[side];
      if (fields.length === 0) continue;
      sides.push({
        key: side,
        value: {
          kind: "map",
          entries: fields.map(({ field, fns }) => ({
            key: field.name,
            value: {
              kind: "map",
              entries: fns.map((agg) => ({
                key: agg,
                value: listAggregate(
                  ctx,
                  agg as ListAggregate,
                  matching((nv, rv) =>
                    prop(v(side === "node" ? nv : rv), field.property),
                  ),
                  { duration: field.type === "Duration" },
                ),
              })),
            },
          })),
        },
      });
    }
    connection.push({
      key: "__aggregate",
      value: {
        kind: "map",
        entries: [
          {
            // Related nodes, and relationships: they differ when several
            // relationships lead to the same node.
            key: "count",
            value: {
              kind: "map",
              entries: [
                {
                  key: "nodes",
                  value: distinctCount(
                    ctx,
                    matching((nv) => prop(v(nv), target.key.property)),
                  ),
                },
                {
                  key: "edges",
                  value: fn(
                    "size",
                    matching(() => lit(1)),
                  ),
                },
              ],
            },
          },
          ...sides,
        ],
      },
    });
  }
  if (sel.totalCount || sel.aggregate) {
    const cx = freshVar(ctx, `${x}_count`);
    const cr = freshVar(ctx, `${cx}_rel`);
    connection.push({
      key: "__totalCount",
      value: fn("size", {
        kind: "comprehension",
        pattern: relationshipPattern(parent, rel, label, cx, cr),
        where: filterFor(cx, cr),
        projection: lit(1),
      }),
    });
    const rule = authValidate(ctx, target, cx, "READ", "BEFORE");
    if (rule) {
      connection.push({
        key: "__denied",
        value: fn("size", {
          kind: "comprehension",
          pattern: relationshipPattern(parent, rel, label, cx, cr),
          where: and(filterFor(cx, cr), not(fn("coalesce", rule, lit(false)))),
          projection: lit(1),
        }),
      });
    }
  }

  if (!sel.page) {
    // Only counts or aggregates: no page to read.
    connection[0] = { key: "__rows", value: { kind: "list", items: [] } };
    return { pre: [], expr: { kind: "map", entries: connection } };
  }
  return {
    pre: [
      {
        kind: "call",
        imports: [parent],
        body: [
          { kind: "match", pattern, where: and(filter, keyset) },
          {
            kind: "with",
            // A READ_EDGE rule may test the parent end: keep it in scope.
            items: [
              { expr: v(r) },
              { expr: v(x) },
              ...(edgeRead !== undefined ? [{ expr: v(parent) }] : []),
            ],
            orderBy: sortItems(x, sort, r),
            limit: bind(ctx, first + 1),
          },
          ...nested.pre,
          {
            kind: "return",
            items: [
              {
                expr: fn("collect", { kind: "map", entries: edgeEntries }),
                alias: rowsVar,
              },
            ],
          },
        ],
      },
    ],
    expr: { kind: "map", entries: connection },
  };
}

function projectProperties(
  ctx: CompileContext,
  propsType: string,
  variable: string,
  sets: SelectionSetNode[],
  guard?: Expr | false,
  /** The relationship being read, for rules over its ends. */
  ends?: { rel: RelationshipField; owner: string; target: string },
): Expr {
  const props = ctx.model.relationshipProperties.get(propsType)!;
  const objType = ctx.schema.getType(propsType) as GraphQLObjectType;
  const entries: ProjectionEntry[] = [];
  for (const [key, nodes] of collectFields(ctx, objType, sets)) {
    const field = props.fields.get(nodes[0]!.name.value);
    if (!field) continue;
    // Rules over the relationship's ends (or the caller's node) decide
    // per relationship: a failing one reads the property as FORBIDDEN.
    const access = propertyReadAccess(ctx, field);
    const own =
      access === "row" && ends
        ? propertyReadGuard(ctx, ends.rel, field, {
            owner: ends.owner,
            target: ends.target,
            rel: variable,
          })
        : undefined;
    if (access === "row" && own !== undefined) {
      const when =
        guard === false || own === false
          ? lit(false)
          : guard === undefined
            ? own
            : and(guard, own)!;
      entries.push({
        kind: "entry",
        key,
        value: guardedValue(
          { when, code: "FORBIDDEN" },
          prop(v(variable), field.property),
        ),
      });
      continue;
    }
    // Rules testing claims only: a request they refuse reads the property
    // as FORBIDDEN, on every row that has it.
    if (access !== "allowed" && !(access === "row" && ends)) {
      entries.push({
        kind: "entry",
        key,
        value: {
          kind: "map",
          entries: [{ key: "__forbidden", value: lit(true) }],
        },
      });
      continue;
    }
    if (guard !== undefined) {
      entries.push({
        kind: "entry",
        key,
        value: guardedValue(
          { when: guard === false ? lit(false) : guard, code: "FORBIDDEN" },
          prop(v(variable), field.property),
        ),
      });
      continue;
    }
    entries.push(
      key === field.property
        ? { kind: "property", key }
        : { kind: "entry", key, value: prop(v(variable), field.property) },
    );
  }
  return { kind: "mapProjection", variable, entries };
}

/** Aggregates a connection's `aggregate { … }` asks for. */
interface AggregateSpec {
  node: Array<{ field: ScalarField; fns: string[] }>;
  edge: Array<{ field: ScalarField; fns: string[] }>;
}

/** What a connection selection asks for, merged across aliases. */
export function connectionSelections(
  ctx: CompileContext,
  connType: GraphQLObjectType,
  edgeType: GraphQLObjectType,
  sets: SelectionSetNode[],
  target: NodeType,
  props: RelationshipPropertiesType | undefined,
): {
  node: SelectionSetNode[];
  properties: SelectionSetNode[];
  totalCount: boolean;
  /** `edges` or `pageInfo` selected: the page must be read. */
  page: boolean;
  aggregate: AggregateSpec | undefined;
} {
  const node: SelectionSetNode[] = [];
  const properties: SelectionSetNode[] = [];
  let totalCount = false;
  let page = false;
  let aggregate: AggregateSpec | undefined;
  for (const [, nodes] of collectFields(ctx, connType, sets)) {
    const name = nodes[0]!.name.value;
    if (name === "totalCount") totalCount = true;
    if (name === "pageInfo") page = true;
    if (name === "aggregate") {
      aggregate ??= { node: [], edge: [] };
      const aggType = getNamedType(
        connType.getFields()["aggregate"]!.type,
      ) as GraphQLObjectType;
      for (const [, side] of collectFields(
        ctx,
        aggType,
        subSelections(nodes),
      )) {
        const sideName = side[0]!.name.value;
        if (sideName !== "node" && sideName !== "edge") continue;
        const sideType = getNamedType(
          aggType.getFields()[sideName]!.type,
        ) as GraphQLObjectType;
        for (const [, fieldNodes] of collectFields(
          ctx,
          sideType,
          subSelections(side),
        )) {
          const fieldName = fieldNodes[0]!.name.value;
          const field =
            sideName === "node"
              ? target.fields.get(fieldName)
              : props?.fields.get(fieldName);
          if (field?.kind !== "scalar") continue;
          if (sideName === "node") {
            checkFieldAuthentication(ctx, target.name, field);
            refuseRowRules(ctx, target, field, "aggregate");
          } else {
            checkPropertyAccess(ctx, props!.name, field, "READ");
          }
          const fieldType = getNamedType(
            sideType.getFields()[fieldName]!.type,
          ) as GraphQLObjectType;
          const fns = [
            ...collectFields(
              ctx,
              fieldType,
              subSelections(fieldNodes),
            ).values(),
          ]
            .map((n) => n[0]!.name.value)
            .filter((f) => AGGREGATES.has(f));
          const list = aggregate[sideName];
          const existing = list.find((e) => e.field === field);
          if (existing) existing.fns = [...new Set([...existing.fns, ...fns])];
          else list.push({ field, fns });
        }
      }
    }
    if (name !== "edges") continue;
    page = true;
    for (const [, edgeNodes] of collectFields(
      ctx,
      edgeType,
      subSelections(nodes),
    )) {
      const edgeField = edgeNodes[0]!.name.value;
      if (edgeField === "node") node.push(...subSelections(edgeNodes));
      if (edgeField === "properties")
        properties.push(...subSelections(edgeNodes));
    }
  }
  return { node, properties, totalCount, page, aggregate };
}
