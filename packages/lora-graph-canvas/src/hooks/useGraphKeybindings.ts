import { useEffect, useRef, type RefObject } from "react";
import type { GraphEngine } from "../engines/types";
import type {
  GraphMode,
  LinkObject,
  NodeObject,
  ToolId,
} from "../types";
import type { GraphDataApi } from "./useGraphData";
import type { GraphDeleteGateApi } from "./useGraphDeleteGate";
import type { SelectionApi } from "./useGraphSelection";

export interface UseGraphKeybindingsParams<
  N extends NodeObject,
  L extends LinkObject,
> {
  engine: GraphEngine<N, L> | null;
  dataApi: GraphDataApi<N, L>;
  deleteGate: GraphDeleteGateApi<L>;
  selection: SelectionApi;
  mode: GraphMode;
  setMode: (next: GraphMode) => void;
  selectedLinkIds: Array<string | number>;
  setSelectedLinkIds: React.Dispatch<
    React.SetStateAction<Array<string | number>>
  >;
  setLinkSourceId: React.Dispatch<
    React.SetStateAction<string | number | null>
  >;
  setActiveTool: React.Dispatch<React.SetStateAction<ToolId>>;
  enableClipboard: boolean;
  copy: () => unknown;
  cut: () => unknown;
  paste: () => unknown;
  duplicate: () => unknown;
  addConnectedNode: () => unknown;
  togglePin: (id: string | number) => void;
  /** Host element. Bindings only fire while focus is inside this
   *  element — otherwise hitting `f` while typing into a sibling text
   *  field on the page would trigger the canvas fit shortcut. */
  hostRef: RefObject<HTMLElement | null>;
}

/** Pan step in graph-space units. The arrow-key handler converts to a
 *  centerAt() call relative to the current view. Tuned so a single tap
 *  shifts the view by ~10% of a typical bbox without feeling laggy on
 *  a held key (the browser's repeat will fire ~30/s). */
const ARROW_PAN_STEP = 40;

/** Global keyboard shortcuts for the canvas. The listener is bound once
 *  per mount; live state is read through a ref so we avoid the
 *  re-binding churn that would otherwise happen on every selection
 *  change. */
export function useGraphKeybindings<
  N extends NodeObject,
  L extends LinkObject,
