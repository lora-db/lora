export interface Point2 {
  x: number;
  y: number;
}

export function distance2(a: Point2, b: Point2): number {
  return Math.hypot(a.x - b.x, a.y - b.y);
}

/** Default snap distances for "drag node onto node → link" gesture. */
export const SNAP_IN = 15;
export const SNAP_OUT = 40;
