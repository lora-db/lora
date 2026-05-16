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
  /** Direct-link hover. Distinct from `highlightedLinkIds`, which
   *  only fires when a node is hovered and its neighbour links light
   *  up. When the user hovers a link itself, only `hoverLinkId` is
   *  set — both signals need to flow into the size/colour wrappers
   *  so the link bumps up on direct hover, not just node-neighbour
   *  hover. */
  hoverLinkId: string | number | null;
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
 *  2. When an overlay is active, the wrapper identity flips on every
 *     selection/hover state change. This is REQUIRED — three-forcegraph's
 *     link/node digest (see `three-forcegraph.mjs:1185`) only re-evaluates
 *     accessors when one of its tracked props changes identity. Keeping
 *     the wrapper "stable" through selection changes — as an earlier
 *     iteration of this hook did — caused the kapsule to cache the colour
 *     and width of whichever link the user first clicked and never update
 *     them on subsequent clicks, despite the React state being correct.
 *
 *     The closure body still reads the latest state from a ref, but the
 *     memo deps include every overlay signal so the identity flips with
 *     each tick of the state machine. The kapsule's own internal digest
 *     is smart enough to only rebuild geometries/materials whose value
 *     actually changed (cheap on small per-click selection deltas), so
 *     the cost of identity churn here is bounded. */
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
    hoverLinkId,
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
    hoverLinkId,
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
    hoverLinkId,
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
    // Identity must flip on any state that affects this accessor's
    // output, or the kapsule's link/node digest won't pick up the
    // change — see the block-level comment above.
  }, [
    nodeColor,
    selectedNodeSet,
    selectedLinkSet,
    hoverNodeId,
    highlightedNodeIds,
  ]);

  const wrappedNodeColor = hasNodeColorOverlay ? stableNodeColor : nodeColor;

  // ─── linkColor ────────────────────────────────────────────────
  const hasHiddenGroups =
    nodeAutoColorBy !== undefined && hiddenGroups.size > 0;
  const hasLinkColorOverlay =
    selectedNodeSet.size > 0 ||
    selectedLinkSet.size > 0 ||
    hoverLinkId !== null ||
    (highlightNeighborsOnHover && highlightedLinkIds.size > 0) ||
    hasHiddenGroups;

  // ── Link palette ──────────────────────────────────────────────
  // Three-tier emphasis hierarchy, matching the canonical graph-viewer
  // mental model:
  //
  //   default        — dim grey, recedes into the background
  //   hover          — "lights up" to a brighter grey (still neutral
  //                    so the cue reads as "you're hovering here",
  //                    not "this is selected"). Fires for BOTH direct
  //                    link hover and node-neighbour highlight.
  //   connected      — tinted toward the accent so the user can trace
  //                    what touches their current selection.
  //   selected       — accent colour at full strength.
  //
  // IMPORTANT: `LINK_DEFAULT` and `LINK_HOVER` share the same alpha
  // channel. Three.js sets `material.transparent = opacity < 1` and
  // `material.depthWrite = opacity >= 1` when the kapsule rebuilds
  // link materials. Flipping a single hovered link from `transparent`
  // to `opaque` (or vice versa) pulls it out of one sort group and
  // into another, which reshuffles the *entire* transparent render
  // pass for that frame — neighbouring lines visibly flicker as they
  // re-sort around the new opaque mesh. Keeping both states in the
  // same transparency class anchors the sort order, so hover changes
  // only the RGB triplet and the renderer never has to reorder.
  // Selection still crosses the class boundary (accent at α=1) but
  // that's a single click, not a 60Hz event.
  const LINK_DEFAULT = "rgba(96, 102, 110, 0.55)";
  const LINK_HOVER = "rgba(180, 188, 198, 0.55)";
  const stableLinkColor = useMemo<Accessor<string, L>>(() => {
    const base = linkColor;
    // The legend-filter / nodeAutoColorBy axis is static for the
    // lifetime of a given prop value, so we can close over it
    // directly — only the *active* hidden-groups Set lives in the
    // overlay ref.
    const colorBy = nodeAutoColorBy;
    return (link: L) => {
      const s = overlayStateRef.current;
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
          else color = (link.color as string | undefined) ?? LINK_DEFAULT;
          return adjustAlpha(color, 0.05);
        }
      }

      // Selected wins over everything else — full accent.
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
      // "Connected to a selected node" — strong cue, still accent but
      // a hair lighter so the directly-selected links stand out.
      if (isConnected) return adjustAlpha(s.accentColor, 0.85);
      // Hover — fires for BOTH direct link hover (`hoverLinkId`) and
      // node-neighbour highlight (`highlightedLinkIds`). The "lit up"
      // grey is distinct from the accent reserved for selection so
      // the user always knows which state they're in.
      const directHover = lid !== undefined && s.hoverLinkId === lid;
      const neighbourHover =
        s.highlightNeighborsOnHover &&
        lid !== undefined &&
        s.highlightedLinkIds.has(lid);
      if (directHover || neighbourHover) {
        return LINK_HOVER;
      }
      // Default — host-provided colour wins; otherwise the dim grey
      // baseline.
      let color: string;
      if (typeof base === "function") color = base(link);
      else if (typeof base === "string") color = base;
      else color = (link.color as string | undefined) ?? LINK_DEFAULT;
      const hasSelection =
        hasNodeSelection || s.selectedLinkSet.size > 0;
      // When SOMETHING is selected, push everything else into the
      // background so the focus reads cleanly. 3D keeps a slightly
      // higher floor (depth gives extra separation) than 2D.
      if (hasSelection)
        return adjustAlpha(color, s.mode === "3d" ? 0.45 : 0.18);
      return color;
    };
    // Identity has to flip on every overlay-state change — see the
    // block-level comment for why stable identity broke the kapsule's
    // material/geometry digest.
  }, [
    linkColor,
    nodeAutoColorBy,
    selectedLinkSet,
    selectedNodeSet,
    hoverLinkId,
    highlightedLinkIds,
    hiddenGroups,
  ]);

  const wrappedLinkColor = hasLinkColorOverlay ? stableLinkColor : linkColor;

  // ─── linkWidth ────────────────────────────────────────────────
  const hasLinkWidthOverlay =
    selectedNodeSet.size > 0 ||
    selectedLinkSet.size > 0 ||
    hoverLinkId !== null ||
    (highlightNeighborsOnHover && highlightedLinkIds.size > 0);

  // Three-tier size hierarchy. Selection is communicated by colour
  // (full accent) rather than stroke, so the SELECTED bump stays at or
  // below the HOVER bump — a fat selected stroke felt heavy next to
  // the neighbour-hover preview and crowded out adjacent edges. Hover
  // is the visual ceiling; selected dips a hair under it so the colour
  // stays the dominant cue. Values are split by mode because the
  // engine maps `linkWidth` differently:
  //   - 3D / cylinder links → linkWidth becomes cylinder radius, so a
  //     +0.6 bump reads chunky. Bumps stay modest.
  //   - 2D / legacy Canvas2D stroke → linkWidth is a CSS-pixel stroke
  //     width, where a +3 bump is visually equivalent to the 3D +1.
  //     Kept for hosts that opt back into Canvas2D rendering.
  const HOVER_3D = 0.6;
  const HOVER_2D = 1.5;
  const CONNECTED_3D = 0.4;
  const CONNECTED_2D = 1.0;
  const SELECTED_3D = 0.5;
  const SELECTED_2D = 1.2;
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
      // Hover counts the same whether the user hovered the LINK
      // directly (`hoverLinkId`) or hovered a node whose neighbours
      // got highlighted (`highlightedLinkIds`) — both bump the width.
      const directHover = lid !== undefined && s.hoverLinkId === lid;
      const neighbourHover =
        s.highlightNeighborsOnHover &&
        lid !== undefined &&
        s.highlightedLinkIds.has(lid);
      const isHovered = directHover || neighbourHover;
      const baseWidth =
        typeof base === "function"
          ? base(link)
          : typeof base === "number"
            ? base
            : ((link.width as number | undefined) ?? 1);
      const is3D = s.mode === "3d";
      // Order of precedence matches the colour wrapper above so the
      // width and colour always agree on which state a link is in.
      if (isSelected) return baseWidth + (is3D ? SELECTED_3D : SELECTED_2D);
      if (isHovered) return baseWidth + (is3D ? HOVER_3D : HOVER_2D);
      if (isConnected) return baseWidth + (is3D ? CONNECTED_3D : CONNECTED_2D);
      return baseWidth;
    };
    // Stable identity by design. three-forcegraph treats `linkWidth`
    // as a `.clear()` trigger (see `three-forcegraph.mjs:1201`), so
    // flipping its identity tears down every link mesh and rebuilds
    // them from scratch — at ~5–10 ms per 1k links, that's the bulk
    // of the cost on a hover-driven 60 Hz update *and* it visibly
    // wipes labels for a frame. Instead the wrapper stays stable and
    // we lean on the `linkColor` digest below (which fires for the
    // same state changes but only walks existing meshes in place) to
    // pick up the new width values — the kapsule re-evaluates both
    // accessors inside its `onUpdateObj`, so a stable wrapper that
    // reads live state through `overlayStateRef` still produces the
    // right per-link value on every digest tick.
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
    // Stable identity by design — the node digest's `.clear()`
    // trigger list doesn't include `nodeVal`, so we *could* put state
    // in deps without triggering mesh recreation. But we don't need
    // to: the digest fires on `nodeColor` identity changes (which DO
    // happen on every state change), and inside its `onUpdateObj`
    // walk both `nodeColor` and `nodeVal` accessors are re-evaluated
    // per node. A stable `nodeVal` wrapper that reads live state
    // through `overlayStateRef` therefore still produces the right
    // per-node value on every state change, at zero churn cost.
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
