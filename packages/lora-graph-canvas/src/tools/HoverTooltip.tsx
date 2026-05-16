import { useEffect, useRef, useState } from "react";

export interface HoverTooltipProps {
  /** Content the tooltip should display. Pass `null` to hide. */
  content: string | HTMLElement | null;
  /** Host element to anchor against (positions are relative to this). */
  hostRef: React.RefObject<HTMLElement | null>;
}

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
  // Cache the host rect for the duration of the hover so we don't
  // call `getBoundingClientRect` per mouse event.
  const rectRef = useRef<DOMRect | null>(null);

  useEffect(() => {
    if (content === null) {
      // Nothing to show → no listener, no state churn.
      setPos(null);
      rectRef.current = null;
      return;
    }
    const host = hostRef.current;
    if (!host) return;
    rectRef.current = host.getBoundingClientRect();
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
      host.removeEventListener("mousemove", onMove);
      window.removeEventListener("resize", onResize);
    };
  }, [hostRef, content]);

  if (content === null || pos === null) return null;

  // Offset 14px below/right of the cursor so it doesn't sit under the
  // mouse arrow.
  const left = pos.x + 14;
  const top = pos.y + 14;

  return (
    <div
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
