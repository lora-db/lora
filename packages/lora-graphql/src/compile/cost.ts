// What a `where` costs, for the cost limit. A projection is charged the
// rows it returns (`projectNode`); a filter is charged the rows it must
// examine to find them:
//
// - a root filter no index answers (CONTAINS, ENDS_WITH, case-insensitive,
//   NOT, OR, a quantifier, a computed field) examines every node of the
//   label: its count from `analyze()`, or the page size without statistics;
// - a root filter an index answers by value (`eq`, `in`, `includes`,
//   `includesAny`) examines the nodes holding those values, from the
//   per-value counts `analyze()` keeps (G-73): an AND its narrowest test,
//   an OR of `includes` on one list (which the engine seeks per element,
//   G-72) the sum of its branches; the label without them;
// - every relationship a filter follows visits each candidate's related
//   nodes, once per level: the mean degree when the candidates are a scan
//   of the label, the maximum when the caller picked them (by key, or as
//   the parents of a nested list), and the page size without statistics.

import { RANGE_UNINDEXABLE } from "../analyze/indexes.js";
import { valueKey } from "../analyze/statistics.js";
import type {
  AbstractType,
  NodeType,
  RelationshipField,
} from "../model/types.js";
import type { CompileContext } from "./context.js";

type Where = Record<string, unknown>;

/** Default `maxFilterDepth`: relationship levels one `where` may nest. */
export const MAX_FILTER_DEPTH = 2;
/** Default `maxListFilter`: items an `in` filter operand may hold. */
export const MAX_LIST_FILTER = 1000;
/** Default `maxStringFilter`: characters a string filter operand may hold. */
export const MAX_STRING_FILTER = 10_000;

const SEEK_OPS = new Set([
  "eq",
  "in",
  "startsWith",
  "withinBBox",
  "distance",
  // A list's elements, in the RANGE index inferred for it (G-63, G-72).
  "includes",
  "includesAny",
]);
const RANGE_OPS = new Set(["lt", "lte", "gt", "gte"]);

/** Nodes of the type, from statistics; `fallback` without them. */
export function labelRows(
  ctx: CompileContext,
  node: NodeType,
  fallback: number,
): number {
  return ctx.statistics?.nodes[node.name] ?? fallback;
}

/**
 * Related nodes a filter visits per parent through `rel`: the maximum
 * degree when the caller chose the parents, the mean when they are every
 * node of the label, the declared `@cardinality` or the page size without
 * statistics. A single relationship visits one.
 */
export function filterDegree(
  ctx: CompileContext,
  rel: RelationshipField,
  chosen: boolean,
): number {
  if (!rel.list) return 1;
  const stats = ctx.statistics?.degrees[`${rel.owner}.${rel.name}`];
  if (stats) return Math.max(chosen ? stats.max : stats.mean, 1);
  if (rel.cardinality !== undefined) return Math.max(rel.cardinality, 1);
  const limit =
    rel.limit ??
    ctx.model.nodes.get(rel.target)?.limit ??
    ctx.model.abstracts.get(rel.target)?.limit;
  return limit?.default ?? 1;
}

/**
 * Related nodes the filter visits per candidate row, over every
 * relationship it follows, nested levels multiplied. Scalar predicates
 * add nothing (the row is examined anyway); a computed field adds its
 * statement, once per row.
 */
export function perRowCost(
  ctx: CompileContext,
  node: NodeType,
  where: Where | null | undefined,
  chosen = false,
): number {
  let cost = 0;
  for (const [key, value] of Object.entries(where ?? {})) {
    if (value === null || value === undefined) continue;
    if (key === "AND" || key === "OR") {
      for (const w of value as Where[])
        cost += perRowCost(ctx, node, w, chosen);
      continue;
    }
    if (key === "NOT") {
      cost += perRowCost(ctx, node, value as Where, chosen);
      continue;
    }
    const f = node.fields.get(key);
    if (f?.kind === "cypher" && f.computed) {
      // A filter on a computed field runs its statement for every row.
      cost += 1 + f.cost;
    } else if (f?.kind === "relationship") {
      cost += relationshipCost(ctx, f, value as Where, chosen);
    } else if (key.endsWith("Exists")) {
      cost += 1;
    } else if (key.endsWith("Connection")) {
      const rel = node.fields.get(key.slice(0, -"Connection".length));
      if (rel?.kind !== "relationship") continue;
      const d = filterDegree(ctx, rel, chosen);
      for (const inner of Object.values(value as Where)) {
        if (inner === null || inner === undefined) continue;
        const nodeWhere = (inner as Where)["node"] as Where | undefined;
        cost += d * (1 + targetCost(ctx, rel, nodeWhere));
      }
    }
  }
  return cost;
}

