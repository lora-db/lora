// Ambient type stub for `d3-force-3d`. Upstream ships JS only — we
// declare the surface our code uses. The shapes are loose on purpose:
// d3-force-3d's chainable API is too dynamic for strict typing without
// duplicating the whole library's source-of-truth, and the engine
// adapter passes most of these straight through to the kapsule.
declare module "d3-force-3d" {
  type Force = {
    strength(strength: number | ((node: unknown) => number)): Force;
    [key: string]: unknown;
  };
  export function forceCollide(
    radius?: number | ((node: unknown) => number),
  ): Force;
  export function forceX(x?: number | ((node: unknown) => number)): Force;
  export function forceY(y?: number | ((node: unknown) => number)): Force;

  // Used by our in-tree force-graph-2d port (canvas.ts).
  type Simulation = {
    force(name: string): unknown;
    force(name: string, force: unknown | null): Simulation;
    alpha(): number;
    alpha(a: number): Simulation;
    alphaMin(): number;
    alphaMin(a: number): Simulation;
    alphaDecay(decay: number): Simulation;
    alphaTarget(target: number): Simulation;
    velocityDecay(decay: number): Simulation;
    nodes(nodes: unknown[]): Simulation;
    tick(): Simulation;
    stop(): Simulation;
    [key: string]: unknown;
  };
  export function forceSimulation(nodes?: unknown[]): Simulation;
  export function forceLink(links?: unknown[]): Force;
  export function forceManyBody(): Force;
  export function forceCenter(x?: number, y?: number, z?: number): Force;
  export function forceRadial<N = unknown>(
    radius: number | ((node: N) => number),
    x?: number,
    y?: number,
    z?: number,
  ): Force;
}
