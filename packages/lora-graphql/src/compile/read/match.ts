// The root MATCH of a read: what it anchors on, its filters and keyset,
// and how its plan is expected to find the first rows.

import { RANGE_UNINDEXABLE } from "../../analyze/indexes.js";
import type { CypherField, NodeType } from "../../model/types.js";
import { authFilter, checkAuthentication } from "../auth.js";
import { bind, freshVar, type CompileContext } from "../context.js";
import { and, bin, lit, prop, v, type Clause, type Expr } from "../cypher.js";
import { compileNodeWhere } from "../filter.js";
import type { SeekExpectation, Where } from "./types.js";
import { type SortKey, leadBound, keysetPredicate } from "./sort.js";
import { keyOf } from "./by-keys.js";
import { projectCypher } from "./cypher-field.js";

// ---------------------------------------------------------------------------
// The root MATCH: anchoring, filters, keyset
// ---------------------------------------------------------------------------

const EXACT_OPS = new Set(["eq", "in"]);

const RANGE_OPS = new Set(["lt", "lte", "gt", "gte"]);

const TEXT_OPS = new Set(["contains", "startsWith", "endsWith"]);

const POINT_OPS = new Set(["withinBBox", "distance"]);

export function rootMatch(
  ctx: CompileContext,
  node: NodeType,
  where: Where | null | undefined,
  sort: SortKey[],
  cursor?: unknown[],
): {
  clauses: Clause[];
  expectation: Omit<SeekExpectation, "statement"> | undefined;
  /** Computed values the following clauses must carry. */
  carry: string[];
} {
  checkAuthentication(ctx, node, "READ");
  const label = node.labels[0]!;
  ctx.reads.labels.add(label);
  const clauses: Clause[] = [];
  let anchor: Expr | undefined;
  let expectation: Omit<SeekExpectation, "statement"> | undefined;
  const rest = where ?? undefined;

  // Computed (@cypher) fields the filter or sort uses: each runs once per
  // node, in a CALL after the MATCH, and the filter moves after them.
  const computed = computedFields(node, rest, sort);
  const carry: string[] = [];
  const computedClauses: Clause[] = [];
  for (const field of computed) {
    const projection = projectCypher(ctx, "this", field, field.name, {}, [], 1);
    const out = (projection.expr as { name: string }).name;
    ctx.computed.set(`this\0${field.name}`, out);
    for (const k of sort) if (k.field.computedBy === field) k.computed = out;
    computedClauses.push(...projection.pre);
    carry.push(out);
  }

  const predicate = compileNodeWhere(ctx, node, "this", rest);
  if (computed.length === 0) {
    expectation ??= rest ? seekFromWhere(node, rest) : undefined;
  }

  // Nothing to seek on, but a relationship filter names a related node by
  // key: start from that node and expand, instead of scanning the label.
  // The filter itself stays in the WHERE, so the result is unchanged.
  let related: RelatedAnchor | undefined;
  if (!expectation && !anchor && rest && computed.length === 0) {
    related = findRelatedAnchor(ctx, node, rest);
    if (related) {
      clauses.push(...related.clauses);
      expectation = related.expectation;
    }
  }

  let keyset: Expr | undefined;
  if (cursor) {
    keyset = keysetPredicate(ctx, "this", sort, cursor);
    if (
      computed.length === 0 &&
      !expectation &&
      leadBound(sort[0]!) &&
      !RANGE_UNINDEXABLE.has(sort[0]!.field.type)
    ) {
      expectation = {
        label,
        access: "range",
        reason: `keyset on ${node.name}.${sort[0]!.field.name}`,
      };
    }
  }

  // Ordered ascending by a string that is always present (the key, or a
  // non-null @sortable field with its existence constraint) and nothing
  // else to seek on: `>= ""` excludes no row, and lets the planner walk the
  // index in order and stop after `limit` rows instead of sorting the label.
  let bound: Expr | undefined;
  const lead = sort[0];
  if (
    !predicate &&
    !anchor &&
    !keyset &&
    !related &&
    computed.length === 0 &&
    lead &&
    lead.direction === "ASC" &&
    leadBound(lead) &&
    (lead.field.type === "String" || lead.field.type === "ID")
  ) {
    bound = bin(">=", prop(v("this"), lead.field.property), lit(""));
    expectation = {
      label,
      access: "range",
      reason: `ordered by ${node.name}.${lead.field.name}`,
    };
  }

  clauses.push({
    kind: "match",
    pattern: related
      ? related.pattern
      : { start: { variable: "this", labels: [label] }, hops: [] },
    where: and(
      anchor,
      computed.length === 0 ? predicate : undefined,
      computed.length === 0 ? keyset : undefined,
      bound,
      authFilter(ctx, node, "this", "READ"),
    ),
  });
  if (related) {
    // Several relationships may lead to the same node.
    clauses.push({
      kind: "with",
      distinct: true,
      items: [{ expr: v("this") }],
    });
  }
  if (computed.length > 0) {
    clauses.push(...computedClauses, {
      kind: "with",
      items: [{ expr: v("this") }, ...carry.map((c) => ({ expr: v(c) }))],
      where: and(predicate, keyset),
    });
  }
  return { clauses, expectation, carry };
}

