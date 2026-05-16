import ForceGraph3DKapsule from "3d-force-graph";
import type {
  GraphEngine,
  CreateEngineOptions,
  CameraState,
} from "./types";
import { runAnim, lerp } from "./rafAnim";
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

  // Handle to the currently running RAF-based camera animation, if
  // any. Held so `stopAnimation` and a fresh focus call can cancel it.
  let cancelAnim: (() => void) | null = null;

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
      // Preserve the camera's current `lookAt` — passing `undefined`
      // would make `three-render-objects` snap the lookAt to (0,0,0),
      // which yanks the orbited view back to the origin.
      const cur = instance.cameraPosition?.() as
        | {
            x: number;
            y: number;
            z: number;
            lookAt?: { x: number; y: number; z: number };
          }
        | undefined;
      instance.cameraPosition?.(position, cur?.lookAt, durationMs ?? 0);
    },
    zoom(scale: number, durationMs?: number) {
      // In 3D there is no "zoom" — we approximate it by moving the
      // camera closer to / further from its current target.
      const cur = instance.cameraPosition?.() as
        | {
            x: number;
            y: number;
            z: number;
            lookAt?: { x: number; y: number; z: number };
          }
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
        cur.lookAt,
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
    d3Force(name: string, fn?: unknown | null) {
      const setter = instance.d3Force;
      if (typeof setter !== "function") return undefined;
      if (fn === undefined) return setter.call(instance, name);
      return setter.call(instance, name, fn);
    },
    emitParticle(link: L) {
      instance.emitParticle?.(link as unknown);
    },
    stopAnimation() {
      // Cancel our own RAF-based animation. The kapsule's internal
      // tween group is unreachable from outside, so any animation we
      // want to be interruptible must be driven here, never via the
      // kapsule's own `transitionDuration` argument.
      cancelAnim?.();
      cancelAnim = null;
    },

    focusOn(target, opts) {
      cancelAnim?.();
      const cur = instance.cameraPosition?.() as
        | {
            x: number;
            y: number;
            z: number;
            lookAt?: { x: number; y: number; z: number };
          }
        | undefined;
      if (!cur) return;
      const tz = target.z ?? 0;
      // Preserve the current viewing angle — place the camera along
      // its existing vector from the target, scaled to `distance`.
      const dx = cur.x - target.x;
      const dy = cur.y - target.y;
      const dz = cur.z - tz;
      const mag = Math.hypot(dx, dy, dz);
      const distance = opts?.distance ?? 120;
      const u =
        mag === 0
          ? { x: 0, y: 0, z: 1 }
          : { x: dx / mag, y: dy / mag, z: dz / mag };
      const endPos = {
        x: target.x + u.x * distance,
        y: target.y + u.y * distance,
        z: tz + u.z * distance,
      };
      const endLookAt = { x: target.x, y: target.y, z: tz };
      const startLookAt = cur.lookAt ?? { x: 0, y: 0, z: 0 };
      const dur = opts?.durationMs ?? 1000;
      cancelAnim = runAnim(dur, (t) => {
        instance.cameraPosition?.(
          {
            x: lerp(cur.x, endPos.x, t),
            y: lerp(cur.y, endPos.y, t),
            z: lerp(cur.z, endPos.z, t),
          },
          {
            x: lerp(startLookAt.x, endLookAt.x, t),
            y: lerp(startLookAt.y, endLookAt.y, t),
            z: lerp(startLookAt.z, endLookAt.z, t),
          },
          0,
        );
      });
    },

    getCameraState(): CameraState {
      const cur = (instance.cameraPosition?.() as
        | {
            x: number;
            y: number;
            z: number;
            lookAt?: { x: number; y: number; z: number };
          }
        | undefined) ?? { x: 0, y: 0, z: 300 };
      return {
        mode: "3d",
        x: cur.x,
        y: cur.y,
        z: cur.z,
        lookAt: cur.lookAt ?? { x: 0, y: 0, z: 0 },
      };
    },

    setCameraState(state, durationMs) {
      if (state.mode !== "3d") return;
      cancelAnim?.();
      const dur = durationMs ?? 0;
      if (dur <= 0) {
        instance.cameraPosition?.(
          { x: state.x, y: state.y, z: state.z },
          state.lookAt,
          0,
        );
        return;
      }
      const cur = (instance.cameraPosition?.() as
        | {
            x: number;
            y: number;
            z: number;
            lookAt?: { x: number; y: number; z: number };
          }
        | undefined) ?? { x: state.x, y: state.y, z: state.z };
      const startLookAt = cur.lookAt ?? state.lookAt;
      cancelAnim = runAnim(dur, (t) => {
        instance.cameraPosition?.(
          {
            x: lerp(cur.x, state.x, t),
            y: lerp(cur.y, state.y, t),
            z: lerp(cur.z, state.z, t),
          },
          {
            x: lerp(startLookAt.x, state.lookAt.x, t),
            y: lerp(startLookAt.y, state.lookAt.y, t),
            z: lerp(startLookAt.z, state.lookAt.z, t),
          },
          0,
        );
      });
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
      cancelAnim?.();
      cancelAnim = null;
      instance._destructor();
    },
  };
}

export type Engine3D = ReturnType<typeof createEngine3D>;
