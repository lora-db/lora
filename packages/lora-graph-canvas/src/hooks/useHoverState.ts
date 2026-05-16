import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type MutableRefObject,
} from "react";
import { EMPTY_ID_SET, readAccessor } from "../utils/accessor";
import type { Accessor, LinkObject, NodeObject } from "../types";

// Grace period between mouseleave and the actual clear of the hover
// pill. Tuned so the cursor can travel from a hovered node up to its
// label (which sits ~`radius * 2 + 4` graph units above the node)
// without the hover state flickering off in between.
const HOVER_GRACE_MS = 250;

export interface UseHoverStateParams<
  N extends NodeObject,
  L extends LinkObject,
> {
  highlightNeighborsOnHover: boolean;
  nodeLabel?: Accessor<string | HTMLElement, N>;
  linkLabel?: Accessor<string | HTMLElement, L>;
  onNodeHover?: (node: N | null, previousNode: N | null) => void;
  onLinkHover?: (link: L | null, previousLink: L | null) => void;
}

export interface HoverState<N extends NodeObject, L extends LinkObject> {
  /** Debounced hovered-node id — drives label visibility / highlight
   *  pipeline. Keeps a 250 ms grace period after mouseleave so labels
   *  stay readable while the cursor moves to them. */
  hoverNodeId: string | number | null;
  /** Debounced hovered-link id — companion to `hoverNodeId`. */
  hoverLinkId: string | number | null;
  /** Live (non-debounced) hovered-node id — updated synchronously from
   *  the kapsule's `onNodeHover` before the grace timer is even
   *  scheduled. Read this (not `hoverNodeId`) from interactions that
   *  need to know whether the cursor is *currently* over a node, e.g.
   *  the marquee hook's shift+mousedown guard. Reading the debounced
   *  value there would tell us "still hovering A" for 250 ms after the
   *  cursor moved off A, which is long enough that the next shift+drag
   *  would never start a marquee. */
  liveHoverNodeIdRef: MutableRefObject<string | number | null>;
  /** Live (non-debounced) hovered-link id — same contract as
   *  `liveHoverNodeIdRef`, used so neighbour-link hits don't get
   *  swallowed during the label-grace window. */
  liveHoverLinkIdRef: MutableRefObject<string | number | null>;
  /** Set of node ids that should currently render with neighbour-
   *  highlight styling (the hovered node plus its `_neighbors`). */
  highlightedNodeIds: ReadonlySet<string | number>;
  /** Set of link ids that should currently render with neighbour-
   *  highlight styling (links touching the hovered node). */
  highlightedLinkIds: ReadonlySet<string | number>;
  /** Set of node ids whose label should currently be drawn for the
   *  hover affordance (hovered node + neighbours when highlighting is
   *  on; just the hovered node otherwise; empty when nothing hovered).
   *  Memoised so referential equality bails the renderer's no-op
   *  frames out cheaply. */
  hoveredNodeSet: ReadonlySet<string | number>;
  /** Sibling of `hoveredNodeSet` for the link-label renderer. */
  hoveredLinkSet: ReadonlySet<string | number>;
  /** Current hover-tooltip content. `null` when no hover. */
  tooltipContent: string | HTMLElement | null;
  /** Callback to wire into `engineProps.onNodeHover`. */
  handleNodeHover: (node: N | null, prev: N | null) => void;
  /** Callback to wire into `engineProps.onLinkHover`. */
  handleLinkHover: (link: L | null, prev: L | null) => void;
  /** Pin the node-hover state to the given node for the duration of
   *  a drag. The kapsule fires `onNodeHover(null)` on mousedown of a
   *  drag gesture; without this pin, the grace timer would clear
   *  `highlightedNodeIds` + `highlightedLinkIds` mid-drag and the
   *  neighbour labels would vanish. Call `pinHover(node)` from the
   *  drag-start handler and `pinHover(null)` from drag-end. Calling
   *  with a node also forces the hover state to that node so a drag
   *  begun without a preceding hover lights up the right
   *  neighbours. */
  pinHover: (node: N | null) => void;
}

