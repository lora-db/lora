import { useEffect, useRef, useState } from "react";
import type { CameraState, GraphEngine } from "../engines/types";
import { createEngine2D } from "../engines/createEngine2D";
import { createEngine3D } from "../engines/createEngine3D";
import type {
  GraphData,
  GraphMode,
  LinkObject,
  LoraGraphCanvasProps,
  NodeObject,
} from "../types";

interface UseGraphEngineParams<
  N extends NodeObject,
  L extends LinkObject,
> {
  mount: HTMLElement | null;
  mode: GraphMode;
  width: number;
  height: number;
  data: GraphData<N, L>;
  props: LoraGraphCanvasProps<N, L>;
  /** Whether the engine should be paused. */
  paused: boolean;
}

/** Owns the kapsule engine lifecycle. Tears down and re-creates the
 *  engine when `mode` changes (the two kapsules are not interchangeable),
 *  and keeps a stable handler-ref so latest props always win without
 *  re-binding event callbacks.
 *
 *  Also owns the state that must survive a 2D ↔ 3D remount: the camera
 *  viewpoint (snapshotted in the mount-effect cleanup, before the
 *  outgoing kapsule is destroyed, and re-applied to each new engine)
 *  and the paused flag (re-applied whenever `engine` or `paused`
 *  changes). Camera is keyed by mode because the kapsules' camera
 *  models aren't interchangeable. */
export function useGraphEngine<
  N extends NodeObject,
  L extends LinkObject,
>(params: UseGraphEngineParams<N, L>): GraphEngine<N, L> | null {
  const { mount, mode, width, height, data, props, paused } = params;

  const [engine, setEngine] = useState<GraphEngine<N, L> | null>(null);

  // Trampoline ref — adapters read latest event handlers from here.
  const handlerRef = useRef<LoraGraphCanvasProps<N, L>>(props);
  handlerRef.current = props;

  // Track the previous prop bag for diffing.
  const prevPropsRef = useRef<LoraGraphCanvasProps<N, L>>(props);

  // Survives engine teardown — captured in the outgoing engine's
  // cleanup, restored on the next engine that mounts in that mode.
  const cameraByModeRef = useRef<Record<GraphMode, CameraState | null>>({
    "2d": null,
    "3d": null,
  });

  // Mount / re-mount on mode or mount-element changes.
  useEffect(() => {
    if (!mount) return;
    const factory = mode === "3d" ? createEngine3D : createEngine2D;
    const next = factory<N, L>(
      mount,
      {
        initialProps: props,
        initialData: data,
        width,
        height,
      },
      handlerRef,
    );
    prevPropsRef.current = props;
    setEngine(next);
    return () => {
      // Capture camera *before* destroy — the kapsule won't answer
      // afterwards. This is why camera survival has to live here and
      // not in a sibling effect: a sibling's cleanup would run after
      // this one, when the engine is already gone.
      cameraByModeRef.current[next.mode] = next.getCameraState();
      next.destroy();
      setEngine(null);
    };
    // Intentionally remount only when mount or mode changes — the other
    // values (width/height/data/props/paused) are forwarded by separate
    // effects below, so listing them here would force needless rebuilds.
  }, [mount, mode]);

  // Restore camera state on each new engine. Mode-specific by design:
  // a 2D snapshot has no meaning for a 3D kapsule (and vice versa), so
  // setCameraState silently no-ops on mismatched shapes — the guard
  // here just short-circuits when there's nothing to restore.
  useEffect(() => {
    if (!engine) return;
    const saved = cameraByModeRef.current[engine.mode];
    if (saved && saved.mode === engine.mode) {
      engine.setCameraState(saved, 0);
    }
  }, [engine]);

  // Apply pause state. Deps include `paused` so a toggle from React
  // state flows to the engine, and so a fresh engine post-remount
  // inherits the current value. The host should drive pause via this
  // prop only — not by calling engine.pause() imperatively — to keep
  // a single source of truth.
  useEffect(() => {
    if (!engine) return;
    if (paused) engine.pause();
    else engine.resume();
  }, [engine, paused]);

  // Forward width/height changes.
  useEffect(() => {
    if (!engine) return;
    engine.resize(width, height);
  }, [engine, width, height]);

  // Forward data changes.
  useEffect(() => {
    if (!engine) return;
    engine.setGraphData(data);
  }, [engine, data]);

  // Forward (diffed) prop changes every render.
  useEffect(() => {
    if (!engine) return;
    engine.applyProps(props, prevPropsRef.current);
    prevPropsRef.current = props;
  });

  return engine;
}
