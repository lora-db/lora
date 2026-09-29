// `where` inputs → predicates. Specialised to the input actually supplied:
// an absent or null operator adds nothing to the statement (rule 7), so
// each variant keeps its predicates visible to the planner.

import { requestError } from "../errors.js";
import type {
  NodeType,
  RelationshipField,
  RelationshipPropertiesType,
  ScalarField,
} from "../model/types.js";
import {
  and,
  bin,
  fn,
  lit,
  not,
  or,
  prop,
  v,
  type BinaryOp,
  type Expr,
  type Pattern,
} from "./cypher.js";
import { toLoraPoint } from "../model/points.js";
import {
  authFilter,
  checkAuthentication,
  checkFieldAuthentication,
} from "./auth.js";
import { bind, freshVar, type CompileContext } from "./context.js";

type Where = Record<string, unknown>;

const SCALAR_OPS: Record<string, BinaryOp> = {
  eq: "=",
  in: "IN",
  lt: "<",
  lte: "<=",
  gt: ">",
  gte: ">=",
  contains: "CONTAINS",
  startsWith: "STARTS WITH",
  endsWith: "ENDS WITH",
};

const COUNT_OPS: Record<string, BinaryOp> = {
  eq: "=",
  lt: "<",
  lte: "<=",
  gt: ">",
  gte: ">=",
};

export function compileNodeWhere(
  ctx: CompileContext,
  node: NodeType,
  variable: string,
  where: Where | null | undefined,
): Expr | undefined {
  return compileWhere(ctx, where, variable, (key) => {
    const f = node.fields.get(key);
    if (!f || f.kind === "cypher") return undefined;
    // Filtering on a field reveals it: the same rule as reading it.
    checkFieldAuthentication(ctx, node.name, f);
    return f;
  });
}

export function compilePropsWhere(
  ctx: CompileContext,
  props: RelationshipPropertiesType,
  variable: string,
  where: Where | null | undefined,
): Expr | undefined {
  return compileWhere(ctx, where, variable, (key) => props.fields.get(key));
}

function compileWhere(
  ctx: CompileContext,
  where: Where | null | undefined,
  variable: string,
  lookup: (key: string) => ScalarField | RelationshipField | undefined,
): Expr | undefined {
  if (!where) return undefined;
  const parts: Array<Expr | undefined> = [];
  for (const [key, value] of Object.entries(where)) {
    if (value === null || value === undefined) continue;
    if (key === "AND") {
      for (const w of value as Where[]) {
        parts.push(compileWhere(ctx, w, variable, lookup));
      }
    } else if (key === "OR") {
      const branches = (value as Where[]).map((w) =>
        compileWhere(ctx, w, variable, lookup),
      );
      // An empty branch matches everything, so the OR is vacuous.
      if (branches.length > 0 && branches.every((b) => b !== undefined)) {
        parts.push(or(...branches));
      }
    } else if (key === "NOT") {
      const inner = compileWhere(ctx, value as Where, variable, lookup);
      if (inner) parts.push(not(inner));
    } else {
      const field = lookup(key);
      if (!field) continue;
      parts.push(
        field.kind === "scalar"
          ? scalarPredicate(
              ctx,
              prop(v(variable), field.property),
              value as Where,
            )
          : relationshipPredicate(ctx, variable, field, value as Where),
      );
    }
  }
  return and(...parts);
}

function scalarPredicate(
  ctx: CompileContext,
  target: Expr,
  ops: Where,
): Expr | undefined {
  const parts: Expr[] = [];
  for (const [op, value] of Object.entries(ops)) {
    if (value === null || value === undefined) continue;
    if (op === "withinBBox") {
      const box = value as { lowerLeft: unknown; upperRight: unknown };
      parts.push(
        fn(
          "geo.within_bbox",
          target,
          bind(ctx, toLoraPoint(box.lowerLeft)),
          bind(ctx, toLoraPoint(box.upperRight)),
        ),
      );
      continue;
    }
    if (op === "distance") {
      const d = value as { from: unknown; lte: number };
      parts.push(
        bin(
          "<=",
          fn("geo.distance", target, bind(ctx, toLoraPoint(d.from))),
          bind(ctx, d.lte),
        ),
      );
      continue;
    }
    const cypherOp = SCALAR_OPS[op];
    if (!cypherOp) continue;
    parts.push(bin(cypherOp, target, bind(ctx, value)));
  }
  return and(...parts);
}

