import { useMemo, useRef } from "react";
import { adjustAlpha } from "../utils/accessor";
import type {
  Accessor,
  GraphMode,
  LinkObject,
  NodeObject,
} from "../types";

export type NodePointerAreaPaint<N extends NodeObject> = (
  node: N,
  color: string,
  ctx: CanvasRenderingContext2D,
  globalScale: number,
) => void;

export interface UseAccessorOverridesParams<
  N extends NodeObject,
  L extends LinkObject,
> {
  mode: GraphMode;
  accentColor: string;
  nodeColor?: Accessor<string, N>;
  linkColor?: Accessor<string, L>;
  linkWidth?: Accessor<number, L>;
  nodeVal?: Accessor<number, N>;
  nodeRelSize?: number;
  nodePointerAreaPaint?: NodePointerAreaPaint<N>;
  nodeAutoColorBy?: Accessor<string | null, N>;
  nodeVisibility?: Accessor<boolean, N>;
  selectedNodeSet: ReadonlySet<string | number>;
  selectedLinkSet: ReadonlySet<string | number>;
  highlightNeighborsOnHover: boolean;
  highlightedNodeIds: ReadonlySet<string | number>;
  highlightedLinkIds: ReadonlySet<string | number>;
  hoverNodeId: string | number | null;
  hiddenGroups: ReadonlySet<string>;
}

export interface AccessorOverrides<
  N extends NodeObject,
  L extends LinkObject,
> {
  nodeColor: Accessor<string, N> | undefined;
  linkColor: Accessor<string, L> | undefined;
  linkWidth: Accessor<number, L> | undefined;
  nodeVal: Accessor<number, N> | undefined;
  nodePointerAreaPaint: NodePointerAreaPaint<N> | undefined;
  nodeVisibility: ((node: N) => boolean) | undefined;
}

/** Wrap the host's color / width / value / visibility accessors so the
 *  current selection + hover-highlight + legend-filter overlay on top
 *  without losing the underlying styling.
 *
 *  Each wrapper is conditional in two ways:
 *
 *  1. When there's nothing to overlay (no selection, no hover, no
 *     legend filter) we return the host's accessor *untouched*. The
 *     kapsule never enters our closure on hot paths in that case.
 *
 *  2. When an overlay is active, the wrapper identity stays stable
 *     across hover / selection state changes — the closure reads live
 *     state through a ref instead of capturing it on each render. This
 *     matters during interactions like dragging a node while a hover
 *     highlight is up: the cursor crosses other nodes 60×/sec and
 *     each crossing changes hoverNodeId. With unstable wrappers, every
 *     change would re-trigger the kapsule's nodeColor / linkColor /
 *     linkWidth / nodeVal setters, each of which walks the full
 *     node-or-link list and rebuilds materials. With stable wrappers,
 *     the setters only fire on the on/off *boundary* (first hover,
 *     last unhover) and the live state is picked up on the next render
 *     frame for free. */
export function useAccessorOverrides<
  N extends NodeObject,
  L extends LinkObject,
