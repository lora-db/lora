import { useImperativeHandle, useRef } from "react";
import type { GraphEngine } from "../engines/types";
import type {
  GraphMode,
  LinkObject,
  LoraGraphCanvasHandle,
  NodeObject,
} from "../types";
import type { GraphDataApi } from "./useGraphData";
import type { GraphDeleteGateApi } from "./useGraphDeleteGate";
import type { SelectionApi } from "./useGraphSelection";
import type { GraphClipboardApi } from "./useGraphClipboard";

export interface UseImperativeGraphHandleParams<
  N extends NodeObject,
  L extends LinkObject,
> {
  ref: React.Ref<LoraGraphCanvasHandle<N, L>>;
  dataApi: GraphDataApi<N, L>;
  deleteGate: GraphDeleteGateApi<L>;
  selection: SelectionApi;
  engine: GraphEngine<N, L> | null;
  mode: GraphMode;
  setMode: (next: GraphMode) => void;
  setPaused: React.Dispatch<React.SetStateAction<boolean>>;
  clipboard: GraphClipboardApi<N>;
  exportJSON: () => string;
  importJSON: (json: string) => void;
  downloadJSON: (filename?: string) => void;
  /** Live link selection — needed for fitToSelection to expand into
   *  link endpoints. */
  selectedLinkIds: Array<string | number>;
}

/** Hooks `useImperativeHandle` to expose the canvas's full
 *  programmatic API surface to consumers via a forwardRef. Kept as a
 *  hook so the main component file stays focused on rendering.
 *
 *  Live state (selection, link selection, data, engine) is read inside
 *  the handle methods via a ref so the handle object identity doesn't
 *  churn on every click. Without this, every selection change rebuilt
 *  the whole handle and any host holding `ref.current` saw a fresh set
 *  of method identities each click — defeating downstream memoisation
 *  on the host side. */
export function useImperativeGraphHandle<
  N extends NodeObject,
  L extends LinkObject,
