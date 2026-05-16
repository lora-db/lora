import { useEffect } from "react";

/** Default press-jitter grace. For this many milliseconds after
 *  `pointerdown` the shim swallows *every* move event regardless of
 *  distance — that way the natural micro-motion caused by physically
 *  pressing a button or trackpad never has a chance to flip the
 *  kapsule's drag flag. Tuned to stay well under the human "this feels
 *  laggy" threshold for drag starts (~100 ms). */
const DEFAULT_PRESS_GRACE_MS = 60;

export interface ClickToleranceShimOptions {
  /** Distance in CSS pixels the cursor may travel from the press
   *  position before the gesture is committed as a drag. */
  tolerancePx?: number;
  /** Milliseconds after `pointerdown` during which every move is
   *  suppressed regardless of distance, so press-induced jitter never
   *  promotes a click into a drag. */
  pressGraceMs?: number;
}

/** Click-vs-drag tolerance shim.
 *
 *  The upstream kapsule (force-graph + 3d-force-graph) flips
 *  `isPointerDragging` to true on the *first* pointermove after
 *  pointerdown for any mouse event — even a 1px jitter — which then
 *  suppresses the click / right-click handler on pointerup. That
 *  makes background-click deselect, node selection, and right-click
 *  context menu all feel fragile.
 *
 *  We add a movement dead-zone by intercepting pointer **and** mouse
 *  move events at the window level in the capture phase. Two
 *  independent thresholds combine to keep clicks from accidentally
 *  becoming drags:
 *
 *    1. **Press-grace window** — for the first `pressGraceMs` after
 *       `pointerdown`, every move is swallowed regardless of distance.
 *       This covers the involuntary jitter spike that fires right
 *       after the button physically engages, which on a HiDPI trackpad
 *       can easily exceed 6–8px in a single event.
 *    2. **Distance dead-zone** — past the grace window, moves within
 *       `tolerancePx` of the press position are still swallowed. Once
 *       the cursor crosses that radius the gesture commits to "drag"
 *       and we stop intercepting, so pan / orbit / drag-node remain
 *       snappy for the rest of the gesture.
 *
 *  Suppressing moves stops them propagating to:
 *    - the kapsule's container-level `pointermove` listener (so its
 *      `isPointerDragging` flag stays false and the click handler
 *      still fires on `pointerup`);
 *    - d3-zoom's `mousemove.zoom` listener on `window` (so a slight
 *      pan doesn't shift a node under the cursor between pointerdown
 *      and pointerup — that was the bug where clicking empty space to
 *      deselect would silently toggle whichever node slid under the
 *      cursor mid-gesture);
 *    - Three.js OrbitControls in 3D (same idea — no camera nudge
 *      during a click). */
export function useClickToleranceShim(
  mount: HTMLElement | null,
  options: number | ClickToleranceShimOptions = {},
): void {
  const opts: ClickToleranceShimOptions =
    typeof options === "number" ? { tolerancePx: options } : options;
  const tolerancePx = opts.tolerancePx ?? 12;
  const pressGraceMs = opts.pressGraceMs ?? DEFAULT_PRESS_GRACE_MS;

  useEffect(() => {
    if (!mount) return;
    const tolSq = tolerancePx * tolerancePx;
    let active = false;
    let exceeded = false;
    let startX = 0;
    let startY = 0;
    let pressedAt = 0;

    const onPointerDown = (e: PointerEvent) => {
      if (!mount.contains(e.target as Node)) return;
      active = true;
      exceeded = false;
      startX = e.clientX;
      startY = e.clientY;
      pressedAt = performance.now();
    };
    // Shared filter — same body for pointermove and mousemove; we
    // need to suppress both because pointer events drive the kapsule
    // while d3-zoom + OrbitControls listen to mouse events directly.
    const onMove = (e: MouseEvent) => {
      if (!active || exceeded) return;
      // Press-grace: swallow every move during the initial jitter
      // window, so a quick click can't be hijacked by the spike of
      // motion that fires the instant the button engages.
      if (performance.now() - pressedAt < pressGraceMs) {
        e.stopPropagation();
        return;
      }
      const dx = e.clientX - startX;
      const dy = e.clientY - startY;
      if (dx * dx + dy * dy < tolSq) {
        e.stopPropagation();
      } else {
        exceeded = true;
      }
    };
    const release = () => {
      active = false;
      exceeded = false;
    };

    window.addEventListener("pointerdown", onPointerDown, true);
    window.addEventListener("pointermove", onMove, true);
    window.addEventListener("mousemove", onMove, true);
    window.addEventListener("pointerup", release, true);
    window.addEventListener("pointercancel", release, true);
    window.addEventListener("mouseup", release, true);
    return () => {
      window.removeEventListener("pointerdown", onPointerDown, true);
      window.removeEventListener("pointermove", onMove, true);
      window.removeEventListener("mousemove", onMove, true);
      window.removeEventListener("pointerup", release, true);
      window.removeEventListener("pointercancel", release, true);
      window.removeEventListener("mouseup", release, true);
    };
  }, [mount, tolerancePx, pressGraceMs]);
}
