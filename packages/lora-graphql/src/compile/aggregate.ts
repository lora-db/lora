// Aggregates over a list expression, e.g. the ages of a node's followers
// as a pattern comprehension. Written with `reduce` rather than aggregate
// functions, so they work inside WHERE and map projections, per parent
// row, without a subquery (and without LoraDB's nested-aggregate bug).

import { freshVar, type CompileContext } from "./context.js";
import { bin, fn, isNull, lit, v, type Expr } from "./cypher.js";

export type ListAggregate =
  | "min"
  | "max"
  | "sum"
  | "avg"
  | "count"
  // Strings: the shortest and longest value, and length statistics.
  | "shortest"
  | "longest"
  | "shortestLength"
  | "longestLength"
  | "averageLength";

export interface AggregateOptions {
  /**
   * The values are durations: sums start from the first value rather than
   * 0, and an average divides the sum (LoraDB's own `sum` / `avg` / `max`
   * over durations are wrong, see E25).
   */
  duration?: boolean;
}

export function listAggregate(
  ctx: CompileContext,
  kind: ListAggregate,
  list: Expr,
  options: AggregateOptions = {},
): Expr {
  if (
    kind === "shortestLength" ||
    kind === "longestLength" ||
    kind === "averageLength"
  ) {
    const s = freshVar(ctx, "agg_s");
    const lengths: Expr = {
      kind: "listComprehension",
      variable: s,
      list,
      where: isNull(v(s), true),
      projection: fn("size", v(s)),
    };
    const of = {
      shortestLength: "min",
      longestLength: "max",
      averageLength: "avg",
    } as const;
    return listAggregate(ctx, of[kind], lengths);
  }
  const x = freshVar(ctx, "agg_v");
  // Nulls take no part, as in Cypher's aggregate functions.
  const values: Expr = {
    kind: "listComprehension",
    variable: x,
    list,
    where: isNull(v(x), true),
    projection: undefined,
  };
  const acc = freshVar(ctx, "agg_acc");
  const item = freshVar(ctx, "agg_item");
  const fold = (init: Expr, step: Expr): Expr => ({
    kind: "reduce",
    accumulator: acc,
    init,
    variable: item,
    list: values,
    step,
  });
  switch (kind) {
    case "count":
      return fn("size", values);
    case "min":
    case "max":
      return fold(lit(null), {
        kind: "case",
        when: {
          kind: "binary",
          op: "OR",
          left: isNull(v(acc)),
          right: bin(kind === "min" ? "<" : ">", v(item), v(acc)),
        },
        then: v(item),
        else: v(acc),
      });
    case "shortest":
    case "longest":
      return fold(lit(null), {
        kind: "case",
        when: {
          kind: "binary",
          op: "OR",
          left: isNull(v(acc)),
          right: bin(
            kind === "shortest" ? "<" : ">",
            fn("size", v(item)),
            fn("size", v(acc)),
          ),
        },
        then: v(item),
        else: v(acc),
      });
    case "sum":
      return options.duration
        ? durationSum()
        : fold(lit(0), bin("+", v(acc), v(item)));
    case "avg":
      return {
        kind: "case",
        when: bin("=", fn("size", values), lit(0)),
        then: lit(null),
        else: bin(
          "/",
          options.duration
            ? durationSum()
            : fn("toFloat", fold(lit(0), bin("+", v(acc), v(item)))),
          fn("size", values),
        ),
      };
  }

  function durationSum(): Expr {
    return fold(lit(null), {
      kind: "case",
      when: isNull(v(acc)),
      then: v(item),
      else: bin("+", v(acc), v(item)),
    });
  }
}

/**
 * The number of distinct values in a list: related nodes counted once
 * even when several relationships lead to the same node.
 */
export function distinctCount(ctx: CompileContext, keys: Expr): Expr {
  const acc = freshVar(ctx, "seen");
  const k = freshVar(ctx, "seen_key");
  return fn("size", {
    kind: "reduce",
    accumulator: acc,
    init: { kind: "list", items: [] },
    variable: k,
    list: keys,
    step: {
      kind: "case",
      when: bin("IN", v(k), v(acc)),
      then: v(acc),
      else: bin("+", v(acc), { kind: "list", items: [v(k)] }),
    },
  });
}
