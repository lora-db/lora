import { useMemo } from "react";
import type { NodeObject } from "../types";

export interface GroupLegendProps<N extends NodeObject = NodeObject> {
  nodes: N[];
  /** Same accessor as `nodeAutoColorBy` — string key or function. */
  groupBy?: string | ((n: N) => string | number | null);
  /** Hidden group keys (stringified). Toggled by clicking. */
  hidden: Set<string>;
  onToggle(group: string): void;
}

/** Auto-colour palette (d3-scale-chromatic's schemeTableau10). We
 *  pick the colour by group-hash so the same group string always maps
 *  to the same swatch across re-renders. */
const PALETTE = [
  "#4e79a7",
  "#f28e2b",
  "#e15759",
  "#76b7b2",
  "#59a14f",
  "#edc948",
  "#b07aa1",
  "#ff9da7",
  "#9c755f",
  "#bab0ac",
];

function hashStringToIndex(input: string, mod: number): number {
  let h = 0;
  for (let i = 0; i < input.length; i++) {
    h = (h * 31 + input.charCodeAt(i)) | 0;
  }
  return Math.abs(h) % mod;
}

export function colorForGroup(group: string): string {
  return PALETTE[hashStringToIndex(group, PALETTE.length)]!;
}

export function GroupLegend<N extends NodeObject = NodeObject>({
  nodes,
  groupBy,
  hidden,
  onToggle,
}: GroupLegendProps<N>) {
  const groups = useMemo(() => {
    if (!groupBy) return [] as string[];
    const set = new Set<string>();
    for (const n of nodes) {
      const v =
        typeof groupBy === "function"
          ? groupBy(n)
          : (n as unknown as Record<string, unknown>)[groupBy];
      if (v === null || v === undefined) continue;
      set.add(String(v));
    }
    return Array.from(set).sort();
  }, [nodes, groupBy]);

  if (groups.length === 0) return null;

  return (
    <div className="lgc-legend" role="region" aria-label="Groups">
      {groups.map((g) => {
        const isHidden = hidden.has(g);
        return (
          <button
            key={g}
            type="button"
            className={[
              "lgc-legend-item",
              isHidden ? "lgc-legend-item--hidden" : "",
            ]
              .join(" ")
              .trim()}
            onClick={() => onToggle(g)}
            aria-pressed={isHidden ? "false" : "true"}
            title={`Toggle ${g}`}
          >
            <span
              className="lgc-legend-swatch"
              style={{ background: colorForGroup(g) }}
            />
            <span>{g}</span>
          </button>
        );
      })}
    </div>
  );
}