function relationshipCost(
  ctx: CompileContext,
  rel: RelationshipField,
  value: Where,
  chosen: boolean,
): number {
  const d = filterDegree(ctx, rel, chosen);
  if (!rel.list) return d * (1 + targetCost(ctx, rel, value));
  let cost = 0;
  for (const [quantifier, inner] of Object.entries(value)) {
    if (inner === null || inner === undefined) continue;
    if (quantifier === "count") cost += d;
    else if (quantifier === "aggregate") {
      cost += d * Math.max(1, Object.keys(inner as Where).length);
    } else cost += d * (1 + targetCost(ctx, rel, inner as Where));
  }
  return cost;
}

/** Per related node: the inner filter's cost, the costliest member's. */
function targetCost(
  ctx: CompileContext,
  rel: RelationshipField,
  where: Where | undefined,
): number {
  const node = ctx.model.nodes.get(rel.target);
  if (node) return perRowCost(ctx, node, where);
  const abstract = ctx.model.abstracts.get(rel.target);
  if (!abstract || !where) return 0;
  return Math.max(
    0,
    ...abstract.members.map((name) =>
      perRowCost(
        ctx,
        ctx.model.nodes.get(name)!,
        memberWhere(abstract, name, where),
      ),
    ),
  );
}

/** The part of an interface's or union's `where` one member sees. */
export function memberWhere(
  abstract: AbstractType,
  member: string,
  where: Where | null | undefined,
): Where | undefined {
  if (!where) return undefined;
  if (abstract.kind === "union") {
    return (where[member] as Where | null | undefined) ?? undefined;
  }
  return where;
}

/**
 * The extra cost of a root `where` on `node`: the label scan when no
 * index answers it, plus the related nodes it visits from each candidate.
 * `fallback` stands in for the label's size without statistics.
 */
export function rootFilterCost(
  ctx: CompileContext,
  node: NodeType,
  where: Where | null | undefined,
  fallback: number,
  /** Every match is read (a count or aggregate), not just a page. */
  all = false,
): number {
  const keyed = keyedRows(node, where);
  if (keyed !== undefined)
    return keyed * (1 + perRowCost(ctx, node, where, true));
  const label = labelRows(ctx, node, fallback);
  const rows = Math.min(
    label,
    anchoredRows(ctx, node, where) ?? label,
    valueRows(ctx, node, where) ?? label,
  );
  const scan = all || (hasPredicate(where) && !seeks(ctx, node, where!));
  return (scan ? rows : 0) + rows * perRowCost(ctx, node, where);
}

/**
 * Rows a root anchored on a related node's key starts from: the anchors
 * times the most nodes one of them reaches back (the inverse field's
 * maximum degree), when statistics measured it.
 */
function anchoredRows(
  ctx: CompileContext,
  node: NodeType,
  where: Where | null | undefined,
): number | undefined {
  for (const [key, value] of Object.entries(where ?? {})) {
    const anchor = anchorOf(ctx, node, key, value);
    if (!anchor) continue;
    const inverse = [...anchor.target.fields.values()].find(
      (f) =>
        f.kind === "relationship" &&
        f.list &&
        f.type === anchor.rel.type &&
        f.target === node.name &&
        f.direction !== anchor.rel.direction,
    );
    const stats =
      inverse &&
      ctx.statistics?.degrees[`${anchor.target.name}.${inverse.name}`];
    if (stats) return anchor.count * Math.max(stats.max, 1);
  }
  return undefined;
}

/**
 * Nodes a `where` selects by indexed value, from the per-value counts of
 * `analyze()`: a test the narrowest of its operators, an AND (or one
 * where's fields) the narrowest of its tests, an OR of `includes` on one
 * list the sum of its branches. Any other OR is a label scan in the
 * engine and narrows nothing. Undefined without statistics.
 */
function valueRows(
  ctx: CompileContext,
  node: NodeType,
  where: Where | null | undefined,
): number | undefined {
  if (!ctx.statistics?.values) return undefined;
  let rows: number | undefined;
  const narrow = (n: number | undefined) => {
    if (n !== undefined) rows = rows === undefined ? n : Math.min(rows, n);
  };
  for (const [key, value] of Object.entries(where ?? {})) {
    if (value === null || value === undefined) continue;
    if (key === "AND") {
      for (const w of value as Where[]) narrow(valueRows(ctx, node, w));
    } else if (key === "OR") {
      if (!includesOr(node, value as Where[])) continue;
      const branches = (value as Where[]).map((w) => valueRows(ctx, node, w));
      if (branches.every((n) => n !== undefined)) {
        narrow(branches.reduce((a, b) => a! + b!, 0));
      }
    } else {
      narrow(fieldRows(ctx, node, key, value as Where));
    }
  }
  return rows;
}

