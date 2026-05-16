import { useImperativeHandle } from "react";
import type { GraphEngine } from "../engines/types";
import type {
  GraphMode,
  LinkObject,
  LoraGraphCanvasHandle,
  NodeObject,
} from "../types";
import type { GraphDataApi } from "./useGraphData";
import type { SelectionApi } from "./useGraphSelection";
import type { GraphClipboardApi } from "./useGraphClipboard";

export interface UseImperativeGraphHandleParams<
  N extends NodeObject,
  L extends LinkObject,
> {
  ref: React.Ref<LoraGraphCanvasHandle<N, L>>;
  dataApi: GraphDataApi<N, L>;
  selection: SelectionApi;
  engine: GraphEngine<N, L> | null;
  mode: GraphMode;
  setMode: (next: GraphMode) => void;
  setPaused: React.Dispatch<React.SetStateAction<boolean>>;
  clipboard: GraphClipboardApi<N>;
  exportJSON: () => string;
  importJSON: (json: string) => void;
  downloadJSON: (filename?: string) => void;
}

/** Hooks `useImperativeHandle` to expose the canvas's full
 *  programmatic API surface to consumers via a forwardRef. Kept as a
 *  hook so the main component file stays focused on rendering. */
export function useImperativeGraphHandle<
  N extends NodeObject,
  L extends LinkObject,
>(params: UseImperativeGraphHandleParams<N, L>): void {
  const {
    ref,
    dataApi,
    selection,
    engine,
    mode,
    setMode,
    setPaused,
    clipboard,
    exportJSON,
    importJSON,
    downloadJSON,
  } = params;

  useImperativeHandle(
    ref,
    () => ({
      getData: () => dataApi.data,
      setData: dataApi.setData,
      addNode: dataApi.addNode,
      addNodes: dataApi.addNodes,
      updateNode: dataApi.updateNode,
      removeNode: dataApi.removeNode,
      removeNodes: dataApi.removeNodes,
      addLink: dataApi.addLink,
      addLinks: dataApi.addLinks,
      removeLink: dataApi.removeLink,
      clear: dataApi.clear,

      getSelection: () => selection.selected,
      setSelection: selection.set,
      selectAll: () => selection.set(dataApi.data.nodes.map((n) => n.id)),
      clearSelection: selection.clear,

      getMode: () => mode,
      setMode,
      fit: (durationMs, padding) => engine?.fit(durationMs, padding),
      centerAt: (x, y, z, durationMs) =>
        engine?.centerAt(x, y, z, durationMs),
      zoom: (scale, durationMs) => engine?.zoom(scale, durationMs),
      zoomIn: (step = 1.2) => {
        if (engine) engine.zoom((engine.getZoom?.() ?? 1) * step, 200);
      },
      zoomOut: (step = 1.2) => {
        if (engine) engine.zoom((engine.getZoom?.() ?? 1) / step, 200);
      },

      pause: () => {
        engine?.pause();
        setPaused(true);
      },
      resume: () => {
        engine?.resume();
        setPaused(false);
      },
      reheat: () => engine?.reheat(),
      screenshot: async () => {
        const canvas = engine?.getCanvasElement();
        if (!canvas) return null;
        return new Promise<Blob | null>((resolve) =>
          canvas.toBlob((b) => resolve(b)),
        );
      },

      copy: clipboard.copy,
      cut: clipboard.cut,
      paste: (opts) => clipboard.paste(opts?.at),
      duplicate: clipboard.duplicate,
      addConnectedNode: clipboard.addConnectedNode,

      togglePin: clipboard.togglePin,

      exportJSON,
      importJSON,
      downloadJSON,

      d3Force: ((name: string, fn?: unknown | null) =>
        engine?.d3Force(name, fn)) as LoraGraphCanvasHandle<N, L>["d3Force"],
      emitParticle: (link: L) => engine?.emitParticle(link),
      stopAnimation: () => engine?.stopAnimation(),

      engine2D: () => (engine?.mode === "2d" ? engine : null),
      engine3D: () => (engine?.mode === "3d" ? engine : null),
    }),
    [
      ref,
      dataApi,
      engine,
      mode,
      setMode,
      setPaused,
      selection,
      clipboard,
      exportJSON,
      importJSON,
      downloadJSON,
    ],
  );
}
