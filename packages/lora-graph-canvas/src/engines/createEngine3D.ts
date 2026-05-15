import ForceGraph3DKapsule from "3d-force-graph";
import type { GraphEngine, CreateEngineOptions } from "./types";
import type {
  GraphData,
  LinkObject,
  LoraGraphCanvasProps,
  NodeObject,
} from "../types";
import {
  EVENT_BINDINGS,
  applyDiffedProps,
  type EventName,
} from "./propBindings";

type Kapsule3D = Record<string, (...args: unknown[]) => unknown> & {
  graphData: (data?: unknown) => unknown;
  _destructor: () => void;
};

export function createEngine3D<
  N extends NodeObject,
  L extends LinkObject,
>(
  mount: HTMLElement,
  opts: CreateEngineOptions<N, L>,
  handlerRef: { current: LoraGraphCanvasProps<N, L> },
): GraphEngine<N, L> {
  const instance = new (ForceGraph3DKapsule as unknown as new (
    el: HTMLElement,
  ) => Kapsule3D)(mount) as Kapsule3D;

  instance.width!(opts.width);
  instance.height!(opts.height);

  for (const name of EVENT_BINDINGS) {
    const setter = instance[name as keyof Kapsule3D];
    if (typeof setter !== "function") continue;
    setter.call(instance, (...args: unknown[]) => {
      const fn = handlerRef.current[name as EventName] as
        | ((...a: unknown[]) => void)
        | undefined;
      if (typeof fn === "function") fn(...args);
    });
  }

  applyDiffedProps(
    instance as unknown as Record<string, (value: unknown) => unknown>,
    opts.initialProps as unknown as LoraGraphCanvasProps<NodeObject, LinkObject>,
    {} as LoraGraphCanvasProps<NodeObject, LinkObject>,
    "3d",
  );

  instance.graphData(opts.initialData);

  let cachedDistance = 300;

  return {
    mode: "3d",

    setGraphData(data: GraphData<N, L>) {
      instance.graphData(data);
    },
    getGraphData() {
      return instance.graphData() as GraphData<N, L>;
    },

    fit(durationMs?: number, padding?: number) {
      instance.zoomToFit?.(durationMs ?? 400, padding ?? 40);
    },
    centerAt(x: number, y: number, z?: number, durationMs?: number) {
      const position = { x, y, z: z ?? cachedDistance };
      instance.cameraPosition?.(position, undefined, durationMs ?? 0);
    },
    zoom(scale: number, durationMs?: number) {
      // In 3D there is no "zoom" — we approximate it by moving the
      // camera closer to / further from its current target.
      const cur = instance.cameraPosition?.() as
        | { x: number; y: number; z: number }
        | undefined;
      if (!cur) return;
      const distance = Math.hypot(cur.x, cur.y, cur.z);
      cachedDistance = distance / Math.max(scale, 0.001);
      const unit = distance === 0 ? { x: 0, y: 0, z: 1 } : {
        x: cur.x / distance,
        y: cur.y / distance,
        z: cur.z / distance,
      };
      instance.cameraPosition?.(
        {
          x: unit.x * cachedDistance,
          y: unit.y * cachedDistance,
          z: unit.z * cachedDistance,
        },
        undefined,
        durationMs ?? 0,
      );
    },
    getZoom() {
      const cur = instance.cameraPosition?.() as
        | { x: number; y: number; z: number }
        | undefined;
      if (!cur) return 1;
      const d = Math.hypot(cur.x, cur.y, cur.z);
      return d === 0 ? 1 : cachedDistance / d;
    },
    screen2Graph(x: number, y: number, distance?: number) {
      const out = instance.screen2GraphCoords?.(x, y, distance ?? 0) as
        | { x: number; y: number; z: number }
        | undefined;
      return out ?? { x, y, z: 0 };
    },
    graph2Screen(x: number, y: number, z?: number) {
      const out = instance.graph2ScreenCoords?.(x, y, z ?? 0) as
        | { x: number; y: number; z: number }
        | undefined;
      return out ?? { x, y, z: z ?? 0 };
    },
    getGraphBbox() {
      const bbox = instance.getGraphBbox?.() as
        | {
            x: [number, number];
            y: [number, number];
            z: [number, number];
          }
        | undefined;
      return bbox ?? { x: [0, 0], y: [0, 0], z: [0, 0] };
    },

    pause() {
      instance.pauseAnimation?.();
    },
    resume() {
      instance.resumeAnimation?.();
    },
    reheat() {
      instance.d3ReheatSimulation?.();
    },
    resize(width: number, height: number) {
      instance.width!(width);
      instance.height!(height);
    },

    applyProps(props, prev) {
      applyDiffedProps(
        instance as unknown as Record<string, (value: unknown) => unknown>,
        props as unknown as LoraGraphCanvasProps<NodeObject, LinkObject>,
        prev as unknown as LoraGraphCanvasProps<NodeObject, LinkObject>,
        "3d",
      );
    },

    getCanvasElement() {
      return mount.querySelector("canvas") as HTMLCanvasElement | null;
    },

    destroy() {
      instance._destructor();
    },
  };
}

export type Engine3D = ReturnType<typeof createEngine3D>;
