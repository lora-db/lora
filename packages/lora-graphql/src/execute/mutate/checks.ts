// Invariants checked after a mutation's writes, inside its transaction:
// single relationships hold one node, @uniqueTogether combinations stay
// unique, required relationships stay set.

import { bind } from "../../compile/context.js";
import { name, printExpr } from "../../compile/cypher.js";
import { keyOf } from "../../compile/read.js";
import { requestError } from "../../errors.js";
import { declared, memberFields } from "../../model/relations.js";
import type { NodeType, RelationshipField } from "../../model/types.js";
import type { EntityRef } from "../changes.js";
import { HIDDEN, requiredMissing } from "./errors.js";
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
  for (const { field, keys: all, fresh } of touched.values()) {
    const keys = all.filter((k) => fresh.get(keyOf(k)) !== 1);
    if (keys.length === 0) continue;
    const owner = model.nodes.get(field.owner)!;
    const ctx = runner.ctx();
    const text =
      `UNWIND ${printExpr(bind(ctx, dedupe(keys)))} AS k\n` +
      `MATCH (a:${name(owner.labels[0]!)}) WHERE a.${name(owner.key.property)} = k\n` +
      `WITH a, size([(a)${arrow(field, "", "x", undefined)} WHERE ${memberTest(model, "x", field)} | 1]) AS c WHERE c > 1\n` +
      `RETURN a.${name(owner.key.property)} AS key, c AS count` +
      `, size([(a)${arrow(field, "", "x", undefined)} WHERE ${field.members
        .map((m) => {
          const member = model.nodes.get(m)!;
          return `(${labelTest("x", member)} AND ${readable(ctx, member, "x")})`;
        })
        .join(" OR ")} | 1]) AS seen`;
    const target = { name: field.target } as NodeType;
    const rows = await runner.run(text, ctx);
    if (rows.length > 0) {
      const row = rows[0]!;
      // Held by a node the caller cannot read: replacing it would write
      // a relationship of a node they cannot see, and the count would
      // reveal it. Refused like a rule would refuse it.
      if (Number(row["seen"]) < Number(row["count"])) {
        throw requestError(
          "FORBIDDEN",
          `not allowed to replace ${owner.name}.${field.name}`,
        );
      }
      throw requestError(
        "CONSTRAINT_VIOLATION",
        `${owner.name}.${field.name} holds one ${target.name}, but ${owner.name} ${JSON.stringify(row["key"])} would have ${String(row["count"])}`,
        undefined,
        { type: owner.name, field: field.name },
      );
    }
  }
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
