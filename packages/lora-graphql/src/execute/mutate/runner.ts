// Runs a write plan inside the mutation's transaction: the statements for
// each kind of write, in order, with the rule checks around them, and what
// they changed for the payload and the write-set.

import type { SelectionSetNode } from "graphql";
import {
  authFilter,
  authValidate,
  checkAuthentication,
  checkFieldAuthentication,
  fieldValidate,
  forbidden,
  relationshipRules,
} from "../../compile/auth.js";
import {
  bind,
  newContext,
  type CompileContext,
} from "../../compile/context.js";
import { and, lit, name, printExpr } from "../../compile/cypher.js";
import { compileNodeWhere } from "../../compile/filter.js";
import { compileByKeys, keyOf } from "../../compile/read/by-keys.js";
import type { DriverTransaction } from "../../driver.js";
import { requestError } from "../../errors.js";
import { toStored } from "../../model/points.js";
import type {
  NodeType,
  RelationshipField,
  ScalarField,
} from "../../model/types.js";
import { assertReadable } from "../../schema/guard.js";
import { settable } from "../../schema/mutations.js";
import type { RelationshipRef, WriteChange } from "../changes.js";
import { compileAdjust } from "./adjust.js";
import { checkCardinality, checkRequired } from "./checks.js";
import { deleteNodes } from "./delete.js";
import { runStatement, type MutationEnv, type MutationInfo } from "./env.js";
import { notConnected } from "./errors.js";
import {
  andText,
  arrow,
  dedupe,
  edgeStamps,
  orFresh,
  relRef,
  seekThenExpand,
  timestamps,
} from "./fragments.js";
import {
  assertUniqueKeys,
  WritePlan,
  type CreateRow,
  type Input,
  type Link,
} from "./plan.js";

export class Runner {
  info: MutationInfo = {
    nodesCreated: 0,
    nodesUpdated: 0,
    nodesDeleted: 0,
    relationshipsCreated: 0,
    relationshipsDeleted: 0,
  };
  connected: RelationshipRef[] = [];
  disconnected: RelationshipRef[] = [];
  /** Nodes updated per type, for AFTER validation. */
  updated = new Map<NodeType, { keys: unknown[]; fields: Set<string> }>();
  /**
   * A type this mutation creates under a key held by a node the caller
   * cannot read; see {@link Runner.moveHiddenKeys}.
   */
  hiddenKey: NodeType | undefined;
  /**
   * A @unique value this mutation creates that another node holds; see
   * {@link Runner.moveTakenUniques}.
   */
  takenUnique: { node: NodeType; field: ScalarField } | undefined;

  constructor(
    readonly env: MutationEnv,
    readonly tx: DriverTransaction,
    readonly change: WriteChange,
  ) {}

  ctx(): CompileContext {
    return newContext(this.env.selection, this.env.model, {
      jwt: this.env.jwt,
      degrees: this.env.degrees,
      statistics: this.env.statistics,
      requestContext: this.env.requestContext,
      maxFilterDepth: this.env.maxFilterDepth,
      maxListFilter: this.env.maxListFilter,
      maxStringFilter: this.env.maxStringFilter,
    });
  }

  async run(
    text: string,
    ctx: CompileContext,
  ): Promise<Array<Record<string, unknown>>> {
    const statement = { text, params: ctx.params };
    const result = await runStatement(this.env, this.tx, statement);
    return result.rows;
  }

  plan(): WritePlan {
    return new WritePlan(this.env, this.ctx());
  }

  /** Every planned write, in order, with the checks that follow them. */
  async apply(plan: WritePlan): Promise<void> {
    for (const task of plan.pending) await task();
    this.assertBatch(plan);
    assertUniqueKeys(plan);
    await this.moveHiddenKeys(plan);
    await this.moveTakenUniques(plan);
    await this.applyCreates(plan);
    await this.applyDisconnects(plan);
    await this.applyLinks(plan);
    await this.applyEdgeUpdates(plan);
    await this.applyNodeUpdates(plan);
    await this.applyNestedDeletes(plan);
    await checkCardinality(this, plan);
    await checkRequired(this);
    await this.validateCreated(plan);
  }

