import { useCallback, useMemo } from "react";
import type {
  DeletionGuard,
  DeletionSource,
  LinkObject,
  NodeObject,
} from "../types";
import type { GraphDataApi } from "./useGraphData";
import { runGuard } from "../internal/runGuard";

export interface UseGraphDeleteGateParams<
  N extends NodeObject,
  L extends LinkObject,
> {
  dataApi: GraphDataApi<N, L>;
  beforeNode?: DeletionGuard<N>;
  beforeLink?: DeletionGuard<L>;
  onNodeDeleted?: (nodes: N[], ctx: { source: DeletionSource }) => void;
  onLinkDeleted?: (links: L[], ctx: { source: DeletionSource }) => void;
  /** Called after a successful node delete so the caller can clear its
   *  own selection / hover state. Skipped if the guard rejected. */
  afterNodeDelete?: (ids: Array<string | number>) => void;
  afterLinkDelete?: (ids: Array<string | number>) => void;
}

export interface GraphDeleteGateApi<L extends LinkObject> {
  /** Resolves the selected node ids against current data, runs the guard,
   *  and removes them. Returns `false` if the guard rejected or nothing
   *  matched. */
  requestNodeDelete: (
    ids: Array<string | number>,
    source: DeletionSource,
  ) => Promise<boolean>;
  /** Same, for links. Accepts either an id list or a predicate so context
   *  menus that hold the link reference can still target it precisely
   *  (links sometimes lack an id). */
  requestLinkDelete: (
    target: Array<string | number> | ((l: L) => boolean),
    source: DeletionSource,
  ) => Promise<boolean>;
  /** Convenience: run node + link guards in sequence. Used by the
   *  "delete selection" sites (toolbar / selection panel / keyboard)
   *  where a mixed selection is common. Each guard fires independently;
   *  rejecting one doesn't cancel the other. Returns true if anything
   *  was actually deleted. */
  requestMixedDelete: (
    nodeIds: Array<string | number>,
    linkIds: Array<string | number>,
    source: DeletionSource,
  ) => Promise<boolean>;
}

/** Single chokepoint for every gated delete in the canvas. Centralising
 *  here keeps the guard semantics (batched calls, post-delete callbacks,
 *  selection cleanup) consistent across keyboard, toolbar, context menu,
 *  selection panel, and imperative paths. */
export function useGraphDeleteGate<
  N extends NodeObject,
  L extends LinkObject,
>(params: UseGraphDeleteGateParams<N, L>): GraphDeleteGateApi<L> {
  const {
    dataApi,
    beforeNode,
    beforeLink,
    onNodeDeleted,
    onLinkDeleted,
    afterNodeDelete,
    afterLinkDelete,
  } = params;

  const requestNodeDelete = useCallback(
    (
      ids: Array<string | number>,
      source: DeletionSource,
    ): Promise<boolean> => {
      if (ids.length === 0) return Promise.resolve(false);
      const idSet = new Set(ids);
      const targets = dataApi.data.nodes.filter((n) => idSet.has(n.id));
      if (targets.length === 0) return Promise.resolve(false);
      const commit = (): true => {
        dataApi.removeNodes(ids);
        onNodeDeleted?.(targets, { source });
        afterNodeDelete?.(ids);
        return true;
      };
      // Sync path when there's no guard — keeps the imperative
      // `handle.removeNode(id)` mutation observable on the same tick,
      // which matters for hosts using ref + assertion sequences.
      if (!beforeNode) return Promise.resolve(commit());
      return Promise.resolve(runGuard(beforeNode, targets, source)).then(
        (ok) => (ok ? commit() : false),
      );
    },
    [dataApi, beforeNode, onNodeDeleted, afterNodeDelete],
  );

  const requestLinkDelete = useCallback(
    (
      target: Array<string | number> | ((l: L) => boolean),
      source: DeletionSource,
    ): Promise<boolean> => {
      const predicate: (l: L) => boolean =
        typeof target === "function"
          ? target
          : (() => {
              const idSet = new Set(target);
              return (l: L) => l.id !== undefined && idSet.has(l.id);
            })();
      const targets = dataApi.data.links.filter(predicate);
      if (targets.length === 0) return Promise.resolve(false);
      const commit = (): true => {
        dataApi.removeLink(predicate);
        onLinkDeleted?.(targets, { source });
        const removedIds = targets
          .map((l) => l.id)
          .filter((id): id is string | number => id !== undefined);
        afterLinkDelete?.(removedIds);
        return true;
      };
      if (!beforeLink) return Promise.resolve(commit());
      return Promise.resolve(runGuard(beforeLink, targets, source)).then(
        (ok) => (ok ? commit() : false),
      );
    },
    [dataApi, beforeLink, onLinkDeleted, afterLinkDelete],
  );

  const requestMixedDelete = useCallback(
    async (
      nodeIds: Array<string | number>,
      linkIds: Array<string | number>,
      source: DeletionSource,
    ): Promise<boolean> => {
      // Fire both guards concurrently — they're independent, the host
      // may be showing one or two modals, and serialising would add a
      // noticeable delay when both prompts auto-resolve.
      const [nodesOk, linksOk] = await Promise.all([
        nodeIds.length > 0
          ? requestNodeDelete(nodeIds, source)
          : Promise.resolve(false),
        linkIds.length > 0
          ? requestLinkDelete(linkIds, source)
          : Promise.resolve(false),
      ]);
      return nodesOk || linksOk;
    },
    [requestNodeDelete, requestLinkDelete],
  );

  return useMemo<GraphDeleteGateApi<L>>(
    () => ({ requestNodeDelete, requestLinkDelete, requestMixedDelete }),
    [requestNodeDelete, requestLinkDelete, requestMixedDelete],
  );
}