>(params: UseGraphKeybindingsParams<N, L>): void {
  const paramsRef = useRef(params);
  paramsRef.current = params;

  useEffect(() => {
    // Index walked by Tab / Shift+Tab. Reset to -1 (== "before first
    // node") so the first Tab lands on node 0. Held outside React state
    // because cycling through 5k nodes shouldn't trigger renders.
    let tabIndex = -1;

    const onKey = (e: KeyboardEvent) => {
      const p = paramsRef.current;
      // Scope to the canvas: only fire when focus is inside our host.
      // Without this, pressing `f` in a page-level text field
      // elsewhere would also fit the canvas — confusing on pages with
      // multiple canvases or any kind of form.
      const host = p.hostRef.current;
      const active = document.activeElement as HTMLElement | null;
      if (host && active && active !== document.body && !host.contains(active)) {
        return;
      }
      // Skip when the focused element is editable — even if it lives
      // inside our host (a property panel, an inline rename, etc).
      const target = e.target as HTMLElement | null;
      const editable =
        target &&
        (target.tagName === "INPUT" ||
          target.tagName === "TEXTAREA" ||
          target.isContentEditable);
      if (editable) return;

      switch (e.key) {
        case "v":
        case "V":
          if (p.enableClipboard && (e.metaKey || e.ctrlKey)) {
            p.paste();
            e.preventDefault();
          } else {
            p.setActiveTool("select");
          }
          break;
        case "h":
        case "H":
          p.setActiveTool("pan");
          break;
        case "n":
        case "N":
          p.setActiveTool("add-node");
          break;
        case "l":
        case "L":
          p.setActiveTool("add-link");
          break;
        case "f":
        case "F":
        case "0":
          // `0` mirrors the figma/photoshop "fit to viewport" convention.
          // Shift+F fits to the current selection instead of the whole
          // graph — convenient for diving into a clicked cluster.
          if (e.shiftKey && (e.key === "f" || e.key === "F")) {
            const ids = [...p.selection.selected, ...p.selectedLinkIds];
            // For links, fold endpoints into the node-id set so the
            // bbox covers the link's reach, not just selected nodes.
            const nodeIdSet = new Set<string | number>(p.selection.selected);
            if (p.selectedLinkIds.length > 0) {
              // O(L) instead of O(L × |selected-links|) — see same
              // pattern in fitToSelection + the fit-on-select effect.
              const linkIdSet = new Set<string | number>(p.selectedLinkIds);
              for (const link of p.dataApi.data.links) {
                const lid = link.id;
                if (lid === undefined || !linkIdSet.has(lid)) {
                  continue;
                }
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
            if (nodeIdSet.size > 0 && p.engine?.fitToNodes) {
              p.engine.fitToNodes([...nodeIdSet], 400, 60);
            } else {
              p.engine?.fit(400, 40);
            }
            e.preventDefault();
            void ids;
            break;
          }
          p.engine?.fit(400, 40);
          e.preventDefault();
          break;
        case "+":
        case "=":
          // US-layout shift+'=' produces '+', plain key gives '='.
          // Accept both so users don't need to learn which one their
          // layout exposes. 1.2× / 0.83× per tap matches the toolbar
          // buttons in LoraGraphCanvas:1188.
          if (p.engine) {
            p.engine.zoom((p.engine.getZoom?.() ?? 1) * 1.2, 200);
            e.preventDefault();
          }
          break;
        case "-":
          if (p.engine) {
            p.engine.zoom((p.engine.getZoom?.() ?? 1) / 1.2, 200);
            e.preventDefault();
          }
          break;
        case "3":
          p.setMode(p.mode === "2d" ? "3d" : "2d");
          break;
        case "ArrowLeft":
        case "ArrowRight":
        case "ArrowUp":
        case "ArrowDown": {
          const eng = p.engine;
          if (!eng) return;
          // Step scales with zoom so the perceived pan size stays
          // consistent across zoom levels.
          const k = eng.getZoom?.() ?? 1;
          const step = ARROW_PAN_STEP / Math.max(k, 0.1);
          let dx = 0;
          let dy = 0;
          let dz = 0;
          if (e.key === "ArrowLeft") dx = -step;
          else if (e.key === "ArrowRight") dx = step;
          else if (e.key === "ArrowUp") dy = -step;
          else dy = step;
          // Shift + Arrow Up/Down → world-Z elevation in 3D (camera
          // and lookAt translate together). Z is locked in 2D, so
          // shift just makes it a slightly larger step there.
          if (e.shiftKey && (e.key === "ArrowUp" || e.key === "ArrowDown")) {
            if (p.mode === "3d") {
              dz = e.key === "ArrowUp" ? step : -step;
              dy = 0;
            }
          }
          if (eng.panBy) {
            eng.panBy({ x: dx, y: dy, z: dz }, 120);
          } else {
            // Fallback for engines without panBy.
            const bbox = eng.getGraphBbox();
            const cx = (bbox.x[0] + bbox.x[1]) / 2;
            const cy = (bbox.y[0] + bbox.y[1]) / 2;
            eng.centerAt(cx + dx, cy + dy, undefined, 120);
          }
          e.preventDefault();
          break;
        }
        case "PageUp":
        case "PageDown":
        case "q":
        case "Q":
        case "e":
        case "E": {
          // World-Z elevation in 3D. PageUp/Q go up, PageDown/E go
          // down. In 2D mode these are no-ops since the camera height
          // is locked.
          const eng = p.engine;
          if (!eng) break;
          if (p.mode !== "3d") {
            if (e.key === "PageUp" || e.key === "PageDown") {
              // Don't swallow PageUp/Down in 2D — let the browser
              // scroll the surrounding doc.
              break;
            }
            // q/e: pass through without preventing default.
            break;
          }
          const k = eng.getZoom?.() ?? 1;
          const step = ARROW_PAN_STEP / Math.max(k, 0.1);
          const up = e.key === "PageUp" || e.key === "q" || e.key === "Q";
          if (eng.panBy) {
            eng.panBy({ z: up ? step : -step }, 120);
          }
          e.preventDefault();
          break;
        }
        case "Tab": {
          // Walk forward (Tab) / backward (Shift+Tab) through the
          // node list and focus each. Wraps. The tabIndex closure
          // above isn't a React state, so this doesn't render.
          const eng = p.engine;
          const nodes = p.dataApi.data.nodes;
          if (!eng || nodes.length === 0) return;
          tabIndex = e.shiftKey
            ? (tabIndex - 1 + nodes.length) % nodes.length
            : (tabIndex + 1) % nodes.length;
          const node = nodes[tabIndex];
          if (
            !node ||
            node.x === undefined ||
            node.y === undefined
          ) {
            return;
          }
          p.selection.set([node.id]);
          eng.focusOn(
            { x: node.x, y: node.y, ...(node.z !== undefined ? { z: node.z } : {}) },
            { distance: 120, zoom: 4, durationMs: 400 },
          );
          e.preventDefault();
          break;
        }
        case "Backspace":
        case "Delete":
          if (
            p.selection.selected.length > 0 ||
            p.selectedLinkIds.length > 0
          ) {
            // Funnel through the gate so the host's confirm-delete
            // prompt has a chance to cancel. The promise is fire-and-
            // forget: the gate's afterNodeDelete / afterLinkDelete
            // callbacks own the selection cleanup, so we don't await
            // here (and shouldn't — the listener is sync).
            void p.deleteGate.requestMixedDelete(
              p.selection.selected,
              p.selectedLinkIds,
              "keyboard",
            );
            e.preventDefault();
          }
          break;
        case "a":
        case "A":
          if (e.metaKey || e.ctrlKey) {
            p.selection.set(p.dataApi.data.nodes.map((n) => n.id));
            // Include every link with an id, too — Ctrl-A is a
            // "select everything" gesture, and the delete / duplicate
            // pipelines already accept mixed node + link selections.
            // Links without an id can't be addressed by the selection
            // model, so they get dropped silently.
            p.setSelectedLinkIds(
              p.dataApi.data.links
                .map((l) => l.id)
                .filter(
                  (id): id is string | number => id !== undefined,
                ),
            );
            e.preventDefault();
          }
          break;
        case "c":
        case "C":
          if (p.enableClipboard && (e.metaKey || e.ctrlKey)) {
            p.copy();
            // Let the OS clipboard event fire too — the user might be
            // copying text from a tooltip or similar.
          }
          break;
        case "x":
        case "X":
          if (p.enableClipboard && (e.metaKey || e.ctrlKey)) {
            p.cut();
            e.preventDefault();
          }
          break;
        case "d":
        case "D":
          if (e.metaKey || e.ctrlKey) {
            p.duplicate();
            e.preventDefault();
          }
          break;
        case "p":
        case "P":
          for (const id of p.selection.selected) p.togglePin(id);
          break;
        case "Enter":
          // Quick "connect to new node" — only when the user has a
          // selection. The editable-target check above already returns
          // early when a focused input owns the key.
          if (p.selection.selected.length > 0) {
            p.addConnectedNode();
            e.preventDefault();
          }
          break;
        case "Escape":
          p.selection.clear();
          p.setSelectedLinkIds([]);
          p.setLinkSourceId(null);
          break;
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);
}