  /**
   * `maxBatch` over the whole mutation: nodes created and updated so far
   * plus those `plan` creates and updates (nested `update` entries
   * included), and relationships written so far plus those `plan`
   * connects, disconnects (each listed key) and updates, at most ten per
   * node allowed. Checked before any of the plan's writes.
   */
  assertBatch(plan?: WritePlan, updating = 0): void {
    const max = this.env.maxBatch;
    let creates = 0;
    for (const rows of plan?.creates.values() ?? []) creates += rows.length;
    const updates =
      this.info.nodesUpdated + updating + (plan?.nodeUpdates.length ?? 0);
    const nodes = this.info.nodesCreated + creates + updates;
    const relationships =
      this.info.relationshipsCreated +
      this.info.relationshipsDeleted +
      (plan
        ? plan.links.length +
          plan.edgeUpdates.length +
          plan.disconnects.reduce((n, d) => n + (d.to ? d.to.length : 1), 0)
        : 0);
    if (nodes > max || relationships > max * 10) {
      throw requestError(
        "LIMIT_EXCEEDED",
        `the mutation writes ${nodes} nodes (${this.info.nodesCreated + creates} created, ${updates} updated) and ${relationships} relationships; the limit is ${max} nodes (and ${max * 10} relationships) per mutation`,
      );
    }
  }

  /**
   * A create under a key held by a node the caller cannot read must not
   * tell them the key exists (G-20). The unique constraint would fail it
   * with CONSTRAINT_VIOLATION before the rules run, while a free key gets
   * the rules' answer: an existence oracle. So the hidden node moves to a
   * placeholder key for the rest of the transaction, and the mutation runs
   * on a graph that differs from the free-key case only in that hidden
   * key: every error it raises is the one a free key gets. When it would
   * succeed, `executeMutation` fails it with the FORBIDDEN of a denied
   * create. Either way the transaction rolls back, and the key with it.
   */
  async moveHiddenKeys(plan: WritePlan): Promise<void> {
    for (const [node, rows] of plan.creates) {
      const ctx = this.ctx();
      const visible = authFilter(ctx, node, "n", "READ");
      if (!visible) continue;
      const moves = rows.map((r) => ({ key: r.key, to: placeholderKey(node) }));
      const key = `n.${name(node.key.property)}`;
      const [moved] = await this.run(
        `UNWIND ${printExpr(bind(ctx, moves))} AS m\n` +
          `MATCH (n:${name(node.labels[0]!)}) WHERE ${key} = m.key` +
          ` AND NOT coalesce(${printExpr(visible)}, false)\n` +
          `SET ${key} = m.to\nRETURN count(n) AS moved`,
        ctx,
      );
      if (Number(moved?.["moved"] ?? 0) > 0) this.hiddenKey ??= node;
    }
  }

  /**
   * The G-20 treatment for @unique values: a create under rules whose
   * unique value another node holds would fail the constraint before the
   * rules answer, telling a caller who may not create the node that the
   * value is taken. The holder's value moves aside for the transaction,
   * so every error is the one a free value gets; when the mutation would
   * succeed, `executeMutation` fails it with the constraint violation a
   * permitted caller gets. The transaction rolls back either way. Types
   * without CREATE rules skip this: there is nothing to leak.
   */
  async moveTakenUniques(plan: WritePlan): Promise<void> {
    for (const [node, rows] of plan.creates) {
      const ruled = node.authorization?.validate.some((r) =>
        r.operations.has("CREATE"),
      );
      if (!ruled) continue;
      for (const f of node.fields.values()) {
        if (f.kind !== "scalar" || !f.unique || f.key) continue;
        const moves = rows
          .map((r) => r.props[f.property])
          .filter((value) => value !== undefined && value !== null)
          .map((value) => ({
            value,
            to: `\u0000taken:${globalThis.crypto.randomUUID()}`,
          }));
        if (moves.length === 0) continue;
        const ctx = this.ctx();
        const p = `n.${name(f.property)}`;
        const [moved] = await this.run(
          `UNWIND ${printExpr(bind(ctx, moves))} AS m\n` +
            `MATCH (n:${name(node.labels[0]!)}) WHERE ${p} = m.value\n` +
            `SET ${p} = m.to\nRETURN count(n) AS moved`,
          ctx,
        );
        if (Number(moved?.["moved"] ?? 0) > 0) {
          this.takenUnique ??= { node, field: f };
        }
      }
    }
  }

  async applyCreates(plan: WritePlan): Promise<void> {
    for (const [node, rows] of plan.creates) {
      // Every property goes into the CREATE itself (timestamps included):
      // existence constraints are checked there, before any SET. Rows are
      // grouped by the properties they set, so absent ones are never
      // written as null.
      const groups = new Map<string, CreateRow[]>();
      for (const row of rows) {
        const shape = Object.keys(row.props).sort().join("\0");
        const group = groups.get(shape) ?? [];
        group.push(row);
        groups.set(shape, group);
      }
      const labels = node.labels.map(name).join(":");
      const stamped = timestamps(node, "CREATE");
      for (const [shape, group] of groups) {
        const ctx = this.ctx();
        const p = printExpr(bind(ctx, group));
        const given = shape ? shape.split("\0") : [];
        const props = [
          `${name(node.key.property)}: row.key`,
          ...given.map((prop) => `${name(prop)}: row.props.${name(prop)}`),
          // A settable @timestamp the input supplied keeps its value.
          ...stamped
            .filter(([prop]) => !given.includes(prop))
            .map(([prop, now]) => `${name(prop)}: ${now}`),
        ];
        await this.run(
          `UNWIND ${p} AS row\nCREATE (n:${labels} { ${props.join(", ")} })`,
          ctx,
        );
      }
      this.info.nodesCreated += rows.length;
      for (const r of rows) {
        this.change.created.push({ type: node.name, key: r.key });
      }
    }
  }

