export interface MarqueeRect {
  x0: number;
  y0: number;
  x1: number;
  y1: number;
}

export interface MarqueeOverlayProps {
  rect: MarqueeRect | null;
}

/** Renders the dashed selection rectangle while the user is dragging a
 *  marquee. Positioned absolutely inside the host. Hidden when `rect`
 *  is null. */
export function MarqueeOverlay({ rect }: MarqueeOverlayProps) {
  if (!rect) return null;
  const left = Math.min(rect.x0, rect.x1);
  const top = Math.min(rect.y0, rect.y1);
  const width = Math.abs(rect.x1 - rect.x0);
  const height = Math.abs(rect.y1 - rect.y0);
  return (
    <div
      className="lgc-marquee"
      style={{ left, top, width, height, pointerEvents: "none" }}
    />
  );
}
