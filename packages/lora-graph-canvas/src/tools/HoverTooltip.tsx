import { useEffect, useState } from "react";

export interface HoverTooltipProps {
  /** Content the tooltip should display. Pass `null` to hide. */
  content: string | HTMLElement | null;
  /** Host element to anchor against (positions are relative to this). */
  hostRef: React.RefObject<HTMLElement | null>;
}

/** Floating tooltip that tracks the mouse position inside the host
 *  element. Hidden when `content` is null. Themed via --lgc-tooltip-*. */
export function HoverTooltip({ content, hostRef }: HoverTooltipProps) {
  const [pos, setPos] = useState<{ x: number; y: number } | null>(null);

  useEffect(() => {
    const host = hostRef.current;
    if (!host) return;
    const onMove = (e: MouseEvent) => {
      const rect = host.getBoundingClientRect();
      setPos({ x: e.clientX - rect.left, y: e.clientY - rect.top });
    };
    host.addEventListener("mousemove", onMove);
    return () => host.removeEventListener("mousemove", onMove);
  }, [hostRef]);

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
