import {
  useEffect,
  useRef,
  useState,
  type MutableRefObject,
} from "react";
import type { GraphEngine } from "../engines/types";
import type { LinkObject, NodeObject } from "../types";
import type { SelectionApi } from "./useGraphSelection";

export interface MarqueeRect {
  x0: number;
  y0: number;
  x1: number;
  y1: number;
  additive: boolean;
}

export interface UseMarqueeAndCursorParams<
  N extends NodeObject,
  L extends LinkObject,
> {
  mount: HTMLDivElement | null;
  /** Trampoline to the live engine — read inside event handlers so we
   *  always pick up the current engine after a 2D↔3D remount. */
  engineRef: MutableRefObject<GraphEngine<N, L> | null>;
  selection: SelectionApi;
  nodes: ReadonlyArray<N>;
  links: ReadonlyArray<L>;
  setSelectedLinkIds: React.Dispatch<
    React.SetStateAction<Array<string | number>>
  >;
  /** Live (non-debounced) hover state from the kapsule's
   *  `onNodeHover`. Used at shift+mousedown time to decide between
   *  "shift+click to add to selection" (a node is under the cursor →
   *  let the click fall through to the kapsule) and "shift+drag to
   *  marquee" (background → start a rectangle).
   *
   *  Must be a ref, not a state value: the host applies a grace-period
   *  debounce to `hoverNodeId` so labels can persist while the cursor
   *  briefly leaves a node. Reading a debounced value here would tell
   *  us "still hovering A" for 250ms after the cursor has actually
   *  moved away — long enough that the user's next shift+drag would
   *  be wrongly treated as a click-falling-through-to-A, and no
   *  marquee would start. */
  liveHoverNodeIdRef: MutableRefObject<string | number | null>;
}

export interface UseMarqueeAndCursorResult {
  marquee: MarqueeRect | null;
  /** Last screen-space cursor position over the mount element. Used by
   *  paste-at-cursor. */
  lastCursorRef: MutableRefObject<{ x: number; y: number } | null>;
}

/** Wires up the canvas-level pointer behaviour that lives outside the
 *  kapsule:
 *
 *  - Shift+drag draws a marquee rectangle. Nodes whose projected screen
 *    coordinates land inside the rectangle get selected on release.
 *  - The last cursor position over the mount is tracked into a ref so
 *    paste-at-cursor knows where to drop new nodes.
 *  - Any wheel / touch / mousedown on the canvas interrupts an in-flight
 *    focus animation.
 *
 *  The effect mounts once per mount element. Live data is read through a
 *  ref so the listener identity stays stable across data mutations —
 *  re-binding ResizeObserver + window listeners on every change would
 *  otherwise glitch the marquee gesture mid-drag. */
export function useMarqueeAndCursor<
  N extends NodeObject,
  L extends LinkObject,
