// Nodes by @key in one statement, for mutation payloads.

import type { SelectionSetNode } from "graphql";
import type { NodeType } from "../../model/types.js";
import { authFilter, checkAuthentication } from "../auth.js";
import { bind, freshVar, type CompileContext } from "../context.js";
import { and, bin, prop, v, type Clause } from "../cypher.js";
import type { CompiledRead } from "./types.js";
import { finish } from "./common.js";
import { projectNode } from "./project.js";

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
