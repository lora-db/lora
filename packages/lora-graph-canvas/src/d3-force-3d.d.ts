// Ambient type stub for `d3-force-3d`. Upstream ships JS only — we
// just need the one function we use, narrowly typed. (Everything else
// stays untyped, which is fine: the engine adapter passes the force
// straight through to the kapsule.)
declare module "d3-force-3d" {
  type Force = {
    strength(strength: number | ((node: unknown) => number)): Force;
    [key: string]: unknown;
  };
  export function forceCollide(
    radius?: number | ((node: unknown) => number),
  ): Force;
  export function forceX(
    x?: number | ((node: unknown) => number),
  ): Force;
  export function forceY(
    y?: number | ((node: unknown) => number),
  ): Force;
}