  async applyDisconnects(plan: WritePlan): Promise<void> {
    // One statement per field and shape (listed keys, or a single
    // relationship's current target), however many nodes it covers.
    const groups = new Map<
      string,
      { rel: RelationshipField; list: boolean; rows: typeof plan.disconnects }
    >();
    for (const d of plan.disconnects) {
      const id = `${d.rel.owner}.${d.rel.name}\0${d.rel.target}\0${d.to ? "list" : "single"}`;
      const group = groups.get(id) ?? { rel: d.rel, list: !!d.to, rows: [] };
      group.rows.push(d);
      groups.set(id, group);
    }
    for (const { rel, list, rows: batch } of groups.values()) {
      const owner = this.env.model.nodes.get(rel.owner)!;
      const target = this.env.model.nodes.get(rel.target)!;
      const ctx = this.ctx();
      const input = printExpr(
        bind(
          ctx,
          batch.map((d) => ({
            from: d.from,
            to: d.to ?? null,
            keep: d.keep ?? null,
          })),
        ),
      );
      // Nodes the caller cannot see keep their relationships, and both
      // ends must allow DELETE_RELATIONSHIP when their rules say so.
      const guard =
        andText(authFilter(ctx, target, "b", "READ")) +
        andText(authFilter(ctx, target, "b", "DELETE_RELATIONSHIP")) +
        andText(authFilter(ctx, owner, "a", "DELETE_RELATIONSHIP"));
      const bKey = `b.${name(target.key.property)}`;
      const matchText =
        `UNWIND ${input} AS row\n` +
        (list ? `UNWIND row.to AS k\n` : "") +
        seekThenExpand("a", owner, "row.from", rel, "r", "b", target) +
        (list
          ? ` AND ${bKey} = k`
          : // Re-connecting the current target keeps its relationship.
            ` AND (row.keep IS NULL OR ${bKey} <> row.keep)`) +
        `${guard}\n`;
      const returned = `row.from AS from, ${bKey} AS key`;
      const pairs = (rows: Array<Record<string, unknown>>) =>
        rows.map((r) => ({ from: r["from"], to: r["key"] }));
      // Rules on the relationship field, before the relationships go.
      if (
        relationshipRules(this.ctx(), rel, "DISCONNECT", {
          owner: "a",
          target: "b",
          rel: "r",
        }) !== undefined
      ) {
        const doomed = await this.run(`${matchText}RETURN ${returned}`, ctx);
        await this.checkRelationshipRules("DISCONNECT", rel, pairs(doomed));
      }
      if (this.endRules(owner, target, "DELETE_RELATIONSHIP", "BEFORE")) {
        const doomed = await this.run(`${matchText}RETURN ${returned}`, ctx);
        await this.checkEnds(
          "DELETE_RELATIONSHIP",
          owner,
          target,
          pairs(doomed),
          "BEFORE",
        );
      }
      const rows = await this.run(
        `${matchText}DELETE r RETURN ${returned}`,
        ctx,
      );
      await this.checkEnds(
        "DELETE_RELATIONSHIP",
        owner,
        target,
        pairs(rows),
        "AFTER",
      );
      this.info.relationshipsDeleted += rows.length;
      for (const row of rows) {
        this.disconnected.push(relRef(rel, row["from"], row["key"]));
      }
    }
  }

