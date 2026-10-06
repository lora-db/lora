// What a subscription's `where` costs: it runs on every change, so its
// relationship filters are bounded in depth and charged once, up front.

import { requestError } from "../errors.js";
import type { GraphModel, NodeType } from "../model/types.js";

export interface FilterCostEnv {
  model: GraphModel;
  /** Measured maximum degrees, by `Owner.field` (see `analyze()`). */
  degrees: ReadonlyMap<string, number>;
  /** The `maxSubscriptionFilterDepth` option. */
  maxDepth: number;
}

/**
 * Estimated rows one changed node's `where` check touches: each
 * relationship filter scans the relationship (its measured degree, else
 * its cardinality, else a page of the target), times what it nests.
 * Nesting deeper than `maxSubscriptionFilterDepth` is LIMIT_EXCEEDED.
 */
export function filterCost(
  env: FilterCostEnv,
  node: NodeType,
  where: Record<string, unknown> | null | undefined,
  depth: number,
): number {
  let cost = 0;
  for (const [key, value] of Object.entries(where ?? {})) {
    if (value === null || value === undefined) continue;
    if (key === "AND" || key === "OR") {
      for (const w of value as Array<Record<string, unknown>>) {
        cost += filterCost(env, node, w, depth);
      }
      continue;
    }
    if (key === "NOT") {
      cost += filterCost(env, node, value as Record<string, unknown>, depth);
      continue;
    }
    let field = node.fields.get(key);
    let shape: "direct" | "connection" | "flat" = "direct";
    if (!field) {
      for (const [suffix, s] of [
        ["Connection", "connection"],
        ["Aggregate", "flat"],
        ["Exists", "flat"],
      ] as const) {
        if (key.endsWith(suffix)) {
          field = node.fields.get(key.slice(0, -suffix.length));
          shape = s;
          break;
        }
      }
    }
    if (field?.kind !== "relationship") continue;
    if (depth + 1 > env.maxDepth) {
      throw requestError(
        "LIMIT_EXCEEDED",
        `a subscription's where may nest relationship filters ${env.maxDepth} deep (${node.name}.${key})`,
      );
    }
    const target = env.model.nodes.get(field.target);
    const fan = field.list
      ? Math.max(
          1,
          env.degrees.get(`${field.owner}.${field.name}`) ??
            field.cardinality ??
            target?.limit.max ??
            100,
        )
      : 1;
    let inner = 0;
    if (target && shape !== "flat" && typeof value === "object") {
      const parts: unknown[] = field.list
        ? Object.values(value as Record<string, unknown>)
        : [value];
      for (const part of parts) {
        if (part === null || typeof part !== "object") continue;
        const w =
          shape === "connection"
            ? (part as Record<string, unknown>)["node"]
            : part;
        inner += filterCost(
          env,
          target,
          w as Record<string, unknown> | null | undefined,
          depth + 1,
        );
      }
    } else if (!target && shape !== "flat" && typeof value === "object") {
      // An abstract target: a filter by member (`{ Person: {...} }`) or
      // on the shared fields; the costliest member counts.
      const parts: unknown[] = field.list
        ? Object.values(value as Record<string, unknown>)
        : [value];
      for (const part of parts) {
        if (part === null || typeof part !== "object") continue;
        let worst = 0;
        for (const member of field.members) {
          const m = env.model.nodes.get(member);
          const p = part as Record<string, unknown>;
          const w = (shape === "connection" ? p["node"] : p) as
            | Record<string, unknown>
            | null
            | undefined;
          if (!m || !w || typeof w !== "object") continue;
          const own = (w[member] ?? w) as Record<string, unknown>;
          worst = Math.max(worst, filterCost(env, m, own, depth + 1));
        }
        inner += worst;
      }
    }
    cost += fan * (1 + inner);
  }
  return cost;
}
