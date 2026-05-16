import { useEffect } from "react";
import type { GraphData, LinkObject, NodeObject } from "../types";

/** When enabled, walk the graph once on every data change and stash
 *  `_neighbors` (Node[]) and `_links` (Link[]) arrays on each node. The
 *  hover-highlight code reads from these refs directly so the per-frame
 *  work stays O(1). */
export function useAutoIndexNeighbors<
  N extends NodeObject,
  L extends LinkObject,
>(enabled: boolean, data: GraphData<N, L>): void {
  useEffect(() => {
    if (!enabled) return;
    const byId = new Map<string | number, N>();
    for (const n of data.nodes) {
      byId.set(n.id, n);
      (n as unknown as Record<string, unknown>)._neighbors = [];
      (n as unknown as Record<string, unknown>)._links = [];
    }
    for (const link of data.links) {
      const sId =
        typeof link.source === "object"
          ? (link.source as N).id
          : link.source;
      const tId =
        typeof link.target === "object"
          ? (link.target as N).id
          : link.target;
      const s = byId.get(sId as string | number);
      const t = byId.get(tId as string | number);
      if (!s || !t) continue;
      (s as unknown as { _neighbors: N[] })._neighbors.push(t);
      (t as unknown as { _neighbors: N[] })._neighbors.push(s);
      (s as unknown as { _links: L[] })._links.push(link);
      (t as unknown as { _links: L[] })._links.push(link);
    }
  }, [enabled, data]);
}