  async applyLinks(plan: WritePlan): Promise<void> {
    // Connecting a pair twice in one input is one relationship: the
    // last mention's properties win.
    const unique = new Map<string, Link>();
    for (const link of plan.links) {
      unique.set(
        `${link.rel.owner}.${link.rel.name}\0${keyOf(link.from)}\0${keyOf(link.to)}`,
        link,
      );
    }
    const groups = new Map<RelationshipField, Link[]>();
    for (const link of unique.values()) {
      const list = groups.get(link.rel) ?? [];
      list.push(link);
      groups.set(link.rel, list);
    }
    for (const [rel, links] of groups) {
      const owner = this.env.model.nodes.get(rel.owner)!;
      const target = this.env.model.nodes.get(rel.target)!;
      const pairs = links.map((l) => ({ from: l.from, to: l.to }));
      await this.checkEnds(
        "CREATE_RELATIONSHIP",
        owner,
        target,
        pairs,
        "BEFORE",
      );
      // Connecting an already-connected pair keeps one relationship and
      // updates its properties: MERGE on both bound ends (E14 is fixed).
      // `existed` tells a new relationship from an updated one.
      const lctx = this.ctx();
      const rows = printExpr(
        bind(
          lctx,
          links.map((l) => ({
            from: l.from,
            to: l.to,
            props: l.props,
            defaults: l.defaults,
            // Created by this mutation: not a node the caller must be
            // able to see already. Its READ filter may well depend on
            // the relationship this statement creates.
            fresh: plan.isFresh(target.name, l.to),
          })),
        ),
      );
      const props = rel.properties
        ? this.env.model.relationshipProperties.get(rel.properties)
        : undefined;
      const onCreate = [
        "r += row.defaults",
        ...(props ? timestamps(props, "CREATE") : []).map(
          ([p, now]) => `r.${name(p)} = ${now}`,
        ),
      ];
      // A re-connect that sets properties updates the relationship.
      const onMatch = (props ? timestamps(props, "UPDATE") : []).map(
        ([p, now]) =>
          `r.${name(p)} = CASE WHEN size(keys(row.props)) > 0 THEN ${now} ELSE r.${name(p)} END`,
      );
      const text =
        `UNWIND ${rows} AS row\n` +
        `MATCH (a:${name(owner.labels[0]!)}) WHERE a.${name(owner.key.property)} = row.from` +
        andText(authFilter(lctx, owner, "a", "CREATE_RELATIONSHIP")) +
        `\nMATCH (b:${name(target.labels[0]!)}) WHERE b.${name(target.key.property)} = row.to` +
        andText(orFresh(authFilter(lctx, target, "b", "READ"))) +
        andText(authFilter(lctx, target, "b", "CREATE_RELATIONSHIP")) +
        `\nWITH a, b, row, size([(a)${arrow(rel, "", "b", undefined)} | 1]) > 0 AS existed` +
        `\nMERGE (a)${arrow(rel, "r", "b", undefined)}\n` +
        // Defaults apply when the relationship is created, never to an
        // existing one (a re-connect keeps what it has).
        `ON CREATE SET ${onCreate.join(", ")}\n` +
        (onMatch.length > 0 ? `ON MATCH SET ${onMatch.join(", ")}\n` : "") +
        `SET r += row.props\nRETURN row.from AS from, row.to AS key, existed`;
      const linked = await this.run(text, lctx);
      // Rules on the properties set, now that new and existing
      // relationships are told apart; a refusal rolls the mutation back.
      const byPair = new Map(
        links.map((l) => [`${keyOf(l.from)}\0${keyOf(l.to)}`, l]),
      );
      for (const row of linked) {
        const refused = byPair.get(
          `${keyOf(row["from"])}\0${keyOf(row["key"])}`,
        )?.refused;
        const error =
          row["existed"] === true ? refused?.update : refused?.create;
        if (error) throw error;
      }
      if (linked.length < links.length) {
        const found = new Set(linked.map((r) => keyOf(r["key"])));
        const missing = links
          .map((l) => l.to)
          .filter((k) => !found.has(keyOf(k)));
        throw requestError(
          "NOT_FOUND",
          `${owner.name}.${rel.name}: no ${target.name} with ${target.key.name} ${missing
            .map((k) => JSON.stringify(k))
            .join(", ")}`,
          undefined,
          { type: target.name, keys: missing },
        );
      }
      // Rules on the relationship field: a new relationship is a CONNECT,
      // an existing one given properties an UPDATE_EDGE.
      await this.checkRelationshipRules(
        "CONNECT",
        rel,
        linked
          .filter((r) => r["existed"] !== true)
          .map((r) => ({ from: r["from"], to: r["key"] })),
      );
      await this.checkRelationshipRules(
        "UPDATE_EDGE",
        rel,
        linked
          .filter(
            (r) =>
              r["existed"] === true &&
              Object.keys(
                byPair.get(`${keyOf(r["from"])}\0${keyOf(r["key"])}`)?.props ??
                  {},
              ).length > 0,
          )
          .map((r) => ({ from: r["from"], to: r["key"] })),
      );
      await this.checkEnds(
        "CREATE_RELATIONSHIP",
        owner,
        target,
        pairs,
        "AFTER",
      );
      this.info.relationshipsCreated += linked.filter(
        (r) => r["existed"] !== true,
      ).length;
      for (const l of links) this.connected.push(relRef(rel, l.from, l.to));
    }
  }

