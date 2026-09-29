// S6: cardinalities sampled from a live database. The compiler uses the
// degree of each list relationship to estimate how many rows an
// operation touches; until `analyze()` runs, `@cardinality(max:)` and
// page limits stand in.

import type { LoraDriver } from "../driver.js";
import { name } from "../compile/cypher.js";
import type { GraphModel } from "../model/types.js";

export interface DegreeStats {
  sampled: number;
  mean: number;
  median: number;
  p99: number;
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
      const target = model.nodes.get(f.target)!;
      const inner = `[:${name(f.type)}]`;
      const arrow =
        f.direction === "OUT"
          ? `-${inner}->(:${name(target.labels[0]!)})`
          : `<-${inner}-(:${name(target.labels[0]!)})`;
      statements.push({
        text: `MATCH (n:${label}) WITH n LIMIT $sample RETURN size([(n)${arrow} | 1]) AS d`,
        params: { sample } as never,
      });
    }
    const [count, ...degrees] = await driver.run(statements, {
      mode: "read",
      timeoutMs: options.timeoutMs,
    });
    stats.nodes[node.name] = Number(count!.rows[0]?.["c"] ?? 0);
    rels.forEach((f, i) => {
      const values = degrees[i]!.rows.map((r) => Number(r["d"])).sort(
        (a, b) => a - b,
      );
      stats.degrees[`${node.name}.${f.name}`] = summarize(values);
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
