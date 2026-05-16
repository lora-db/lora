import { useEffect, useRef } from "react";
import type { GraphEngine } from "../engines/types";
import type {
  GraphMode,
  LinkObject,
  NodeObject,
  ToolId,
} from "../types";
import type { GraphDataApi } from "./useGraphData";
import type { SelectionApi } from "./useGraphSelection";

export interface UseGraphKeybindingsParams<
  N extends NodeObject,
  L extends LinkObject,
> {
  engine: GraphEngine<N, L> | null;
  dataApi: GraphDataApi<N, L>;
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
}

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
    const onKey = (e: KeyboardEvent) => {
      // Only handle when the focus is inside our host or the body.
      const target = e.target as HTMLElement | null;
      const editable =
        target &&
        (target.tagName === "INPUT" ||
          target.tagName === "TEXTAREA" ||
          target.isContentEditable);
      if (editable) return;

      const p = paramsRef.current;

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
          p.engine?.fit(400, 40);
          break;
        case "3":
          p.setMode(p.mode === "2d" ? "3d" : "2d");
          break;
        case "Backspace":
        case "Delete":
          if (p.selection.selected.length > 0) {
            p.dataApi.removeNodes(p.selection.selected);
            p.selection.clear();
            e.preventDefault();
          }
          if (p.selectedLinkIds.length > 0) {
            const linkIdSet = new Set(p.selectedLinkIds);
            p.dataApi.removeLink(
              (l) => l.id !== undefined && linkIdSet.has(l.id),
            );
            p.setSelectedLinkIds([]);
            e.preventDefault();
          }
          break;
        case "a":
        case "A":
          if (e.metaKey || e.ctrlKey) {
            p.selection.set(p.dataApi.data.nodes.map((n) => n.id));
            p.setSelectedLinkIds([]);
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