  /**
   * Rules on the relationship field (and on a field declaring the same
   * relationship from the other side) for `op`, on each pair's
   * relationship as it stands: after the write for CONNECT and
   * UPDATE_EDGE, before it for DISCONNECT. FORBIDDEN when any fails; the
   * mutation rolls back.
   */
  async checkRelationshipRules(
    op: "CONNECT" | "DISCONNECT" | "UPDATE_EDGE",
    rel: RelationshipField,
    pairs: Array<{ from: unknown; to: unknown }>,
  ): Promise<void> {
    if (pairs.length === 0) return;
    const ctx = this.ctx();
    const cond = relationshipRules(ctx, rel, op, {
      owner: "a",
      target: "b",
      rel: "r",
    });
    if (cond === undefined) return;
    const owner = this.env.model.nodes.get(rel.owner)!;
    const target = this.env.model.nodes.get(rel.target)!;
    const rows = await this.run(
      `UNWIND ${printExpr(bind(ctx, pairs))} AS row\n` +
        `MATCH (a:${name(owner.labels[0]!)}) WHERE a.${name(owner.key.property)} = row.from\n` +
        `MATCH (b:${name(target.labels[0]!)}) WHERE b.${name(target.key.property)} = row.to\n` +
        `MATCH (a)${arrow(rel, "r", "b", undefined)}\n` +
        `RETURN coalesce(${printExpr(cond === false ? lit(false) : cond)}, false) AS ok`,
      ctx,
    );
    if (rows.some((r) => r["ok"] !== true)) {
      throw requestError(
        "FORBIDDEN",
        `not allowed to ${op.toLowerCase().replace("_", " ")} ${owner.name}.${rel.name}`,
      );
    }
  }

  /** Whether either end has validate rules for `op` at `when`. */
  endRules(
    owner: NodeType,
    target: NodeType,
    op: "CREATE_RELATIONSHIP" | "DELETE_RELATIONSHIP",
    when: "BEFORE" | "AFTER",
  ): boolean {
    return [owner, target].some((n) =>
      n.authorization?.validate.some(
        (r) => r.operations.has(op) && r.when.has(when),
      ),
    );
  }

  /**
   * Validate rules for connecting or disconnecting, on both ends of each
   * pair: FORBIDDEN when either end fails them.
   */
  async checkEnds(
    op: "CREATE_RELATIONSHIP" | "DELETE_RELATIONSHIP",
    owner: NodeType,
    target: NodeType,
    pairs: Array<{ from: unknown; to: unknown }>,
    when: "BEFORE" | "AFTER",
  ): Promise<void> {
    if (pairs.length === 0) return;
    const ctx = this.ctx();
    const a = authValidate(ctx, owner, "a", op, when);
    const b = authValidate(ctx, target, "b", op, when);
    if (!a && !b) return;
    const rows = await this.run(
      `UNWIND ${printExpr(bind(ctx, pairs))} AS row\n` +
        `MATCH (a:${name(owner.labels[0]!)}) WHERE a.${name(owner.key.property)} = row.from\n` +
        `MATCH (b:${name(target.labels[0]!)}) WHERE b.${name(target.key.property)} = row.to\n` +
        `RETURN ${a ? printExpr(a) : "true"} AS a, ${b ? printExpr(b) : "true"} AS b`,
      ctx,
    );
    for (const row of rows) {
      if (row["a"] !== true) throw forbidden(owner, op);
      if (row["b"] !== true) throw forbidden(target, op);
    }
  }

  /** `update: [{ key, edge }]`: properties of connected pairs, in place. */
  async applyEdgeUpdates(plan: WritePlan): Promise<void> {
    for (const u of plan.edgeUpdates) {
      const owner = this.env.model.nodes.get(u.rel.owner)!;
      const target = this.env.model.nodes.get(u.rel.target)!;
      const ctx = this.ctx();
      const text =
        seekThenExpand(
          "a",
          owner,
          printExpr(bind(ctx, u.from)),
          u.rel,
          "r",
          "b",
          target,
        ) +
        (u.to !== undefined
          ? ` AND b.${name(target.key.property)} = ${printExpr(bind(ctx, u.to))}`
          : "") +
        andText(authFilter(ctx, target, "b", "READ")) +
        `\nSET r += ${printExpr(bind(ctx, u.set))}` +
        edgeStamps(this.env.model, u.rel) +
        (u.remove.length > 0
          ? `\nREMOVE ${u.remove.map((p) => `r.${name(p)}`).join(", ")}`
          : "") +
        `\nRETURN b.${name(target.key.property)} AS key`;
      const rows = await this.run(text, ctx);
      if (rows.length === 0) throw notConnected(owner, u.rel, target, u.to);
      await this.checkRelationshipRules(
        "UPDATE_EDGE",
        u.rel,
        rows.map((r) => ({ from: u.from, to: r["key"] })),
      );
      for (const row of rows) {
        this.connected.push(relRef(u.rel, u.from, row["key"]));
      }
    }
  }

