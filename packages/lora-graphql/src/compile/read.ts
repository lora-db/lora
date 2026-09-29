// Read operations → one parameterised statement each (plus a count
// statement in the same read transaction when a connection asks for
// `totalCount`). See the Translation Rules in
// docs/design/graphql-implementation-plan.md.

import {
  getNamedType,
  type FieldNode,
  type GraphQLObjectType,
  type SelectionSetNode,
} from "graphql";
import { RANGE_UNINDEXABLE } from "../analyze/indexes.js";
import type { QueryResult, Statement } from "../driver.js";
import { requestError } from "../errors.js";
import { renameParams } from "../model/cypher-lexer.js";
import type {
  CypherField,
  NodeType,
  SearchIndex,
  PageLimit,
  RelationshipField,
  ScalarField,
} from "../model/types.js";
import { hasOwnConnection, names } from "../schema/names.js";
import {
  authFilter,
  authValidate,
  checkAuthentication,
  checkFieldAuthentication,
} from "./auth.js";
import { bind, freshVar, type CompileContext } from "./context.js";
import { decodeCursor } from "./cursor.js";
import {
  and,
  bin,
  fn,
  isNull,
  lit,
  not,
  or,
  printClauses,
  prop,
  v,
  type Clause,
  type Expr,
  type ProjectionEntry,
  type SortItem,
} from "./cypher.js";
import {
  compileNodeWhere,
  compilePropsWhere,
  relationshipPattern,
} from "./filter.js";
import { collectFields, fieldArgs, subSelections } from "./selection.js";

export type RootKind = "list" | "single" | "connection" | "aggregate";

/** How the plan of a statement is expected to find its first rows (S2). */
export interface SeekExpectation {
  statement: number;
  label: string;
  access: "exact" | "range" | "text" | "point";
  reason: string;
}

export interface ReadSet {
  labels: string[];
  relationships: string[];
}

export interface CompiledRead {
  statements: Statement[];
  /** Turns the statements' results into the root field's value. */
  shape: (results: QueryResult[]) => unknown;
  /** Result columns the first statement must produce. */
  columns: string[];
  reads: ReadSet;
  expectations: SeekExpectation[];
  /** Estimated rows touched: the bound the cost limit checks. */
  cost: number;
  /** `read` for queries; `write` for @cypher mutations. */
  mode: "read" | "write";
}

type Args = Record<string, unknown>;
type Where = Record<string, unknown>;

interface SortKey {
  field: ScalarField;
  direction: "ASC" | "DESC";
}

/** A connection value before its resolvers turn it into edges/pageInfo. */
export interface RawConnection {
  __rows: RawEdge[];
  __first: number;
  /** A cursor (`after` or `before`) was given. */
  __after: boolean;
  /** `last` / `before`: rows arrive in reverse and are flipped back. */
  __backward: boolean;
  __sort: string;
  __totalCount?: number;
}

export interface RawEdge {
  node: unknown;
  properties?: unknown;
  __cursor: unknown[];
}

export function compileRoot(
  ctx: CompileContext,
  kind: RootKind,
  node: NodeType,
  args: Args,
  fieldNodes: readonly FieldNode[],
): CompiledRead {
  const sets = subSelections(fieldNodes);
  switch (kind) {
    case "list":
      return compileList(ctx, node, args, sets);
    case "single":
      return compileSingle(ctx, node, args, sets);
    case "connection":
      return compileConnection(ctx, node, args, sets);
    case "aggregate":
      return compileAggregate(ctx, node, args, sets);
  }
}

export function finish(
  ctx: CompileContext,
  clauses: Clause[],
  columns: string[],
  shape: CompiledRead["shape"],
  expectation: Omit<SeekExpectation, "statement"> | undefined,
  extra: Statement[] = [],
  mode: "read" | "write" = "read",
): CompiledRead {
  return {
    statements: [{ text: printClauses(clauses), params: ctx.params }, ...extra],
    shape,
    columns,
    reads: readSet(ctx),
    expectations: expectation ? [{ statement: 0, ...expectation }] : [],
    cost: ctx.cost,
    mode,
  };
}

export function readSet(ctx: CompileContext): ReadSet {
  return {
    labels: [...ctx.reads.labels].sort(),
    relationships: [...ctx.reads.relationships].sort(),
  };
}

// ---------------------------------------------------------------------------
// Root fields
// ---------------------------------------------------------------------------

function compileList(
  ctx: CompileContext,
  node: NodeType,
  args: Args,
  sets: SelectionSetNode[],
): CompiledRead {
  const limit = resolveLimit(args["limit"], node.limit, "limit");
  const sort = resolveSort(ctx, node, args["sort"], false);
  const root = rootMatch(ctx, node, args["where"] as Where | undefined, sort);
  const projection = projectNode(ctx, node, "this", sets, limit);
  const clauses: Clause[] = [
    ...root.clauses,
    {
      kind: "with",
      items: [{ expr: v("this") }],
      orderBy: sortItems("this", sort),
      limit: bind(ctx, limit),
    },
    ...projection.pre,
    {
      kind: "return",
      items: [{ expr: projection.expr, alias: "this" }],
    },
  ];
  return finish(
    ctx,
    clauses,
    ["this"],
    ([r]) => r!.rows.map((row) => row["this"]),
    root.expectation,
  );
}

