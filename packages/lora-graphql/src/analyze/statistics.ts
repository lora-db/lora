// S6: cardinalities sampled from a live database. The compiler uses the
// degree of each list relationship to estimate how many rows an
// operation touches; until `analyze()` runs, `@cardinality(max:)` and
// page limits stand in.
//
// The estimate uses the maximum degree, measured over every node, not a
// percentile of the sample: callers pick the parents of a nested list
// (by key, or by traversing into a hub), so any lower percentile is a
// bound they can exceed at will. A hub-rooted four-level query estimated
// from the p99 came to 22 621 rows and returned 537 790 objects. The
// page limit still caps each level, so the maximum only lowers an
// estimate where every node stays under the limit.

//
// G-73: a filter an index answers is charged the nodes holding the value
// it seeks, not the label. Values are skewed (a root genre is on most
// events), so the estimate is per value, never an average: every value's
// count when a field has at most `VALUE_COUNTS` of them, else the most
// common ones and, for any other value, the largest count among the rest.
// A `@cypher` root without a declared `cost:` is charged the largest row
// estimate of its statement's plan.

import type { LoraDriver, PlanNode } from "../driver.js";
import { name } from "../compile/cypher.js";
import type { GraphModel, NodeType, ScalarField } from "../model/types.js";

/** Values whose counts `analyze()` keeps exactly, per field. */
export const VALUE_COUNTS = 1024;
/** The most common values kept beyond that. */
export const COMMON_VALUES = 64;

export interface DegreeStats {
  sampled: number;
  mean: number;
  median: number;
  p99: number;
  /** The largest degree of any node, not only of the sample. */
  max: number;
}

/** Nodes per value of an indexed equality or list field (G-73). */
export interface ValueStats {
  /** Distinct values (of a list field: distinct elements), at most `VALUE_COUNTS + 1`. */
  distinct: number;
  /**
   * Nodes holding each value, keyed by the value as JSON: every value
   * when `rest` is 0, else the most common ones.
   */
  counts: Record<string, number>;
  /** The most nodes any value not in `counts` is held by. */
  rest: number;
}

export interface Statistics {
  /** Node count per @node type. */
  nodes: Record<string, number>;
  /** Degree per list relationship, keyed `Type.field`. */
  degrees: Record<string, DegreeStats>;
  /** Nodes per value of each filterable field an index seeks, `Type.field`. */
  values?: Record<string, ValueStats>;
  /**
   * Rows a `@cypher` root's statement examines, from its plan, keyed
   * `Query.field`; a declared `@cypher(cost:)` wins.
   */
  cypher?: Record<string, number>;
}

/** Field types whose values compare as their JSON. */
const COUNTED_TYPES = new Set(["String", "ID", "Int", "Boolean", "Enum"]);

/**
 * Fields a root filter seeks by value: `eq`, `in`, `includes` or
 * `includesAny` on a stored field other than a unique one (that seek
 * finds one node).
 */
export function countedFields(node: NodeType): ScalarField[] {
  return [...node.fields.values()].filter(
    (f): f is ScalarField =>
      f.kind === "scalar" &&
      !f.private &&
      !f.unique &&
      f.name !== node.key.name &&
      COUNTED_TYPES.has(f.type) &&
      (f.list
        ? f.filters.has("INCLUDES")
        : f.filters.has("EQ") || f.filters.has("IN")),
  );
}

/** The key a value is counted under in `ValueStats.counts`. */
export function valueKey(value: unknown): string {
  return JSON.stringify(value);
}

