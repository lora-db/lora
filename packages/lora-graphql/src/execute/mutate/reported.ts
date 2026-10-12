// What a hand-written statement (a `@trigger`'s, a `@cypher` mutation's)
// says it wrote: the nodes it returns as `created` and `updated`.

import { bind, type CompileContext } from "../../compile/context.js";
import { name, printExpr } from "../../compile/cypher.js";
import { keyOf } from "../../compile/read/by-keys.js";
import type { QueryResult, Statement } from "../../driver.js";
import { OUTBOX } from "../../events/outbox.js";
import type { GraphModel, NodeType } from "../../model/types.js";
import type { EntityRef } from "../changes.js";

export interface Reported {
  created: EntityRef[];
  updated: EntityRef[];
  /** The statement wrote something listeners cannot be told about. */
  broad: boolean;
}

export const refId = (e: EntityRef): string => `${e.type}\0${keyOf(e.key)}`;

/**
 * The nodes `result` returns as `created` or `updated`, each once (a node
 * in both is created). A statement that writes and names neither is
 * broad, and so is one that returns something that is not a node of the
 * model (a key, a node of an unknown label).
 */
export function reportedWrites(
  model: GraphModel,
  statement: { writes: boolean; reports: boolean },
  result: QueryResult,
): Reported {
  const out: Reported = { created: [], updated: [], broad: false };
  // The result's own columns say what it reports. One that matched no row
  // has none: then the statement's text does.
  const reports =
    result.rows.length > 0
      ? result.columns.includes("created") || result.columns.includes("updated")
      : statement.reports;
  if (!reports) {
    out.broad = statement.writes;
    return out;
  }
  const seen = new Set<string>();
  for (const column of ["created", "updated"] as const) {
    for (const row of result.rows) {
      const value = row[column];
      for (const item of Array.isArray(value) ? value : [value]) {
        if (item === null || item === undefined) continue;
        const ref = entityOf(model, item);
        if (!ref) {
          if (statement.writes) out.broad = true;
          continue;
        }
        const id = refId(ref);
        if (seen.has(id)) continue;
        seen.add(id);
        out[column].push(ref);
      }
    }
  }
  return out;
}

/**
 * A statement's CREATE knows nothing of @outbox: the nodes it reports as
 * created become work with these, in the same transaction.
 */
export function outboxStatements(
  model: GraphModel,
  created: readonly EntityRef[],
  ctx: () => CompileContext,
): Statement[] {
  const work = new Map<NodeType, unknown[]>();
  for (const ref of created) {
    const node = model.nodes.get(ref.type);
    if (node?.outbox) work.set(node, [...(work.get(node) ?? []), ref.key]);
  }
  return [...work].map(([node, keys]) => {
    const c = ctx();
    return {
      text:
        `UNWIND ${printExpr(bind(c, keys))} AS k\n` +
        `MATCH (n:${name(node.labels[0]!)}) WHERE n.${name(node.key.property)} = k AND n.${name(OUTBOX.state)} IS NULL\n` +
        `SET n.${name(OUTBOX.state)} = "PENDING"`,
      params: c.params,
    };
  });
}

/** The @node type and key of a node value the engine returned. */
function entityOf(model: GraphModel, value: unknown): EntityRef | undefined {
  const node = value as
    | { labels?: unknown; properties?: Record<string, unknown> }
    | null
    | undefined;
  if (!node || !Array.isArray(node.labels) || !node.properties) {
    return undefined;
  }
  for (const type of model.nodes.values()) {
    if (!node.labels.includes(type.labels[0])) continue;
    const key = node.properties[type.key.property];
    return key === undefined || key === null
      ? undefined
      : { type: type.name, key };
  }
  return undefined;
}