function compileSingle(
  ctx: CompileContext,
  node: NodeType,
  args: Args,
  sets: SelectionSetNode[],
): CompiledRead {
  checkAuthentication(ctx, node, "READ");
  const label = node.labels[0]!;
  ctx.reads.labels.add(label);
  const where = and(
    bin(
      "=",
      prop(v("this"), node.key.property),
      bind(ctx, args[node.key.name]),
    ),
    authFilter(ctx, node, "this", "READ"),
  );
  const projection = projectNode(ctx, node, "this", sets, 1);
  const clauses: Clause[] = [
    {
      kind: "match",
      pattern: { start: { variable: "this", labels: [label] }, hops: [] },
      where,
    },
    ...projection.pre,
    { kind: "return", items: [{ expr: projection.expr, alias: "this" }] },
  ];
  return finish(ctx, clauses, ["this"], ([r]) => r!.rows[0]?.["this"] ?? null, {
    label,
    access: "exact",
    reason: `lookup by @key ${node.key.name}`,
  });
}

function compileConnection(
  ctx: CompileContext,
  node: NodeType,
  args: Args,
  sets: SelectionSetNode[],
): CompiledRead {
  const { first, sort, signature, cursor, backward } = resolvePage(
    ctx,
    node,
    args,
    node.limit,
  );

  const where = args["where"] as Where | undefined;
  const connType = ctx.schema.getType(
    names.connection(node.name),
  ) as GraphQLObjectType;
  const edgeType = ctx.schema.getType(
    names.edge(node.name),
  ) as GraphQLObjectType;
  const sel = connectionSelections(ctx, connType, edgeType, sets);

  const root = rootMatch(ctx, node, where, sort, cursor);
  const projection = projectNode(ctx, node, "this", sel.node, first + 1);
  const clauses: Clause[] = [
    ...root.clauses,
    {
      kind: "with",
      items: [{ expr: v("this") }],
      orderBy: sortItems("this", sort),
      limit: bind(ctx, first + 1),
    },
    ...projection.pre,
    {
      kind: "return",
      items: [
        { expr: projection.expr, alias: "node" },
        { expr: cursorValues("this", sort), alias: "__cursor" },
      ],
    },
  ];

  const extra: Statement[] = [];
  if (sel.totalCount) {
    const countCtx: CompileContext = {
      ...ctx,
      params: {},
      vars: new Set(["this"]),
    };
    const countRoot = rootMatch(countCtx, node, where, []);
    extra.push({
      text: printClauses([
        ...countRoot.clauses,
        {
          kind: "return",
          items: [
            { expr: fn("count", v("this")), alias: "totalCount" },
            ...deniedItem(countCtx, node, "this"),
          ],
        },
      ]),
      params: countCtx.params,
    });
  }

  return finish(
    ctx,
    clauses,
    ["node", "__cursor"],
    ([page, count]): RawConnection => {
      if (count) assertNoneDenied(count.rows[0]);
      return {
        __rows: page!.rows.map((row) => ({
          node: row["node"],
          __cursor: row["__cursor"] as unknown[],
        })),
        __first: first,
        __after: cursor !== undefined,
        __backward: backward,
        __sort: signature,
        ...(count
          ? { __totalCount: count.rows[0]?.["totalCount"] as number }
          : {}),
      };
    },
    root.expectation,
    extra,
  );
}

const AGGREGATES = new Set(["min", "max", "avg", "sum"]);

function compileAggregate(
  ctx: CompileContext,
  node: NodeType,
  args: Args,
  sets: SelectionSetNode[],
): CompiledRead {
  const aggType = ctx.schema.getType(
    names.aggregate(node.name),
  ) as GraphQLObjectType;
  const root = rootMatch(ctx, node, args["where"] as Where | undefined, []);
  const items: Array<{ expr: Expr; alias: string }> = [
    { expr: fn("count", v("this")), alias: "count" },
  ];
  const plan: Array<{ field: string; fn: string; column: string }> = [];
  for (const [, nodes] of collectFields(ctx, aggType, sets)) {
    const fieldName = nodes[0]!.name.value;
    const field = node.fields.get(fieldName);
    if (!field || field.kind !== "scalar") continue;
    checkFieldAuthentication(ctx, node.name, field);
    const fieldType = getNamedType(
      aggType.getFields()[fieldName]!.type,
    ) as GraphQLObjectType;
    for (const [, sub] of collectFields(ctx, fieldType, subSelections(nodes))) {
      const agg = sub[0]!.name.value;
      if (!AGGREGATES.has(agg)) continue;
      const column = `${fieldName}_${agg}`;
      if (plan.some((p) => p.column === column)) continue;
      plan.push({ field: fieldName, fn: agg, column });
      items.push({
        expr: fn(agg, prop(v("this"), field.property)),
        alias: column,
      });
    }
  }
  items.push(...deniedItem(ctx, node, "this"));
  const clauses: Clause[] = [...root.clauses, { kind: "return", items }];
  return finish(
    ctx,
    clauses,
    items.map((i) => i.alias),
    ([r]) => {
      assertNoneDenied(r!.rows[0]);
      const row = r!.rows[0] ?? { count: 0 };
      const out: Record<string, unknown> = { count: row["count"] ?? 0 };
      for (const p of plan) {
        const bucket = (out[p.field] ??= {}) as Record<string, unknown>;
        bucket[p.fn] = row[p.column] ?? null;
      }
      return out;
    },
    root.expectation,
  );
}

