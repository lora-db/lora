// `@uniqueTogether` at write time: the statement that finds a node sharing
// its combination with another, and the check for writes whose write-set
// is unknown.

import {
  bind,
  newContext,
  type CompileContext,
} from "../../compile/context.js";
import { name, printExpr } from "../../compile/cypher.js";
import { compileNodeWhere } from "../../compile/filter.js";
import type { DriverTransaction } from "../../driver.js";
import { requestError } from "../../errors.js";
import type {
  GraphModel,
  NodeType,
  UniqueTogether,
} from "../../model/types.js";
import { runStatement, type MutationEnv } from "./env.js";
import { arrow, dedupe, labelTest } from "./fragments.js";

export function notUniqueTogether(
  node: NodeType,
  u: UniqueTogether,
  key: unknown,
) {
  return requestError(
    "CONSTRAINT_VIOLATION",
    `${node.name} must be unique by (${u.fields.join(", ")}); ${node.name} ${JSON.stringify(key)} would share it with another`,
    undefined,
    { type: node.name, fields: [...u.fields] },
  );
}

/**
 * `@uniqueTogether` after a write whose write-set is unknown (a `@cypher`
 * mutation): every node of each type in `nodes` is compared, in `tx`.
 */
export async function checkUniqueTogetherOf(
  env: MutationEnv,
  tx: DriverTransaction,
  nodes: readonly NodeType[],
): Promise<void> {
  for (const node of nodes) {
    for (const u of node.uniqueTogether) {
      const ctx = newContext(env.selection, env.model, {
        jwt: env.jwt,
        degrees: env.degrees,
        requestContext: env.requestContext,
      });
      ctx.inAuth = true;
      const text = uniqueTogetherStatement(ctx, env.model, node, u, undefined);
      const { rows } = await runStatement(env, tx, {
        text,
        params: ctx.params,
      });
      if (rows.length > 0) throw notUniqueTogether(node, u, rows[0]!["key"]);
    }
  }
}

/**
 * The first node among `keys` whose `@uniqueTogether` combination another
 * node of its type shares: seeks each by key, binds its combination, then
 * compares it with the nodes that share its first relationship end (or,
 * with scalars only, its first scalar's value) — never a scan of the type.
 * Without `keys`, every node of the type is compared (a scan).
 */
export function uniqueTogetherStatement(
  ctx: CompileContext,
  model: GraphModel,
  node: NodeType,
  u: UniqueTogether,
  keys: unknown[] | undefined,
): string {
  const label = name(node.labels[0]!);
  const key = name(node.key.property);
  const rels = [...u.singles, ...(u.set ? [u.set] : [])];
  // Target keys through each relationship, from `at`.
  const ends = (at: string) =>
    rels.map((f) => {
      const target = model.nodes.get(f.target)!;
      return `[(${at})${arrow(f, "", "x", undefined)} WHERE (${labelTest("x", target)}) | x.${name(target.key.property)}]`;
    });
  const nWhere = u.where
    ? compileNodeWhere(ctx, node, "n", u.where)
    : undefined;
  const mWhere = u.where
    ? compileNodeWhere(ctx, node, "m", u.where)
    : undefined;
  const mine = rels.map((_, i) => `u${i}`);
  const theirs = rels.map((_, i) => `v${i}`);
  // Exempt: a null scalar, a missing single relationship, an empty set.
  const present = [
    ...(nWhere ? [`(${printExpr(nWhere)})`] : []),
    ...u.scalars.map((f) => `n.${name(f.property)} IS NOT NULL`),
    ...rels.map((f, i) => (f.list ? `size(u${i}) > 0` : `size(u${i}) = 1`)),
  ];
  const anchor = rels[0];
  let candidates: string;
  if (anchor) {
    const target = model.nodes.get(anchor.target)!;
    const back =
      anchor.direction === "OUT"
        ? `<-[:${name(anchor.type)}]-(m)`
        : `-[:${name(anchor.type)}]->(m)`;
    candidates =
      `MATCH (n)${arrow(anchor, "", "t", undefined)} WHERE ${labelTest("t", target)}\n` +
      `MATCH (t)${back} WHERE ${labelTest("m", node)} AND m.${key} <> n.${key}\n`;
  } else {
    const first = u.scalars[0]!;
    candidates = `MATCH (m:${label}) WHERE m.${name(first.property)} = n.${name(first.property)} AND m.${key} <> n.${key}\n`;
  }
  const same = [
    ...u.scalars.map((f) => `m.${name(f.property)} = n.${name(f.property)}`),
    ...rels.map((f, i) =>
      f.list
        ? `size(v${i}) = size(u${i}) AND all(y IN v${i} WHERE y IN u${i})`
        : `v${i} = u${i}`,
    ),
    ...(mWhere ? [`(${printExpr(mWhere)})`] : []),
  ];
  return (
    (keys
      ? `UNWIND ${printExpr(bind(ctx, dedupe(keys)))} AS k\n` +
        `MATCH (n:${label}) WHERE n.${key} = k\n`
      : `MATCH (n:${label})\n`) +
    `WITH ${["n", ...ends("n").map((e, i) => `${e} AS ${mine[i]!}`)].join(", ")}` +
    (present.length > 0 ? ` WHERE ${present.join(" AND ")}` : "") +
    "\n" +
    candidates +
    `WITH ${["n", "m", ...mine, ...ends("m").map((e, i) => `${e} AS ${theirs[i]!}`)].join(", ")}` +
    (same.length > 0 ? ` WHERE ${same.join(" AND ")}` : "") +
    `\nRETURN n.${key} AS key LIMIT 1`
  );
}
