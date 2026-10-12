// Connecting nodes: the statement that links a relationship field's pairs
// and the rule checks around it, on both ends and on the field. In a
// transaction each is awaited and judged in JavaScript; in a single call
// they are all issued at once, each decision an assertion.

import {
  authFilter,
  authValidate,
  forbidden,
  relationshipRules,
} from "../../compile/auth.js";
import { bind, type CompileContext } from "../../compile/context.js";
import { lit, name, printExpr, type Expr } from "../../compile/cypher.js";
import { keyOf } from "../../compile/read/by-keys.js";
import type { NodeType, RelationshipField } from "../../model/types.js";
import { noTarget, refusedRule } from "./errors.js";
import { andText, arrow, orFresh, relRef, timestamps } from "./fragments.js";
import type { Link, WritePlan } from "./plan.js";
import type { Runner } from "./runner.js";
import { refusal } from "./single-call.js";

/** `plan`'s links per relationship field, each pair once. */
function linkGroups(plan: WritePlan): Map<RelationshipField, Link[]> {
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
  return groups;
}

/**
 * The links of a single call: what {@link applyLinks} does, with
 * nothing awaited. The rules on both ends and on the relationship field
 * are assertions, and so is every target being there: the link reports
 * the rows it connected in the message of the assertion that fails
 * when one is missing.
 */
export function linkInOne(runner: Runner, plan: WritePlan): void {
  runner.linking();
  const issue = (
    statement: { text: string; ctx: CompileContext } | undefined,
  ) => {
    if (statement) {
      runner.defer(runner.run(statement.text, statement.ctx));
    }
  };
  for (const [rel, links] of linkGroups(plan)) {
    const owner = runner.env.model.nodes.get(rel.owner)!;
    const target = runner.env.model.nodes.get(rel.target)!;
    const pairs = links.map((l) => ({ from: l.from, to: l.to }));
    const ends = (when: "BEFORE" | "AFTER") =>
      runner.inPlace(() =>
        issue(
          endsStatement(
            runner,
            "CREATE_RELATIONSHIP",
            owner,
            target,
            pairs,
            when,
          ),
        ),
      );
    ends("BEFORE");
    const missing = (detail: string) => {
      const linked = new Set(detail.split(",").filter(Boolean).map(Number));
      const found = new Set(
        links.filter((_, i) => linked.has(i)).map((l) => keyOf(l.to)),
      );
      return noTarget(
        owner,
        rel,
        target,
        links.map((l) => l.to).filter((k) => !found.has(keyOf(k))),
      );
    };
    if (plan.reconnects) {
      // One statement connects, tells the new relationships from those
      // that were there, and holds each to its rules: a new one to
      // CONNECT, one that was there and is given properties to
      // UPDATE_EDGE. In that order, after every target was found.
      runner.inPlace(() => {
        const { text, ctx } = linkStatement(runner, plan, rel, links, (ctx) => {
          const rule = (op: "CONNECT" | "UPDATE_EDGE") => {
            try {
              const cond = relationshipRules(ctx, rel, op, {
                owner: "a",
                target: "b",
                rel: "r",
              });
              return {
                ok:
                  cond === undefined
                    ? "true"
                    : `coalesce(${printExpr(cond === false ? lit(false) : cond)}, false)`,
                error: (): unknown => refusedRule(op, rel),
              };
            } catch (err) {
              // The claims refuse: an error once a row comes to it.
              return { ok: "false", error: (): unknown => err };
            }
          };
          const connect = rule("CONNECT");
          const edge = rule("UPDATE_EDGE");
          return (
            `WITH row, existed, ${connect.ok} AS c, (NOT row.sets OR ${edge.ok}) AS e\n` +
            `WITH collect({ i: row.i, existed: existed, c: c, e: e }) AS linked\n` +
            `RETURN linked, assert(size(linked) = ${links.length}, ${runner.assertions.failsAs(
              missing,
              "reduce(s = '', l IN linked | s + toString(l.i) + ',')",
            )}) AS found` +
            `, assert(all(l IN linked WHERE l.existed OR l.c), ${runner.assertions.failsAs(connect.error)}) AS connect` +
            `, assert(all(l IN linked WHERE NOT l.existed OR l.e), ${runner.assertions.failsAs(edge.error)}) AS edge`
          );
        });
        runner.defer(
          runner.run(text, ctx).then(([row]) => {
            const linked = (row?.["linked"] ?? []) as Array<{
              existed: boolean;
            }>;
            runner.info.relationshipsCreated += linked.filter(
              (l) => l.existed !== true,
            ).length;
          }),
        );
      });
      ends("AFTER");
      for (const l of links) runner.connected.push(relRef(rel, l.from, l.to));
      continue;
    }
    runner.inPlace(() => {
      issue(
        linkStatement(
          runner,
          plan,
          rel,
          links,
          `WITH collect(row.i) AS linked\n` +
            `RETURN assert(size(linked) = ${links.length}, ${runner.assertions.failsAs(
              missing,
              "reduce(s = '', i IN linked | s + toString(i) + ',')",
            )}) AS ok`,
        ),
      );
    });
    runner.inPlace(() => issue(rulesStatement(runner, "CONNECT", rel, pairs)));
    ends("AFTER");
    runner.info.relationshipsCreated += links.length;
    for (const l of links) runner.connected.push(relRef(rel, l.from, l.to));
  }
}