/**
 * A root `@cypher` field on Query or Mutation: the statement runs in a
 * `CALL { }`, and returned nodes are projected like any other read.
 */
export function compileCypherRoot(
  ctx: CompileContext,
  field: CypherField,
  args: Args,
  fieldNodes: readonly FieldNode[],
): CompiledRead {
  const call: Clause = {
    kind: "call",
    imports: [],
    body: [{ kind: "raw", text: bindStatement(ctx, field, args) }],
  };
  const column = field.columnName;
  const clauses: Clause[] = [call];
  if (field.node) {
    const node = ctx.model.nodes.get(field.node)!;
    checkAuthentication(ctx, node, "READ");
    ctx.reads.labels.add(node.labels[0]!);
    const rows = field.type.list ? node.limit.max : 1;
    const projection = projectNode(
      ctx,
      node,
      "this",
      subSelections(fieldNodes),
      rows,
    );
    clauses.push(
      {
        kind: "with",
        items: [{ expr: v(column), alias: "this" }],
        where: authFilter(ctx, node, "this", "READ"),
      },
      ...projection.pre,
      { kind: "return", items: [{ expr: projection.expr, alias: "this" }] },
    );
  } else {
    clauses.push({
      kind: "return",
      items: [{ expr: v(column), alias: "this" }],
    });
  }
  const list = field.type.list;
  return finish(
    ctx,
    clauses,
    ["this"],
    ([r]) => {
      const values = r!.rows.map((row) => row["this"]);
      return list ? values : (values[0] ?? null);
    },
    undefined,
    [],
    field.owner === "Mutation" ? "write" : "read",
  );
}

/** A search root field's value: nodes with their scores. */
export interface SearchResult {
  node: unknown;
  score: number;
}

/**
 * A full-text or vector search root field:
 *
 *   CALL db.index.fulltext.queryNodes($index, $query) YIELD node AS this, score
 *   WHERE <where> AND <read filter>
 *   WITH this, score ORDER BY score DESC, this.key ASC LIMIT $limit
 *
 * Vector search asks the index for more candidates than the page when a
 * filter may drop some, because the index returns the top k before WHERE.
 */
export function compileSearch(
  ctx: CompileContext,
  node: NodeType,
  index: SearchIndex,
  args: Args,
  fieldNodes: readonly FieldNode[],
): CompiledRead {
  checkAuthentication(ctx, node, "READ");
  const label = node.labels[0]!;
  ctx.reads.labels.add(label);
  const limit = resolveLimit(args["limit"], node.limit, "limit");
  const where = and(
    compileNodeWhere(ctx, node, "this", args["where"] as Where | undefined),
    authFilter(ctx, node, "this", "READ"),
  );
  const clauses: Clause[] = [];
  if (index.kind === "fulltext") {
    const query = args["query"] as string;
    clauses.push({
      kind: "procedure",
      procedure: "db.index.fulltext.queryNodes",
      args: [bind(ctx, index.name), bind(ctx, query)],
      yields: [{ item: "node", alias: "this" }, { item: "score" }],
      where,
    });
  } else {
    const vector = args["vector"] as number[] | null | undefined;
    const to = args["to"];
    if ((vector == null) === (to == null)) {
      throw requestError(
        "BAD_USER_INPUT",
        `${index.queryName} takes exactly one of \`vector\` and \`to\``,
      );
    }
    if (vector && vector.length !== index.dimensions) {
      throw requestError(
        "BAD_USER_INPUT",
        `\`vector\` has ${vector.length} dimensions; ${node.name}.${index.field.name} has ${index.dimensions}`,
      );
    }
    const candidates = Math.min(
      (limit + (to != null ? 1 : 0)) * (where ? 4 : 1),
      Math.max(node.limit.max * 4, 1),
    );
    let source: Expr;
    let exclude: Expr | undefined;
    if (to != null) {
      clauses.push({
        kind: "match",
        pattern: { start: { variable: "anchor", labels: [label] }, hops: [] },
        where: and(
          bin("=", prop(v("anchor"), node.key.property), bind(ctx, to)),
          authFilter(ctx, node, "anchor", "READ"),
        ),
      });
      source = prop(v("anchor"), index.field.property);
      exclude = bin(
        "<>",
        prop(v("this"), node.key.property),
        prop(v("anchor"), node.key.property),
      );
    } else {
      source = bind(ctx, vector);
    }
    clauses.push({
      kind: "procedure",
      procedure: "db.index.vector.queryNodes",
      args: [bind(ctx, index.name), bind(ctx, candidates), source],
      yields: [{ item: "node", alias: "this" }, { item: "score" }],
      where: and(exclude, where),
    });
  }
  const projection = projectNode(
    ctx,
    node,
    "this",
    searchNodeSelections(ctx, node, fieldNodes),
    limit,
  );
  clauses.push(
    {
      kind: "with",
      items: [{ expr: v("this") }, { expr: v("score") }],
      orderBy: [
        { expr: v("score"), direction: "DESC" },
        { expr: prop(v("this"), node.key.property), direction: "ASC" },
      ],
      limit: bind(ctx, limit),
    },
    ...projection.pre,
    {
      kind: "return",
      items: [
        { expr: projection.expr, alias: "node" },
        { expr: v("score"), alias: "score" },
      ],
    },
  );
  return finish(
    ctx,
    clauses,
    ["node", "score"],
    ([r]): SearchResult[] =>
      r!.rows.map((row) => ({
        node: row["node"],
        score: row["score"] as number,
      })),
    undefined,
  );
}

