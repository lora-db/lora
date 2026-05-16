import { useCallback, useMemo, useRef, type MutableRefObject } from "react";
import type { GraphEngine } from "../engines/types";
import type { LinkObject, NodeObject } from "../types";
import type { GraphDataApi } from "./useGraphData";
import type { SelectionApi } from "./useGraphSelection";

export interface UseGraphClipboardParams<
  N extends NodeObject,
  L extends LinkObject,
> {
  enableClipboard: boolean;
  dataApi: GraphDataApi<N, L>;
  selection: SelectionApi;
  setSelectedLinkIds: React.Dispatch<
    React.SetStateAction<Array<string | number>>
  >;
  engineRef: MutableRefObject<GraphEngine<N, L> | null>;
  lastCursorRef: MutableRefObject<{ x: number; y: number } | null>;
  onCopy?: (nodes: N[]) => void;
  onCut?: (nodes: N[]) => void;
  onPaste?: (nodes: N[]) => void;
}

export interface GraphClipboardApi<N extends NodeObject> {
  /** Live "is clipboard non-empty" indicator — read once per render for
   *  the selection panel chrome. The ref itself is also exposed for
   *  callers that need to react on every keystroke. */
  hasClipboard(): boolean;
  copy(): N[];
  cut(): N[];
  paste(at?: { x: number; y: number; z?: number }): N[];
  duplicate(): N[];
  addConnectedNode(opts?: {
    at?: { x: number; y: number; z?: number };
    label?: string;
  }): N | null;
  togglePin(id: string | number): void;
}

/** Bundles the editing primitives that read from / write to a private
 *  per-instance clipboard. The clipboard lives in a ref so writes don't
 *  trigger re-renders, and the OS clipboard is intentionally not
 *  touched — copy/paste shouldn't disturb the user's other apps. */
export function useGraphClipboard<
  N extends NodeObject,
  L extends LinkObject,
