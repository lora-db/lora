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

  if (mode === "3d") {
    // ngraph is ~3× faster than d3 once you cross a few thousand
    // nodes — switch to it whenever the user hasn't picked an engine.
    out.forceEngine = "ngraph";
    // Lower-poly node spheres and link cylinders. The kapsule's
    // defaults are 8 / 6 — overkill at this density.
    out.nodeResolution = tier === "large" ? 8 : tier === "xlarge" ? 6 : 4;
    out.linkResolution = tier === "large" ? 4 : tier === "xlarge" ? 2 : 0;
    // Full-opaque materials skip the alpha blending pass.
    out.nodeOpacity = 1;
    out.linkOpacity = 1;
    // Raycast on every frame is wasted work — bump the throttle so
    // hover / click hit-tests run at most a handful of times per
    // second. Selection / hover still feel instant because the
    // raycast happens on the very next tick anyway.
    out.pointerRaycasterThrottleMs =
      tier === "large" ? 32 : tier === "xlarge" ? 75 : 150;
  } else {
    // 2D: keep the renderer paused while idle (kapsule default is
    // already true, but be explicit), and skip the expensive
    // dashed-line code path.
    out.autoPauseRedraw = true;
    out.linkLineDash = null;
  }

  return out;
}