/** The `node` selections of a `<Type>Match` list, merged across aliases. */
function searchNodeSelections(
  ctx: CompileContext,
  node: NodeType,
  fieldNodes: readonly FieldNode[],
): SelectionSetNode[] {
  const matchType = ctx.schema.getType(
    names.match(node.name),
  ) as GraphQLObjectType;
  const out: SelectionSetNode[] = [];
  for (const [, nodes] of collectFields(
    ctx,
    matchType,
    subSelections(fieldNodes),
  )) {
    if (nodes[0]!.name.value === "node") out.push(...subSelections(nodes));
  }
  return out;
}

/**
 * Nodes by @key, in one statement, for mutation payloads: the read runs
 * inside the mutation's transaction and sees its writes.
 */
export function compileByKeys(
  ctx: CompileContext,
  node: NodeType,
  keys: unknown[],
  sets: SelectionSetNode[],
): CompiledRead {
  checkAuthentication(ctx, node, "READ");
  const label = node.labels[0]!;
  ctx.reads.labels.add(label);
  const k = freshVar(ctx, "this_key");
  const projection = projectNode(ctx, node, "this", sets, keys.length);
  const clauses: Clause[] = [
    { kind: "unwind", expr: bind(ctx, keys), alias: k },
    {
      kind: "match",
      pattern: { start: { variable: "this", labels: [label] }, hops: [] },
      where: and(
        bin("=", prop(v("this"), node.key.property), v(k)),
        authFilter(ctx, node, "this", "READ"),
      ),
    },
    ...projection.pre,
    {
      kind: "return",
      items: [
        { expr: projection.expr, alias: "this" },
        { expr: prop(v("this"), node.key.property), alias: "__key" },
      ],
    },
  ];
  return finish(
    ctx,
    clauses,
    ["this", "__key"],
    ([r]) => {
      const byKey = new Map(
        r!.rows.map((row) => [keyOf(row["__key"]), row["this"]]),
      );
      return keys.map((key) => byKey.get(keyOf(key)) ?? null);
    },
    { label, access: "exact", reason: "mutation payload by @key" },
  );
}

export function keyOf(value: unknown): string {
  return typeof value === "bigint" ? `n${value}` : JSON.stringify(value);
}

/** The statement text with its parameters renamed to bound `$pN`s. */
export function bindStatement(
  ctx: CompileContext,
  field: CypherField,
  args: Args,
): string {
  const names = new Map<string, string>();
  for (const p of field.params) {
    const value = p === "jwt" ? (ctx.jwt ?? null) : (args[p] ?? null);
    names.set(p, (bind(ctx, value) as { name: string }).name);
  }
  return renameParams(field.statement, (p) => names.get(p) ?? p);
}

// ---------------------------------------------------------------------------
// The root MATCH: anchoring, filters, keyset
// ---------------------------------------------------------------------------

const EXACT_OPS = new Set(["eq", "in"]);
const RANGE_OPS = new Set(["lt", "lte", "gt", "gte"]);
const TEXT_OPS = new Set(["contains", "startsWith", "endsWith"]);
const POINT_OPS = new Set(["withinBBox", "distance"]);