/** The computed (`@cypher`) fields a root filter or sort refers to. */
function computedFields(
  node: NodeType,
  where: Where | undefined,
  sort: SortKey[],
): CypherField[] {
  const out = new Set<CypherField>();
  const walk = (w: Where | undefined) => {
    for (const [key, value] of Object.entries(w ?? {})) {
      if (value === null || value === undefined) continue;
      if (key === "AND" || key === "OR") {
        for (const item of value as Where[]) walk(item);
      } else if (key === "NOT") {
        walk(value as Where);
      } else {
        const f = node.fields.get(key);
        if (f?.kind === "cypher" && f.computed) out.add(f);
      }
    }
  };
  walk(where);
  for (const k of sort) if (k.field.computedBy) out.add(k.field.computedBy);
  return [...out];
}

interface RelatedAnchor {
  clauses: Clause[];
  pattern: import("../cypher.js").Pattern;
  expectation: Omit<SeekExpectation, "statement">;
}

function findRelatedAnchor(
  ctx: CompileContext,
  node: NodeType,
  where: Where,
): RelatedAnchor | undefined {
  for (const [key, value] of Object.entries(where)) {
    const rel = node.fields.get(key);
    if (rel?.kind !== "relationship" || value == null) continue;
    // Through an interface or union there is no single key to seek on.
    if (ctx.model.abstracts.has(rel.target)) continue;
    const target = ctx.model.nodes.get(rel.target)!;
    const inner = rel.list
      ? ((value as Where)["some"] ?? (value as Where)["single"])
      : value;
    if (!inner || typeof inner !== "object") continue;
    const ops = (inner as Where)[target.key.name] as Where | undefined;
    const eq = ops?.["eq"];
    const inList = ops?.["in"];
    if (eq == null && !Array.isArray(inList)) continue;

    const a = freshVar(ctx, `this_${rel.name}_anchor`);
    const clauses: Clause[] = [];
    let seek: Expr;
    if (eq != null) {
      seek = bin("=", prop(v(a), target.key.property), bind(ctx, eq));
    } else {
      // `IN` seeks once per distinct value (E18).
      seek = bin(
        "IN",
        prop(v(a), target.key.property),
        bind(ctx, dedupe(inList as unknown[])),
      );
    }
    clauses.push({
      kind: "match",
      pattern: {
        start: { variable: a, labels: [target.labels[0]!] },
        hops: [],
      },
      where: seek,
    });
    return {
      clauses,
      // (anchor)<-[:T]-(this:Label): the field's direction, reversed.
      pattern: {
        start: { variable: a, labels: [] },
        hops: [
          {
            rel: {
              type: rel.type,
              direction: rel.direction === "OUT" ? "IN" : "OUT",
            },
            node: { variable: "this", labels: [node.labels[0]!] },
          },
        ],
      },
      expectation: {
        label: target.labels[0]!,
        access: "exact",
        reason: `anchored on ${target.name}.${target.key.name} through ${node.name}.${rel.name}`,
      },
    };
  }
  return undefined;
}

function seekFromWhere(
  node: NodeType,
  where: Where,
): Omit<SeekExpectation, "statement"> | undefined {
  const label = node.labels[0]!;
  for (const access of ["exact", "range", "text", "point"] as const) {
    for (const [key, value] of Object.entries(where)) {
      const field = node.fields.get(key);
      if (field?.kind !== "scalar" || value == null) continue;
      const ops = Object.entries(value as Where)
        .filter(([, x]) => x != null)
        .map(([op]) => op);
      const family = {
        exact: EXACT_OPS,
        range: RANGE_OPS,
        text: TEXT_OPS,
        point: POINT_OPS,
      }[access];
      if (access === "range" && RANGE_UNINDEXABLE.has(field.type)) continue;
      const op = ops.find((o) => family.has(o));
      if (op) {
        return {
          label,
          access,
          reason: `${node.name}.${field.name} ${op}`,
        };
      }
    }
  }
  return undefined;
}

function dedupe(values: unknown[]): unknown[] {
  const seen = new Set<string>();
  return values.filter((x) => {
    const k = keyOf(x);
    if (seen.has(k)) return false;
    seen.add(k);
    return true;
  });
}
