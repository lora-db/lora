// Invariants checked after a mutation's writes, inside its transaction:
// single relationships hold one node, @uniqueTogether combinations stay
// unique, required relationships stay set.

import { bind, type CompileContext } from "../../compile/context.js";
import { name, printExpr } from "../../compile/cypher.js";
import { keyOf } from "../../compile/read/by-keys.js";
import { declared, memberFields } from "../../model/relations.js";
import type { NodeType, RelationshipField } from "../../model/types.js";
import type { EntityRef } from "../changes.js";
import { HIDDEN, requiredMissing, tooMany } from "./errors.js";
import {
  arrow,
  dedupe,
  inverseFields,
  labelTest,
  memberTest,
  readable,
} from "./fragments.js";
import type { WritePlan } from "./plan.js";
import type { Runner } from "./runner.js";
import {
  notUniqueTogether,
  uniqueTogetherStatement,
} from "./unique-together.js";

/**
 * A single relationship field allows one related node, whichever side
 * the relationship was created from. Check every node that gained one.
 */
export async function checkCardinality(
  runner: Runner,
  plan: WritePlan,
): Promise<void> {
  const model = runner.env.model;
  for (const { field, keys } of singlesTouched(runner, plan)) {
    const owner = model.nodes.get(field.owner)!;
    const ctx = runner.ctx();
    const text =
      `UNWIND ${printExpr(bind(ctx, keys))} AS k\n` +
      `MATCH (a:${name(owner.labels[0]!)}) WHERE a.${name(owner.key.property)} = k\n` +
      `WITH a, ${related(runner, field)} AS c WHERE c > 1\n` +
      `RETURN a.${name(owner.key.property)} AS key, c AS count` +
      `, ${relatedSeen(runner, ctx, field)} AS seen`;
    const rows = await runner.run(text, ctx);
    if (rows.length > 0) {
      const row = rows[0]!;
      throw tooMany(
        field,
        row["key"],
        Number(row["count"]),
        Number(row["seen"]),
      );
    }
  }
}

/**
 * {@link checkCardinality} for a single call: each check is a statement
 * that fails on the first node holding more than one, naming the node and
 * its counts in the message. Nothing is awaited.
 */
export function assertCardinality(runner: Runner, plan: WritePlan): void {
  const model = runner.env.model;
  for (const { field, keys } of singlesTouched(runner, plan)) {
    runner.inPlace(() => {
      const owner = model.nodes.get(field.owner)!;
      const ctx = runner.ctx();
      const message = runner.assertions.failsAs((detail) => {
        const [i, count, seen] = detail.split(",").map(Number);
        return tooMany(field, keys[i!], count!, seen!);
      }, "toString(row.i) + ',' + toString(c) + ',' + toString(seen)");
      void runner.run(
        `UNWIND ${printExpr(
          bind(
            ctx,
            keys.map((k, i) => ({ i, k })),
          ),
        )} AS row\n` +
          `MATCH (a:${name(owner.labels[0]!)}) WHERE a.${name(owner.key.property)} = row.k\n` +
          `WITH row, a, ${related(runner, field)} AS c\n` +
          `WITH row, c, ${relatedSeen(runner, ctx, field)} AS seen\n` +
          `RETURN assert(c <= 1, ${message}) AS ok`,
        ctx,
      );
    });
  }
}

/** How many nodes `a` holds on `field`. */
function related(runner: Runner, field: RelationshipField): string {
  return `size([(a)${arrow(field, "", "x", undefined)} WHERE ${memberTest(runner.env.model, "x", field)} | 1])`;
}

/** How many of them the caller can read. */
function relatedSeen(
  runner: Runner,
  ctx: CompileContext,
  field: RelationshipField,
): string {
  const model = runner.env.model;
  return `size([(a)${arrow(field, "", "x", undefined)} WHERE ${field.members
    .map((m) => {
      const member = model.nodes.get(m)!;
      return `(${labelTest("x", member)} AND ${readable(ctx, member, "x")})`;
    })
    .join(" OR ")} | 1])`;
}

/**
 * The single relationship fields the plan's links touch, each with the
 * keys of the nodes to check: those that may now hold more than one.
 */
function singlesTouched(
  runner: Runner,
  plan: WritePlan,
): Array<{ field: RelationshipField; keys: unknown[] }> {
  const model = runner.env.model;
  interface Touched {
    field: RelationshipField;
    keys: unknown[];
    /** Links per key, for keys of nodes this plan creates. */
    fresh: Map<string, number>;
  }
  const touched = new Map<string, Touched>();
  // `type` is the node's concrete type: a node this plan creates that
  // gains exactly one link on the field cannot hold more than one.
  const add = (field: RelationshipField, key: unknown, type: string) => {
    const id = `${field.owner}.${field.name}`;
    const entry: Touched = touched.get(id) ?? {
      field,
      keys: [],
      fresh: new Map(),
    };
    entry.keys.push(key);
    if (plan.isFresh(type, key)) {
      const k = keyOf(key);
      entry.fresh.set(k, (entry.fresh.get(k) ?? 0) + 1);
    }
    touched.set(id, entry);
  };
  for (const link of plan.links) {
    if (!link.rel.list) add(declared(link.rel), link.from, link.rel.owner);
    for (const f of inverseFields(model, link.rel)) {
      if (!f.list) add(f, link.to, link.rel.target);
    }
  }
  const checks: Array<{ field: RelationshipField; keys: unknown[] }> = [];
  for (const { field, keys: all, fresh } of touched.values()) {
    const keys = dedupe(all.filter((k) => fresh.get(keyOf(k)) !== 1));
    if (keys.length > 0) checks.push({ field, keys });
  }
  return checks;
}