>(
  params: UseMarqueeAndCursorParams<N, L>,
): UseMarqueeAndCursorResult {
  const {
    mount,
    engineRef,
    selection,
    nodes,
    links,
    setSelectedLinkIds,
    liveHoverNodeIdRef,
  } = params;

  // Trampoline ref — lets the effect mount once and read latest
  // data/selection on each gesture without re-binding ResizeObserver +
  // window listeners on every node/link mutation.
  const latestRef = useRef({
    selection,
    nodes,
    links,
    setSelectedLinkIds,
    liveHoverNodeIdRef,
  });
  latestRef.current.selection = selection;
  latestRef.current.nodes = nodes;
  latestRef.current.links = links;
  latestRef.current.setSelectedLinkIds = setSelectedLinkIds;
  latestRef.current.liveHoverNodeIdRef = liveHoverNodeIdRef;

  const [marquee, setMarquee] = useState<MarqueeRect | null>(null);
  const lastCursorRef = useRef<{ x: number; y: number } | null>(null);

  useEffect(() => {
    if (!mount) return;

    // Cache the mount rect so the cursor-tracking handler doesn't force
    // a layout on every mousemove. The rect only needs to update on
    // resize / scroll — at 60Hz the per-move getBoundingClientRect()
    // was the bulk of the listener overhead.
    let cachedRect = mount.getBoundingClientRect();
    const refreshRect = () => {
      cachedRect = mount.getBoundingClientRect();
    };
    const ro = new ResizeObserver(refreshRect);
    ro.observe(mount);
    window.addEventListener("scroll", refreshRect, {
      passive: true,
      capture: true,
    });

    const onMouseMove = (e: MouseEvent) => {
      lastCursorRef.current = {
        x: e.clientX - cachedRect.left,
        y: e.clientY - cachedRect.top,
      };
    };
    mount.addEventListener("mousemove", onMouseMove);

    const onMouseDown = (e: MouseEvent) => {
      // Any mousedown on the canvas — pan, orbit, marquee, drag —
      // interrupts an in-flight focus animation. Safe even when no
      // animation is running.
      engineRef.current?.stopAnimation();

      if (e.button !== 0 || !e.shiftKey) return;
      // Shift+press on a node should add it to the selection, not
      // start a marquee. Defer to the kapsule's click pipeline — its
      // `onNodeClick` fires through to `handleNodeClick`, which
      // already does the additive toggle when `event.shiftKey` is
      // set. We rely on the kapsule's hover state (fed back to us
      // via the `onNodeHover` → `setHoverNodeId` path) to detect
      // whether the press landed on a node.
      if (latestRef.current.liveHoverNodeIdRef.current !== null) return;
      const rect = mount.getBoundingClientRect();
      const x0 = e.clientX - rect.left;
      const y0 = e.clientY - rect.top;
      e.stopPropagation();
      e.preventDefault();
      setMarquee({ x0, y0, x1: x0, y1: y0, additive: e.shiftKey });

      const onMove = (ev: MouseEvent) => {
        const r = mount.getBoundingClientRect();
        const x1 = ev.clientX - r.left;
        const y1 = ev.clientY - r.top;
        setMarquee((cur) => (cur ? { ...cur, x1, y1 } : cur));
      };
      const onUp = (ev: MouseEvent) => {
        window.removeEventListener("mousemove", onMove);
        window.removeEventListener("mouseup", onUp, true);
        const r = mount.getBoundingClientRect();
        const x1 = ev.clientX - r.left;
        const y1 = ev.clientY - r.top;
        setMarquee(null);
        const eng = engineRef.current;
        if (!eng) return;

        const xMin = Math.min(x0, x1);
        const yMin = Math.min(y0, y1);
        const xMax = Math.max(x0, x1);
        const yMax = Math.max(y0, y1);
        // Tiny box = no-op at the marquee level. Marquee only runs
        // when shift is held, so the intent is always additive — let
        // the kapsule's click pipeline take it from here (node click →
        // additive toggle, background click → preserves selection
        // when shift is held; see handleBackgroundClick). Previously
        // we cleared here, which raced the kapsule's onNodeClick when
        // its hover-state lagged the cursor (the shadow-canvas hit
        // test is throttled on dense graphs), wiping the running
        // selection right before the new node was toggled in.
        if (xMax - xMin < 3 && yMax - yMin < 3) {
          return;
        }
        const latest = latestRef.current;
        const latestNodes = latest.nodes;
        const latestLinks = latest.links;
        const latestSelection = latest.selection;
        const latestSetSelectedLinkIds = latest.setSelectedLinkIds;
        const nodeHits: Array<string | number> = [];
        const inBox = (sx: number, sy: number) =>
          sx >= xMin && sx <= xMax && sy >= yMin && sy <= yMax;
        // Build the id→node map once at release — used by both the
        // node-hit pass and the link-endpoint resolver.
        const nodeById = new Map<string | number, N>();
        for (const node of latestNodes) {
          nodeById.set(node.id, node);
          if (node.x === undefined || node.y === undefined) continue;
          const sc = eng.graph2Screen(node.x, node.y, node.z);
          if (inBox(sc.x, sc.y)) nodeHits.push(node.id);
        }

        // Link hits: a link is grabbed if its midpoint falls inside the
        // marquee, OR if both of its endpoints do. After the kapsule's
        // first tick, `source` and `target` are resolved to node objects
        // with x/y/z — before that, they're still string/number ids, so
        // we fall back to looking them up.
        const linkHits: Array<string | number> = [];
        const resolveEndpoint = (
          end: unknown,
        ): { x: number; y: number; z?: number } | null => {
          if (end && typeof end === "object") {
            const n = end as N;
            if (n.x !== undefined && n.y !== undefined) {
              return n.z !== undefined
                ? { x: n.x, y: n.y, z: n.z }
                : { x: n.x, y: n.y };
            }
          }
          if (typeof end === "string" || typeof end === "number") {
            const n = nodeById.get(end);
            if (n && n.x !== undefined && n.y !== undefined) {
              return n.z !== undefined
                ? { x: n.x, y: n.y, z: n.z }
                : { x: n.x, y: n.y };
            }
          }
          return null;
        };
        for (const link of latestLinks) {
          if (link.id === undefined) continue;
          const s = resolveEndpoint(link.source);
          const t = resolveEndpoint(link.target);
          if (!s || !t) continue;
          const mid = eng.graph2Screen(
            (s.x + t.x) / 2,
            (s.y + t.y) / 2,
            s.z !== undefined && t.z !== undefined
              ? (s.z + t.z) / 2
              : undefined,
          );
          const sc1 = eng.graph2Screen(s.x, s.y, s.z);
          const sc2 = eng.graph2Screen(t.x, t.y, t.z);
          if (
            inBox(mid.x, mid.y) ||
            (inBox(sc1.x, sc1.y) && inBox(sc2.x, sc2.y))
          ) {
            linkHits.push(link.id);
          }
        }

        const additive = ev.shiftKey || ev.metaKey || ev.ctrlKey;
        if (additive) {
          const existing = latestSelection.selected;
          const merged: Array<string | number> = existing.slice();
          const seen = new Set<string | number>(existing);
          for (const id of nodeHits) {
            if (!seen.has(id)) {
              seen.add(id);
              merged.push(id);
            }
          }
          latestSelection.set(merged);
          latestSetSelectedLinkIds((cur) => {
            const out = cur.slice();
            const seenL = new Set<string | number>(cur);
            for (const id of linkHits) {
              if (!seenL.has(id)) {
                seenL.add(id);
                out.push(id);
              }
            }
            return out;
          });
        } else {
          latestSelection.set(nodeHits);
          latestSetSelectedLinkIds(linkHits);
        }
      };
      window.addEventListener("mousemove", onMove);
      // Capture-phase so we beat d3-zoom's mouseup, which clears its
      // own internal state.
      window.addEventListener("mouseup", onUp, true);
    };

    // Capture-phase listener so we run before d3-zoom's handler.
    mount.addEventListener("mousedown", onMouseDown, true);

    // Wheel / touch interaction also kills any focus animation —
    // otherwise zooming or pinch-zooming during the camera tween would
    // fight the tween until it ends.
    const onWheel = () => engineRef.current?.stopAnimation();
    const onTouchStart = () => engineRef.current?.stopAnimation();
    mount.addEventListener("wheel", onWheel, {
      passive: true,
      capture: true,
    });
    mount.addEventListener("touchstart", onTouchStart, {
      passive: true,
      capture: true,
    });

    return () => {
      mount.removeEventListener("mousemove", onMouseMove);
      mount.removeEventListener("mousedown", onMouseDown, true);
      mount.removeEventListener("wheel", onWheel, {
        capture: true,
      } as EventListenerOptions);
      mount.removeEventListener("touchstart", onTouchStart, {
        capture: true,
      } as EventListenerOptions);
      window.removeEventListener("scroll", refreshRect, {
        capture: true,
      } as EventListenerOptions);
      ro.disconnect();
    };
    // Re-bind only when the mount element itself changes. The marquee
    // reads latest selection/nodes/links from the latestRef on each
    // gesture, so we don't need to re-bind ResizeObserver + window
    // listeners on every data mutation.
  }, [mount, engineRef]);

  return { marquee, lastCursorRef };
}
