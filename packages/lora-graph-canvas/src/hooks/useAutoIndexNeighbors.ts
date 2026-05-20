import { useEffect, useRef } from "react";
import type { GraphData, LinkObject, NodeObject } from "../types";

/** When enabled, walk the graph once on every data change and stash
 *  `_neighbors` (Node[]) and `_links` (Link[]) arrays on each node. The
 *  hover-highlight code reads from these refs directly so the per-frame
 *  work stays O(1). */
export function useAutoIndexNeighbors<
  N extends NodeObject,
  L extends LinkObject,
>(enabled: boolean, data: GraphData<N, L>): void {
  // We warn at most once per component lifetime — repeating on every
  // data update would flood the console when the host passes a fresh
  // (still-frozen) array each render.
  const warnedRef = useRef(false);
  useEffect(() => {
    if (!enabled) return;
    // Detect dev-mode frozen nodes before we try to mutate them. The
    // index attaches `_neighbors` / `_links` directly to each node
    // object; if the host has frozen them (common in stores using
    // immer / Object.freeze), the mutation silently no-ops in
    // non-strict-mode JS and the highlight stays empty — opaque from
    // the host's side. A one-shot warning here points them at the
    // actual prop. (Empty datasets skip the check; sample the first
    // node only — checking every node would add per-render cost on
    // large graphs for a startup-time concern.)
    if (
      !warnedRef.current &&
      data.nodes.length > 0 &&
      Object.isFrozen(data.nodes[0])
    ) {
      warnedRef.current = true;

      console.warn(
        "[lora-graph-canvas] autoIndexNeighbors is enabled but node objects appear to be frozen. The hover-highlight index needs to mutate nodes to attach `_neighbors`/`_links`. Either pass `autoIndexNeighbors={false}` (and `highlightNeighborsOnHover={false}`) or supply mutable node objects.",
      );
    }
    const byId = new Map<string | number, N>();
    for (const n of data.nodes) {
      byId.set(n.id, n);
      (n as unknown as Record<string, unknown>)._neighbors = [];
      (n as unknown as Record<string, unknown>)._links = [];
    }
    for (const link of data.links) {
      const sId =
        typeof link.source === "object" ? (link.source as N).id : link.source;
      const tId =
        typeof link.target === "object" ? (link.target as N).id : link.target;
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
