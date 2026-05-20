import { useEffect } from "react";
import { forceCollide, forceX, forceY } from "d3-force-3d";
import type { GraphEngine } from "../engines/types";
import type { LinkObject, LoraGraphCanvasProps, NodeObject } from "../types";

export interface UseGraphForcesParams<
  N extends NodeObject,
  L extends LinkObject,
> {
  engine: GraphEngine<N, L> | null;
  collideNodes: NonNullable<LoraGraphCanvasProps<N, L>["collideNodes"]>;
  beeswarm: NonNullable<LoraGraphCanvasProps<N, L>["beeswarm"]>;
  nodeRelSize?: number;
}

/** Drive optional d3-force-3d forces (collide + beeswarm) onto the
 *  active engine. These are deliberately kept as separate effects in
 *  one hook so the simulation knobs can be toggled independently. */
export function useGraphForces<N extends NodeObject, L extends LinkObject>(
  params: UseGraphForcesParams<N, L>,
): void {
  const { engine, collideNodes, beeswarm, nodeRelSize } = params;

  // ─── Collide force ────────────────────────────────────────────────
  // Inject `forceCollide` when the user opts in. Skipped while beeswarm
  // mode is on — that layout wires its own collide force with a tuned
  // radius.
  useEffect(() => {
    if (!engine) return;
    if (beeswarm) return;
    if (!collideNodes) {
      engine.d3Force("collide", null);
      return;
    }
    const radius =
      typeof collideNodes === "number" ? collideNodes : (nodeRelSize ?? 4) + 2;
    engine.d3Force("collide", forceCollide(radius));
    engine.reheat();
    return () => {
      engine.d3Force("collide", null);
    };
  }, [engine, collideNodes, nodeRelSize, beeswarm]);

  // ─── Beeswarm layout ──────────────────────────────────────────────
  // Mirrors the upstream force-graph `beeswarm` example: deactivate
  // center + charge, add a positional force on the chosen axis (driven
  // by a per-node accessor), a weak orthogonal pull, and collision so
  // the dots don't overlap. Reverting to the default layout requires a
  // remount today — d3-force-3d doesn't expose the pristine defaults
  // for us to restore.
  useEffect(() => {
    if (!engine) return;
    if (!beeswarm) {
      engine.d3Force("x", null);
      engine.d3Force("y", null);
      return;
    }
    const cfg = beeswarm === true ? {} : beeswarm;
    const axis = cfg.axis ?? "x";
    const strength = cfg.strength ?? 0.2;
    const valueProp = cfg.value;

    // Stable id hash → [-300, 300]. Used when the host hasn't supplied
    // a value accessor.
    const hashSpread = (id: string | number) => {
      const s = String(id);
      let h = 0;
      for (let i = 0; i < s.length; i++) h = (h * 31 + s.charCodeAt(i)) | 0;
      return (((h % 600) + 600) % 600) - 300;
    };

    const valueFn = (n: unknown): number => {
      const node = n as N;
      if (typeof valueProp === "function") return valueProp(node);
      if (typeof valueProp === "string") {
        const v = (node as unknown as Record<string, unknown>)[valueProp];
        if (typeof v === "number") return v;
        if (typeof v === "string") return Number(v) || 0;
      }
      return hashSpread(node.id);
    };

    // Beeswarm topology: drop the radial forces.
    engine.d3Force("center", null);
    engine.d3Force("charge", null);

    if (axis === "x") {
      engine.d3Force("x", forceX(valueFn));
      engine.d3Force("y", forceY(0).strength(strength));
    } else {
      engine.d3Force("y", forceY(valueFn));
      engine.d3Force("x", forceX(0).strength(strength));
    }

    const radius = (nodeRelSize ?? 4) + 2;
    engine.d3Force("collide", forceCollide(radius));
    engine.reheat();
  }, [engine, beeswarm, nodeRelSize]);
}
