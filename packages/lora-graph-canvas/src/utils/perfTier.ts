import type {
  GraphMode,
  LinkObject,
  LoraGraphCanvasProps,
  NodeObject,
} from "../types";

/** Performance tier — picked from the live node + link count and used
 *  to inject sensible defaults on top of the user's props. Higher
 *  tiers trade visual quality for frame rate so the renderer stays
 *  responsive on large graphs. */
export type PerfTier = "default" | "large" | "xlarge" | "huge";

export interface PickPerfTierInput {
  nodeCount: number;
  linkCount: number;
}

/** Map graph size to a tier. Links count for half a node since they're
 *  cheaper than nodes to render (one canvas line vs a circle + label),
 *  but they still drive force ticks. Thresholds were picked empirically
 *  to keep 60fps on a 2019 MacBook Pro:
 *    – default  : < 2k         (no tuning, full quality)
 *    – large    : 2k …  10k    (mild tweaks, still pretty)
 *    – xlarge   : 10k … 50k    (aggressive: ngraph layout in 3D, no
 *                                particles, faster cooldown)
 *    – huge     : 50k+         (all bets off — 100k target)
 */
export function pickPerfTier({
  nodeCount,
  linkCount,
}: PickPerfTierInput): PerfTier {
  const weighted = nodeCount + linkCount * 0.5;
  if (weighted >= 50_000) return "huge";
  if (weighted >= 10_000) return "xlarge";
  if (weighted >= 2_000) return "large";
  return "default";
}

/** Tier-specific prop defaults. Returned as a plain object so it can
 *  be spread *before* the user's props in the engine prop bag — the
 *  host's explicit values always win, so this only fills holes.
 *
 *  We deliberately only set the props that move the needle on the
 *  bottleneck for that tier (drawing or simulation), and only when the
 *  user hasn't already opted in to a more conservative value. */
export function perfTierDefaults<
  N extends NodeObject = NodeObject,
  L extends LinkObject = LinkObject,
>(tier: PerfTier, mode: GraphMode): Partial<LoraGraphCanvasProps<N, L>> {
  if (tier === "default") return {};

  const out: Partial<LoraGraphCanvasProps<N, L>> = {
    // Faster simulation cool-down — the layout still settles, just
    // gives up on the long tail. cooldownTicks caps the iteration
    // count regardless of frame timing, so it works the same on slow
    // and fast machines.
    cooldownTicks: tier === "large" ? 100 : tier === "xlarge" ? 60 : 30,
    d3AlphaDecay:
      tier === "large" ? 0.04 : tier === "xlarge" ? 0.08 : 0.15,
    d3VelocityDecay: tier === "huge" ? 0.6 : 0.4,
    // Default link-hover precision is 8px; shrink it as the graph
    // grows so the shadow-canvas hit-test stays cheap.
    linkHoverPrecision: tier === "large" ? 4 : 2,
    // Skip the warmup pass — it runs the simulation N times before
    // the first paint, blocking the main thread on mount. At this
    // scale users would rather see the graph "settle in" than wait.
    warmupTicks: 0,
    // Strip the expensive optional render passes. These are
    // accessor-driven so writing `0` short-circuits the per-link
    // particle / arrow geometry the kapsule would otherwise build
    // every frame. Host accessors still win through the spread in
    // `engineProps`.
    linkDirectionalParticles: 0,
    linkDirectionalArrowLength: 0,
  };

  // The unified engine always renders via Three.js (the "mode" the
  // host sees is a presentation overlay — top-down camera + z-pin in
  // 2D mode, orbit camera in 3D — but the underlying engine is one
  // and the same). So perf knobs are universal: nothing here is
  // mode-specific anymore. ngraph swap, sphere/cylinder resolution,
  // opacity, and raycaster throttle all apply in both modes.
  void mode;
  out.forceEngine = "ngraph";
  // Sphere segment count by tier. The visible difference between 6
  // and 4 is hard to see at typical node sizes, while the triangle
  // count halves — so xlarge drops to 4 for the geometry savings.
  out.nodeResolution =
    tier === "large" ? 8 : tier === "xlarge" ? 4 : 3;
  // `linkResolution: 0` renders links as 1px THREE.LineSegments
  // instead of cylinders. At 10k+ links the cylinder geometry path
  // builds 10k tiny meshes — each one a draw call, vertex buffer and
  // material rebind. Lines coalesce into a single buffer geometry
  // (one draw call total), which is the single biggest geometry win
  // available without going to instancing. We give up cylinder
  // thickness (which already wasn't legible at this density anyway).
  out.linkResolution = tier === "large" ? 4 : 0;
  out.nodeOpacity = 1;
  out.linkOpacity = 1;
  // Higher throttle = less raycaster work. Hover events are coalesced
  // by the throttle, but CLICK selection rides the same raycaster path
  // — and a >50 ms throttle drops fast clicks landing inside the
  // window, which surfaces as "selection broken on huge graphs."
  // Cap at one frame for `large`, two for `xlarge`, four for `huge`
  // so the worst case is still well below the ~80 ms human click
  // duration. The hover-cost story we used to justify higher values
  // is now mitigated by accessor-wrapper stability + the shadow-paint
  // pin (see useAccessorOverrides.wrappedNodePointerAreaPaint).
  out.pointerRaycasterThrottleMs =
    tier === "large" ? 16 : tier === "xlarge" ? 32 : 64;

  return out;
}
