// Aggregates: the `<plural>Aggregate` and `<plural>Grouped` root fields,
// and the aggregate statement a root connection shares with them.

import {
  getNamedType,
  type GraphQLObjectType,
  type SelectionSetNode,
} from "graphql";
import { rootFilterCost } from "../cost.js";
import { requestError } from "../../errors.js";
import { listAggregate, type ListAggregate } from "../aggregate.js";
import type { NodeType, ScalarField } from "../../model/types.js";
import { names } from "../../schema/names.js";
import { checkFieldAuthentication } from "../auth.js";
import { bind, freshVar, type CompileContext } from "../context.js";
import { fn, prop, v, type Clause, type Expr } from "../cypher.js";
import { collectFields, subSelections } from "../selection.js";
import type { CompiledRead, Args, Where } from "./types.js";
import {
  finish,
  refuseRowRules,
  deniedItem,
  assertNoneDenied,
} from "./common.js";
import { resolveLimit } from "./sort.js";
import { rootMatch } from "./match.js";

export const AGGREGATES = new Set([
  "min",
  "max",
  "avg",
  "sum",
  "shortest",
  "longest",
]);

export interface WantedAggregate {
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
export function aggregateReturn(
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

export function compileAggregate(
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
export function compileGrouped(
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
