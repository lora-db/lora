// Deleting nodes by key: what onDelete: CASCADE reaches, the rules and
// constraints checked before anything goes, and the relationships the
// deleted nodes take with them, for the write-set.

import {
  authFilter,
  authValidate,
  checkAuthentication,
  forbidden,
} from "../../compile/auth.js";
import { bind } from "../../compile/context.js";
import { lit, name, printExpr } from "../../compile/cypher.js";
import { keyOf } from "../../compile/read.js";
import { requestError } from "../../errors.js";
import { memberFields } from "../../model/relations.js";
import type { NodeType, RelationshipField } from "../../model/types.js";
import type { RelationshipRef } from "../changes.js";
import { runStatement } from "./env.js";
import { HIDDEN, requiredMissing } from "./errors.js";
import { arrow, readable, relRef, seekThenExpand } from "./fragments.js";
import type { Runner } from "./runner.js";

/**
 * Delete nodes by key, with what `onDelete: CASCADE` reaches. Checks
 * BEFORE rules, RESTRICT, and required relationships of the survivors.
 */
export async function deleteNodes(
  runner: Runner,
  root: NodeType,
  keys: unknown[],
): Promise<void> {
  const model = runner.env.model;
  const doomed = new Map<NodeType, Map<string, unknown>>();
  let total = 0;
  let queue: Array<[NodeType, unknown[]]> = [[root, keys]];
  while (queue.length > 0) {
    const next: Array<[NodeType, unknown[]]> = [];
    for (const [node, batch] of queue) {
      const seen = doomed.get(node) ?? new Map<string, unknown>();
      const fresh = batch.filter((k) => !seen.has(keyOf(k)));
      if (fresh.length === 0) continue;
      checkAuthentication(runner.ctx(), node, "DELETE");
      const ctx = runner.ctx();
      const before = authValidate(ctx, node, "n", "DELETE", "BEFORE");
      const rows = await runner.run(
        `UNWIND ${printExpr(bind(ctx, fresh))} AS k\n` +
          `MATCH (n:${name(node.labels[0]!)}) WHERE n.${name(node.key.property)} = k\n` +
          `RETURN n.${name(node.key.property)} AS key, ${before ? printExpr(before) : "true"} AS ok` +
          `, ${printExpr(authFilter(ctx, node, "n", "DELETE") ?? lit(true))} AS visible`,
        ctx,
      );
      for (const r of rows) {
        // Cascading into a node the caller may not delete is refused.
        if (r["ok"] !== true || r["visible"] !== true) {
          throw forbidden(node, "DELETE");
        }
        seen.set(keyOf(r["key"]), r["key"]);
      }
      doomed.set(node, seen);
      total += rows.length;
      if (total > runner.env.maxBatch) {
        throw requestError(
          "LIMIT_EXCEEDED",
          `the delete reaches more than ${runner.env.maxBatch} nodes through onDelete: CASCADE`,
        );
      }
      const found = rows.map((r) => r["key"]);
      for (const declaredField of node.fields.values()) {
        if (
          declaredField.kind !== "relationship" ||
          declaredField.onDelete !== "CASCADE"
        )
          continue;
        for (const f of memberFields(model, declaredField)) {
          const target = model.nodes.get(f.target)!;
          const c = runner.ctx();
          const related = await runner.run(
            `UNWIND ${printExpr(bind(c, found))} AS k\n` +
              seekThenExpand("n", node, "k", f, "", "m", target) +
              "\n" +
              `RETURN DISTINCT m.${name(target.key.property)} AS key`,
            c,
          );
          if (related.length > 0)
            next.push([target, related.map((r) => r["key"])]);
        }
      }
    }
    queue = next;
  }
  if (total === 0) return;

  const isDoomed = (type: string, key: unknown) =>
    !!model.nodes.get(type) &&
    doomed.get(model.nodes.get(type)!)?.has(keyOf(key)) === true;

  for (const [node, keysByNode] of doomed) {
    const nodeKeys = [...keysByNode.values()];
    // RESTRICT: related nodes must be gone first (or go in this delete).
    const restricting = [...node.fields.values()].flatMap((f) =>
      f.kind === "relationship" && f.onDelete === "RESTRICT"
        ? memberFields(model, f)
        : [],
    );
    for (const f of restricting) {
      const target = model.nodes.get(f.target)!;
      const ctx = runner.ctx();
      const rows = await runner.run(
        `UNWIND ${printExpr(bind(ctx, nodeKeys))} AS k\n` +
          seekThenExpand("n", node, "k", f, "", "m", target) +
          "\n" +
          `RETURN n.${name(node.key.property)} AS key, m.${name(target.key.property)} AS related` +
          `, ${readable(ctx, target, "m")} AS visible`,
        ctx,
      );
      const blocking = rows.filter((r) => !isDoomed(target.name, r["related"]));
      // A blocker the caller can read may be named; one they cannot
      // must not be revealed by the message.
      const seen = blocking.find((r) => r["visible"] === true);
      if (seen) {
        throw requestError(
          "CONSTRAINT_VIOLATION",
          `${node.name} ${JSON.stringify(seen["key"])} still has ${f.name} (onDelete: RESTRICT); remove them first`,
          undefined,
          { type: node.name, field: f.name },
        );
      }
      if (blocking.length > 0) {
        throw requestError(
          "CONSTRAINT_VIOLATION",
          `${node.name} ${JSON.stringify(blocking[0]!["key"])} cannot be deleted: ${node.name}.${f.name} has onDelete: RESTRICT`,
          undefined,
          { type: node.name, field: f.name },
        );
      }
    }
    // Required single relationships of surviving nodes that point here.
    for (const owner of model.nodes.values()) {
      for (const f of owner.fields.values()) {
        if (f.kind !== "relationship" || f.list || !f.required) continue;
        if (!f.members.includes(node.name)) continue;
        const ctx = runner.ctx();
        const back = {
          ...f,
          direction: f.direction === "OUT" ? "IN" : "OUT",
        } as const;
        const rows = await runner.run(
          `UNWIND ${printExpr(bind(ctx, nodeKeys))} AS k\n` +
            seekThenExpand("n", node, "k", back, "", "o", owner) +
            "\n" +
            `RETURN DISTINCT o.${name(owner.key.property)} AS key` +
            `, ${readable(ctx, owner, "o")} AS visible`,
          ctx,
        );
        const orphans = rows.filter((r) => !isDoomed(owner.name, r["key"]));
        const orphan = orphans.find((r) => r["visible"] === true) ?? orphans[0];
        if (orphan) {
          throw requiredMissing(
            owner,
            f,
            f.target,
            orphan["visible"] === true ? orphan["key"] : HIDDEN,
          );
        }
      }
    }
  }

  await runner.env.beforeDelete?.(
    runner.change,
    [...doomed].map(([node, keysByNode]) => ({
      node,
      keys: [...keysByNode.values()],
    })),
    (statement) => runStatement(runner.env, runner.tx, statement),
  );

  const relIds = new Set<number>();
  for (const [node, keysByNode] of doomed) {
    const nodeKeys = [...keysByNode.values()];
    for (const key of nodeKeys) {
      runner.disconnected.push(...(await neighbours(runner, node, key)));
    }
    const ctx = runner.ctx();
    const rows = await runner.run(
      `UNWIND ${printExpr(bind(ctx, nodeKeys))} AS k\n` +
        `MATCH (n:${name(node.labels[0]!)}) WHERE n.${name(node.key.property)} = k\n` +
        `RETURN [(n)-[r]-() | id(r)] AS ids`,
      ctx,
    );
    for (const r of rows) for (const id of r["ids"] as number[]) relIds.add(id);
  }
  for (const [node, keysByNode] of doomed) {
    const nodeKeys = [...keysByNode.values()];
    if (node.subscriptionOptions.previousState) {
      const pctx = runner.ctx();
      const rows = await runner.run(
        `UNWIND ${printExpr(bind(pctx, nodeKeys))} AS k\n` +
          `MATCH (n:${name(node.labels[0]!)}) WHERE n.${name(node.key.property)} = k\n` +
          `RETURN n.${name(node.key.property)} AS key, properties(n) AS props`,
        pctx,
      );
      for (const r of rows) {
        (runner.change.before ??= []).push({
          type: node.name,
          key: r["key"],
          properties: (r["props"] as Record<string, unknown>) ?? {},
        });
      }
    }
    const ctx = runner.ctx();
    await runner.run(
      `UNWIND ${printExpr(bind(ctx, nodeKeys))} AS k\n` +
        `MATCH (n:${name(node.labels[0]!)}) WHERE n.${name(node.key.property)} = k\n` +
        `DETACH DELETE n`,
      ctx,
    );
    runner.info.nodesDeleted += nodeKeys.length;
    for (const key of nodeKeys) {
      runner.change.deleted.push({ type: node.name, key });
    }
  }
  runner.info.relationshipsDeleted += relIds.size;
}