/**
 * `@uniqueTogether`: after every write of the mutation, no two nodes of
 * a constrained type share a combination. Checks the nodes it created
 * or updated and the nodes whose constrained relationships it connected
 * or disconnected, from either side, against the nodes reachable
 * through the same ends. For every caller: a data invariant, not a
 * rule, so the bypass does not skip it.
 */
export async function checkUniqueTogether(runner: Runner): Promise<void> {
  const model = runner.env.model;
  const touched = new Map<NodeType, Map<string, unknown>>();
  const touch = (type: string, key: unknown) => {
    const node = model.nodes.get(type);
    if (!node?.uniqueTogether.length) return;
    const keys = touched.get(node) ?? new Map<string, unknown>();
    keys.set(keyOf(key), key);
    touched.set(node, keys);
  };
  for (const e of runner.change.created) touch(e.type, e.key);
  for (const [node, { keys }] of runner.updated) {
    for (const key of keys) touch(node.name, key);
  }
  // An edge (start)-[:T]->(end) changes the combination of `start` when
  // its type constrains an OUT field of T reaching `end`'s type, and of
  // `end` for an IN field reaching `start`'s.
  const end = (
    at: EntityRef,
    direction: "IN" | "OUT",
    other: string,
    type: string,
  ) => {
    const node = model.nodes.get(at.type);
    const hit = node?.uniqueTogether.some((u) =>
      [...u.singles, ...(u.set ? [u.set] : [])].some(
        (f) =>
          f.type === type &&
          f.direction === direction &&
          f.members.includes(other),
      ),
    );
    if (hit) touch(at.type, at.key);
  };
  for (const ref of [...runner.connected, ...runner.disconnected]) {
    const [ownerName, fieldName] = ref.field.split(".") as [string, string];
    const field = model.nodes.get(ownerName)?.fields.get(fieldName);
    if (field?.kind !== "relationship") continue;
    const [start, finish] =
      field.direction === "OUT" ? [ref.from, ref.to] : [ref.to, ref.from];
    end(start, "OUT", finish.type, ref.type);
    end(finish, "IN", start.type, ref.type);
  }
  if (runner.single) {
    await assertUniqueTogether(
      runner,
      [...touched].map(([node, keys]) => ({ node, keys: [...keys.values()] })),
    );
    return;
  }
  for (const [node, keys] of touched) {
    for (const u of node.uniqueTogether) {
      const ctx = runner.ctx();
      // The constraint's filter is the data's, not the caller's view.
      ctx.inAuth = true;
      const rows = await runner.run(
        uniqueTogetherStatement(ctx, model, node, u, [...keys.values()]),
        ctx,
      );
      if (rows.length > 0) throw notUniqueTogether(node, u, rows[0]!["key"]);
    }
  }
}

/**
 * `@uniqueTogether` for a single call, on the nodes with `keys` of each
 * type: every check is issued before this first waits, as a statement
 * that fails on the node sharing its combination.
 */
export function assertUniqueTogether(
  runner: Runner,
  touched: Array<{ node: NodeType; keys: unknown[] }>,
): Promise<void> {
  const issued: Array<Promise<unknown>> = [];
  for (const { node, keys: all } of touched) {
    const keys = dedupe(all);
    for (const u of node.uniqueTogether) {
      runner.inPlace(() => {
        const ctx = runner.ctx();
        // The constraint's filter is the data's, not the caller's view.
        ctx.inAuth = true;
        issued.push(
          runner.run(
            uniqueTogetherStatement(ctx, runner.env.model, node, u, keys, (i) =>
              runner.assertions.failsAs(
                (detail) => notUniqueTogether(node, u, keys[Number(detail)]),
                i,
              ),
            ),
            ctx,
          ),
        );
      });
    }
  }
  return Promise.all(issued).then(() => undefined);
}

/**
 * A required single relationship stays set: disconnecting from the other
 * side must not leave the owner without one.
 */
export async function checkRequired(runner: Runner): Promise<void> {
  const model = runner.env.model;
  const byField = new Map<RelationshipField, unknown[]>();
  for (const ref of runner.disconnected) {
    const [ownerName, fieldName] = ref.field.split(".") as [string, string];
    const field = model.nodes
      .get(ownerName)!
      .fields.get(fieldName) as RelationshipField;
    // The concrete copy for the node that was disconnected.
    const rel =
      memberFields(model, field).find((m) => m.target === ref.to.type) ?? field;
    if (!field.list && field.required) {
      byField.set(field, [...(byField.get(field) ?? []), ref.from.key]);
    }
    for (const f of inverseFields(model, rel)) {
      if (!f.list && f.required) {
        byField.set(f, [...(byField.get(f) ?? []), ref.to.key]);
      }
    }
  }
  for (const [field, keys] of byField) {
    const owner = model.nodes.get(field.owner)!;
    const ctx = runner.ctx();
    const rows = await runner.run(
      `UNWIND ${printExpr(bind(ctx, dedupe(keys)))} AS k\n` +
        `MATCH (a:${name(owner.labels[0]!)}) WHERE a.${name(owner.key.property)} = k\n` +
        // The count is bound in the WITH and tested after it: the form
        // every LoraDB release plans as a seek (G-11).
        `WITH a, size([(a)${arrow(field, "", "x", undefined)} WHERE ${memberTest(model, "x", field)} | 1]) AS related WHERE related = 0\n` +
        `RETURN a.${name(owner.key.property)} AS key, ${readable(ctx, owner, "a")} AS visible`,
      ctx,
    );
    if (rows.length > 0) {
      const row = rows.find((r) => r["visible"] === true) ?? rows[0]!;
      throw requiredMissing(
        owner,
        field,
        field.target,
        row["visible"] === true ? row["key"] : HIDDEN,
      );
    }
  }
}
