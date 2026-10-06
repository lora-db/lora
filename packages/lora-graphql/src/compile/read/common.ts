// Helpers every read shares: finishing a compiled read, and the READ rule
// checks that aggregates, counts and sorts apply.

import type { Statement } from "../../driver.js";
import { requestError } from "../../errors.js";
import type { NodeType, ScalarField } from "../../model/types.js";
import { authValidate, fieldValidate, maskSettled } from "../auth.js";
import type { CompileContext } from "../context.js";
import { fn, lit, printClauses, v, type Clause, type Expr } from "../cypher.js";
import type { SeekExpectation, ReadSet, CompiledRead } from "./types.js";

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
    ...(ctx.reads.opaque ? { opaque: true } : {}),
  };
}

/**
 * Sorting or aggregating over a field with row-level READ rules would
 * reveal values of rows the rules hide: refused unless the claims alone
 * already settle the rules.
 */
export function refuseRowRules(
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

/**
 * `sum(CASE WHEN <READ validate rule> THEN 0 ELSE 1 END) AS __denied`:
 * aggregates and counts cover nodes a READ validate rule rejects, so they
 * fail like reading those nodes would.
 */
export function deniedItem(
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

export function assertNoneDenied(
  row: Record<string, unknown> | undefined,
): void {
  if (Number(row?.["__denied"] ?? 0) > 0) {
    throw requestError("FORBIDDEN", "not allowed to read some of these nodes");
  }
}

/**
 * `RETURN head(collect(x)) AS out`, written as two clauses: LoraDB 0.15
 * does not treat an aggregate nested inside another call as aggregating.
 */
export function collectOne(value: Expr, out: string): Clause[] {
  return [
    { kind: "with", items: [{ expr: fn("collect", value), alias: out }] },
    { kind: "return", items: [{ expr: fn("head", v(out)), alias: out }] },
  ];
}

/** A rule condition as a WHERE predicate: an unknown result excludes. */
export function coalesceFalse(e: Expr | undefined): Expr | undefined {
  return e && fn("coalesce", e, lit(false));
}