/** `(from)-[:TYPE]->(to:Target)` for a relationship field. */
export function relationshipPattern(
  from: string,
  rel: RelationshipField,
  targetLabel: string,
  to: string,
  relVariable?: string,
): Pattern {
  return {
    start: { variable: from, labels: [] },
    hops: [
      {
        rel: {
          ...(relVariable !== undefined ? { variable: relVariable } : {}),
          type: rel.type,
          direction: rel.direction,
        },
        node: { variable: to, labels: [targetLabel] },
      },
    ],
  };
}

function relationshipPredicate(
  ctx: CompileContext,
  variable: string,
  rel: RelationshipField,
  value: Where,
): Expr | undefined {
  const target = ctx.model.nodes.get(rel.target)!;
  checkAuthentication(ctx, target, "READ");
  const label = target.labels[0]!;
  ctx.reads.labels.add(label);
  ctx.reads.relationships.add(rel.type);

  // size([(this)-[:T]->(x:Target) WHERE pred | 1]) — LoraDB has no
  // EXISTS { } subquery, and a pattern comprehension beats OPTIONAL MATCH.
  const count = (x: string, pred: Expr | undefined): Expr =>
    fn("size", {
      kind: "comprehension",
      pattern: relationshipPattern(variable, rel, label, x),
      where: pred,
      projection: lit(1),
    });
  // Related nodes the reader may not see do not count, in any quantifier.
  const visible = (x: string) => authFilter(ctx, target, x, "READ");
  const matches = (where: Where | undefined): Expr => {
    const x = freshVar(ctx, `${variable}_${rel.name}`);
    return count(x, and(visible(x), compileNodeWhere(ctx, target, x, where)));
  };
  // A quantifier over an empty (or all-null) filter is left out, like any
  // absent filter; `count: { gt: 0 }` asks for "has any".
  const nonEmpty = (where: Where): boolean => {
    const x = freshVar(ctx, `${variable}_${rel.name}_probe`);
    const probe = { ...ctx, params: {}, vars: new Set(ctx.vars) };
    return compileNodeWhere(probe, target, x, where) !== undefined;
  };
  // Related nodes failing `where`; undefined when `where` is empty.
  const failing = (where: Where): Expr | undefined => {
    const x = freshVar(ctx, `${variable}_${rel.name}`);
    const pred = compileNodeWhere(ctx, target, x, where);
    return (
      pred && count(x, and(visible(x), not(fn("coalesce", pred, lit(false)))))
    );
  };

  if (!rel.list) {
    // A single relationship filters by its target directly.
    return nonEmpty(value) ? bin(">", matches(value), lit(0)) : undefined;
  }

  const parts: Array<Expr | undefined> = [];
  for (const [quantifier, inner] of Object.entries(value)) {
    if (inner === null || inner === undefined) continue;
    const w = inner as Where;
    if (quantifier !== "count" && !nonEmpty(w)) continue;
    switch (quantifier) {
      case "some":
        parts.push(bin(">", matches(w), lit(0)));
        break;
      case "none":
        parts.push(bin("=", matches(w), lit(0)));
        break;
      case "single":
        parts.push(bin("=", matches(w), lit(1)));
        break;
      case "all":
        // Every related node matches: none fails. Unknown (null) counts
        // as failing, so `all` never holds on missing properties. An
        // empty `all` holds vacuously.
        {
          const f = failing(w);
          if (f) parts.push(bin("=", f, lit(0)));
        }
        break;
      case "count": {
        const total = matches(undefined);
        for (const [op, n] of Object.entries(w)) {
          if (n === null || n === undefined) continue;
          const cypherOp = COUNT_OPS[op];
          if (!cypherOp) continue;
          if (!Number.isInteger(n) || (n as number) < 0) {
            throw requestError(
              "BAD_USER_INPUT",
              `${rel.name}.count.${op} must be a non-negative integer`,
            );
          }
          parts.push(bin(cypherOp, total, bind(ctx, n)));
        }
        break;
      }
    }
  }
  return and(...parts);
}