  /**
   * `delete: { where, limit }`: the connected nodes `where` matches, under
   * their DELETE rules, deleted like a bulk delete (onDelete followed). More
   * than `limit` (default `maxBatch`) is an error, not a truncation.
   */
  async applyNestedDeletes(plan: WritePlan): Promise<void> {
    for (const d of plan.nestedDeletes) {
      const owner = this.env.model.nodes.get(d.rel.owner)!;
      const target = this.env.model.nodes.get(d.rel.target)!;
      const limit = d.limit ?? this.env.maxBatch;
      if (!Number.isInteger(limit) || limit < 1) {
        throw requestError(
          "BAD_USER_INPUT",
          "`limit` must be a positive integer",
        );
      }
      const ctx = this.ctx();
      const rows = await this.run(
        seekThenExpand(
          "a",
          owner,
          printExpr(bind(ctx, d.from)),
          d.rel,
          "",
          "b",
          target,
        ) +
          andText(compileNodeWhere(ctx, target, "b", d.where)) +
          andText(authFilter(ctx, target, "b", "READ")) +
          andText(authFilter(ctx, target, "b", "DELETE")) +
          `\nRETURN DISTINCT b.${name(target.key.property)} AS key ORDER BY key LIMIT ${printExpr(bind(ctx, limit + 1))}`,
        ctx,
      );
      if (rows.length > limit) {
        throw requestError(
          "LIMIT_EXCEEDED",
          `more than ${limit} connected ${target.name} nodes match; narrow \`where\` or raise \`limit\``,
        );
      }
      if (rows.length > 0) {
        await deleteNodes(
          this,
          target,
          rows.map((r) => r["key"]),
        );
      }
    }
  }

  /** `update: [{ key, node }]`: connected nodes, through the update path. */
  async applyNodeUpdates(plan: WritePlan): Promise<void> {
    for (const u of plan.nodeUpdates) {
      const owner = this.env.model.nodes.get(u.rel.owner)!;
      const target = this.env.model.nodes.get(u.rel.target)!;
      const ctx = this.ctx();
      const rows = await this.run(
        seekThenExpand(
          "a",
          owner,
          printExpr(bind(ctx, u.from)),
          u.rel,
          "",
          "b",
          target,
        ) +
          (u.to !== undefined
            ? ` AND b.${name(target.key.property)} = ${printExpr(bind(ctx, u.to))}`
            : "") +
          `\nRETURN DISTINCT b.${name(target.key.property)} AS key`,
        ctx,
      );
      if (rows.length === 0) throw notConnected(owner, u.rel, target, u.to);
      const sub = this.plan();
      const done = await this.update(
        sub,
        target,
        rows.map((r) => ({ key: r["key"], input: u.input })),
      );
      if (done.length < rows.length) {
        throw notConnected(owner, u.rel, target, u.to);
      }
      await this.apply(sub);
    }
  }

