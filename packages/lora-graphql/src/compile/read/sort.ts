// Sort keys, limits and paging: `sort`, `limit`, `first` / `after`,
// `last` / `before`, and the keyset predicate a cursor turns into.

import { requestError } from "../../errors.js";
import type {
  NodeType,
  RelationshipPropertiesType,
  PageLimit,
  ScalarField,
} from "../../model/types.js";
import { checkFieldAuthentication, checkPropertyAccess } from "../auth.js";
import { bind, type CompileContext } from "../context.js";
import { decodeCursor } from "../cursor.js";
import {
  and,
  bin,
  isNull,
  lit,
  or,
  prop,
  v,
  type Expr,
  type SortItem,
} from "../cypher.js";
import type { Args } from "./types.js";
import { refuseRowRules } from "./common.js";

export interface SortKey {
  field: ScalarField;
  direction: "ASC" | "DESC";
  /** A relationship property (`sort: [{ edge: { ... } }]`), not a node field. */
  edge?: boolean;
  /** For a computed (`@cypher`) field: the variable holding its value. */
  computed?: string;
}

/** The value a sort key orders by: `node.prop`, or `rel.prop` for edge keys. */
function keyExpr(k: SortKey, variable: string, edge?: string): Expr {
  if (k.field.computedBy) {
    if (!k.computed) {
      throw requestError(
        "BAD_USER_INPUT",
        `${k.field.computedBy.owner}.${k.field.name} is computed by @cypher: sort by it in a root field only`,
      );
    }
    return v(k.computed);
  }
  return prop(v(k.edge ? edge! : variable), k.field.property);
}

// ---------------------------------------------------------------------------
// Sorting, limits, keyset
// ---------------------------------------------------------------------------

export function resolveLimit(
  value: unknown,
  limit: PageLimit,
  argument: string,
): number {
  if (value === null || value === undefined) return limit.default;
  const n = value as number;
  if (!Number.isInteger(n) || n < 0) {
    throw requestError(
      "BAD_USER_INPUT",
      `\`${argument}\` must be a non-negative integer`,
    );
  }
  if (n > limit.max) {
    throw requestError(
      "LIMIT_EXCEEDED",
      `\`${argument}\` is ${n}; the maximum is ${limit.max}`,
    );
  }
  return n;
}

/**
 * The sort keys for `value`. Connections (`tieBreak`) always end on a
 * unique field so cursors are stable; plain lists do not, because LoraDB
 * 0.15 streams an index in order only for a single sort key, and a
 * tie-breaker would turn a `LIMIT` into a sort of the whole label.
 */
export function resolveSort(
  ctx: CompileContext,
  node: NodeType,
  value: unknown,
  tieBreak: boolean,
  props?: RelationshipPropertiesType,
): SortKey[] {
  const keys: SortKey[] = [];
  const one = (item: Record<string, unknown>): [string, unknown] => {
    const entries = Object.entries(item).filter(([, d]) => d != null);
    if (entries.length !== 1) {
      throw requestError(
        "BAD_USER_INPUT",
        "each `sort` item names exactly one field",
      );
    }
    return entries[0]!;
  };
  for (const item of (value as Array<Record<string, unknown>> | null) ?? []) {
    const [fieldName, direction] = one(item);
    if (fieldName === "edge" && props) {
      const [propName, dir] = one(direction as Record<string, unknown>);
      const field = props.fields.get(propName)!;
      checkPropertyAccess(ctx, props.name, field, "READ");
      if (keys.some((k) => k.edge && k.field === field)) {
        throw requestError(
          "BAD_USER_INPUT",
          `\`sort\` names edge.${propName} more than once`,
        );
      }
      keys.push({ field, direction: dir as "ASC" | "DESC", edge: true });
      continue;
    }
    const declared = node.fields.get(fieldName)!;
    const field =
      declared.kind === "cypher"
        ? declared.computed!
        : (declared as ScalarField);
    // Ordering by a field reveals it, and cursors carry its values.
    checkFieldAuthentication(ctx, node.name, declared);
    refuseRowRules(ctx, node, field, "sort by");
    if (keys.some((k) => !k.edge && k.field === field)) {
      throw requestError(
        "BAD_USER_INPUT",
        `\`sort\` names ${fieldName} more than once`,
      );
    }
    keys.push({ field, direction: direction as "ASC" | "DESC" });
    // A unique, always-present field orders totally: later items could
    // never break a tie. (Unique fields may repeat null, so a nullable
    // one still needs the key.)
    if (field.key || (field.unique && field.required)) return keys;
  }
  if (tieBreak || keys.length === 0) {
    keys.push({ field: node.key, direction: "ASC" });
  }
  return keys;
}

