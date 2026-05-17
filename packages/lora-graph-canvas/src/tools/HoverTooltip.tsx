import { useEffect, useLayoutEffect, useRef, useState } from "react";

export interface HoverTooltipProps {
  /** Content the tooltip should display. Pass `null` to hide. */
  content: string | HTMLElement | null;
  /** Host element to anchor against (positions are relative to this). */
  hostRef: React.RefObject<HTMLElement | null>;
}

/** Delay before the tooltip appears after the host starts having content
 *  to show. Passing through a dense graph touches many nodes per second;
 *  without this the pill strobes on every hover. 120ms is short enough
 *  not to feel laggy on an intentional pause, long enough to swallow
 *  drive-by hovers. */
const APPEAR_DELAY_MS = 120;

/** Cursor offset for the tooltip. We bias the pill 14px right + down by
 *  default so it doesn't sit under the cursor arrow. Near the right or
 *  bottom edge of the host we mirror the offset to the other side so
 *  the pill doesn't clip out of the overflow:hidden container. */
const CURSOR_OFFSET = 14;

/** Floating tooltip that tracks the mouse position inside the host
 *  element. Hidden when `content` is null.
 *
 *  Perf note: the mousemove listener (and the per-move
 *  `getBoundingClientRect`) is only installed while there is content
 *  to show. Otherwise we'd be doing a forced layout + React render at
 *  60Hz whenever the user's mouse is over the canvas, even though
 *  nothing is being displayed. Themed via --lgc-tooltip-*. */
export function HoverTooltip({ content, hostRef }: HoverTooltipProps) {
  const [pos, setPos] = useState<{ x: number; y: number } | null>(null);
  // Gate the appear delay behind a separate flag so the cursor still
  // tracks during the wait — we want the pill to land at the user's
  // current position, not at the spot where they first paused.
  const [visible, setVisible] = useState(false);
  // Cache the host rect for the duration of the hover so we don't
  // call `getBoundingClientRect` per mouse event.
  const rectRef = useRef<DOMRect | null>(null);
  // Live tooltip-element ref so the edge-clamp layout effect can read
  // its measured size after content lands.
  const tooltipRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    if (content === null) {
      // Nothing to show → no listener, no state churn, no visibility.
      setPos(null);
      setVisible(false);
      rectRef.current = null;
      return;
    }
    const host = hostRef.current;
    if (!host) return;
    rectRef.current = host.getBoundingClientRect();
    const delayTimer = window.setTimeout(() => setVisible(true), APPEAR_DELAY_MS);
    const onMove = (e: MouseEvent) => {
      const rect = rectRef.current ?? host.getBoundingClientRect();
      setPos({ x: e.clientX - rect.left, y: e.clientY - rect.top });
    };
    const onResize = () => {
      rectRef.current = host.getBoundingClientRect();
    };
    host.addEventListener("mousemove", onMove);
    window.addEventListener("resize", onResize);
    return () => {
      window.clearTimeout(delayTimer);
      host.removeEventListener("mousemove", onMove);
      window.removeEventListener("resize", onResize);
    };
  }, [hostRef, content]);

  // Clamp the tooltip inside the host: if the default down/right offset
  // would push it past an edge, mirror it to the opposite side. Runs in
  // layout-effect so the first paint shows the corrected position.
  const [flipX, setFlipX] = useState(false);
  const [flipY, setFlipY] = useState(false);
  useLayoutEffect(() => {
    if (!visible || !pos) return;
    const host = hostRef.current;
    const el = tooltipRef.current;
    if (!host || !el) return;
    const hostRect = rectRef.current ?? host.getBoundingClientRect();
    const w = el.offsetWidth;
    const h = el.offsetHeight;
    setFlipX(pos.x + CURSOR_OFFSET + w > hostRect.width);
    setFlipY(pos.y + CURSOR_OFFSET + h > hostRect.height);
  }, [visible, pos, content, hostRef]);

  if (content === null || pos === null || !visible) return null;

  // Mirror the offset to the opposite side when the default would clip.
  // For the X flip, also subtract the (estimated) tooltip width via the
  // measured tooltipRef once it's been laid out. We can't know the width
  // for the first frame, but the layout-effect above sets `flipX` *before*
  // paint based on a measured size, so the user never sees the clipped
  // intermediate state on subsequent re-renders.
  const tooltipW = tooltipRef.current?.offsetWidth ?? 0;
  const tooltipH = tooltipRef.current?.offsetHeight ?? 0;
  const left = flipX ? pos.x - CURSOR_OFFSET - tooltipW : pos.x + CURSOR_OFFSET;
  const top = flipY ? pos.y - CURSOR_OFFSET - tooltipH : pos.y + CURSOR_OFFSET;

  return (
    <div
      ref={tooltipRef}
      className="lgc-tooltip"
      role="tooltip"
      style={{ left, top, pointerEvents: "none" }}
    >
      {typeof content === "string" ? (
        content
      ) : (
        <span ref={(el) => el?.appendChild(content)} />
      )}
    </div>
  );
}
