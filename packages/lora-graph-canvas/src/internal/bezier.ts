// Minimal quadratic / cubic Bezier helpers. Covers only the surface
// our paint pipeline uses (`get(t)` for point evaluation, `length()`
// for arc-length approximation along curved links). Two-control-point
// curves are quadratic; four-control-point curves are cubic.
//
// LORA: replaces the `bezier-js` dependency (MIT, © Pomax). The
// upstream library is ~2k LOC and supports the full Bezier algebra
// (intersections, derivatives, offset curves, etc.) — we don't use
// any of that. This 50-LOC version is enough for our paint paths.

export class Bezier {
  /** Control-point coordinates, ordered as
   *  [x0, y0, ...c1?, c2?, ..., xn, yn]. */
  readonly #pts: readonly number[];

  constructor(...pts: number[]) {
    if (pts.length !== 6 && pts.length !== 8) {
      throw new Error(
        `Bezier expects 6 (quadratic) or 8 (cubic) coordinates, got ${pts.length}`,
      );
    }
    this.#pts = pts;
  }

  /** Point on the curve at parameter `t` ∈ [0, 1]. Implementation
   *  uses the explicit cubic / quadratic forms (faster than
   *  de Casteljau for two/three subdivisions). */
  get(t: number): { x: number; y: number } {
    const p = this.#pts;
    if (p.length === 6) {
      const u = 1 - t;
      const x = u * u * p[0]! + 2 * u * t * p[2]! + t * t * p[4]!;
      const y = u * u * p[1]! + 2 * u * t * p[3]! + t * t * p[5]!;
      return { x, y };
    }
    // Cubic
    const u = 1 - t;
    const uu = u * u;
    const tt = t * t;
    const uuu = uu * u;
    const ttt = tt * t;
    const x =
      uuu * p[0]! + 3 * uu * t * p[2]! + 3 * u * tt * p[4]! + ttt * p[6]!;
    const y =
      uuu * p[1]! + 3 * uu * t * p[3]! + 3 * u * tt * p[5]! + ttt * p[7]!;
    return { x, y };
  }

  /** Arc length via Gauss-Legendre quadrature would be cleaner, but
   *  this consumer only uses the length to position arrowheads and
   *  photons — a 40-step polyline approximation is plenty accurate.
   *  Allocation-free in the steady state (one number per step). */
  length(): number {
    const STEPS = 40;
    let total = 0;
    let prev = this.get(0);
    for (let i = 1; i <= STEPS; i++) {
      const cur = this.get(i / STEPS);
      const dx = cur.x - prev.x;
      const dy = cur.y - prev.y;
      total += Math.sqrt(dx * dx + dy * dy);
      prev = cur;
    }
    return total;
  }
}
