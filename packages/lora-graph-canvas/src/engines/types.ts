import type {
  GraphData,
  GraphMode,
  LinkObject,
  NodeObject,
  LoraGraphCanvasProps,
} from "../types";

/** Camera state snapshot. The shape differs per mode so we can
 *  faithfully restore in both 2D (pan + zoom) and 3D (position +
 *  lookAt). */
export type CameraState =
  | { mode: "2d"; x: number; y: number; k: number }
  | {
      mode: "3d";
      x: number;
      y: number;
      z: number;
      lookAt: { x: number; y: number; z: number };
    };

/** Subset of the kapsule API both 2D and 3D engines implement. Each
 *  adapter normalises naming so the React layer never has to special-case
 *  the dimension. */
export interface GraphEngine<
  N extends NodeObject = NodeObject,
  L extends LinkObject = LinkObject,
> {
  mode: GraphMode;

  // Data
  setGraphData(data: GraphData<N, L>): void;
  getGraphData(): GraphData<N, L>;

  // View
  fit(durationMs?: number, padding?: number): void;
  centerAt(x: number, y: number, z?: number, durationMs?: number): void;
  zoom(scale: number, durationMs?: number): void;
  getZoom(): number;
  screen2Graph(
    x: number,
    y: number,
    distance?: number,
  ): { x: number; y: number; z?: number };
  graph2Screen(
    x: number,
    y: number,
    z?: number,
  ): { x: number; y: number; z?: number };
  getGraphBbox(): {
    x: [number, number];
    y: [number, number];
    z?: [number, number];
  };

  // Engine
  pause(): void;
  resume(): void;
  reheat(): void;
  resize(width: number, height: number): void;
  /** Get / set / clear a d3-force by name. Pass `null` to remove. */
  d3Force(name: string, fn?: unknown | null): unknown;
  /** Emit a one-off animated particle along the given link. */
  emitParticle(link: L): void;
  /** Snap any in-flight camera tween (centerAt / zoom / cameraPosition)
   *  to its current state. Used to interrupt a focus animation when
   *  the user starts a new interaction. No-op when nothing is
   *  animating. */
  stopAnimation(): void;

  /** Animate the camera so the given target point is centered, while
   *  preserving the current viewing angle. In 3D the camera flies
   *  along its current vector from the target so the user's orbit
   *  is kept; in 2D it's a `centerAt` + `zoom`. */
  focusOn(
    target: { x: number; y: number; z?: number },
    opts?: { distance?: number; zoom?: number; durationMs?: number },
  ): void;

  /** Translate the view by world-space delta. Moves camera AND
   *  lookAt by the same vector so the orbit / view direction is
   *  preserved (a true "pan" rather than an orbit step). In 2D the
   *  z component is ignored — the top-down camera is locked to a
   *  constant height. */
  panBy(
    delta: { x?: number; y?: number; z?: number },
    durationMs?: number,
  ): void;

  /** Jump the view to a world coordinate, preserving the current
   *  viewing direction. Differs from `focusOn` in that it accepts a
   *  raw coordinate rather than a node-style target and doesn't
   *  re-tighten the zoom. Useful for "go to coordinates" UI. */
  goTo(
    target: { x: number; y: number; z?: number },
    opts?: { durationMs?: number },
  ): void;

  /** Fit the camera to a subset of nodes. Same camera math as `fit()`
   *  but the bbox is computed over `nodeIds` instead of the whole
   *  graph. Falls back to a full fit when `nodeIds` is empty. */
  fitToNodes(
    nodeIds: ReadonlyArray<string | number>,
    durationMs?: number,
    padding?: number,
  ): void;

  /** Snapshot the current camera so it can be restored later. */
  getCameraState(): CameraState;
  /** Restore a snapshot produced by `getCameraState`. */
  setCameraState(state: CameraState, durationMs?: number): void;

  // Prop pipe: receive the full prop bag every render; the adapter is
  // responsible for diffing what it cares about and calling the
  // underlying kapsule setter only when something changed.
  applyProps(
    props: LoraGraphCanvasProps<N, L>,
    prev: LoraGraphCanvasProps<N, L>,
  ): void;

  // Renderer escape hatch — returns the canvas / WebGL DOM element so
  // the `screenshot` ref method can call `toBlob`.
  getCanvasElement(): HTMLCanvasElement | null;

  destroy(): void;
}

export interface CreateEngineOptions<
  N extends NodeObject = NodeObject,
  L extends LinkObject = LinkObject,
> {
  initialProps: LoraGraphCanvasProps<N, L>;
  initialData: GraphData<N, L>;
  width: number;
  height: number;
}
