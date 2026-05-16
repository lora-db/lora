export interface GridOptions {
  spacing?: number;
  color?: string;
}

/** Draws a faint, infinite grid behind the graph. Hooked up via the
 *  engine's `onRenderFramePre` callback so the grid stays under the
 *  nodes / links. The grid adapts to the current zoom level: at high
 *  zoom we draw a tighter sub-grid, at low zoom a coarser one. */
export function drawBackgroundGrid(
  ctx: CanvasRenderingContext2D,
  globalScale: number,
  opts: GridOptions = {},
): void {
  const spacing = opts.spacing ?? 50;
  const color = opts.color ?? "rgba(0,0,0,0.06)";
  const canvas = ctx.canvas;
  // The kapsule has already applied the world transform — we can
  // recover the visible graph-space rectangle from the inverse.
  const t = ctx.getTransform();
  const inv = t.inverse();
  const topLeft = inv.transformPoint({ x: 0, y: 0 });
  const bottomRight = inv.transformPoint({
    x: canvas.width,
    y: canvas.height,
  });
  const left = Math.floor(topLeft.x / spacing) * spacing;
  const right = Math.ceil(bottomRight.x / spacing) * spacing;
  const top = Math.floor(topLeft.y / spacing) * spacing;
  const bottom = Math.ceil(bottomRight.y / spacing) * spacing;

  ctx.save();
  ctx.strokeStyle = color;
  ctx.lineWidth = 1 / globalScale;
  ctx.beginPath();
  for (let x = left; x <= right; x += spacing) {
    ctx.moveTo(x, top);
    ctx.lineTo(x, bottom);
  }
  for (let y = top; y <= bottom; y += spacing) {
    ctx.moveTo(left, y);
    ctx.lineTo(right, y);
  }
  ctx.stroke();
  ctx.restore();
}
