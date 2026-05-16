/** A tiny RAF-driven tween we use instead of the kapsule's built-in
 *  tween group. The kapsule's tweens live inside a private state
 *  object — there's no public API for cancelling them once started.
 *  Doing the animation ourselves lets us stop it the instant the
 *  user interacts with the canvas.
 *
 *  Each `runAnim` returns a `cancel()` function; calling it freezes
 *  the animation at the current frame (no further `step` invocations).
 *  Returns null when running in an environment without
 *  `requestAnimationFrame` (jsdom in unit tests). */
export function runAnim(
  durationMs: number,
  step: (t: number) => void,
  onDone?: () => void,
): () => void {
  if (typeof requestAnimationFrame !== "function") {
    // No raf available — apply the final frame and bail. Keeps tests
    // and SSR-ish environments deterministic.
    step(1);
    onDone?.();
    return () => {};
  }
  const start = performance.now();
  let raf: number | null = null;
  let cancelled = false;

  const tick = (now: number) => {
    if (cancelled) return;
    const elapsed = now - start;
    const t = Math.min(1, elapsed / Math.max(durationMs, 1));
    step(easeOutQuad(t));
    if (t < 1) {
      raf = requestAnimationFrame(tick);
    } else {
      raf = null;
      onDone?.();
    }
  };
  raf = requestAnimationFrame(tick);

  return () => {
    cancelled = true;
    if (raf !== null) cancelAnimationFrame(raf);
    raf = null;
  };
}

export function easeOutQuad(t: number): number {
  return 1 - (1 - t) * (1 - t);
}

export function lerp(a: number, b: number, t: number): number {
  return a + (b - a) * t;
}