  /**
   * Update nodes by key: visibility (filter rules) and BEFORE rules, then
   * one SET per shape of input. Returns the keys it updated; keys the
   * caller cannot see are skipped. Relationship inputs go into `plan`.
   */
  async update(
    plan: WritePlan,
    node: NodeType,
    rows: Array<{ key: unknown; input: Input }>,
    adjust?: Input,
  ): Promise<unknown[]> {
    if (rows.length === 0) return [];
    this.assertBatch(undefined, rows.length);
    const ctx0 = this.ctx();
    checkAuthentication(ctx0, node, "UPDATE");
    const fields = new Set<string>();
    for (const { input } of rows) {
      for (const name of Object.keys(input)) fields.add(name);
    }
    for (const name of Object.keys(adjust ?? {})) fields.add(name);
    const written = [...fields]
      .map((n) => node.fields.get(n))
      .filter((f): f is ScalarField => f?.kind === "scalar" && !f.key);
    for (const f of written) {
      checkFieldAuthentication(ctx0, node.name, f, "UPDATE");
    }

    // Visible to the updater (filter rules) and allowed before the write?
    const ctx = this.ctx();
    const before = and(
      authValidate(ctx, node, "n", "UPDATE", "BEFORE"),
      ...written.map((f) =>
        fieldValidate(ctx, node, f, "n", "UPDATE", "BEFORE"),
      ),
    );
    const found = await this.run(
      `UNWIND ${printExpr(
        bind(
          ctx,
          rows.map((r) => r.key),
        ),
      )} AS k\n` +
        `MATCH (n:${name(node.labels[0]!)}) WHERE n.${name(node.key.property)} = k` +
        // A node the caller cannot read is missing, not forbidden.
        andText(authFilter(ctx, node, "n", "READ")) +
        andText(authFilter(ctx, node, "n", "UPDATE")) +
        `\nRETURN n.${name(node.key.property)} AS key, ${before ? printExpr(before) : "true"} AS ok` +
        (node.subscriptionOptions.previousState
          ? ", properties(n) AS props"
          : ""),
      ctx,
    );
    if (found.some((r) => r["ok"] !== true)) throw forbidden(node, "UPDATE");
    if (node.subscriptionOptions.previousState) {
      for (const r of found) {
        (this.change.before ??= []).push({
          type: node.name,
          key: r["key"],
          properties: (r["props"] as Record<string, unknown>) ?? {},
        });
      }
    }
    const visible = new Set(found.map((r) => keyOf(r["key"])));
    const todo = rows.filter((r) => visible.has(keyOf(r.key)));
    if (todo.length === 0) return [];

    // Per row: properties to set (with @populatedBy) and to remove.
    const prepared: Array<{ key: unknown; set: Input; remove: string[] }> = [];
    for (const { key, input } of todo) {
      const set: Input = {};
      const remove: string[] = [];
      for (const f of node.fields.values()) {
        if (f.kind === "relationship") {
          const value = input[f.name];
          if (value && !settable(f, "UPDATE")) {
            throw requestError(
              "BAD_USER_INPUT",
              `${node.name}.${f.name} cannot be changed after create`,
            );
          }
          if (value) plan.relate(node, key, f, value as Input, true);
          continue;
        }
        if (f.kind !== "scalar" || f.key) continue;
        if (f.name in input) {
          if (!settable(f, "UPDATE")) {
            throw requestError(
              "BAD_USER_INPUT",
              `${node.name}.${f.name} cannot be changed after create`,
            );
          }
          if (adjust?.[f.name] != null) {
            throw requestError(
              "BAD_USER_INPUT",
              `${node.name}.${f.name} is both set and adjusted`,
            );
          }
          const value = input[f.name];
          if (value === null) {
            if (f.required) {
              throw requestError(
                "BAD_USER_INPUT",
                `${node.name}.${f.name} is required and cannot be null`,
              );
            }
            remove.push(f.property);
          } else {
            set[f.property] = toStored(f, value);
          }
        } else if (f.populatedBy?.operations.has("UPDATE")) {
          set[f.property] = toStored(
            f,
            await plan.callback(f, "UPDATE", node, key, input),
          );
        }
      }
      prepared.push({ key, set, remove });
    }

    const actx = this.ctx();
    const adjustments = compileAdjust(actx, node, adjust);
    const stamped = timestamps(node, "UPDATE");
    const suppliable = new Set(
      [...node.fields.values()]
        .filter(
          (f): f is ScalarField =>
            f.kind === "scalar" && !!f.timestamp && settable(f, "UPDATE"),
        )
        .map((f) => f.property),
    );
    // One statement per shape: which properties are removed.
    const shapes = new Map<string, typeof prepared>();
    for (const p of prepared) {
      const shape = p.remove.slice().sort().join("\0");
      const group = shapes.get(shape) ?? [];
      group.push(p);
      shapes.set(shape, group);
    }
    for (const [shape, group] of shapes) {
      const sctx = { ...actx, params: { ...actx.params } };
      const rowsParam = printExpr(
        bind(
          sctx,
          group.map((g) => ({ key: g.key, set: g.set })),
        ),
      );
      const sets = [
        "n += row.set",
        ...adjustments,
        // A @timestamp settable on update keeps a value the input supplied.
        ...stamped.map(([p, now]) =>
          suppliable.has(p)
            ? `n.${name(p)} = coalesce(row.set.${name(p)}, ${now})`
            : `n.${name(p)} = ${now}`,
        ),
      ];
      const removes = shape ? shape.split("\0") : [];
      await this.run(
        `UNWIND ${rowsParam} AS row\n` +
          `MATCH (n:${name(node.labels[0]!)}) WHERE n.${name(node.key.property)} = row.key\n` +
          `SET ${sets.join(", ")}` +
          (removes.length > 0
            ? `\nREMOVE ${removes.map((p) => `n.${name(p)}`).join(", ")}`
            : ""),
        sctx,
      );
    }
    const keys = todo.map((r) => r.key);
    this.info.nodesUpdated += keys.length;
    for (const key of keys) this.change.updated.push({ type: node.name, key });
    const entry = this.updated.get(node) ?? { keys: [], fields: new Set() };
    entry.keys.push(...keys);
    for (const f of written) entry.fields.add(f.name);
    this.updated.set(node, entry);
    return keys;
  }

