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

import type { LoraDriver } from "../driver.js";
import { name } from "../compile/cypher.js";
import type { GraphModel } from "../model/types.js";

export interface DegreeStats {
  sampled: number;
  mean: number;
  median: number;
  p99: number;
  /** The largest degree of any node, not only of the sample. */
  max: number;
}

export interface Statistics {
  /** Node count per @node type. */
  nodes: Record<string, number>;
  /** Degree per list relationship, keyed `Type.field`. */
  degrees: Record<string, DegreeStats>;
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
    const [count, ...degrees] = await driver.run(statements, {
      mode: "read",
      timeoutMs: options.timeoutMs,
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
  return stats;
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