/**
 * Whether an OR's branches each test one list field, the same one, with
 * `includes` or `includesAny` only: the engine seeks it element by element
 * (G-72).
 */
function includesOr(node: NodeType, branches: Where[]): boolean {
  let field: string | undefined;
  return (
    branches.length > 0 &&
    branches.every((branch) => {
      const tests = Object.entries(branch).filter(([, v]) => v != null);
      if (tests.length !== 1) return false;
      const [key, ops] = tests[0]!;
      const f = node.fields.get(key);
      if (f?.kind !== "scalar" || !f.list) return false;
      if ((field ??= key) !== key) return false;
      const used = Object.entries(ops as Where).filter(([, v]) => v != null);
      return (
        used.length > 0 &&
        used.every(([op]) => op === "includes" || op === "includesAny")
      );
    })
  );
}

/** Nodes one field's operators select by value, from its counts. */
function fieldRows(
  ctx: CompileContext,
  node: NodeType,
  key: string,
  ops: Where,
): number | undefined {
  const values = ctx.statistics?.values?.[`${node.name}.${key}`];
  if (!values) return undefined;
  const held = (v: unknown) => values.counts[valueKey(v)] ?? values.rest;
  const sum = (vs: unknown) =>
    Array.isArray(vs) ? vs.reduce((n: number, v) => n + held(v), 0) : undefined;
  let rows: number | undefined;
  for (const [op, operand] of Object.entries(ops)) {
    if (operand === null || operand === undefined) continue;
    const n =
      op === "eq" || op === "includes"
        ? held(operand)
        : op === "in" || op === "includesAny"
          ? sum(operand)
          : undefined;
    if (n !== undefined) rows = rows === undefined ? n : Math.min(rows, n);
  }
  return rows;
}

/** A relationship filter naming related nodes by key, as the root seeks it. */
function anchorOf(
  ctx: CompileContext,
  node: NodeType,
  key: string,
  value: unknown,
): { rel: RelationshipField; target: NodeType; count: number } | undefined {
  const rel = node.fields.get(key);
  if (rel?.kind !== "relationship" || value == null) return undefined;
  if (ctx.model.abstracts.has(rel.target)) return undefined;
  const target = ctx.model.nodes.get(rel.target)!;
  const inner = rel.list
    ? ((value as Where)["some"] ?? (value as Where)["single"])
    : value;
  const ops = (inner as Where | undefined)?.[target.key.name] as
    | Where
    | undefined;
  if (ops?.["eq"] != null) return { rel, target, count: 1 };
  if (Array.isArray(ops?.["in"])) {
    return { rel, target, count: ops["in"].length };
  }
  return undefined;
}

/** Rows an exact @key or @unique seek returns, when the where has one. */
function keyedRows(
  node: NodeType,
  where: Where | null | undefined,
): number | undefined {
  for (const [key, value] of Object.entries(where ?? {})) {
    const f = node.fields.get(key);
    if (f?.kind !== "scalar" || value == null) continue;
    if (key !== node.key.name && !f.unique) continue;
    const ops = value as Where;
    if (ops["eq"] != null) return 1;
    if (Array.isArray(ops["in"])) return ops["in"].length;
  }
  return undefined;
}

function hasPredicate(where: Where | null | undefined): boolean {
  return Object.values(where ?? {}).some((v) => v !== null && v !== undefined);
}

/**
 * Whether an index narrows the root: an equality, range, prefix or point
 * predicate on a stored field, or a relationship filter naming a related
 * node by key (the root then starts from that node). CONTAINS and
 * ENDS_WITH are charged as scans even with a TEXT index: a short operand
 * matches most of the label.
 */
function seeks(ctx: CompileContext, node: NodeType, where: Where): boolean {
  for (const [key, value] of Object.entries(where)) {
    if (value === null || value === undefined) continue;
    const f = node.fields.get(key);
    if (f?.kind === "cypher" && f.computed) return false;
  }
  for (const [key, value] of Object.entries(where)) {
    if (value === null || value === undefined) continue;
    const f = node.fields.get(key);
    if (f?.kind === "scalar") {
      for (const [op, operand] of Object.entries(value as Where)) {
        if (operand === null || operand === undefined) continue;
        if (SEEK_OPS.has(op)) return true;
        if (RANGE_OPS.has(op) && !RANGE_UNINDEXABLE.has(f.type)) return true;
      }
    } else if (key === "OR" && includesOr(node, value as Where[])) {
      return true;
    } else if (anchorOf(ctx, node, key, value)) {
      return true;
    }
  }
  return false;
}