export async function analyze(
  driver: LoraDriver,
  model: GraphModel,
  options: { sample?: number; timeoutMs?: number } = {},
): Promise<Statistics> {
  const sample = options.sample ?? 1000;
  const stats: Statistics = { nodes: {}, degrees: {} };
  for (const node of model.nodes.values()) {
    const label = name(node.labels[0]!);
    const statements = [
      { text: `MATCH (n:${label}) RETURN count(n) AS c`, params: {} },
    ];
    const rels = [...node.fields.values()].filter(
      (f) => f.kind === "relationship" && f.list,
    );
    for (const f of rels) {
      if (f.kind !== "relationship") continue;
      // Any member of an interface or union counts.
      const labels = f.members
        .map((m) => model.nodes.get(m)!.labels[0]!)
        .map((l) => `x:${name(l)}`)
        .join(" OR ");
      const inner = `[:${name(f.type)}]`;
      const arrow =
        (f.direction === "OUT" ? `-${inner}->(x)` : `<-${inner}-(x)`) +
        ` WHERE ${labels}`;
      statements.push(
        {
          text: `MATCH (n:${label}) WITH n LIMIT $sample RETURN size([(n)${arrow} | 1]) AS d`,
          params: { sample } as never,
        },
        // The hub a sample may miss: one pass over every relationship.
        {
          text: `MATCH (n:${label}) RETURN max(size([(n)${arrow} | 1])) AS m`,
          params: {},
        },
      );
    }
    const counted = countedFields(node);
    for (const f of counted) {
      // A node counts once per value, however often its list repeats it.
      const values = f.list
        ? `UNWIND list.unique(n.${name(f.name)}) AS v WITH v WHERE v IS NOT NULL`
        : `WITH n.${name(f.name)} AS v WHERE v IS NOT NULL`;
      statements.push({
        text: `MATCH (n:${label}) ${values} RETURN v, count(*) AS c ORDER BY c DESC LIMIT ${VALUE_COUNTS + 1}`,
        params: {},
      });
    }
    const results = await driver.run(statements, {
      mode: "read",
      timeoutMs: options.timeoutMs,
    });
    const [count, ...rest] = results;
    const degrees = rest.slice(0, 2 * rels.length);
    counted.forEach((f, i) => {
      const rows = rest[2 * rels.length + i]!.rows;
      const all = rows.length <= VALUE_COUNTS;
      const kept = all ? rows : rows.slice(0, COMMON_VALUES);
      const counts: Record<string, number> = {};
      for (const r of kept) counts[valueKey(r["v"])] = Number(r["c"]);
      stats.values ??= {};
      stats.values[`${node.name}.${f.name}`] = {
        distinct: rows.length,
        counts,
        rest: all ? 0 : Number(rows[COMMON_VALUES]!["c"]),
      };
    });
    stats.nodes[node.name] = Number(count!.rows[0]?.["c"] ?? 0);
    rels.forEach((f, i) => {
      const values = degrees[2 * i]!.rows.map((r) => Number(r["d"])).sort(
        (a, b) => a - b,
      );
      const summary = summarize(values);
      const max = Number(degrees[2 * i + 1]!.rows[0]?.["m"] ?? 0);
      summary.max = Math.max(summary.max, max);
      stats.degrees[`${node.name}.${f.name}`] = summary;
    });
  }
  if (driver.explain) {
    for (const field of model.queries) {
      if (field.cost > 0) continue;
      // Planned with each argument's SDL default, null without one: the
      // estimate is the statement's shape, not one request's.
      const params = Object.fromEntries(
        field.params.map((p) => [
          p,
          field.args.find((a) => a.name === p)?.defaultValue ?? null,
        ]),
      );
      try {
        const plan = await driver.explain({
          text: field.statement,
          params: params as never,
        });
        const rows = largestEstimate(plan.tree);
        if (rows !== undefined) {
          stats.cypher ??= {};
          stats.cypher[`${field.owner}.${field.name}`] = rows;
        }
      } catch {
        // A statement the engine cannot plan without its arguments keeps
        // its projection's charge; declare `@cypher(cost:)` for it.
      }
    }
  }
  return stats;
}

/** The largest row estimate of any operator in a plan. */
function largestEstimate(node: PlanNode): number | undefined {
  let max = node.estimatedRows ?? undefined;
  for (const child of node.children) {
    const rows = largestEstimate(child);
    if (rows !== undefined && (max === undefined || rows > max)) max = rows;
  }
  return max;
}

function summarize(values: number[]): DegreeStats {
  const at = (q: number) =>
    values.length === 0
      ? 0
      : values[Math.min(values.length - 1, Math.floor(q * values.length))]!;
  return {
    sampled: values.length,
    mean:
      values.length === 0
        ? 0
        : values.reduce((a, b) => a + b, 0) / values.length,
    median: at(0.5),
    p99: at(0.99),
    max: values.length === 0 ? 0 : values[values.length - 1]!,
  };
}