  /**
   * AFTER rules on created nodes: the type's rules on every row, a field's
   * rules on the rows whose input sets the field. Rows are checked in
   * groups that set the same guarded fields.
   */
  async validateCreated(plan: WritePlan): Promise<void> {
    for (const [node, rows] of plan.creates) {
      const guarded = [...node.fields.values()]
        .filter((f) => f.kind === "scalar" && f.authorization)
        .map((f) => f.name);
      const groups = new Map<string, { fields: string[]; keys: unknown[] }>();
      for (const r of rows) {
        const fields = guarded.filter((f) => r.written.has(f));
        const id = fields.join("\0");
        const group = groups.get(id) ?? { fields, keys: [] };
        group.keys.push(r.key);
        groups.set(id, group);
      }
      for (const { fields, keys } of groups.values()) {
        await this.validateAfter(node, keys, "CREATE", new Set(fields));
      }
    }
  }

  async validateUpdated(): Promise<void> {
    for (const [node, { keys, fields }] of this.updated) {
      await this.validateAfter(node, keys, "UPDATE", fields);
    }
  }

  /** AFTER rules for `op` on the nodes with `keys` (and written fields). */
  async validateAfter(
    node: NodeType,
    keys: unknown[],
    op: "CREATE" | "UPDATE",
    fields: Set<string>,
  ): Promise<void> {
    if (keys.length === 0) return;
    const ctx = this.ctx();
    const rule = and(
      authValidate(ctx, node, "n", op, "AFTER"),
      ...[...fields].map((f) => {
        const field = node.fields.get(f);
        return field
          ? fieldValidate(ctx, node, field, "n", op, "AFTER")
          : undefined;
      }),
    );
    if (!rule) return;
    const text =
      `UNWIND ${printExpr(bind(ctx, dedupe(keys)))} AS k\n` +
      `MATCH (n:${name(node.labels[0]!)}) WHERE n.${name(node.key.property)} = k\n` +
      `RETURN n.${name(node.key.property)} AS key, ${printExpr(rule)} AS ok`;
    const rows = await this.run(text, ctx);
    if (rows.some((r) => r["ok"] !== true)) throw forbidden(node, op);
  }

  /** Nodes by key, projected for the payload, inside the transaction. */
  async payload(
    node: NodeType,
    keys: unknown[],
    sets: SelectionSetNode[],
  ): Promise<unknown[]> {
    if (sets.length === 0) return keys.map(() => ({}));
    const ctx = this.ctx();
    const compiled = compileByKeys(ctx, node, keys, sets);
    const statement = compiled.statements[0]!;
    const result = await runStatement(this.env, this.tx, statement);
    return assertReadable(compiled.shape([result]) as unknown[]);
  }

  /**
   * Keys of the nodes `where` matches, under the filter rules for `op`.
   * More than `limit` is an error: bulk writes are bounded, not truncated.
   */
  async resolveKeys(
    node: NodeType,
    where: Input,
    op: "UPDATE" | "DELETE",
    limit: number,
  ): Promise<unknown[]> {
    const ctx = this.ctx();
    const pred = compileNodeWhere(ctx, node, "this", where);
    if (!pred) {
      throw requestError(
        "BAD_USER_INPUT",
        `\`where\` must filter something: bulk ${op.toLowerCase()}s never apply to every ${node.name} by accident`,
      );
    }
    const rows = await this.run(
      `MATCH (this:${name(node.labels[0]!)}) WHERE ${printExpr(pred)}` +
        andText(authFilter(ctx, node, "this", "READ")) +
        andText(authFilter(ctx, node, "this", op)) +
        `\nRETURN this.${name(node.key.property)} AS key ORDER BY key LIMIT ${printExpr(bind(ctx, limit + 1))}`,
      ctx,
    );
    if (rows.length > limit) {
      throw requestError(
        "LIMIT_EXCEEDED",
        `more than ${limit} ${node.name} nodes match; narrow \`where\` or raise \`limit\``,
      );
    }
    return rows.map((r) => r["key"]);
  }
}

/**
 * A key no request names, of the key's type, for a node moved out of the
 * way by {@link Runner.moveHiddenKeys}: never committed.
 */
function placeholderKey(node: NodeType): unknown {
  if (node.key.type === "Int" || node.key.type === "BigInt") {
    const [hi, lo] = globalThis.crypto.getRandomValues(new Uint32Array(2));
    // Below -2^62: outside GraphQL's Int, and a BigInt key there is taken
    // with odds of 1 in 2^62 (the unique constraint would still catch it).
    return -(2n ** 62n) - ((BigInt(hi! & 0x3fffffff) << 32n) | BigInt(lo!));
  }
  return `\u0000lora-graphql:hidden:${globalThis.crypto.randomUUID()}`;
}
