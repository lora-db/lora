// Read operations → one parameterised statement each (plus a count
// statement in the same read transaction when a connection asks for
// `totalCount`). See the translation rules in
// apps/loradb.com/docs/graphql/translation-rules.md.

import {
  getNamedType,
  parse,
  type FieldNode,
  type GraphQLObjectType,
  type OperationDefinitionNode,
  type SelectionSetNode,
} from "graphql";
import { RANGE_UNINDEXABLE } from "../analyze/indexes.js";
import {
  filterDegree,
  memberWhere,
  perRowCost,
  rootFilterCost,
} from "./cost.js";
import type { QueryResult, Statement } from "../driver.js";
import { requestError } from "../errors.js";
import { scanParams } from "../model/cypher-lexer.js";
import {
  distinctCount,
  listAggregate,
  type ListAggregate,
} from "./aggregate.js";
import type {
  AbstractType,
  CypherField,
  NodeType,
  RelationshipPropertiesType,
  SearchIndex,
  PageLimit,
  RelationshipField,
  ScalarField,
} from "../model/types.js";
import {
  connectionTypeNames,
  hasOwnConnection,
  names,
} from "../schema/names.js";
import {
  authFilter,
  authValidate,
  checkAuthentication,
  checkFieldAuthentication,
  checkPropertyAccess,
  fieldReadGuard,
  fieldValidate,
  maskedValue,
  maskSettled,
  propertyAccess,
  refusedEdgeRead,
  relationshipRules,
  viewerKey,
} from "./auth.js";
import { bind, freshVar, noteClaim, type CompileContext } from "./context.js";
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
  printExpr,
  prop,
  v,
  type Clause,
  type Expr,
  type ProjectionEntry,
  type SortItem,
} from "./cypher.js";
import {
  compileMemberWhere,
  compileNodeWhere,
  compilePropsWhere,
  relationshipPattern,
} from "./filter.js";
import { memberFields } from "../model/relations.js";
import { collectFields, fieldArgs, subSelections } from "./selection.js";

export type RootKind =
  | "list"
  | "single"
  | "connection"
  | "aggregate"
  | "grouped";

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
  /**
   * At most one row, found by an exact @key seek, projecting only stored
   * properties: no relationship, @cypher field or rule traversal. Cheap
   * enough for a driver to run synchronously.
   */
  bounded?: boolean;
}

type Args = Record<string, unknown>;
type Where = Record<string, unknown>;

interface SortKey {
  field: ScalarField;
  direction: "ASC" | "DESC";
  /** A relationship property (`sort: [{ edge: { ... } }]`), not a node field. */
  edge?: boolean;
  /** For a computed (`@cypher`) field: the variable holding its value. */
  computed?: string;
}

