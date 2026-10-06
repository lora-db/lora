// `@cypher` fields: root fields on Query and Mutation, object fields run
// once per parent, and the binding of their statement's parameters.

import type { FieldNode, SelectionSetNode } from "graphql";
import { requestError } from "../../errors.js";
import { scanParams } from "../../model/cypher-lexer.js";
import type { CypherField } from "../../model/types.js";
import { authFilter, checkAuthentication, viewerKey } from "../auth.js";
import { bind, freshVar, noteClaim, type CompileContext } from "../context.js";
import { fn, printExpr, v, type Clause, type Expr } from "../cypher.js";
import { outOfRange } from "../../model/inputs.js";
import { subSelections } from "../selection.js";
import type { CompiledRead, Args, Projection } from "./types.js";
import { finish, collectOne } from "./common.js";
import { projectAbstract } from "./abstract.js";
import { projectNode } from "./project.js";

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

/** The statement text with its parameters renamed to bound `$pN`s. */
export function bindStatement(
  ctx: CompileContext,
  field: CypherField,
  args: Args,
): string {
  checkListArguments(ctx, field, args);
  ctx.reads.opaque = true;
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
 * Int and Float arguments with `@range(min:, max:)` stay inside it.
 */
function checkListArguments(
  ctx: CompileContext,
  field: CypherField,
  args: Args,
): void {
  for (const a of field.args) {
    if (a.range) {
      const bad = outOfRange(args[a.name], a.range);
      if (bad !== undefined) {
        const { min, max } = a.range;
        const bounds =
          min !== undefined && max !== undefined
            ? `between ${min} and ${max}`
            : min !== undefined
              ? `at least ${min}`
              : `at most ${max}`;
        throw requestError(
          "BAD_USER_INPUT",
          `${field.owner}.${field.name}: argument ${a.name} must be ${bounds} (got ${bad})`,
        );
      }
    }
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

/**
 * An object `@cypher` field, run once per parent:
 *
 *   CALL { WITH parent WITH parent AS this
 *          CALL { WITH this <statement> }
 *          RETURN collect(<column>) AS out }
 */
export function projectCypher(
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
