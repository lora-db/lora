import { useEffect, useRef, useState } from "react";
import type { GraphEngine } from "../engines/types";
import {
  createEngineUnified,
  type UnifiedEngine,
} from "../engines/createEngineUnified";
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
  /** Duration (ms) of the camera + node-z tween when switching modes.
   *  0 disables the animation. */
  modeTransitionMs?: number;
}

/** Owns the kapsule engine lifecycle. The engine is created once on
 *  mount and survives mode changes — `mode` flips flow through
 *  `engine.setMode()` so the nodes smoothly move between 2D (z pinned
 *  to 0, top-down camera) and 3D (z released, orbit camera). The
 *  engine is destroyed only when the host element changes or the
 *  component unmounts. */
export function useGraphEngine<
  N extends NodeObject,
  L extends LinkObject,
>(params: UseGraphEngineParams<N, L>): GraphEngine<N, L> | null {
  const {
    mount,
    mode,
    width,
    height,
    data,
    props,
    paused,
    modeTransitionMs = 800,
  } = params;

  const [engine, setEngine] = useState<UnifiedEngine<N, L> | null>(null);

  // Trampoline ref — adapters read latest event handlers from here.
  const handlerRef = useRef<LoraGraphCanvasProps<N, L>>(props);
  handlerRef.current = props;

  // Track the previous prop bag for diffing.
  const prevPropsRef = useRef<LoraGraphCanvasProps<N, L>>(props);

  // Hold onto the initial mode so the mount effect can pass it once
  // without re-mounting whenever React state's `mode` changes
  // (mode flips are handled by setMode in a sibling effect below).
  const initialModeRef = useRef<GraphMode>(mode);

  // Mount / unmount on host element changes only — mode flips
  // intentionally do not remount.
  useEffect(() => {
    if (!mount) return;
    const next = createEngineUnified<N, L>(
      mount,
      {
        initialProps: props,
        initialData: data,
        width,
        height,
        initialMode: initialModeRef.current,
      },
      handlerRef,
    );
    prevPropsRef.current = props;
    setEngine(next);
    return () => {
      next.destroy();
      setEngine(null);
    };
    // Width / height / data / props / paused / mode are forwarded by
    // separate effects so a change in any of them doesn't tear down
    // the engine.
  }, [mount]);

  // Mode transition: when the React `mode` flips and we have a live
  // engine, ask it to tween. First mount is a no-op (the engine
  // already started in the right mode via initialModeRef).
  useEffect(() => {
    if (!engine) return;
    if (engine.mode === mode) return;
    engine.setMode(mode, modeTransitionMs);
  }, [engine, mode, modeTransitionMs]);

  // Apply pause state. Deps include `paused` so a toggle from React
  // state flows to the engine, and so a fresh engine inherits the
  // current value. The host should drive pause via this prop only —
  // not by calling engine.pause() imperatively — to keep a single
  // source of truth.
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

  // Forward (diffed) prop changes when the engineProps identity moves.
  // The host memoises engineProps in LoraGraphCanvas — limiting this
  // effect's deps to `[engine, props]` lets React itself bail out when
  // nothing in the prop bag changed, so we don't pay the diff walk on
  // every parent render. The inner `applyDiffedProps` short-circuits
  // when `props === prev` anyway, but reaching it required running
  // the effect first; now we don't.
  useEffect(() => {
    if (!engine) return;
    engine.applyProps(props, prevPropsRef.current);
    prevPropsRef.current = props;
  }, [engine, props]);

  return engine;
}