>(params: UseImperativeGraphHandleParams<N, L>): void {
  const { ref } = params;

  // Single trampoline ref — every handle method below reads through
  // this. Keeps the handle's identity stable across selection / data
  // mutations while still exposing the *latest* values when called.
  const latestRef = useRef(params);
  latestRef.current = params;

  useImperativeHandle(
    ref,
    () => ({
      getData: () => latestRef.current.dataApi.data,
      setData: (next) => latestRef.current.dataApi.setData(next),
      addNode: (node, opts) => latestRef.current.dataApi.addNode(node, opts),
      addNodes: (nodes) => latestRef.current.dataApi.addNodes(nodes),
      updateNode: (id, patch) =>
        latestRef.current.dataApi.updateNode(id, patch),
      // Funnel imperative removes through the delete-gate. Hosts that
      // didn't supply a guard get a resolved-true promise on every call.
      removeNode: (id) =>
        latestRef.current.deleteGate.requestNodeDelete([id], "imperative"),
      removeNodes: (ids) =>
        latestRef.current.deleteGate.requestNodeDelete(ids, "imperative"),
      addLink: (link) => latestRef.current.dataApi.addLink(link),
      addLinks: (links) => latestRef.current.dataApi.addLinks(links),
      removeLink: (predicate) =>
        latestRef.current.deleteGate.requestLinkDelete(predicate, "imperative"),
      clear: () => latestRef.current.dataApi.clear(),

      getSelection: () => [...latestRef.current.selection.selected],
      setSelection: (ids) => latestRef.current.selection.set(ids),
      selectAll: () =>
        latestRef.current.selection.set(
          latestRef.current.dataApi.data.nodes.map((n) => n.id),
        ),
      clearSelection: () => latestRef.current.selection.clear(),

      getMode: () => latestRef.current.mode,
      setMode: (next) => latestRef.current.setMode(next),
      fit: (durationMs, padding) =>
        latestRef.current.engine?.fit(durationMs, padding),
      centerAt: (x, y, z, durationMs) =>
        latestRef.current.engine?.centerAt(x, y, z, durationMs),
      zoom: (scale, durationMs) =>
        latestRef.current.engine?.zoom(scale, durationMs),
      zoomIn: (step = 1.2) => {
        const eng = latestRef.current.engine;
        if (eng) eng.zoom((eng.getZoom?.() ?? 1) * step, 200);
      },
      zoomOut: (step = 1.2) => {
        const eng = latestRef.current.engine;
        if (eng) eng.zoom((eng.getZoom?.() ?? 1) / step, 200);
      },
      panBy: (delta, durationMs) =>
        latestRef.current.engine?.panBy?.(delta, durationMs),
      goTo: (target, opts) => latestRef.current.engine?.goTo?.(target, opts),
      fitToNodes: (ids, durationMs, padding) =>
        latestRef.current.engine?.fitToNodes?.(ids, durationMs, padding),
      fitToSelection: (durationMs, padding) => {
        const p = latestRef.current;
        if (!p.engine) return;
        const nodeIdSet = new Set<string | number>(p.selection.selected);
        if (p.selectedLinkIds.length > 0) {
          // O(L) instead of O(L × selected-links) — build a Set once
          // so the per-link membership test is O(1). Matters when both
          // the link list and the link selection are large.
          const linkIdSet = new Set<string | number>(p.selectedLinkIds);
          for (const link of p.dataApi.data.links) {
            const lid = link.id;
            if (lid === undefined || !linkIdSet.has(lid)) continue;
            const s =
              typeof link.source === "object"
                ? (link.source as { id: string | number }).id
                : link.source;
            const t =
              typeof link.target === "object"
                ? (link.target as { id: string | number }).id
                : link.target;
            if (s !== undefined) nodeIdSet.add(s);
            if (t !== undefined) nodeIdSet.add(t);
          }
        }
        if (nodeIdSet.size === 0) return;
        p.engine.fitToNodes?.([...nodeIdSet], durationMs ?? 400, padding ?? 60);
      },

      pause: () => {
        latestRef.current.engine?.pause();
        latestRef.current.setPaused(true);
      },
      resume: () => {
        latestRef.current.engine?.resume();
        latestRef.current.setPaused(false);
      },
      reheat: () => latestRef.current.engine?.reheat(),
      screenshot: async () => {
        const canvas = latestRef.current.engine?.getCanvasElement();
        if (!canvas) return null;
        return new Promise<Blob | null>((resolve) =>
          canvas.toBlob((b) => resolve(b)),
        );
      },

      copy: () => latestRef.current.clipboard.copy(),
      cut: () => latestRef.current.clipboard.cut(),
      paste: (opts) => latestRef.current.clipboard.paste(opts?.at),
      duplicate: () => latestRef.current.clipboard.duplicate(),
      addConnectedNode: (opts) =>
        latestRef.current.clipboard.addConnectedNode(opts),

      togglePin: (id) => latestRef.current.clipboard.togglePin(id),

      exportJSON: () => latestRef.current.exportJSON(),
      importJSON: (json) => latestRef.current.importJSON(json),
      downloadJSON: (filename) => latestRef.current.downloadJSON(filename),

      d3Force: ((name: string, fn?: unknown | null) =>
        latestRef.current.engine?.d3Force(name, fn)) as LoraGraphCanvasHandle<
        N,
        L
      >["d3Force"],
      emitParticle: (link: L) => latestRef.current.engine?.emitParticle(link),
      stopAnimation: () => latestRef.current.engine?.stopAnimation(),

      engine2D: () =>
        latestRef.current.engine?.mode === "2d"
          ? latestRef.current.engine
          : null,
      engine3D: () =>
        latestRef.current.engine?.mode === "3d"
          ? latestRef.current.engine
          : null,
    }),
    // Empty deps: every method reads through `latestRef`, so the
    // handle object's identity stays stable for the component's
    // entire lifetime. `ref` itself is excluded because React's
    // imperative-handle internals manage that attachment.
    [],
  );
}
