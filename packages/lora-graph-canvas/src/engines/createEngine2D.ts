import ForceGraph2DKapsule from "force-graph";
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

/** Loose alias for the chainable kapsule instance. The upstream `.d.ts`
 *  is a heavily generic class — we route through `unknown` rather than
 *  reproduce the full chain locally. */
type Kapsule2D = Record<string, (...args: unknown[]) => unknown> & {
  graphData: (data?: unknown) => unknown;
  _destructor: () => void;
};

export function createEngine2D<
  N extends NodeObject,
  L extends LinkObject,
>(
  mount: HTMLElement,
  opts: CreateEngineOptions<N, L>,
  /** Stable trampoline that always reads the latest event handlers from
   *  the React layer; lets us bind once and never re-bind. */
  handlerRef: { current: LoraGraphCanvasProps<N, L> },
): GraphEngine<N, L> {
  const instance = new (ForceGraph2DKapsule as unknown as new (
    el: HTMLElement,
  ) => Kapsule2D)(mount) as Kapsule2D;

  // Container sizing.
  instance.width!(opts.width);
  instance.height!(opts.height);

  // Wire event handlers through the trampoline.
  for (const name of EVENT_BINDINGS) {
    const setter = instance[name as keyof Kapsule2D];
    if (typeof setter !== "function") continue;
    setter.call(
      instance,
      // Forward through handlerRef so latest props always win.
      (...args: unknown[]) => {
        const fn = handlerRef.current[name as EventName] as
          | ((...a: unknown[]) => void)
          | undefined;
        if (typeof fn === "function") fn(...args);
      },
    );
  }

  // Initial prop pass (diffed against an empty bag so every supported
  // prop gets applied once).
  applyDiffedProps(
    instance as unknown as Record<string, (value: unknown) => unknown>,
    opts.initialProps as unknown as LoraGraphCanvasProps<NodeObject, LinkObject>,
    {} as LoraGraphCanvasProps<NodeObject, LinkObject>,
    "2d",
  );

  // Seed data.
  instance.graphData(opts.initialData);

  return {
    mode: "2d",

    setGraphData(data: GraphData<N, L>) {
      instance.graphData(data);
    },
    getGraphData() {
      return instance.graphData() as GraphData<N, L>;
    },

    fit(durationMs?: number, padding?: number) {
      instance.zoomToFit?.(durationMs ?? 400, padding ?? 40);
    },
    centerAt(x: number, y: number, _z?: number, durationMs?: number) {
      instance.centerAt?.(x, y, durationMs ?? 0);
    },
    zoom(scale: number, durationMs?: number) {
      instance.zoom?.(scale, durationMs ?? 0);
    },
    getZoom() {
      return (instance.zoom?.() as number) ?? 1;
    },
    screen2Graph(x: number, y: number) {
      const out = instance.screen2GraphCoords?.(x, y) as
        | { x: number; y: number }
        | undefined;
      return out ?? { x, y };
    },
    graph2Screen(x: number, y: number) {
      const out = instance.graph2ScreenCoords?.(x, y) as
        | { x: number; y: number }
        | undefined;
      return out ?? { x, y };
    },
    getGraphBbox() {
      const bbox = instance.getGraphBbox?.() as
        | { x: [number, number]; y: [number, number] }
        | undefined;
      return bbox ?? { x: [0, 0], y: [0, 0] };
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
        "2d",
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

export type Engine2D = ReturnType<typeof createEngine2D>;
