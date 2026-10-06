// Root fields over one node type: the list, the lookup by @key and the
// connection (the aggregate and grouped roots are in aggregates.ts).

import type { FieldNode, GraphQLObjectType, SelectionSetNode } from "graphql";
import { rootFilterCost } from "../cost.js";
import type { Statement } from "../../driver.js";
import type { NodeType } from "../../model/types.js";
import { names } from "../../schema/names.js";
import { authFilter, checkAuthentication } from "../auth.js";
import { bind, type CompileContext } from "../context.js";
import {
  and,
  bin,
  fn,
  printClauses,
  prop,
  v,
  type Clause,
  type Expr,
} from "../cypher.js";
import { collectFields, subSelections } from "../selection.js";
import type {
  RootKind,
  SeekExpectation,
  CompiledRead,
  Args,
  Where,
  RawConnection,
} from "./types.js";
import { finish, readSet, deniedItem, assertNoneDenied } from "./common.js";
import {
  resolveLimit,
  resolveSort,
  resolvePage,
  sortItems,
  cursorValues,
} from "./sort.js";
import { rootMatch } from "./match.js";
import {
  type WantedAggregate,
  aggregateReturn,
  compileAggregate,
  compileGrouped,
} from "./aggregates.js";
import { projectNode } from "./project.js";
import { connectionSelections } from "./connection.js";

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