/** The value a sort key orders by: `node.prop`, or `rel.prop` for edge keys. */
function keyExpr(k: SortKey, variable: string, edge?: string): Expr {
  if (k.field.computedBy) {
    if (!k.computed) {
      throw requestError(
        "BAD_USER_INPUT",
        `${k.field.computedBy.owner}.${k.field.name} is computed by @cypher: sort by it in a root field only`,
      );
    }
    return v(k.computed);
  }
  return prop(v(k.edge ? edge! : variable), k.field.property);
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
    case "grouped":
      return compileGrouped(ctx, node, args, sets);
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
  const where = args["where"] as Where | undefined;
  const root = rootMatch(ctx, node, where, sort);
  ctx.cost += rootFilterCost(ctx, node, where, limit);
  const projection = projectNode(ctx, node, "this", sets, limit);
  const clauses: Clause[] = [
    ...root.clauses,
    {
      kind: "with",
      items: [{ expr: v("this") }, ...root.carry.map((c) => ({ expr: v(c) }))],
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
  const compiled = finish(
    ctx,
    clauses,
    ["this"],
    ([r]) => r!.rows[0]?.["this"] ?? null,
    { label, access: "exact", reason: `lookup by @key ${node.key.name}` },
  );
  // Every selected field is a stored property, and nothing traversed a
  // relationship (READ rules included: their filters record what they read).
  const objType = ctx.schema.getType(node.name) as GraphQLObjectType;
  const storedOnly = [...collectFields(ctx, objType, sets).values()].every(
    (nodes) => {
      const name = nodes[0]!.name.value;
      return (
        name === "__typename" ||
        (name === "id" && node.key.relayId) ||
        node.fields.get(name)?.kind === "scalar"
      );
    },
  );
  if (
    storedOnly &&
    compiled.reads.relationships.length === 0 &&
    compiled.statements.length === 1
  ) {
    compiled.bounded = true;
  }
  return compiled;
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
  const sel = connectionSelections(
    ctx,
    connType,
    edgeType,
    sets,
    node,
    undefined,
  );

  const statements: Statement[] = [];
  let expectation: Omit<SeekExpectation, "statement"> | undefined;
  let columns: string[] = [];
  // The page, unless only counts or aggregates were asked for.
  if (sel.page) {
    const root = rootMatch(ctx, node, where, sort, cursor);
    ctx.cost += rootFilterCost(ctx, node, where, first + 1);
    expectation = root.expectation;
    const projection = projectNode(ctx, node, "this", sel.node, first + 1);
    statements.push({
      text: printClauses([
        ...root.clauses,
        {
          kind: "with",
          items: [
            { expr: v("this") },
            ...root.carry.map((c) => ({ expr: v(c) })),
          ],
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
      ]),
      params: ctx.params,
    });
    columns = ["node", "__cursor"];
  }

  // totalCount and aggregates: one pass over every match, in the same
  // read transaction.
  const stats = sel.totalCount || sel.aggregate !== undefined;
  let statsColumns: Array<{ column: string; path: string[] }> = [];
  if (stats) {
    const statsCtx: CompileContext = {
      ...ctx,
      params: {},
      vars: new Set(["this"]),
      computed: new Map(),
    };
    const statsRoot = rootMatch(statsCtx, node, where, []);
    expectation ??= statsRoot.expectation;
    const items: Array<{ expr: Expr; alias: string }> = [
      { expr: fn("count", v("this")), alias: "totalCount" },
      ...deniedItem(statsCtx, node, "this"),
    ];
    const wanted: WantedAggregate[] = [];
    for (const { field, fns } of sel.aggregate?.node ?? []) {
      for (const agg of fns) {
        const column = `node_${field.name}_${agg}`;
        wanted.push({ field, agg, column });
        statsColumns.push({ column, path: ["node", field.name, agg] });
      }
    }
    statements.push({
      text: printClauses([
        ...statsRoot.clauses,
        ...aggregateReturn(statsCtx, "this", items, wanted),
      ]),
      params: statsCtx.params,
    });
  }
  statsColumns = statsColumns.slice();

  return {
    statements,
    columns,
    reads: readSet(ctx),
    expectations: expectation ? [{ statement: 0, ...expectation }] : [],
    // Counting reads every match, not a page.
    cost:
      ctx.cost +
      (stats
        ? Math.max(1, rootFilterCost(ctx, node, where, first + 1, true))
        : 0),
    mode: "read",
    shape: (results): RawConnection => {
      const page = sel.page ? results[0] : undefined;
      const count = stats ? results[sel.page ? 1 : 0] : undefined;
      const row = count?.rows[0];
      if (count) assertNoneDenied(row);
      let aggregate: Record<string, unknown> | undefined;
      if (sel.aggregate) {
        aggregate = { count: row?.["totalCount"] ?? 0 };
        for (const { column, path } of statsColumns) {
          setPath(aggregate, path, row?.[column] ?? null);
        }
      }
      return {
        __rows: (page?.rows ?? []).map((r) => ({
          node: r["node"],
          __cursor: r["__cursor"] as unknown[],
        })),
        __first: first,
        __after: cursor !== undefined,
        __backward: backward,
        __sort: signature,
        ...(count ? { __totalCount: row?.["totalCount"] as number } : {}),
        ...(aggregate ? { __aggregate: aggregate } : {}),
      };
    },
  };
}

/**
 * Sorting or aggregating over a field with row-level READ rules would
 * reveal values of rows the rules hide: refused unless the claims alone
 * already settle the rules.
 */
function refuseRowRules(
  ctx: CompileContext,
  node: NodeType,
  field: ScalarField,
  what: string,
): void {
  const probe = { ...ctx, params: {}, vars: new Set(ctx.vars) };
  if (fieldValidate(probe, node, field, "probe", "READ")) {
    throw requestError(
      "FORBIDDEN",
      `cannot ${what} ${node.name}.${field.name}: it has row-level read rules`,
    );
  }
  if (!maskSettled(ctx, node, field)) {
    throw requestError(
      "FORBIDDEN",
      `cannot ${what} ${node.name}.${field.name}: it is masked per row`,
    );
  }
}

function setPath(
  target: Record<string, unknown>,
  path: string[],
  value: unknown,
) {
  let cur = target;
  for (const part of path.slice(0, -1)) {
    cur = (cur[part] ??= {}) as Record<string, unknown>;
  }
  cur[path[path.length - 1]!] = value;
}

const AGGREGATES = new Set(["min", "max", "avg", "sum", "shortest", "longest"]);

interface WantedAggregate {
  field: ScalarField;
  agg: string;
  column: string;
}

/**
 * Whether an aggregate function computes this correctly. Shortest and
 * longest have none; LoraDB's `max`, `sum` and `avg` over durations are
 * wrong (E25). Those are folded from a `collect()` instead.
 */
function nativeAggregate(w: WantedAggregate): boolean {
  return (
    w.field.type !== "Duration" && w.agg !== "shortest" && w.agg !== "longest"
  );
}

/**
 * The RETURN of an aggregate statement over `variable`: `base` items
 * (already aggregate expressions) and one column per wanted aggregate.
 * Folded aggregates need their values collected first, in a WITH: an
 * aggregate nested in another call is not aggregated (E16).
 */
function aggregateReturn(
  ctx: CompileContext,
  variable: string,
  base: Array<{ expr: Expr; alias: string }>,
  wanted: WantedAggregate[],
  group?: { keys: Array<{ expr: Expr; alias: string }>; limit: Expr },
): Clause[] {
  const native = wanted.filter(nativeAggregate).map((w) => ({
    expr: fn(w.agg, prop(v(variable), w.field.property)),
    alias: w.column,
  }));
  const folded = wanted.filter((w) => !nativeAggregate(w));
  const keys = group?.keys ?? [];
  const order = group
    ? {
        orderBy: keys.map((k) => ({
          expr: v(k.alias),
          direction: "ASC" as const,
        })),
        limit: group.limit,
      }
    : {};
  if (folded.length === 0) {
    return [{ kind: "return", items: [...keys, ...base, ...native], ...order }];
  }
  const lists = new Map<ScalarField, string>();
  for (const w of folded) {
    if (!lists.has(w.field)) {
      lists.set(w.field, freshVar(ctx, `${w.field.name}_values`));
    }
  }
  const carried = [...base, ...native];
  return [
    {
      kind: "with",
      items: [
        ...keys,
        ...carried,
        ...[...lists].map(([f, alias]) => ({
          expr: fn("collect", prop(v(variable), f.property)),
          alias,
        })),
      ],
    },
    {
      kind: "return",
      ...order,
      items: [
        ...[...keys, ...carried].map((i) => ({
          expr: v(i.alias),
          alias: i.alias,
        })),
        ...folded.map((w) => ({
          expr: listAggregate(
            ctx,
            w.agg as ListAggregate,
            v(lists.get(w.field)!),
            { duration: w.field.type === "Duration" },
          ),
          alias: w.column,
        })),
      ],
    },
  ];
}

function compileAggregate(
  ctx: CompileContext,
  node: NodeType,
  args: Args,
  sets: SelectionSetNode[],
): CompiledRead {
  const aggType = ctx.schema.getType(
    names.aggregate(node.name),
  ) as GraphQLObjectType;
  const where = args["where"] as Where | undefined;
  const root = rootMatch(ctx, node, where, []);
  ctx.cost += rootFilterCost(ctx, node, where, node.limit.default, true);
  const items: Array<{ expr: Expr; alias: string }> = [
    { expr: fn("count", v("this")), alias: "count" },
    ...deniedItem(ctx, node, "this"),
  ];
  const wanted = aggregateWanted(ctx, node, aggType, sets);
  const clauses: Clause[] = [
    ...root.clauses,
    ...aggregateReturn(ctx, "this", items, wanted),
  ];
  return finish(
    ctx,
    clauses,
    [...items.map((i) => i.alias), ...wanted.map((w) => w.column)],
    ([r]) => {
      assertNoneDenied(r!.rows[0]);
      return aggregateRow(r!.rows[0] ?? { count: 0 }, wanted);
    },
    root.expectation,
  );
}

/**
 * `<plural>Grouped(by: [...])`: one row per group of values, ordered by
 * them, at most `limit` groups. The values are collected per group and
 * folded (an aggregate nested in another call is not aggregated, E16).
 */
function compileGrouped(
  ctx: CompileContext,
  node: NodeType,
  args: Args,
  sets: SelectionSetNode[],
): CompiledRead {
  const by = [...new Set((args["by"] as string[] | null) ?? [])];
  if (by.length === 0) {
    throw requestError("BAD_USER_INPUT", "`by` names at least one field");
  }
  const fields = by.map((name) => {
    const f = node.fields.get(name) as ScalarField;
    checkFieldAuthentication(ctx, node.name, f);
    refuseRowRules(ctx, node, f, "group by");
    return f;
  });
  const limit = resolveLimit(args["limit"], node.limit, "limit");
  const groupType = ctx.schema.getType(
    `${node.name}Group`,
  ) as GraphQLObjectType;
  const aggType = ctx.schema.getType(
    names.aggregate(node.name),
  ) as GraphQLObjectType;
  const aggSets: SelectionSetNode[] = [];
  for (const [, nodes] of collectFields(ctx, groupType, sets)) {
    if (nodes[0]!.name.value === "aggregate") {
      aggSets.push(...subSelections(nodes));
    }
  }
  const wanted = aggregateWanted(ctx, node, aggType, aggSets);
  const where = args["where"] as Where | undefined;
  const root = rootMatch(ctx, node, where, []);
  ctx.cost += rootFilterCost(ctx, node, where, limit, true);
  const keys = fields.map((f, i) => ({
    expr: prop(v("this"), f.property),
    alias: `by_${i}`,
  }));
  const items: Array<{ expr: Expr; alias: string }> = [
    { expr: fn("count", v("this")), alias: "count" },
    ...deniedItem(ctx, node, "this"),
  ];
  ctx.cost += limit;
  const clauses: Clause[] = [
    ...root.clauses,
    ...aggregateReturn(ctx, "this", items, wanted, {
      keys,
      limit: bind(ctx, limit),
    }),
  ];
  return finish(
    ctx,
    clauses,
    [
      ...keys.map((k) => k.alias),
      ...items.map((i) => i.alias),
      ...wanted.map((w) => w.column),
    ],
    ([r]) =>
      r!.rows.map((row) => {
        assertNoneDenied(row);
        return {
          by: Object.fromEntries(
            fields.map((f, i) => [f.name, row[`by_${i}`] ?? null]),
          ),
          aggregate: aggregateRow(row, wanted),
        };
      }),
    root.expectation,
  );
}

/** The field aggregates an aggregate selection asks for. */
function aggregateWanted(
  ctx: CompileContext,
  node: NodeType,
  aggType: GraphQLObjectType,
  sets: SelectionSetNode[],
): WantedAggregate[] {
  const wanted: WantedAggregate[] = [];
  for (const [, nodes] of collectFields(ctx, aggType, sets)) {
    const fieldName = nodes[0]!.name.value;
    const field = node.fields.get(fieldName);
    if (!field || field.kind !== "scalar") continue;
    checkFieldAuthentication(ctx, node.name, field);
    refuseRowRules(ctx, node, field, "aggregate");
    const fieldType = getNamedType(
      aggType.getFields()[fieldName]!.type,
    ) as GraphQLObjectType;
    for (const [, sub] of collectFields(ctx, fieldType, subSelections(nodes))) {
      const agg = sub[0]!.name.value;
      if (!AGGREGATES.has(agg)) continue;
      const column = `${fieldName}_${agg}`;
      if (wanted.some((w) => w.column === column)) continue;
      wanted.push({ field, agg, column });
    }
  }
  return wanted;
}

/** `{ count, <field>: { <fn>: value } }` from an aggregate row. */
function aggregateRow(
  row: Record<string, unknown>,
  wanted: WantedAggregate[],
): Record<string, unknown> {
  const out: Record<string, unknown> = { count: row["count"] ?? 0 };
  for (const w of wanted) {
    const bucket = (out[w.field.name] ??= {}) as Record<string, unknown>;
    bucket[w.agg] = row[w.column] ?? null;
  }
  return out;
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
  } else if (field.abstract) {
    const abstract = ctx.model.abstracts.get(field.abstract)!;
    const rows = field.type.list ? abstract.limit.max : 1;
    const projection = projectAbstract(
      ctx,
      abstract,
      "this",
      subSelections(fieldNodes),
      rows,
    );
    clauses.push(
      {
        kind: "with",
        items: [{ expr: v(column), alias: "this" }],
        where: projection.where,
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

// ---------------------------------------------------------------------------
// Interfaces and unions
// ---------------------------------------------------------------------------

interface MemberSort {
  name: string;
  direction: "ASC" | "DESC";
}

function resolveAbstractSort(
  ctx: CompileContext,
  abstract: AbstractType,
  value: unknown,
): MemberSort[] {
  const keys: MemberSort[] = [];
  for (const item of (value as Array<Record<string, unknown>> | null) ?? []) {
    const entries = Object.entries(item).filter(([, d]) => d != null);
    if (entries.length !== 1) {
      throw requestError(
        "BAD_USER_INPUT",
        "each `sort` item names exactly one field",
      );
    }
    const [name, direction] = entries[0]!;
    for (const member of abstract.members) {
      const f = ctx.model.nodes.get(member)!.fields.get(name);
      if (f?.kind === "scalar") {
        checkFieldAuthentication(ctx, member, f);
        refuseRowRules(ctx, ctx.model.nodes.get(member)!, f, "sort by");
      }
    }
    keys.push({ name, direction: direction as "ASC" | "DESC" });
  }
  return keys;
}

/**
 * One member's slice of an interface or union list: its nodes filtered,
 * sorted and limited on their own (so each can use its indexes), projected
 * with `__typename` and the sort values the merge orders by.
 */
function memberList(
  ctx: CompileContext,
  abstract: AbstractType,
  member: NodeType,
  source: (x: string) => {
    pattern: import("./cypher.js").Pattern;
    where: Expr | undefined;
  },
  where: Record<string, unknown> | null | undefined,
  sort: MemberSort[],
  limit: Expr,
  sets: SelectionSetNode[],
  rows: number,
): { body: Clause[]; list: string } | undefined {
  const x = freshVar(ctx, `this_${member.name}`);
  const filter = compileMemberWhere(ctx, abstract, member, x, where);
  if (filter === false) return undefined;
  checkAuthentication(ctx, member, "READ");
  ctx.reads.labels.add(member.labels[0]!);
  const from = source(x);
  const sortExprs: SortItem[] = [
    ...sort.map((k) => {
      const f = member.fields.get(k.name) as ScalarField;
      return { expr: prop(v(x), f.property), direction: k.direction };
    }),
    { expr: prop(v(x), member.key.property), direction: "ASC" as const },
  ];
  const projection = projectNode(ctx, member, x, sets, rows);
  const expr = projection.expr as Extract<Expr, { kind: "mapProjection" }>;
  const list = freshVar(ctx, `${x}_list`);
  const tagged: Expr = {
    ...expr,
    entries: [
      ...expr.entries,
      { kind: "entry", key: "__typename", value: lit(member.name) },
      { kind: "entry", key: "__key", value: prop(v(x), member.key.property) },
      ...sort.map((k, i) => ({
        kind: "entry" as const,
        key: `__s${i}`,
        value: sortExprs[i]!.expr,
      })),
    ],
  };
  return {
    list,
    body: [
      {
        kind: "match",
        pattern: from.pattern,
        where: and(from.where, filter, authFilter(ctx, member, x, "READ")),
      },
      { kind: "with", items: [{ expr: v(x) }], orderBy: sortExprs, limit },
      ...projection.pre,
      { kind: "return", items: [{ expr: fn("collect", tagged), alias: list }] },
    ],
  };
}

/** The merged order: requested fields, then type name, then key. */
function mergedOrder(item: string, sort: MemberSort[]): SortItem[] {
  return [
    ...sort.map((k, i) => ({
      expr: prop(v(item), `__s${i}`),
      direction: k.direction,
    })),
    { expr: prop(v(item), "__typename"), direction: "ASC" as const },
    { expr: prop(v(item), "__key"), direction: "ASC" as const },
  ];
}

/** `events(where, sort, limit)` over an interface's implementations or a union's members. */
export function compileAbstractRoot(
  ctx: CompileContext,
  abstract: AbstractType,
  args: Args,
  fieldNodes: readonly FieldNode[],
): CompiledRead {
  const limitValue = resolveLimit(args["limit"], abstract.limit, "limit");
  const sort = resolveAbstractSort(ctx, abstract, args["sort"]);
  const limit = bind(ctx, limitValue);
  const sets = subSelections(fieldNodes);
  const calls: Clause[] = [];
  const lists: string[] = [];
  for (const name of abstract.members) {
    const member = ctx.model.nodes.get(name)!;
    const slice = memberList(
      ctx,
      abstract,
      member,
      (x) => ({
        pattern: {
          start: { variable: x, labels: [member.labels[0]!] },
          hops: [],
        },
        where: undefined,
      }),
      args["where"] as Record<string, unknown> | undefined,
      sort,
      limit,
      sets,
      limitValue,
    );
    if (!slice) continue;
    ctx.cost += rootFilterCost(
      ctx,
      member,
      memberWhere(abstract, member.name, args["where"] as Where | undefined),
      limitValue,
    );
    calls.push({ kind: "call", imports: [], body: slice.body });
    lists.push(slice.list);
  }
  const clauses: Clause[] = [
    ...calls,
    {
      kind: "unwind",
      expr:
        lists.length === 0
          ? { kind: "list", items: [] }
          : lists.map((l) => v(l)).reduce((a, b) => bin("+", a, b)),
      alias: "this",
    },
    {
      kind: "with",
      items: [{ expr: v("this") }],
      orderBy: mergedOrder("this", sort),
      limit,
    },
    { kind: "return", items: [{ expr: v("this"), alias: "this" }] },
  ];
  return finish(
    ctx,
    clauses,
    ["this"],
    ([r]) => r!.rows.map((row) => row["this"]),
    undefined,
  );
}

/** A relationship field whose target is an interface or union. */
function projectPolymorphic(
  ctx: CompileContext,
  parent: string,
  rel: RelationshipField,
  key: string,
  args: Args,
  sets: SelectionSetNode[],
  rows: number,
): Projection {
  const abstract = ctx.model.abstracts.get(rel.target)!;
  ctx.reads.relationships.add(rel.type);
  const limitValue = rel.list
    ? resolveLimit(args["limit"], rel.limit ?? abstract.limit, "limit")
    : 1;
  const sort = rel.list ? resolveAbstractSort(ctx, abstract, args["sort"]) : [];
  const limit = bind(ctx, limitValue);
  const body: Clause[] = [];
  const lists: string[] = [];
  for (const field of memberFields(ctx.model, rel)) {
    const member = ctx.model.nodes.get(field.target)!;
    const slice = memberList(
      ctx,
      abstract,
      member,
      (x) => ({
        pattern: relationshipPattern(parent, field, member.labels[0]!, x),
        where: undefined,
      }),
      args["where"] as Record<string, unknown> | undefined,
      sort,
      limit,
      sets,
      rows * limitValue,
    );
    if (!slice) continue;
    body.push({ kind: "call", imports: [parent], body: slice.body });
    lists.push(slice.list);
  }
  if (lists.length === 0) {
    return {
      pre: [],
      expr: rel.list ? { kind: "list", items: [] } : lit(null),
    };
  }
  const item = freshVar(ctx, `${parent}_${key}_item`);
  const out = freshVar(ctx, `${parent}_${key}`);
  body.push(
    {
      kind: "unwind",
      expr: lists.map((l) => v(l)).reduce((a, b) => bin("+", a, b)),
      alias: item,
    },
    {
      kind: "with",
      items: [{ expr: v(item) }],
      orderBy: mergedOrder(item, sort),
      limit,
    },
    ...(rel.list
      ? [
          {
            kind: "return" as const,
            items: [{ expr: fn("collect", v(item)), alias: out }],
          },
        ]
      : collectOne(v(item), out)),
  );
  return { pre: [{ kind: "call", imports: [parent], body }], expr: v(out) };
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
/** The order search results come in, as a cursor signature. */
const SEARCH_SORT = "score:DESC,key:ASC";

/**
 * A search as a connection (`connection: true`): pages forward by keyset
 * on (score DESC, key ASC). A vector index returns its top candidates
 * before any filter, so vector connections page within a fixed pool of
 * `4 × @limit(max:)` candidates.
 */
export function compileSearch(
  ctx: CompileContext,
  node: NodeType,
  index: SearchIndex,
  args: Args,
  fieldNodes: readonly FieldNode[],
  connection = false,
): CompiledRead {
  checkAuthentication(ctx, node, "READ");
  const label = node.labels[0]!;
  ctx.reads.labels.add(label);
  const limit = connection
    ? resolveLimit(args["first"], node.limit, "first")
    : resolveLimit(args["limit"], node.limit, "limit");
  const after = connection
    ? (args["after"] as string | null | undefined)
    : null;
  const cursor =
    after != null
      ? decodeCursor(after, SEARCH_SORT, 2, ctx.model.cursorSecret)
      : undefined;
  const keyset = cursor
    ? or(
        bin("<", v("score"), bind(ctx, cursor[0])),
        and(
          bin("=", v("score"), bind(ctx, cursor[0])),
          bin(">", prop(v("this"), node.key.property), bind(ctx, cursor[1])),
        ),
      )
    : undefined;
  // The index call and its filters, for the page (with the keyset) and
  // for `totalCount` (every match, in its own parameter scope).
  const matches = (c: CompileContext, keyset: Expr | undefined): Clause[] => {
    const where = and(
      compileNodeWhere(c, node, "this", args["where"] as Where | undefined),
      authFilter(c, node, "this", "READ"),
      keyset,
    );
    const clauses: Clause[] = [];
    if (index.kind === "fulltext") {
      const query = args["query"] as string;
      clauses.push({
        kind: "procedure",
        procedure: "db.index.fulltext.queryNodes",
        args: [bind(c, index.name), bind(c, query)],
        yields: [{ item: "node", alias: "this" }, { item: "score" }],
        where,
      });
      return clauses;
    }
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
    const pool = Math.max(node.limit.max * 4, 1);
    const candidates = connection
      ? pool
      : Math.min((limit + (to != null ? 1 : 0)) * (where ? 4 : 1), pool);
    let source: Expr;
    let exclude: Expr | undefined;
    if (to != null) {
      checkFieldAuthentication(c, node.name, index.field);
      clauses.push({
        kind: "match",
        pattern: { start: { variable: "anchor", labels: [label] }, hops: [] },
        // The anchor's vector is read to rank the others: it must be
        // readable, or an unreadable node's vector would leak through them.
        where: and(
          bin("=", prop(v("anchor"), node.key.property), bind(c, to)),
          authFilter(c, node, "anchor", "READ"),
          coalesceFalse(authValidate(c, node, "anchor", "READ", "BEFORE")),
          coalesceFalse(fieldValidate(c, node, index.field, "anchor", "READ")),
        ),
      });
      source = prop(v("anchor"), index.field.property);
      exclude = bin(
        "<>",
        prop(v("this"), node.key.property),
        prop(v("anchor"), node.key.property),
      );
    } else {
      source = bind(c, vector);
    }
    clauses.push({
      kind: "procedure",
      procedure: "db.index.vector.queryNodes",
      args: [bind(c, index.name), bind(c, candidates), source],
      yields: [{ item: "node", alias: "this" }, { item: "score" }],
      where: and(exclude, where),
    });
    return clauses;
  };
  const sel = connection
    ? searchConnectionSelections(ctx, node, fieldNodes)
    : undefined;
  if (sel && !sel.page) {
    // Only `totalCount`: no page is read.
    return searchCount(ctx, node, matches, (rows) => ({
      __rows: [],
      __first: limit,
      __after: cursor !== undefined,
      __backward: false,
      __sort: SEARCH_SORT,
      __totalCount: rows,
    }));
  }
  const clauses = matches(ctx, keyset);
  const selections = sel
    ? sel.node
    : searchNodeSelections(ctx, node, fieldNodes);
  const projection = projectNode(ctx, node, "this", selections, limit);
  clauses.push(
    {
      kind: "with",
      items: [{ expr: v("this") }, { expr: v("score") }],
      orderBy: [
        { expr: v("score"), direction: "DESC" },
        { expr: prop(v("this"), node.key.property), direction: "ASC" },
      ],
      // One more than the page, to tell whether another follows.
      limit: bind(ctx, connection ? limit + 1 : limit),
    },
    ...projection.pre,
    {
      kind: "return",
      items: [
        { expr: projection.expr, alias: "node" },
        { expr: v("score"), alias: "score" },
        ...(connection
          ? [
              {
                expr: {
                  kind: "list" as const,
                  items: [v("score"), prop(v("this"), node.key.property)],
                },
                alias: "__cursor",
              },
            ]
          : []),
      ],
    },
  );
  if (sel) {
    const count = sel.totalCount
      ? searchCount(ctx, node, matches, () => undefined)
      : undefined;
    const compiled = finish(
      ctx,
      clauses,
      ["node", "score", "__cursor"],
      ([r, c]): RawConnection => {
        const row = c?.rows[0];
        if (c) assertNoneDenied(row);
        return {
          __rows: r!.rows.map((row) => ({
            node: row["node"],
            score: row["score"],
            __cursor: row["__cursor"] as unknown[],
          })),
          __first: limit,
          __after: cursor !== undefined,
          __backward: false,
          __sort: SEARCH_SORT,
          ...(c ? { __totalCount: Number(row?.["totalCount"] ?? 0) } : {}),
        };
      },
      undefined,
      count?.statements ?? [],
    );
    if (count) compiled.cost += count.cost;
    return compiled;
  }
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

/**
 * `totalCount` of a search connection: every match after \`where\` and the
 * read rules (a vector search counts within its candidate window), in its
 * own parameter scope. A row the READ rules would refuse makes the count
 * FORBIDDEN, as on other connections.
 */
function searchCount(
  ctx: CompileContext,
  node: NodeType,
  matches: (c: CompileContext, keyset: Expr | undefined) => Clause[],
  shape: (count: number) => RawConnection | undefined,
): CompiledRead {
  const statsCtx: CompileContext = {
    ...ctx,
    params: {},
    vars: new Set(["this"]),
    computed: new Map(),
  };
  const clauses: Clause[] = [
    ...matches(statsCtx, undefined),
    {
      kind: "return",
      items: [
        { expr: fn("count", v("this")), alias: "totalCount" },
        ...deniedItem(statsCtx, node, "this"),
      ],
    },
  ];
  const compiled = finish(
    statsCtx,
    clauses,
    ["totalCount"],
    ([r]) => {
      const row = r!.rows[0];
      assertNoneDenied(row);
      return shape(Number(row?.["totalCount"] ?? 0));
    },
    undefined,
  );
  compiled.cost = 1;
  return compiled;
}

/** The `edges { node }` selections of a search connection. */
function searchConnectionSelections(
  ctx: CompileContext,
  node: NodeType,
  fieldNodes: readonly FieldNode[],
): ReturnType<typeof connectionSelections> {
  const connType = ctx.schema.getType(
    names.searchConnection(node.name),
  ) as GraphQLObjectType;
  const edgeType = ctx.schema.getType(
    names.searchEdge(node.name),
  ) as GraphQLObjectType;
  return connectionSelections(
    ctx,
    connType,
    edgeType,
    subSelections(fieldNodes),
    node,
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
  checkListArguments(ctx, field, args);
  const texts = new Map<string, string>();
  for (const p of field.params) {
    if (p === "viewer") {
      const expr = viewerKey(ctx);
      texts.set(
        p,
        expr.kind === "param" ? printExpr(expr) : `(${printExpr(expr)})`,
      );
      continue;
    }
    if (p === "jwt") noteClaim(ctx, "", ctx.jwt, false);
    // An explicit null for an argument with an SDL default takes the
    // default, as the library's own `limit` arguments do.
    const declared = field.args.find((a) => a.name === p);
    const value =
      p === "jwt"
        ? (ctx.jwt ?? null)
        : (args[p] ?? declared?.defaultValue ?? null);
    texts.set(p, printExpr(bind(ctx, value)));
  }
  let out = "";
  let last = 0;
  for (const ref of scanParams(field.statement)) {
    out +=
      field.statement.slice(last, ref.start) +
      (texts.get(ref.name) ?? `$${ref.name}`);
    last = ref.end;
  }
  return out + field.statement.slice(last);
}

/** Default `maxListArgument`: the most items a @cypher list argument takes. */
export const MAX_LIST_ARGUMENT = 1000;

/**
 * List arguments of a @cypher field hold at most `@size(max:)` items, or
 * `maxListArgument` without it: the statement sees the input as is, so
 * the cap is checked before it runs. Every level of a nested list counts.
 */
function checkListArguments(
  ctx: CompileContext,
  field: CypherField,
  args: Args,
): void {
  for (const a of field.args) {
    if (!a.type.list) continue;
    const max = a.maxItems ?? ctx.maxListArgument ?? MAX_LIST_ARGUMENT;
    const tooLong = (value: unknown): boolean =>
      Array.isArray(value) && (value.length > max || value.some(tooLong));
    if (tooLong(args[a.name])) {
      throw requestError(
        "BAD_USER_INPUT",
        `${field.owner}.${field.name}: argument ${a.name} takes at most ${max} items`,
      );
    }
  }
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
  /** Computed values the following clauses must carry. */
  carry: string[];
} {
  checkAuthentication(ctx, node, "READ");
  const label = node.labels[0]!;
  ctx.reads.labels.add(label);
  const clauses: Clause[] = [];
  let anchor: Expr | undefined;
  let expectation: Omit<SeekExpectation, "statement"> | undefined;
  const rest = where ?? undefined;

  // Computed (@cypher) fields the filter or sort uses: each runs once per
  // node, in a CALL after the MATCH, and the filter moves after them.
  const computed = computedFields(node, rest, sort);
  const carry: string[] = [];
  const computedClauses: Clause[] = [];
  for (const field of computed) {
    const projection = projectCypher(ctx, "this", field, field.name, {}, [], 1);
    const out = (projection.expr as { name: string }).name;
    ctx.computed.set(`this\0${field.name}`, out);
    for (const k of sort) if (k.field.computedBy === field) k.computed = out;
    computedClauses.push(...projection.pre);
    carry.push(out);
  }

  const predicate = compileNodeWhere(ctx, node, "this", rest);
  if (computed.length === 0) {
    expectation ??= rest ? seekFromWhere(node, rest) : undefined;
  }

  // Nothing to seek on, but a relationship filter names a related node by
  // key: start from that node and expand, instead of scanning the label.
  // The filter itself stays in the WHERE, so the result is unchanged.
  let related: RelatedAnchor | undefined;
  if (!expectation && !anchor && rest && computed.length === 0) {
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
      computed.length === 0 &&
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
    computed.length === 0 &&
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
      computed.length === 0 ? predicate : undefined,
      computed.length === 0 ? keyset : undefined,
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
  if (computed.length > 0) {
    clauses.push(...computedClauses, {
      kind: "with",
      items: [{ expr: v("this") }, ...carry.map((c) => ({ expr: v(c) }))],
      where: and(predicate, keyset),
    });
  }
  return { clauses, expectation, carry };
}

/** The computed (`@cypher`) fields a root filter or sort refers to. */
function computedFields(
  node: NodeType,
  where: Where | undefined,
  sort: SortKey[],
): CypherField[] {
  const out = new Set<CypherField>();
  const walk = (w: Where | undefined) => {
    for (const [key, value] of Object.entries(w ?? {})) {
      if (value === null || value === undefined) continue;
      if (key === "AND" || key === "OR") {
        for (const item of value as Where[]) walk(item);
      } else if (key === "NOT") {
        walk(value as Where);
      } else {
        const f = node.fields.get(key);
        if (f?.kind === "cypher" && f.computed) out.add(f);
      }
    }
  };
  walk(where);
  for (const k of sort) if (k.field.computedBy) out.add(k.field.computedBy);
  return [...out];
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
    // Through an interface or union there is no single key to seek on.
    if (ctx.model.abstracts.has(rel.target)) continue;
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
      // `IN` seeks once per distinct value (E18).
      seek = bin(
        "IN",
        prop(v(a), target.key.property),
        bind(ctx, dedupe(inList as unknown[])),
      );
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
  props?: RelationshipPropertiesType,
): SortKey[] {
  const keys: SortKey[] = [];
  const one = (item: Record<string, unknown>): [string, unknown] => {
    const entries = Object.entries(item).filter(([, d]) => d != null);
    if (entries.length !== 1) {
      throw requestError(
        "BAD_USER_INPUT",
        "each `sort` item names exactly one field",
      );
    }
    return entries[0]!;
  };
  for (const item of (value as Array<Record<string, unknown>> | null) ?? []) {
    const [fieldName, direction] = one(item);
    if (fieldName === "edge" && props) {
      const [propName, dir] = one(direction as Record<string, unknown>);
      const field = props.fields.get(propName)!;
      checkPropertyAccess(ctx, props.name, field, "READ");
      if (keys.some((k) => k.edge && k.field === field)) {
        throw requestError(
          "BAD_USER_INPUT",
          `\`sort\` names edge.${propName} more than once`,
        );
      }
      keys.push({ field, direction: dir as "ASC" | "DESC", edge: true });
      continue;
    }
    const declared = node.fields.get(fieldName)!;
    const field =
      declared.kind === "cypher"
        ? declared.computed!
        : (declared as ScalarField);
    // Ordering by a field reveals it, and cursors carry its values.
    checkFieldAuthentication(ctx, node.name, declared);
    refuseRowRules(ctx, node, field, "sort by");
    if (keys.some((k) => !k.edge && k.field === field)) {
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
  props?: RelationshipPropertiesType,
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
  const requested = resolveSort(ctx, node, args["sort"], true, props);
  const signature = sortSignature(requested);
  const raw = (backward ? args["before"] : args["after"]) as
    | string
    | null
    | undefined;
  const cursor =
    raw != null
      ? decodeCursor(raw, signature, requested.length, ctx.model.cursorSecret)
      : undefined;
  const sort = backward
    ? requested.map((k) => ({
        ...k,
        direction: k.direction === "ASC" ? ("DESC" as const) : ("ASC" as const),
      }))
    : requested;
  return { first, sort, signature, cursor, backward };
}

function sortSignature(sort: SortKey[]): string {
  return sort
    .map((k) => `${k.edge ? "edge." : ""}${k.field.name}:${k.direction}`)
    .join(",");
}

function sortItems(
  variable: string,
  sort: SortKey[],
  edge?: string,
): SortItem[] {
  return sort.map((k) => ({
    expr: keyExpr(k, variable, edge),
    direction: k.direction,
  }));
}

function cursorValues(variable: string, sort: SortKey[], edge?: string): Expr {
  return {
    kind: "list",
    items: sort.map((k) => keyExpr(k, variable, edge)),
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
  edge?: string,
): Expr | undefined {
  const branches: Expr[] = [];
  const equal: Expr[] = [];
  let lead: Expr | undefined;
  sort.forEach((k, i) => {
    const target = keyExpr(k, variable, edge);
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

const requiresCache = new Map<string, SelectionSetNode>();

/** A `requires` string as a selection set, parsed once. */
function requiresSelection(requires: string): SelectionSetNode {
  const cached = requiresCache.get(requires);
  if (cached) return cached;
  const op = parse(`{ ${requires} }`).definitions[0] as OperationDefinitionNode;
  requiresCache.set(requires, op.selectionSet);
  return op.selectionSet;
}

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
function guardedValue(
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
      value: projectProperties(ctx, props.name, r, sel.properties, edgeRead),
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

/**
 * A node `variable` of an interface or union, projected as the member its
 * label says, with `__typename`. `where` keeps the nodes of some member
 * the caller may read; others (and non-members) are dropped.
 */
function projectAbstract(
  ctx: CompileContext,
  abstract: AbstractType,
  variable: string,
  sets: SelectionSetNode[],
  rows: number,
): { pre: Clause[]; where: Expr; expr: Expr } {
  const pre: Clause[] = [];
  const branches: Array<{ when: Expr; then: Expr }> = [];
  for (const name of abstract.members) {
    const member = ctx.model.nodes.get(name)!;
    checkAuthentication(ctx, member, "READ");
    ctx.reads.labels.add(member.labels[0]!);
    const projection = projectNode(ctx, member, variable, sets, rows);
    pre.push(...projection.pre);
    const expr = projection.expr as Extract<Expr, { kind: "mapProjection" }>;
    branches.push({
      when:
        and(
          { kind: "hasLabels", variable, labels: [member.labels[0]!] },
          coalesceFalse(authFilter(ctx, member, variable, "READ")),
        ) ?? lit(true),
      then: {
        ...expr,
        entries: [
          ...expr.entries,
          { kind: "entry", key: "__typename", value: lit(member.name) },
        ],
      },
    });
  }
  const chain = branches.reduceRight<Expr>(
    (otherwise, b) => ({
      kind: "case",
      when: b.when,
      then: b.then,
      else: otherwise,
    }),
    lit(null),
  );
  return {
    pre,
    where: or(...branches.map((b) => b.when)) ?? lit(false),
    expr: chain,
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
  } else if (field.abstract) {
    const x = freshVar(ctx, `${out}_node`);
    const abstract = ctx.model.abstracts.get(field.abstract)!;
    const per = field.type.list ? abstract.limit.default : 1;
    const nested = projectAbstract(ctx, abstract, x, sets, rows * per);
    body.push(
      {
        kind: "with",
        items: [{ expr: v(column), alias: x }],
        where: nested.where,
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
  guard?: Expr | false,
): Expr {
  const props = ctx.model.relationshipProperties.get(propsType)!;
  const objType = ctx.schema.getType(propsType) as GraphQLObjectType;
  const entries: ProjectionEntry[] = [];
  for (const [key, nodes] of collectFields(ctx, objType, sets)) {
    const field = props.fields.get(nodes[0]!.name.value);
    if (!field) continue;
    // Rules on a relationship property test claims only: a request they
    // refuse reads it as FORBIDDEN, on every row that has it.
    if (propertyAccess(ctx, field, "READ") !== "allowed") {
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
function connectionSelections(
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

/** A rule condition as a WHERE predicate: an unknown result excludes. */
function coalesceFalse(e: Expr | undefined): Expr | undefined {
  return e && fn("coalesce", e, lit(false));
}
