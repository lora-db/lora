import type {
  GraphData,
  GraphMode,
  LinkObject,
  NodeObject,
  LoraGraphCanvasProps,
} from "../types";

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
  getGraphBbox(): { x: [number, number]; y: [number, number]; z?: [number, number] };

  // Engine
  pause(): void;
  resume(): void;
  reheat(): void;
  resize(width: number, height: number): void;

  // Prop pipe: receive the full prop bag every render; the adapter is
  // responsible for diffing what it cares about and calling the
  // underlying kapsule setter only when something changed.
  applyProps(props: LoraGraphCanvasProps<N, L>, prev: LoraGraphCanvasProps<N, L>): void;

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