/** Owns the hover-driven state that the canvas reacts to: the
 *  debounced ids for label visibility, the live refs for gesture
 *  guards, the neighbour-highlight sets, and the hover-tooltip
 *  content. Pulled out of `LoraGraphCanvas` so the live-vs-debounced
 *  split is a first-class contract instead of a tangle of useState +
 *  useRef calls in the canvas body. */
export function useHoverState<
  N extends NodeObject,
  L extends LinkObject,
>(params: UseHoverStateParams<N, L>): HoverState<N, L> {
  const { highlightNeighborsOnHover, nodeLabel, linkLabel, onNodeHover, onLinkHover } =
    params;

  const [hoverNodeId, setHoverNodeId] = useState<string | number | null>(null);
  const [hoverLinkId, setHoverLinkId] = useState<string | number | null>(null);
  const [highlightedNodeIds, setHighlightedNodeIds] = useState<
    ReadonlySet<string | number>
  >(EMPTY_ID_SET);
  const [highlightedLinkIds, setHighlightedLinkIds] = useState<
    ReadonlySet<string | number>
  >(EMPTY_ID_SET);
  const [tooltipContent, setTooltipContent] = useState<
    string | HTMLElement | null
  >(null);

  const liveHoverNodeIdRef = useRef<string | number | null>(null);
  const liveHoverLinkIdRef = useRef<string | number | null>(null);
  // Set by `pinHover` while a drag is in progress. When non-null,
  // `handleNodeHover` swallows the kapsule's `onNodeHover(null)`
  // event so the neighbour-highlight stays lit for the whole drag.
  // A ref (not state) so the suppression check is synchronous with
  // the kapsule event — a useState update wouldn't have flushed by
  // the time the next hover event arrives.
  const dragPinRef = useRef<string | number | null>(null);
  // Per-kind pending clears. They used to share one ref so re-entering
  // either kind cancelled the other's clear — but that meant a link
  // hover after a node mouseleave cancelled the node-clear timer
  // without re-scheduling it, leaving `hoverNodeId` stuck "on" until
  // another node-hover event arrived. Keeping them separate lets each
  // kind manage its own 250 ms grace independently.
  const nodeClearTimerRef = useRef<ReturnType<typeof setTimeout> | null>(
    null,
  );
  const linkClearTimerRef = useRef<ReturnType<typeof setTimeout> | null>(
    null,
  );
  useEffect(() => {
    return () => {
      if (nodeClearTimerRef.current !== null) {
        clearTimeout(nodeClearTimerRef.current);
        nodeClearTimerRef.current = null;
      }
      if (linkClearTimerRef.current !== null) {
        clearTimeout(linkClearTimerRef.current);
        linkClearTimerRef.current = null;
      }
    };
  }, []);

  const handleNodeHover = useCallback(
    (node: N | null, prev: N | null) => {
      onNodeHover?.(node, prev);
      // Live ref updates synchronously so interactions that need the
      // *current* hover state (marquee shift+mousedown guard, etc) see
      // the truth, not the still-running 250 ms grace window below.
      liveHoverNodeIdRef.current = node?.id ?? null;
      // Swallow the kapsule's mid-drag `onNodeHover(null)`. Without
      // this, the grace timer below would clear the neighbour
      // highlight 250 ms into the drag and the connected labels
      // would disappear from under the user's cursor.
      if (!node && dragPinRef.current !== null) return;
      // Cancel only our own pending clear — a link's grace timer is
      // independent. The node-grace exists to keep the node label
      // readable while the cursor moves toward it; it doesn't care
      // what the link hover is doing.
      if (nodeClearTimerRef.current !== null) {
        clearTimeout(nodeClearTimerRef.current);
        nodeClearTimerRef.current = null;
      }
      if (!node) {
        nodeClearTimerRef.current = setTimeout(() => {
          nodeClearTimerRef.current = null;
          setHoverNodeId(null);
          if (highlightNeighborsOnHover) {
            // Reuse the sentinel so repeat mouseleave events bail out
            // at useState's identity comparison instead of triggering
            // a no-op repaint.
            setHighlightedNodeIds(EMPTY_ID_SET);
            setHighlightedLinkIds(EMPTY_ID_SET);
          }
          // Only clear the tooltip if no link hover is currently
          // showing it — otherwise we'd hide the link's tooltip just
          // because the node grace timer fired first.
          if (liveHoverLinkIdRef.current === null) {
            setTooltipContent(null);
          }
        }, HOVER_GRACE_MS);
        return;
      }
      if (highlightNeighborsOnHover) {
        const nIds = new Set<string | number>([node.id]);
        const lIds = new Set<string | number>();
        const neighbours =
          (node as unknown as { _neighbors?: N[] })._neighbors ?? [];
        for (const n of neighbours) if (n.id !== undefined) nIds.add(n.id);
        const links = (node as unknown as { _links?: L[] })._links ?? [];
        for (const l of links) if (l.id !== undefined) lIds.add(l.id);
        setHoverNodeId(node.id);
        setHighlightedNodeIds(nIds);
        setHighlightedLinkIds(lIds);
      } else {
        setHoverNodeId(node.id);
      }
      const label = readAccessor<string | HTMLElement, N>(nodeLabel, node);
      setTooltipContent(label ?? null);
    },
    [onNodeHover, nodeLabel, highlightNeighborsOnHover],
  );

  const handleLinkHover = useCallback(
    (link: L | null, prev: L | null) => {
      onLinkHover?.(link, prev);
      liveHoverLinkIdRef.current = link?.id ?? null;
      if (linkClearTimerRef.current !== null) {
        clearTimeout(linkClearTimerRef.current);
        linkClearTimerRef.current = null;
      }
      if (!link) {
        linkClearTimerRef.current = setTimeout(() => {
          linkClearTimerRef.current = null;
          setHoverLinkId(null);
          if (liveHoverNodeIdRef.current === null) {
            setTooltipContent(null);
          }
        }, HOVER_GRACE_MS);
        return;
      }
      setHoverLinkId(link.id ?? null);
      const label = readAccessor<string | HTMLElement, L>(linkLabel, link);
      setTooltipContent(label ?? null);
    },
    [onLinkHover, linkLabel],
  );

  // Hovered-node label set: hovered node alone, hovered node +
  // neighbours, or empty. Singleton EMPTY_ID_SET reuse means a no-hover
  // frame bails out at the label renderer's early-out reference check.
  const hoveredNodeSet = useMemo<ReadonlySet<string | number>>(() => {
    if (hoverNodeId === null) return EMPTY_ID_SET;
    if (highlightNeighborsOnHover) return highlightedNodeIds;
    return new Set([hoverNodeId]);
  }, [hoverNodeId, highlightNeighborsOnHover, highlightedNodeIds]);

  const hoveredLinkSet = useMemo<ReadonlySet<string | number>>(() => {
    if (hoverLinkId !== null) {
      // Hovering the link itself wins — show its label even if
      // neighbour highlight is off.
      if (highlightNeighborsOnHover && hoverNodeId !== null) {
        const next = new Set(highlightedLinkIds);
        next.add(hoverLinkId);
        return next;
      }
      return new Set([hoverLinkId]);
    }
    if (hoverNodeId !== null && highlightNeighborsOnHover) {
      return highlightedLinkIds;
    }
    return EMPTY_ID_SET;
  }, [
    hoverLinkId,
    hoverNodeId,
    highlightNeighborsOnHover,
    highlightedLinkIds,
  ]);

  const pinHover = useCallback(
    (node: N | null) => {
      dragPinRef.current = node?.id ?? null;
      if (node) {
        // Force the hover state to the dragged node so a drag that
        // didn't start from a hover (cursor moved fast, mousedown
        // landed on the node before the kapsule fired its enter
        // event) still lights up neighbours.
        handleNodeHover(node, null);
        return;
      }
      // Released: re-arm the grace timer so the highlight fades out
      // naturally if the cursor isn't actually over a node anymore.
      // (If it is, the kapsule will fire `onNodeHover(node)` next
      // tick and cancel this timer.)
      handleNodeHover(null, null);
    },
    [handleNodeHover],
  );

  return {
    hoverNodeId,
    hoverLinkId,
    liveHoverNodeIdRef,
    liveHoverLinkIdRef,
    highlightedNodeIds,
    highlightedLinkIds,
    hoveredNodeSet,
    hoveredLinkSet,
    tooltipContent,
    handleNodeHover,
    handleLinkHover,
    pinHover,
  };
}