>(
  params: UseAccessorOverridesParams<N, L>,
): AccessorOverrides<N, L> {
  const {
    mode,
    accentColor,
    nodeColor,
    linkColor,
    linkWidth,
    nodeVal,
    nodeRelSize,
    nodePointerAreaPaint,
    nodeAutoColorBy,
    nodeVisibility,
    selectedNodeSet,
    selectedLinkSet,
    highlightNeighborsOnHover,
    highlightedNodeIds,
    highlightedLinkIds,
    hoverNodeId,
    hiddenGroups,
  } = params;

  // Single ref read by every wrapped accessor below. Updated on every
  // render so the closures always see fresh state, but the closure
  // identities themselves stay stable as long as the *kind* of overlay
  // doesn't change (which is decided by the boundary-toggle memos
  // further down).
  const overlayStateRef = useRef({
    mode,
    accentColor,
    selectedNodeSet,
    selectedLinkSet,
    highlightNeighborsOnHover,
    highlightedNodeIds,
    highlightedLinkIds,
    hoverNodeId,
    hiddenGroups,
  });
  overlayStateRef.current = {
    mode,
    accentColor,
    selectedNodeSet,
    selectedLinkSet,
    highlightNeighborsOnHover,
    highlightedNodeIds,
    highlightedLinkIds,
    hoverNodeId,
    hiddenGroups,
  };

  // ─── nodeColor ────────────────────────────────────────────────
  const hasNodeColorOverlay =
    selectedNodeSet.size > 0 ||
    selectedLinkSet.size > 0 ||
    (highlightNeighborsOnHover && highlightedNodeIds.size > 0);

  const stableNodeColor = useMemo<Accessor<string, N>>(() => {
    const base = nodeColor;
    return (node: N) => {
      const s = overlayStateRef.current;
      if (node.id !== undefined && s.selectedNodeSet.has(node.id)) {
        return s.accentColor;
      }
      const hasHover =
        s.highlightNeighborsOnHover && s.highlightedNodeIds.size > 0;
      if (
        hasHover &&
        node.id !== undefined &&
        s.highlightedNodeIds.has(node.id)
      ) {
        return s.hoverNodeId === node.id
          ? s.accentColor
          : adjustAlpha(s.accentColor, 0.7);
      }
      let color: string;
      if (typeof base === "function") color = base(node);
      else if (typeof base === "string") color = base;
      else color = (node.color as string | undefined) ?? "#888";
      const hasSelection =
        s.selectedNodeSet.size > 0 || s.selectedLinkSet.size > 0;
      // In 3D we deliberately skip the dim — the user can read the
      // scene by parallax/depth so a faded sphere mostly just looks
      // broken. Only links get the focus cue (lighter alpha below).
      // 2D has no depth cue so it keeps the heavier dim to direct
      // the eye to the selection.
      if (hasSelection && s.mode !== "3d") return adjustAlpha(color, 0.25);
      return color;
    };
  }, [nodeColor]);

  const wrappedNodeColor = hasNodeColorOverlay ? stableNodeColor : nodeColor;

  // ─── linkColor ────────────────────────────────────────────────
  const hasHiddenGroups =
    nodeAutoColorBy !== undefined && hiddenGroups.size > 0;
  const hasLinkColorOverlay =
    selectedNodeSet.size > 0 ||
    selectedLinkSet.size > 0 ||
    (highlightNeighborsOnHover && highlightedLinkIds.size > 0) ||
    hasHiddenGroups;

  const stableLinkColor = useMemo<Accessor<string, L>>(() => {
    const base = linkColor;
    // The legend-filter / nodeAutoColorBy axis is static for the
    // lifetime of a given prop value, so we can close over it
    // directly — only the *active* hidden-groups Set lives in the
    // overlay ref.
    const colorBy = nodeAutoColorBy;
    return (link: L) => {
      const s = overlayStateRef.current;
      const fallback =
        s.mode === "3d" ? "rgba(80,80,80,0.7)" : "rgba(0,0,0,0.25)";
      const lid = link.id;
      const srcNode =
        typeof link.source === "object" ? (link.source as N) : null;
      const tgtNode =
        typeof link.target === "object" ? (link.target as N) : null;

      // Group-legend overlay: if either endpoint sits in a hidden
      // group, fade the link so it reads as "out of scope" alongside
      // the now-invisible node. Wins over the selection/hover
      // overlays.
      if (s.hiddenGroups.size > 0 && colorBy !== undefined) {
        const groupOf = (node: N | null): string | null => {
          if (!node) return null;
          const g =
            typeof colorBy === "function"
              ? (colorBy as (n: N) => unknown)(node)
              : (node as unknown as Record<string, unknown>)[
                  colorBy as string
                ];
          return g === null || g === undefined ? null : String(g);
        };
        const sg = groupOf(srcNode);
        const tg = groupOf(tgtNode);
        if (
          (sg !== null && s.hiddenGroups.has(sg)) ||
          (tg !== null && s.hiddenGroups.has(tg))
        ) {
          let color: string;
          if (typeof base === "function") color = base(link);
          else if (typeof base === "string") color = base;
          else color = (link.color as string | undefined) ?? fallback;
          return adjustAlpha(color, 0.05);
        }
      }

      if (lid !== undefined && s.selectedLinkSet.has(lid)) {
        return s.accentColor;
      }
      const sId =
        srcNode?.id ?? (link.source as string | number | undefined);
      const tId =
        tgtNode?.id ?? (link.target as string | number | undefined);
      const hasNodeSelection = s.selectedNodeSet.size > 0;
      const isConnected =
        hasNodeSelection &&
        ((sId !== undefined && s.selectedNodeSet.has(sId)) ||
          (tId !== undefined && s.selectedNodeSet.has(tId)));
      if (isConnected) return s.accentColor;
      const hasHover =
        s.highlightNeighborsOnHover && s.highlightedLinkIds.size > 0;
      if (hasHover && lid !== undefined && s.highlightedLinkIds.has(lid)) {
        return s.accentColor;
      }
      let color: string;
      if (typeof base === "function") color = base(link);
      else if (typeof base === "string") color = base;
      else color = (link.color as string | undefined) ?? fallback;
      const hasSelection =
        hasNodeSelection || s.selectedLinkSet.size > 0;
      // Slight fade in 3D (≈60%), heavier fade in 2D (≈15%) — depth
      // cues do part of the focus work in 3D, so we don't have to
      // strip the unselected links down to a whisper.
      if (hasSelection)
        return adjustAlpha(color, s.mode === "3d" ? 0.6 : 0.15);
      return color;
    };
  }, [linkColor, nodeAutoColorBy]);

  const wrappedLinkColor = hasLinkColorOverlay ? stableLinkColor : linkColor;

  // ─── linkWidth ────────────────────────────────────────────────
  const hasLinkWidthOverlay =
    selectedNodeSet.size > 0 ||
    selectedLinkSet.size > 0 ||
    (highlightNeighborsOnHover && highlightedLinkIds.size > 0);

  const stableLinkWidth = useMemo<Accessor<number, L>>(() => {
    const base = linkWidth;
    return (link: L) => {
      const s = overlayStateRef.current;
      const lid = link.id;
      const isSelected = lid !== undefined && s.selectedLinkSet.has(lid);
      const sId =
        typeof link.source === "object"
          ? (link.source as N).id
          : (link.source as string | number | undefined);
      const tId =
        typeof link.target === "object"
          ? (link.target as N).id
          : (link.target as string | number | undefined);
      const hasNodeSelection = s.selectedNodeSet.size > 0;
      const isConnected =
        hasNodeSelection &&
        ((sId !== undefined && s.selectedNodeSet.has(sId)) ||
          (tId !== undefined && s.selectedNodeSet.has(tId)));
      const hasHover =
        s.highlightNeighborsOnHover && s.highlightedLinkIds.size > 0;
      const isHighlighted =
        hasHover && lid !== undefined && s.highlightedLinkIds.has(lid);
      const baseWidth =
        typeof base === "function"
          ? base(link)
          : typeof base === "number"
            ? base
            : ((link.width as number | undefined) ?? 1);
      // 3D maps `linkWidth` to cylinder radius, so the same absolute
      // bump reads roughly 3× chunkier than a canvas stroke. Scale
      // the selection/hover boosts down so the cue is visible without
      // turning links into pipes.
      const is3D = s.mode === "3d";
      if (isSelected) return baseWidth + (is3D ? 0.5 : 1.5);
      if (isConnected) return baseWidth + (is3D ? 0.3 : 1);
      if (isHighlighted) return baseWidth + (is3D ? 0.8 : 2.5);
      return baseWidth;
    };
  }, [linkWidth]);

  const wrappedLinkWidth = hasLinkWidthOverlay ? stableLinkWidth : linkWidth;

  // ─── nodeVal ──────────────────────────────────────────────────
  // Selection / hover size boost. On busy canvases the colour-only
  // selection cue gets lost in the noise — bumping `val` enlarges the
  // node geometry so the user can actually see what they picked.
  //
  // Multipliers are deliberately modest because `nodeVal` flows to
  // the kapsule's *shadow* canvas too (force-graph pipes it to both
  // the visible and the shadow graph). An oversized selected node
  // would start swallowing clicks aimed at small neighbours — the bug
  // that breaks selection on 10k+ stress graphs. `wrappedNodePointer-
  // AreaPaint` below pins the 2D shadow paint back to the original
  // `val`; for 3D we just keep the boost subtle since the raycaster
  // reads the actual mesh.
  const hasNodeValOverlay =
    selectedNodeSet.size > 0 ||
    (highlightNeighborsOnHover && highlightedNodeIds.size > 0);

  const stableNodeVal = useMemo<Accessor<number, N>>(() => {
    const base = nodeVal;
    return (node: N) => {
      const s = overlayStateRef.current;
      const id = node.id;
      const baseVal =
        typeof base === "function"
          ? base(node)
          : typeof base === "number"
            ? base
            : ((node.val as number | undefined) ?? 1);
      if (id !== undefined && s.selectedNodeSet.has(id)) return baseVal * 2.25;
      const hasHover =
        s.highlightNeighborsOnHover && s.highlightedNodeIds.size > 0;
      if (hasHover && id !== undefined && s.highlightedNodeIds.has(id)) {
        return s.hoverNodeId === id ? baseVal * 1.8 : baseVal * 1.4;
      }
      return baseVal;
    };
  }, [nodeVal]);

  const wrappedNodeVal = hasNodeValOverlay ? stableNodeVal : nodeVal;

  // ─── nodePointerAreaPaint (2D only) ──────────────────────────
  // Pin the kapsule's shadow-canvas (hit-test) paint to the *original*
  // node val regardless of what `wrappedNodeVal` does to the visible
  // size. Without this, selecting a node grows its hit zone too — on
  // dense graphs that zone then intercepts clicks aimed at nearby
  // unselected nodes ("wrong node selected" bug on the 10k+ stress
  // story).
  //
  // We install unconditionally in 2D (rather than gating on selection)
  // so the shadow canvas doesn't have to repaint on every click — the
  // kapsule throttles shadow refreshes to once every 800ms and any
  // re-bind triggers another throttled repaint cycle.
  const wrappedNodePointerAreaPaint = useMemo<
    NodePointerAreaPaint<N> | undefined
  >(() => {
    if (mode !== "2d") return nodePointerAreaPaint;
    if (nodePointerAreaPaint) return nodePointerAreaPaint;
    const base = nodeVal;
    const relSize = nodeRelSize ?? 4;
    return (
      node: N,
      color: string,
      ctx: CanvasRenderingContext2D,
      globalScale: number,
    ) => {
      const v =
        typeof base === "function"
          ? base(node)
          : typeof base === "number"
            ? base
            : ((node.val as number | undefined) ?? 1);
      const r =
        Math.sqrt(Math.max(0, v)) * relSize + 1 / Math.max(globalScale, 1e-6);
      ctx.fillStyle = color;
      ctx.beginPath();
      ctx.arc(node.x ?? 0, node.y ?? 0, r, 0, 2 * Math.PI, false);
      ctx.fill();
    };
  }, [mode, nodeVal, nodeRelSize, nodePointerAreaPaint]);

  // ─── nodeVisibility ───────────────────────────────────────────
  // Memoised wrapper for nodeVisibility. Only constructed when the
  // legend filter is active OR the host supplied a nodeVisibility.
  const wrappedNodeVisibility = useMemo<
    ((node: N) => boolean) | undefined
  >(() => {
    const needLegend = nodeAutoColorBy && hiddenGroups.size > 0;
    if (!needLegend && nodeVisibility === undefined) return undefined;
    const colorBy = nodeAutoColorBy;
    const baseVis = nodeVisibility;
    return (node: N): boolean => {
      if (needLegend && colorBy) {
        const groupVal =
          typeof colorBy === "function"
            ? (colorBy as (n: N) => unknown)(node)
            : (node as unknown as Record<string, unknown>)[
                colorBy as string
              ];
        if (
          groupVal !== null &&
          groupVal !== undefined &&
          hiddenGroups.has(String(groupVal))
        ) {
          return false;
        }
      }
      if (typeof baseVis === "function") return Boolean(baseVis(node));
      if (typeof baseVis === "boolean") return baseVis;
      if (typeof baseVis === "string") {
        return Boolean(
          (node as unknown as Record<string, unknown>)[baseVis],
        );
      }
      return true;
    };
  }, [nodeAutoColorBy, nodeVisibility, hiddenGroups]);

  return useMemo<AccessorOverrides<N, L>>(
    () => ({
      nodeColor: wrappedNodeColor,
      linkColor: wrappedLinkColor,
      linkWidth: wrappedLinkWidth,
      nodeVal: wrappedNodeVal,
      nodePointerAreaPaint: wrappedNodePointerAreaPaint,
      nodeVisibility: wrappedNodeVisibility,
    }),
    [
      wrappedNodeColor,
      wrappedLinkColor,
      wrappedLinkWidth,
      wrappedNodeVal,
      wrappedNodePointerAreaPaint,
      wrappedNodeVisibility,
    ],
  );
}