function rootMatch(
  ctx: CompileContext,
  node: NodeType,
  where: Where | null | undefined,
  sort: SortKey[],
  cursor?: unknown[],
): {
  clauses: Clause[];
  expectation: Omit<SeekExpectation, "statement"> | undefined;
} {
  checkAuthentication(ctx, node, "READ");
  const label = node.labels[0]!;
  ctx.reads.labels.add(label);
  const clauses: Clause[] = [];
  let anchor: Expr | undefined;
  let expectation: Omit<SeekExpectation, "statement"> | undefined;
  let rest = where ?? undefined;

  // `x IN $list` does not use an index in LoraDB; UNWIND + equality does.
  // Anchor on the first top-level `in` so the MATCH seeks per value.
  const anchorField = rest ? findInAnchor(node, rest) : undefined;
  if (anchorField && rest) {
    const ops = rest[anchorField.name] as Where;
    const values = dedupe(ops["in"] as unknown[]);
    const { in: _in, ...otherOps } = ops;
    rest = { ...rest, [anchorField.name]: otherOps };
    const item = freshVar(ctx, `this_${anchorField.name}`);
    clauses.push({ kind: "unwind", expr: bind(ctx, values), alias: item });
    anchor = bin("=", prop(v("this"), anchorField.property), v(item));
    expectation = {
      label,
      access: "exact",
      reason: `${node.name}.${anchorField.name} in [...] is unwound into equality seeks`,
    };
  }

  const predicate = compileNodeWhere(ctx, node, "this", rest);
  expectation ??= rest ? seekFromWhere(node, rest) : undefined;

  // Nothing to seek on, but a relationship filter names a related node by
  // key: start from that node and expand, instead of scanning the label.
  // The filter itself stays in the WHERE, so the result is unchanged.
  let related: RelatedAnchor | undefined;
  if (!expectation && !anchor && rest) {
    related = findRelatedAnchor(ctx, node, rest);
    if (related) {
      clauses.push(...related.clauses);
      expectation = related.expectation;
    }
  }

  let keyset: Expr | undefined;
  if (cursor) {
    keyset = keysetPredicate(ctx, "this", sort, cursor);
    if (
      !expectation &&
      leadBound(sort[0]!) &&
      !RANGE_UNINDEXABLE.has(sort[0]!.field.type)
    ) {
      expectation = {
        label,
        access: "range",
        reason: `keyset on ${node.name}.${sort[0]!.field.name}`,
      };
    }
  }

  // Ordered ascending by a string that is always present (the key, or a
  // non-null @sortable field with its existence constraint) and nothing
  // else to seek on: `>= ""` excludes no row, and lets the planner walk the
  // index in order and stop after `limit` rows instead of sorting the label.
  let bound: Expr | undefined;
  const lead = sort[0];
  if (
    !predicate &&
    !anchor &&
    !keyset &&
    !related &&
    lead &&
    lead.direction === "ASC" &&
    leadBound(lead) &&
    (lead.field.type === "String" || lead.field.type === "ID")
  ) {
    bound = bin(">=", prop(v("this"), lead.field.property), lit(""));
    expectation = {
      label,
      access: "range",
      reason: `ordered by ${node.name}.${lead.field.name}`,
    };
  }

  clauses.push({
    kind: "match",
    pattern: related
      ? related.pattern
      : { start: { variable: "this", labels: [label] }, hops: [] },
    where: and(
      anchor,
      predicate,
      keyset,
      bound,
      authFilter(ctx, node, "this", "READ"),
    ),
  });
  if (related) {
    // Several relationships may lead to the same node.
    clauses.push({
      kind: "with",
      distinct: true,
      items: [{ expr: v("this") }],
    });
  }
  return { clauses, expectation };
}

interface RelatedAnchor {
  clauses: Clause[];
  pattern: import("./cypher.js").Pattern;
  expectation: Omit<SeekExpectation, "statement">;
}

function findRelatedAnchor(
  ctx: CompileContext,
  node: NodeType,
  where: Where,
): RelatedAnchor | undefined {
  for (const [key, value] of Object.entries(where)) {
    const rel = node.fields.get(key);
    if (rel?.kind !== "relationship" || value == null) continue;
    const target = ctx.model.nodes.get(rel.target)!;
    const inner = rel.list
      ? ((value as Where)["some"] ?? (value as Where)["single"])
      : value;
    if (!inner || typeof inner !== "object") continue;
    const ops = (inner as Where)[target.key.name] as Where | undefined;
    const eq = ops?.["eq"];
    const inList = ops?.["in"];
    if (eq == null && !Array.isArray(inList)) continue;

    const a = freshVar(ctx, `this_${rel.name}_anchor`);
    const clauses: Clause[] = [];
    let seek: Expr;
    if (eq != null) {
      seek = bin("=", prop(v(a), target.key.property), bind(ctx, eq));
    } else {
      const item = freshVar(ctx, `${a}_key`);
      clauses.push({
        kind: "unwind",
        expr: bind(ctx, dedupe(inList as unknown[])),
        alias: item,
      });
      seek = bin("=", prop(v(a), target.key.property), v(item));
    }
    clauses.push({
      kind: "match",
      pattern: {
        start: { variable: a, labels: [target.labels[0]!] },
        hops: [],
      },
      where: seek,
    });
    return {
      clauses,
      // (anchor)<-[:T]-(this:Label): the field's direction, reversed.
      pattern: {
        start: { variable: a, labels: [] },
        hops: [
          {
            rel: {
              type: rel.type,
              direction: rel.direction === "OUT" ? "IN" : "OUT",
            },
            node: { variable: "this", labels: [node.labels[0]!] },
          },
        ],
      },
      expectation: {
        label: target.labels[0]!,
        access: "exact",
        reason: `anchored on ${target.name}.${target.key.name} through ${node.name}.${rel.name}`,
      },
    };
  }
  return undefined;
}

function findInAnchor(node: NodeType, where: Where): ScalarField | undefined {
  for (const [key, value] of Object.entries(where)) {
    const field = node.fields.get(key);
    if (field?.kind !== "scalar" || value == null) continue;
    const list = (value as Where)["in"];
    if (Array.isArray(list)) return field;
  }
  return undefined;
}