/**
 * The statement that connects `links`. A transaction reads back which
 * pairs it connected and whether each was connected before; a single
 * call ends in `ending` instead: text when its links are all new, a
 * function of the statement's context when it must tell (`existed`).
 */
function linkStatement(
  runner: Runner,
  plan: WritePlan,
  rel: RelationshipField,
  links: Link[],
  ending?: string | ((ctx: CompileContext) => string),
): { text: string; ctx: CompileContext } {
  const owner = runner.env.model.nodes.get(rel.owner)!;
  const target = runner.env.model.nodes.get(rel.target)!;
  // Connecting an already-connected pair keeps one relationship and
  // updates its properties: MERGE on both bound ends (E14 is fixed).
  // `existed` tells a new relationship from an updated one.
  const lctx = runner.ctx();
  const rows = printExpr(
    bind(
      lctx,
      links.map((l, i) => ({
        i,
        from: l.from,
        to: l.to,
        sets: Object.keys(l.props).length > 0,
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
    ? runner.env.model.relationshipProperties.get(rel.properties)
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
    (typeof ending === "string"
      ? ""
      : `\nWITH a, b, row, size([(a)${arrow(rel, "", "b", undefined)} | 1]) > 0 AS existed`) +
    `\nMERGE (a)${arrow(rel, "r", "b", undefined)}\n` +
    // Defaults apply when the relationship is created, never to an
    // existing one (a re-connect keeps what it has).
    `ON CREATE SET ${onCreate.join(", ")}\n` +
    (onMatch.length > 0 ? `ON MATCH SET ${onMatch.join(", ")}\n` : "") +
    `SET r += row.props\n` +
    (typeof ending === "function"
      ? ending(lctx)
      : (ending ?? `RETURN row.from AS from, row.to AS key, existed`));
  return { text, ctx: lctx };
}

/**
 * Connect `plan`'s links in a transaction, a relationship field at a
 * time: the rules on both ends, the MERGE, then the rules on what it
 * created or found.
 */
export async function applyLinks(
  runner: Runner,
  plan: WritePlan,
): Promise<void> {
  for (const [rel, links] of linkGroups(plan)) {
    const owner = runner.env.model.nodes.get(rel.owner)!;
    const target = runner.env.model.nodes.get(rel.target)!;
    const pairs = links.map((l) => ({ from: l.from, to: l.to }));
    await checkEnds(
      runner,
      "CREATE_RELATIONSHIP",
      owner,
      target,
      pairs,
      "BEFORE",
    );
    const { text, ctx: lctx } = linkStatement(runner, plan, rel, links);
    const linked = await runner.run(text, lctx);
    // Rules on the properties set, now that new and existing
    // relationships are told apart; a refusal rolls the mutation back.
    const byPair = new Map(
      links.map((l) => [`${keyOf(l.from)}\0${keyOf(l.to)}`, l]),
    );
    for (const row of linked) {
      const refused = byPair.get(
        `${keyOf(row["from"])}\0${keyOf(row["key"])}`,
      )?.refused;
      const error = row["existed"] === true ? refused?.update : refused?.create;
      if (error) throw error;
    }
    if (linked.length < links.length) {
      const found = new Set(linked.map((r) => keyOf(r["key"])));
      throw noTarget(
        owner,
        rel,
        target,
        links.map((l) => l.to).filter((k) => !found.has(keyOf(k))),
      );
    }
    // Rules on the relationship field: a new relationship is a CONNECT,
    // an existing one given properties an UPDATE_EDGE.
    await checkRelationshipRules(
      runner,
      "CONNECT",
      rel,
      linked
        .filter((r) => r["existed"] !== true)
        .map((r) => ({ from: r["from"], to: r["key"] })),
    );
    await checkRelationshipRules(
      runner,
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
    await checkEnds(
      runner,
      "CREATE_RELATIONSHIP",
      owner,
      target,
      pairs,
      "AFTER",
    );
    const made = linked.filter((r) => r["existed"] !== true);
    runner.info.relationshipsCreated += made.length;
    for (const r of made) runner.linked.push(relRef(rel, r["from"], r["key"]));
    for (const l of links) runner.connected.push(relRef(rel, l.from, l.to));
  }
}

/**
 * Rules on the relationship field (and on a field declaring the same
 * relationship from the other side) for `op`, on each pair's
 * relationship as it stands: after the write for CONNECT and
 * UPDATE_EDGE, before it for DISCONNECT. FORBIDDEN when any fails; the
 * mutation rolls back.
 */
export async function checkRelationshipRules(
  runner: Runner,
  op: "CONNECT" | "DISCONNECT" | "UPDATE_EDGE",
  rel: RelationshipField,
  pairs: Array<{ from: unknown; to: unknown }>,
): Promise<void> {
  const statement = rulesStatement(runner, op, rel, pairs);
  if (!statement) return;
  const rows = await runner.run(statement.text, statement.ctx);
  if (rows.some((r) => r["ok"] !== true)) throw refusedRule(op, rel);
}

/**
 * The statement of {@link checkRelationshipRules}, when a rule
 * applies: a row per pair, `ok` when its rules hold. In a single call
 * the row that fails them fails the statement.
 */
function rulesStatement(
  runner: Runner,
  op: "CONNECT" | "DISCONNECT" | "UPDATE_EDGE",
  rel: RelationshipField,
  pairs: Array<{ from: unknown; to: unknown }>,
): { text: string; ctx: CompileContext } | undefined {
  if (pairs.length === 0) return undefined;
  const ctx = runner.ctx();
  const cond = relationshipRules(ctx, rel, op, {
    owner: "a",
    target: "b",
    rel: "r",
  });
  if (cond === undefined) return undefined;
  const owner = runner.env.model.nodes.get(rel.owner)!;
  const target = runner.env.model.nodes.get(rel.target)!;
  const ok = `coalesce(${printExpr(cond === false ? lit(false) : cond)}, false)`;
  return {
    text:
      `UNWIND ${printExpr(bind(ctx, pairs))} AS row\n` +
      `MATCH (a:${name(owner.labels[0]!)}) WHERE a.${name(owner.key.property)} = row.from\n` +
      `MATCH (b:${name(target.labels[0]!)}) WHERE b.${name(target.key.property)} = row.to\n` +
      `MATCH (a)${arrow(rel, "r", "b", undefined)}\n` +
      `RETURN ${
        runner.single
          ? `assert(${ok}, ${runner.assertions.failsAs(() => refusedRule(op, rel))})`
          : ok
      } AS ok`,
    ctx,
  };
}

/** Whether either end has validate rules for `op` at `when`. */
export function endRules(
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
export async function checkEnds(
  runner: Runner,
  op: "CREATE_RELATIONSHIP" | "DELETE_RELATIONSHIP",
  owner: NodeType,
  target: NodeType,
  pairs: Array<{ from: unknown; to: unknown }>,
  when: "BEFORE" | "AFTER",
): Promise<void> {
  const statement = endsStatement(runner, op, owner, target, pairs, when);
  if (!statement) return;
  for (const row of await runner.run(statement.text, statement.ctx)) {
    if (row["a"] !== true) throw forbidden(owner, op);
    if (row["b"] !== true) throw forbidden(target, op);
  }
}

/**
 * The statement of {@link checkEnds}, when either end has a
 * rule: a row per pair, `a` and `b` whether each end's rules hold. In a
 * single call the end that fails them fails the statement.
 */
function endsStatement(
  runner: Runner,
  op: "CREATE_RELATIONSHIP" | "DELETE_RELATIONSHIP",
  owner: NodeType,
  target: NodeType,
  pairs: Array<{ from: unknown; to: unknown }>,
  when: "BEFORE" | "AFTER",
): { text: string; ctx: CompileContext } | undefined {
  if (pairs.length === 0) return undefined;
  const ctx = runner.ctx();
  const a = authValidate(ctx, owner, "a", op, when);
  const b = authValidate(ctx, target, "b", op, when);
  if (!a && !b) return undefined;
  const holds = (rule: Expr | undefined, node: NodeType) =>
    !rule
      ? "true"
      : runner.single
        ? `assert(${printExpr(rule)}, ${printExpr(lit(refusal("forbidden", node.name, op)))})`
        : printExpr(rule);
  return {
    text:
      `UNWIND ${printExpr(bind(ctx, pairs))} AS row\n` +
      `MATCH (a:${name(owner.labels[0]!)}) WHERE a.${name(owner.key.property)} = row.from\n` +
      `MATCH (b:${name(target.labels[0]!)}) WHERE b.${name(target.key.property)} = row.to\n` +
      `RETURN ${holds(a, owner)} AS a, ${holds(b, target)} AS b`,
    ctx,
  };
}