>(params: UseGraphClipboardParams<N, L>): GraphClipboardApi<N> {
  const {
    enableClipboard,
    dataApi,
    selection,
    setSelectedLinkIds,
    engineRef,
    lastCursorRef,
    onCopy,
    onCut,
    onPaste,
  } = params;

  const clipboardRef = useRef<Array<Partial<N>>>([]);

  const snapshotSelection = useCallback((): Array<Partial<N>> => {
    const idSet = new Set(selection.selected);
    const out: Array<Partial<N>> = [];
    for (const node of dataApi.data.nodes) {
      if (!idSet.has(node.id)) continue;
      // Strip id + simulation fields so paste generates fresh ones.
      // eslint-disable-next-line @typescript-eslint/no-unused-vars
      const { id, x, y, z, vx, vy, vz, fx, fy, fz, ...rest } =
        node as N & Record<string, unknown>;
      out.push(rest as unknown as Partial<N>);
    }
    return out;
  }, [dataApi.data.nodes, selection.selected]);

  const copy = useCallback((): N[] => {
    if (!enableClipboard) return [];
    const idSet = new Set(selection.selected);
    const snapshot = dataApi.data.nodes.filter((n) => idSet.has(n.id));
    clipboardRef.current = snapshotSelection();
    onCopy?.(snapshot);
    return snapshot;
  }, [
    enableClipboard,
    dataApi.data.nodes,
    selection.selected,
    snapshotSelection,
    onCopy,
  ]);

  const cut = useCallback((): N[] => {
    if (!enableClipboard) return [];
    const idSet = new Set(selection.selected);
    const snapshot = dataApi.data.nodes.filter((n) => idSet.has(n.id));
    clipboardRef.current = snapshotSelection();
    onCut?.(snapshot);
    if (selection.selected.length > 0) {
      dataApi.removeNodes(selection.selected);
      selection.clear();
    }
    return snapshot;
  }, [enableClipboard, dataApi, selection, snapshotSelection, onCut]);

  const paste = useCallback(
    (at?: { x: number; y: number; z?: number }): N[] => {
      if (!enableClipboard) return [];
      const clipboard = clipboardRef.current;
      if (clipboard.length === 0) return [];
      const target = at
        ? at
        : (() => {
            const c = lastCursorRef.current;
            if (!c || !engineRef.current) return undefined;
            return engineRef.current.screen2Graph(c.x, c.y);
          })();
      const created = dataApi.addNodes(
        clipboard.map((tmpl, i) => {
          const offsetX = (i % 3) * 24;
          const offsetY = Math.floor(i / 3) * 24;
          return {
            ...tmpl,
            ...(target
              ? {
                  x: target.x + offsetX,
                  y: target.y + offsetY,
                  ...(target.z !== undefined
                    ? { z: target.z + offsetY }
                    : {}),
                }
              : {}),
          } as Partial<N> & { id?: string | number };
        }),
      );
      selection.set(created.map((n) => n.id));
      setSelectedLinkIds([]);
      onPaste?.(created);
      return created;
    },
    [
      enableClipboard,
      dataApi,
      selection,
      setSelectedLinkIds,
      onPaste,
      engineRef,
      lastCursorRef,
    ],
  );

  // Duplicate is a self-contained primitive — it doesn't touch the
  // clipboard, so it works even when `enableClipboard` is false.
  const duplicate = useCallback((): N[] => {
    const idSet = new Set(selection.selected);
    const templates: Array<Partial<N> & { id?: string | number }> = [];
    let i = 0;
    for (const node of dataApi.data.nodes) {
      if (!idSet.has(node.id)) continue;
      // eslint-disable-next-line @typescript-eslint/no-unused-vars
      const { id, vx, vy, vz, fx, fy, fz, ...rest } =
        node as N & Record<string, unknown>;
      const offsetX = (i % 3) * 24;
      const offsetY = Math.floor(i / 3) * 24;
      templates.push({
        ...(rest as Partial<N>),
        ...(node.x !== undefined ? { x: node.x + offsetX } : {}),
        ...(node.y !== undefined ? { y: node.y + offsetY } : {}),
        ...(node.z !== undefined ? { z: node.z } : {}),
      });
      i++;
    }
    if (templates.length === 0) return [];
    const created = dataApi.addNodes(templates);
    selection.set(created.map((n) => n.id));
    setSelectedLinkIds([]);
    return created;
  }, [dataApi, selection, setSelectedLinkIds]);

  // Creates one new node and links every currently selected node to it.
  // Placed near the selection's centroid with a small offset so the new
  // node doesn't overlap. Selects the new node so the user can keep
  // building outward from it.
  const addConnectedNode = useCallback(
    (opts?: {
      at?: { x: number; y: number; z?: number };
      label?: string;
    }): N | null => {
      const ids = selection.selected;
      if (ids.length === 0) return null;
      const sources = dataApi.data.nodes.filter((n) => ids.includes(n.id));
      if (sources.length === 0) return null;

      let pos = opts?.at;
      if (!pos) {
        let sx = 0;
        let sy = 0;
        let sz = 0;
        let any = false;
        for (const s of sources) {
          sx += s.x ?? 0;
          sy += s.y ?? 0;
          sz += s.z ?? 0;
          if (s.x !== undefined || s.y !== undefined) any = true;
        }
        if (any) {
          pos = {
            x: sx / sources.length + 30,
            y: sy / sources.length + 30,
            z: sz / sources.length,
          };
        }
      }

      const seed: Partial<N> = (opts?.label !== undefined
        ? { label: opts.label }
        : {}) as Partial<N>;
      const newNode = dataApi.addNode(seed, pos ? { at: pos } : undefined);

      for (const source of sources) {
        dataApi.addLink({
          source: source.id,
          target: newNode.id,
        } as Parameters<typeof dataApi.addLink>[0]);
      }

      selection.set([newNode.id]);
      setSelectedLinkIds([]);
      return newNode;
    },
    [dataApi, selection, setSelectedLinkIds],
  );

  const togglePin = useCallback(
    (id: string | number) => {
      const node = dataApi.data.nodes.find((n) => n.id === id);
      if (!node) return;
      if (node.fx !== undefined) {
        dataApi.updateNode(id, {
          fx: undefined,
          fy: undefined,
          fz: undefined,
        } as unknown as Partial<N>);
      } else {
        dataApi.updateNode(id, {
          fx: node.x,
          fy: node.y,
          ...(node.z !== undefined ? { fz: node.z } : {}),
        } as unknown as Partial<N>);
      }
    },
    [dataApi],
  );

  const hasClipboard = useCallback(
    () => clipboardRef.current.length > 0,
    [],
  );

  return useMemo<GraphClipboardApi<N>>(
    () => ({
      hasClipboard,
      copy,
      cut,
      paste,
      duplicate,
      addConnectedNode,
      togglePin,
    }),
    [hasClipboard, copy, cut, paste, duplicate, addConnectedNode, togglePin],
  );
}
