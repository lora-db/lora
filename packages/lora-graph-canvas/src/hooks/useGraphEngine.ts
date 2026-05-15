import { useEffect, useRef, useState } from "react";
import type { GraphEngine } from "../engines/types";
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
}

/** Owns the kapsule engine lifecycle. Tears down and re-creates the
 *  engine when `mode` changes (the two kapsules are not interchangeable),
 *  and keeps a stable handler-ref so latest props always win without
 *  re-binding event callbacks. */
export function useGraphEngine<
  N extends NodeObject,
  L extends LinkObject,
>(params: UseGraphEngineParams<N, L>): GraphEngine<N, L> | null {
  const { mount, mode, width, height, data, props } = params;

  const [engine, setEngine] = useState<GraphEngine<N, L> | null>(null);

  // Trampoline ref — adapters read latest event handlers from here.
  const handlerRef = useRef<LoraGraphCanvasProps<N, L>>(props);
  handlerRef.current = props;

  // Track the previous prop bag for diffing.
  const prevPropsRef = useRef<LoraGraphCanvasProps<N, L>>(props);

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
      next.destroy();
      setEngine(null);
    };
    // Intentionally remount only when mount or mode changes — the other
    // values (width/height/data/props) are forwarded by separate effects
    // below, so listing them here would force needless engine rebuilds.
  }, [mount, mode]);

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
