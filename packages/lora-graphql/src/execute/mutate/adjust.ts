// `adjust` inputs: arithmetic on numbers and push / pop / pull on lists,
// compiled to SET assignments.

import { bind, type CompileContext } from "../../compile/context.js";
import {
  bin,
  fn,
  lit,
  name,
  printExpr,
  prop,
  v,
  type Expr,
} from "../../compile/cypher.js";
import { requestError } from "../../errors.js";
import type { NodeType } from "../../model/types.js";
import type { Input } from "./plan.js";

/** `n.p = <expr>` assignments for an `adjust` input. */
export function compileAdjust(
  ctx: CompileContext,
  node: NodeType,
  adjust: Input | undefined,
): string[] {
  const out: string[] = [];
  for (const [fieldName, ops] of Object.entries(adjust ?? {})) {
    if (ops === null || ops === undefined) continue;
    const f = node.fields.get(fieldName);
    if (f?.kind !== "scalar") continue;
    const entries = Object.entries(ops as Input).filter(
      ([, x]) => x !== null && x !== undefined,
    );
    if (entries.length !== 1) {
      throw requestError(
        "BAD_USER_INPUT",
        `adjust.${fieldName} takes exactly one operation`,
      );
    }
    const [op, value] = entries[0]!;
    const target = prop(v("n"), f.property);
    let expr: Expr;
    if (f.list) {
      const current = fn("coalesce", target, { kind: "list", items: [] });
      if (op === "push") {
        expr = bin("+", current, bind(ctx, value));
      } else if (op === "pop") {
        if (!Number.isInteger(value) || (value as number) < 0) {
          throw requestError(
            "BAD_USER_INPUT",
            `adjust.${fieldName}.pop must be a non-negative integer`,
          );
        }
        // `l[..-n]` is broken in LoraDB 0.15; size arithmetic is not.
        expr = {
          kind: "slice",
          target: current,
          from: undefined,
          to: bin("-", fn("size", current), bind(ctx, value)),
        };
      } else {
        const x = "adjust_item";
        expr = {
          kind: "listComprehension",
          variable: x,
          list: current,
          where: { kind: "not", expr: bin("IN", v(x), bind(ctx, value)) },
          projection: undefined,
        };
      }
    } else {
      const current = fn("coalesce", target, lit(0));
      const operand = bind(ctx, value);
      if (op === "divide" && Number(value) === 0) {
        throw requestError(
          "BAD_USER_INPUT",
          `adjust.${fieldName}.divide by zero`,
        );
      }
      const opMap: Record<string, "+" | "-" | "*" | "/"> = {
        add: "+",
        subtract: "-",
        multiply: "*",
        divide: "/",
      };
      expr = bin(opMap[op]!, current, operand);
      // LoraDB divides integers as floats; keep Int fields integral.
      if (op === "divide" && f.type === "Int") expr = fn("toInteger", expr);
    }
    out.push(`n.${name(f.property)} = ${printExpr(expr)}`);
  }
  return out;
}