function seekFromWhere(
  node: NodeType,
  where: Where,
): Omit<SeekExpectation, "statement"> | undefined {
  const label = node.labels[0]!;
  for (const access of ["exact", "range", "text", "point"] as const) {
    for (const [key, value] of Object.entries(where)) {
      const field = node.fields.get(key);
      if (field?.kind !== "scalar" || value == null) continue;
      const ops = Object.entries(value as Where)
        .filter(([, x]) => x != null)
        .map(([op]) => op);
      const family = {
        exact: EXACT_OPS,
        range: RANGE_OPS,
        text: TEXT_OPS,
        point: POINT_OPS,
      }[access];
      if (access === "range" && RANGE_UNINDEXABLE.has(field.type)) continue;
      const op = ops.find((o) => family.has(o));
      if (op) {
        return {
          label,
          access,
          reason: `${node.name}.${field.name} ${op}`,
        };
      }
    }
  }
  return undefined;
}

function dedupe(values: unknown[]): unknown[] {
  const seen = new Set<string>();
  return values.filter((x) => {
    const k = keyOf(x);
    if (seen.has(k)) return false;
    seen.add(k);
    return true;
  });
}

// ---------------------------------------------------------------------------
// Sorting, limits, keyset
// ---------------------------------------------------------------------------

export function resolveLimit(
  value: unknown,
  limit: PageLimit,
  argument: string,
): number {
  if (value === null || value === undefined) return limit.default;
  const n = value as number;
  if (!Number.isInteger(n) || n < 0) {
    throw requestError(
      "BAD_USER_INPUT",
      `\`${argument}\` must be a non-negative integer`,
    );
  }
  if (n > limit.max) {
    throw requestError(
      "LIMIT_EXCEEDED",
      `\`${argument}\` is ${n}; the maximum is ${limit.max}`,
    );
  }
  return n;
}

/**
 * The sort keys for `value`. Connections (`tieBreak`) always end on a
 * unique field so cursors are stable; plain lists do not, because LoraDB
 * 0.15 streams an index in order only for a single sort key, and a
 * tie-breaker would turn a `LIMIT` into a sort of the whole label.
 */
function resolveSort(
  ctx: CompileContext,
  node: NodeType,
  value: unknown,
  tieBreak: boolean,
): SortKey[] {
  const keys: SortKey[] = [];
  for (const item of (value as Array<Record<string, unknown>> | null) ?? []) {
    const entries = Object.entries(item).filter(([, d]) => d != null);
    if (entries.length !== 1) {
      throw requestError(
        "BAD_USER_INPUT",
        "each `sort` item names exactly one field",
      );
    }
    const [fieldName, direction] = entries[0]!;
    const field = node.fields.get(fieldName) as ScalarField;
    // Ordering by a field reveals it, and cursors carry its values.
    checkFieldAuthentication(ctx, node.name, field);
    if (keys.some((k) => k.field === field)) {
      throw requestError(
        "BAD_USER_INPUT",
        `\`sort\` names ${fieldName} more than once`,
      );
    }
    keys.push({ field, direction: direction as "ASC" | "DESC" });
    // A unique, always-present field orders totally: later items could
    // never break a tie. (Unique fields may repeat null, so a nullable
    // one still needs the key.)
    if (field.key || (field.unique && field.required)) return keys;
  }
  if (tieBreak || keys.length === 0) {
    keys.push({ field: node.key, direction: "ASC" });
  }
  return keys;
}

/**
 * `first` / `after`, or `last` / `before`. A backward page runs the sort
 * reversed from the cursor and is flipped back by the resolvers; cursors
 * carry the requested sort's signature, so they work in both directions.
 */
function resolvePage(
  ctx: CompileContext,
  node: NodeType,
  args: Args,
  limit: PageLimit,
): {
  first: number;
  sort: SortKey[];
  signature: string;
  cursor: unknown[] | undefined;
  backward: boolean;
} {
  const backward = args["last"] != null || args["before"] != null;
  if (backward && (args["first"] != null || args["after"] != null)) {
    throw requestError(
      "BAD_USER_INPUT",
      "page forward with first/after or backward with last/before, not both",
    );
  }
  const first = resolveLimit(
    backward ? args["last"] : args["first"],
    limit,
    backward ? "last" : "first",
  );
  const requested = resolveSort(ctx, node, args["sort"], true);
  const signature = sortSignature(requested);
  const raw = (backward ? args["before"] : args["after"]) as
    | string
    | null
    | undefined;
  const cursor =
    raw != null ? decodeCursor(raw, signature, requested.length) : undefined;
  const sort = backward
    ? requested.map((k) => ({
        ...k,
        direction: k.direction === "ASC" ? ("DESC" as const) : ("ASC" as const),
      }))
    : requested;
  return { first, sort, signature, cursor, backward };
}

function sortSignature(sort: SortKey[]): string {
  return sort.map((k) => `${k.field.name}:${k.direction}`).join(",");
}

function sortItems(variable: string, sort: SortKey[]): SortItem[] {
  return sort.map((k) => ({
    expr: prop(v(variable), k.field.property),
    direction: k.direction,
  }));
}

function cursorValues(variable: string, sort: SortKey[]): Expr {
  return {
    kind: "list",
    items: sort.map((k) => prop(v(variable), k.field.property)),
  };
}