/**
 * `first` / `after`, or `last` / `before`. A backward page runs the sort
 * reversed from the cursor and is flipped back by the resolvers; cursors
 * carry the requested sort's signature, so they work in both directions.
 */
export function resolvePage(
  ctx: CompileContext,
  node: NodeType,
  args: Args,
  limit: PageLimit,
  props?: RelationshipPropertiesType,
): {
  first: number;
  sort: SortKey[];
  signature: string;
  cursor: unknown[] | undefined;
  backward: boolean;
} {
  const backward = args["last"] != null || args["before"] != null;
  if (backward && (args["first"] != null || args["after"] != null)) {
    throw requestError(
      "BAD_USER_INPUT",
      "page forward with first/after or backward with last/before, not both",
    );
  }
  const first = resolveLimit(
    backward ? args["last"] : args["first"],
    limit,
    backward ? "last" : "first",
  );
  const requested = resolveSort(ctx, node, args["sort"], true, props);
  const signature = sortSignature(requested);
  const raw = (backward ? args["before"] : args["after"]) as
    | string
    | null
    | undefined;
  const cursor =
    raw != null
      ? decodeCursor(raw, signature, requested.length, ctx.model.cursorSecret)
      : undefined;
  const sort = backward
    ? requested.map((k) => ({
        ...k,
        direction: k.direction === "ASC" ? ("DESC" as const) : ("ASC" as const),
      }))
    : requested;
  return { first, sort, signature, cursor, backward };
}

function sortSignature(sort: SortKey[]): string {
  return sort
    .map((k) => `${k.edge ? "edge." : ""}${k.field.name}:${k.direction}`)
    .join(",");
}

export function sortItems(
  variable: string,
  sort: SortKey[],
  edge?: string,
): SortItem[] {
  return sort.map((k) => ({
    expr: keyExpr(k, variable, edge),
    direction: k.direction,
  }));
}

export function cursorValues(
  variable: string,
  sort: SortKey[],
  edge?: string,
): Expr {
  return {
    kind: "list",
    items: sort.map((k) => keyExpr(k, variable, edge)),
  };
}

/** Whether a sort key is guaranteed present, so a `>=` bound loses no rows. */
export function leadBound(k: SortKey): boolean {
  return k.field.key || (k.field.required && k.field.sortable);
}

/**
 * Rows strictly after `values` in `sort` order, written out term by term
 * (`[a, b] > $list` silently matches nothing in LoraDB). Null ordering
 * follows Cypher: nulls last ascending, first descending.
 */
export function keysetPredicate(
  ctx: CompileContext,
  variable: string,
  sort: SortKey[],
  values: unknown[],
  edge?: string,
): Expr | undefined {
  const branches: Expr[] = [];
  const equal: Expr[] = [];
  let lead: Expr | undefined;
  sort.forEach((k, i) => {
    const target = keyExpr(k, variable, edge);
    const value = values[i];
    const p = value === null ? undefined : bind(ctx, value);
    const present = leadBound(k);
    let after: Expr | undefined;
    if (p === undefined) {
      // Past a null: ascending, nothing but more nulls; descending, every
      // non-null value.
      after = k.direction === "ASC" ? undefined : isNull(target, true);
    } else if (k.direction === "ASC") {
      after = present
        ? bin(">", target, p)
        : or(bin(">", target, p), isNull(target));
    } else {
      after = bin("<", target, p);
    }
    if (after) branches.push(and(...equal, after)!);
    equal.push(p === undefined ? isNull(target) : bin("=", target, p));
    if (i === 0 && p !== undefined && present) {
      lead = bin(k.direction === "ASC" ? ">=" : "<=", target, p);
    }
  });
  const disjunction = or(...branches) ?? lit(false);
  return and(lead, disjunction);
}
