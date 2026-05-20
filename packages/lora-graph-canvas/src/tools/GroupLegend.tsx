import { useMemo } from "react";
import type { NodeObject } from "../types";
import { colorForGroup } from "../theme/palette";

export interface GroupLegendProps<N extends NodeObject = NodeObject> {
  nodes: N[];
  /** Same accessor as `nodeAutoColorBy` — string key or function. */
  groupBy?: string | ((n: N) => string | number | null);
  /** Hidden group keys (stringified). Toggled by clicking. */
  hidden: Set<string>;
  onToggle(group: string): void;
  /** Palette used for swatch backgrounds — must match the canvas's
   *  node palette so legend swatches and node fills line up. Defaults
   *  to the package's Tableau10 ramp. */
  palette?: readonly string[];
}

export function GroupLegend<N extends NodeObject = NodeObject>({
  nodes,
  groupBy,
  hidden,
  onToggle,
  palette,
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
              style={{ background: colorForGroup(g, palette) }}
            />
            <span>{g}</span>
          </button>
        );
      })}
    </div>
  );
}