/** Whether a sort key is guaranteed present, so a `>=` bound loses no rows. */
function leadBound(k: SortKey): boolean {
  return k.field.key || (k.field.required && k.field.sortable);
}

/**
 * Rows strictly after `values` in `sort` order, written out term by term
 * (`[a, b] > $list` silently matches nothing in LoraDB). Null ordering
 * follows Cypher: nulls last ascending, first descending.
 */
function keysetPredicate(
  ctx: CompileContext,
  variable: string,
  sort: SortKey[],
  values: unknown[],
): Expr | undefined {
  const branches: Expr[] = [];
  const equal: Expr[] = [];
  let lead: Expr | undefined;
  sort.forEach((k, i) => {
    const target = prop(v(variable), k.field.property);
    const value = values[i];
    const p = value === null ? undefined : bind(ctx, value);
    const present = leadBound(k);
    let after: Expr | undefined;
    if (p === undefined) {
      // Past a null: ascending, nothing but more nulls; descending, every
      // non-null value.
      after = k.direction === "ASC" ? undefined : isNull(target, true);
    } else if (k.direction === "ASC") {
      after = present
        ? bin(">", target, p)
        : or(bin(">", target, p), isNull(target));
    } else {
      after = bin("<", target, p);
    }
    if (after) branches.push(and(...equal, after)!);
    equal.push(p === undefined ? isNull(target) : bin("=", target, p));
    if (i === 0 && p !== undefined && present) {
      lead = bin(k.direction === "ASC" ? ">=" : "<=", target, p);
    }
  });
  const disjunction = or(...branches) ?? lit(false);
  return and(lead, disjunction);
}

// ---------------------------------------------------------------------------
// Projection
// ---------------------------------------------------------------------------