/** The node's current relationships in the model, for the write-set. */
async function neighbours(
  runner: Runner,
  node: NodeType,
  key: unknown,
): Promise<RelationshipRef[]> {
  // Relationship fields declared on this type, and those on other types
  // that point here: a relationship may be declared on either side.
  const model = runner.env.model;
  const own = [...node.fields.values()].flatMap((f) =>
    f.kind === "relationship" ? memberFields(model, f) : [],
  );
  const inverse = [...model.nodes.values()].flatMap((n) =>
    [...n.fields.values()].filter(
      (f): f is RelationshipField =>
        f.kind === "relationship" && f.members.includes(node.name),
    ),
  );
  if (own.length + inverse.length === 0) return [];
  const ctx = runner.ctx();
  const k = printExpr(bind(ctx, key));
  const entries = [
    ...own.map((f, i) => {
      const target = model.nodes.get(f.target)!;
      return `r${i}: [(n)${arrow(f, "", "m", target)} | m.${name(target.key.property)}]`;
    }),
    ...inverse.map((f, i) => {
      const owner = model.nodes.get(f.owner)!;
      // Seen from this node, the declared direction is reversed.
      const back = {
        ...f,
        direction: f.direction === "OUT" ? "IN" : "OUT",
      } as const;
      return `i${i}: [(n)${arrow(back, "", "m", owner)} | m.${name(owner.key.property)}]`;
    }),
  ];
  const rows = await runner.run(
    `MATCH (n:${name(node.labels[0]!)}) WHERE n.${name(node.key.property)} = ${k}\nRETURN { ${entries.join(", ")} } AS related`,
    ctx,
  );
  const related = rows[0]?.["related"] as Record<string, unknown[]> | undefined;
  if (!related) return [];
  const refs = [
    ...own.flatMap((f, i) =>
      (related[`r${i}`] ?? []).map((to) => relRef(f, key, to)),
    ),
    ...inverse.flatMap((f, i) =>
      (related[`i${i}`] ?? []).map((from) => relRef(f, from, key)),
    ),
  ];
  // A relationship declared on both sides is reported once.
  const seen = new Set<string>();
  return refs.filter((r) => {
    const ends = [
      `${r.from.type}:${keyOf(r.from.key)}`,
      `${r.to.type}:${keyOf(r.to.key)}`,
    ].sort();
    const id = `${r.type}\0${ends.join("\0")}`;
    if (seen.has(id)) return false;
    seen.add(id);
    return true;
  });
}
