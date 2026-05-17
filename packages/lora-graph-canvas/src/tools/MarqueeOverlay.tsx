export interface MarqueeRect {
  x0: number;
  y0: number;
  x1: number;
  y1: number;
}

export interface MarqueeOverlayProps {
  rect: MarqueeRect | null;
  /** Live count of nodes whose projected position falls inside the
   *  rectangle. Hidden when 0 or undefined — small rectangles in dead
   *  space shouldn't render an empty "0 nodes" pill. */
  count?: number;
}

/** Renders the dashed selection rectangle while the user is dragging a
 *  marquee. Positioned absolutely inside the host. Hidden when `rect`
 *  is null. Shows a live node-count badge in the bottom-right of the
 *  rectangle so the user can tell how many nodes they'd grab before
 *  releasing. */
export function MarqueeOverlay({ rect, count }: MarqueeOverlayProps) {
  if (!rect) return null;
  const left = Math.min(rect.x0, rect.x1);
  const top = Math.min(rect.y0, rect.y1);
  const width = Math.abs(rect.x1 - rect.x0);
  const height = Math.abs(rect.y1 - rect.y0);
  return (
    <div
      className="lgc-marquee"
      style={{ left, top, width, height, pointerEvents: "none" }}
    >
      {count && count > 0 ? (
        <span className="lgc-marquee-count">{count}</span>
      ) : null}
    </div>
  );
}