export interface Projection {
  /** Subqueries that must run before the projection is evaluated. */
  pre: Clause[];
  expr: Expr;
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
  for (const [key, nodes] of collectFields(ctx, objType, sets)) {
    const fieldName = nodes[0]!.name.value;
    if (fieldName === "__typename") continue;
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
    const rel = field ?? connectionOf(node, fieldName);
    if (rel?.authentication?.has("READ") && !ctx.jwt) {
      throw requestError(
        "UNAUTHENTICATED",
        `${node.name}.${fieldName} needs an authenticated request`,
      );
    }
    if (field?.kind === "scalar") {
      entries.push(
        key === field.property
          ? { kind: "property", key }
          : { kind: "entry", key, value: prop(v(variable), field.property) },
      );
      continue;
    }
    const args = def ? fieldArgs(ctx, def, nodes[0]!) : {};
    const nested = subSelections(nodes);
    let result: Projection;
    if (field?.kind === "relationship") {
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
    entries.push({ kind: "entry", key, value: result.expr });
  }
  // Read validation: the resolvers turn `false` into a FORBIDDEN error.
  const validate = authValidate(ctx, node, variable, "READ", "BEFORE");
  if (validate)
    entries.push({ kind: "entry", key: "__authorized", value: validate });
  return { pre, expr: { kind: "mapProjection", variable, entries } };
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
function fanOut(
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
  const nested = projectNode(
    ctx,
    target,
    x,
    sets,
    rows * fanOut(ctx, rel, limit),
  );
  if ((!userSort || userSort.length === 0) && nested.pre.length === 0) {
    // [(this)-[:T]->(x:Target) WHERE … | x { … }][..$limit]
    return {
      pre: [],
      expr: {
        kind: "slice",
        target: {
          kind: "comprehension",
          pattern,
          where,
          projection: nested.expr,
        },
        from: undefined,
        to: limitParam,
      },
    };
  }
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

function projectRelationshipConnection(
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
  );

  const own = hasOwnConnection(rel);
  const connType = ctx.schema.getType(
    own
      ? names.relConnection(rel.owner, rel.name)
      : names.connection(target.name),
  ) as GraphQLObjectType;
  const edgeType = ctx.schema.getType(
    own ? names.relEdge(rel.owner, rel.name) : names.edge(target.name),
  ) as GraphQLObjectType;
  const sel = connectionSelections(ctx, connType, edgeType, sets);

  const x = freshVar(ctx, `${parent}_${key}`);
  const r = freshVar(ctx, `${x}_rel`);
  const pattern = relationshipPattern(parent, rel, label, x, r);

  const whereArg = args["where"] as Where | null | undefined;
  const nodeWhere = own ? (whereArg?.["node"] as Where | undefined) : whereArg;
  const edgeWhere = own ? (whereArg?.["edge"] as Where | undefined) : undefined;
  const filterFor = (nv: string, rv: string) =>
    and(
      compileNodeWhere(ctx, target, nv, nodeWhere),
      props ? compilePropsWhere(ctx, props, rv, edgeWhere) : undefined,
      authFilter(ctx, target, nv, "READ"),
    );
  const filter = filterFor(x, r);
  const keyset = cursor ? keysetPredicate(ctx, x, sort, cursor) : undefined;

  const nested = projectNode(
    ctx,
    target,
    x,
    sel.node,
    rows * fanOut(ctx, rel, first + 1),
  );
  const edgeEntries = [
    { key: "node", value: nested.expr },
    { key: "__cursor", value: cursorValues(x, sort) },
  ];
  if (props && sel.properties.length > 0) {
    edgeEntries.push({
      key: "properties",
      value: projectProperties(ctx, props.name, r, sel.properties),
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
  if (sel.totalCount) {
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

  return {
    pre: [
      {
        kind: "call",
        imports: [parent],
        body: [
          { kind: "match", pattern, where: and(filter, keyset) },
          {
            kind: "with",
            items: [{ expr: v(r) }, { expr: v(x) }],
            orderBy: sortItems(x, sort),
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

/**
 * An object `@cypher` field, run once per parent:
 *
 *   CALL { WITH parent WITH parent AS this
 *          CALL { WITH this <statement> }
 *          RETURN collect(<column>) AS out }
 */
function projectCypher(
  ctx: CompileContext,
  parent: string,
  field: CypherField,
  key: string,
  args: Args,
  sets: SelectionSetNode[],
  rows: number,
): Projection {
  const out = freshVar(ctx, `${parent}_${key}`);
  const body: Clause[] = [];
  if (parent !== "this") {
    body.push({ kind: "with", items: [{ expr: v(parent), alias: "this" }] });
  }
  body.push({
    kind: "call",
    imports: ["this"],
    body: [{ kind: "raw", text: bindStatement(ctx, field, args) }],
  });
  const column = field.columnName;
  const gather = (value: Expr): Clause[] =>
    field.type.list
      ? [
          {
            kind: "return",
            items: [{ expr: fn("collect", value), alias: out }],
          },
        ]
      : collectOne(value, out);
  if (field.node) {
    const target = ctx.model.nodes.get(field.node)!;
    checkAuthentication(ctx, target, "READ");
    ctx.reads.labels.add(target.labels[0]!);
    const x = freshVar(ctx, `${out}_node`);
    const per = field.type.list ? target.limit.default : 1;
    const nested = projectNode(ctx, target, x, sets, rows * per);
    body.push(
      {
        kind: "with",
        items: [{ expr: v(column), alias: x }],
        where: authFilter(ctx, target, x, "READ"),
      },
      ...nested.pre,
      ...gather(nested.expr),
    );
  } else {
    ctx.cost += rows;
    body.push(...gather(v(column)));
  }
  return {
    pre: [{ kind: "call", imports: [parent], body }],
    expr: v(out),
  };
}

/**
 * `sum(CASE WHEN <READ validate rule> THEN 0 ELSE 1 END) AS __denied`:
 * aggregates and counts cover nodes a READ validate rule rejects, so they
 * fail like reading those nodes would.
 */
function deniedItem(
  ctx: CompileContext,
  node: NodeType,
  variable: string,
): Array<{ expr: Expr; alias: string }> {
  const rule = authValidate(ctx, node, variable, "READ", "BEFORE");
  if (!rule) return [];
  return [
    {
      expr: fn("sum", {
        kind: "case",
        when: fn("coalesce", rule, lit(false)),
        then: lit(0),
        else: lit(1),
      }),
      alias: "__denied",
    },
  ];
}

function assertNoneDenied(row: Record<string, unknown> | undefined): void {
  if (Number(row?.["__denied"] ?? 0) > 0) {
    throw requestError("FORBIDDEN", "not allowed to read some of these nodes");
  }
}

/**
 * `RETURN head(collect(x)) AS out`, written as two clauses: LoraDB 0.15
 * does not treat an aggregate nested inside another call as aggregating.
 */
function collectOne(value: Expr, out: string): Clause[] {
  return [
    { kind: "with", items: [{ expr: fn("collect", value), alias: out }] },
    { kind: "return", items: [{ expr: fn("head", v(out)), alias: out }] },
  ];
}

function projectProperties(
  ctx: CompileContext,
  propsType: string,
  variable: string,
  sets: SelectionSetNode[],
): Expr {
  const props = ctx.model.relationshipProperties.get(propsType)!;
  const objType = ctx.schema.getType(propsType) as GraphQLObjectType;
  const entries: ProjectionEntry[] = [];
  for (const [key, nodes] of collectFields(ctx, objType, sets)) {
    const field = props.fields.get(nodes[0]!.name.value);
    if (!field) continue;
    entries.push(
      key === field.property
        ? { kind: "property", key }
        : { kind: "entry", key, value: prop(v(variable), field.property) },
    );
  }
  return { kind: "mapProjection", variable, entries };
}

/** What a connection selection asks for, merged across aliases. */
function connectionSelections(
  ctx: CompileContext,
  connType: GraphQLObjectType,
  edgeType: GraphQLObjectType,
  sets: SelectionSetNode[],
): {
  node: SelectionSetNode[];
  properties: SelectionSetNode[];
  totalCount: boolean;
} {
  const node: SelectionSetNode[] = [];
  const properties: SelectionSetNode[] = [];
  let totalCount = false;
  for (const [, nodes] of collectFields(ctx, connType, sets)) {
    const name = nodes[0]!.name.value;
    if (name === "totalCount") totalCount = true;
    if (name !== "edges") continue;
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
  return { node, properties, totalCount };
}
