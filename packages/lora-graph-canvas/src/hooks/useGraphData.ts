import { useCallback, useEffect, useRef, useState } from "react";
import type { GraphData, LinkObject, NodeObject } from "../types";
import { createId } from "../utils/ids";

const EMPTY_DATA: GraphData = { nodes: [], links: [] };

export interface UseGraphDataOptions<
  N extends NodeObject,
  L extends LinkObject,
> {
  /** Controlled data — when present, props win. */
  controlled?: GraphData<N, L>;
  /** Uncontrolled seed — used only on mount. */
  defaultData?: GraphData<N, L>;
  /** Fires for every mutation regardless of controlled/uncontrolled. */
  onChange?: (next: GraphData<N, L>) => void;
}

export interface GraphDataApi<N extends NodeObject, L extends LinkObject> {
  data: GraphData<N, L>;
  setData(next: GraphData<N, L>): void;
  addNode(
    node?: Partial<N> & { id?: string | number },
    opts?: { at?: { x: number; y: number; z?: number } },
  ): N;
  addNodes(nodes: Array<Partial<N> & { id?: string | number }>): N[];
  updateNode(id: string | number, patch: Partial<N>): void;
  removeNode(id: string | number): void;
  removeNodes(ids: Array<string | number>): void;
  addLink(link: {
    source: string | number;
    target: string | number;
    id?: string | number;
  } & Partial<L>): L;
  addLinks(
    links: Array<
      { source: string | number; target: string | number } & Partial<L>
    >,
  ): L[];
  removeLink(predicate: (l: L) => boolean): void;
  clear(): void;
}

/** Owns the canonical `GraphData` for a `LoraGraphCanvas`. Supports
 *  controlled and uncontrolled usage; mutators always notify `onChange`. */
export function useGraphData<
  N extends NodeObject,
  L extends LinkObject,
>(opts: UseGraphDataOptions<N, L>): GraphDataApi<N, L> {
  const isControlled = opts.controlled !== undefined;

  // Ensure every link has an id. Without this, link selection breaks
  // — we key the selectedLinkIds set by id, and any link the host
  // passed in `{ source, target }` (no id) would silently fail every
  // click. We mutate in place to keep referential equality with the
  // host's data (the kapsule already mutates these objects for
  // simulation state). Memoised so it's stable across renders and
  // safe to put in dependency arrays.
  const ensureLinkIds = useCallback((data: GraphData<N, L>) => {
    for (const link of data.links) {
      if ((link as { id?: unknown }).id === undefined) {
        (link as { id?: string | number }).id = createId("l");
      }
    }
    return data;
  }, []);

  const [internal, setInternal] = useState<GraphData<N, L>>(() => {
    const seed =
      opts.defaultData ??
      opts.controlled ??
      (EMPTY_DATA as GraphData<N, L>);
    return ensureLinkIds(seed);
  });

  // Mirror controlled data into the internal slot so the mutators
  // always read from the same source.
  // `ensureLinkIds` is stable inside this hook; declared inline below.
  useEffect(() => {
    if (isControlled && opts.controlled) {
      setInternal(ensureLinkIds(opts.controlled));
    }
  }, [isControlled, opts.controlled, ensureLinkIds]);

  const dataRef = useRef(internal);
  dataRef.current = internal;

  const onChangeRef = useRef(opts.onChange);
  onChangeRef.current = opts.onChange;

  const commit = useCallback(
    (next: GraphData<N, L>) => {
      dataRef.current = next;
      if (!isControlled) setInternal(next);
      onChangeRef.current?.(next);
    },
    [isControlled],
  );

  const setData = useCallback(
    (next: GraphData<N, L>) => commit(ensureLinkIds(next)),
    [commit, ensureLinkIds],
  );

  const addNode = useCallback(
    (
      node?: Partial<N> & { id?: string | number },
      addOpts?: { at?: { x: number; y: number; z?: number } },
    ): N => {
      const id = node?.id ?? createId("n");
      const created = {
        ...(node ?? {}),
        id,
        ...(addOpts?.at ?? {}),
      } as unknown as N;
      const next: GraphData<N, L> = {
        nodes: [...dataRef.current.nodes, created],
        links: dataRef.current.links,
      };
      commit(next);
      return created;
    },
    [commit],
  );

  const addNodes = useCallback(
    (nodes: Array<Partial<N> & { id?: string | number }>): N[] => {
      const created = nodes.map(
        (n) => ({ ...n, id: n.id ?? createId("n") }) as unknown as N,
      );
      const next: GraphData<N, L> = {
        nodes: [...dataRef.current.nodes, ...created],
        links: dataRef.current.links,
      };
      commit(next);
      return created;
    },
    [commit],
  );

  const updateNode = useCallback(
    (id: string | number, patch: Partial<N>) => {
      const nodes = dataRef.current.nodes.map((n) =>
        n.id === id ? ({ ...n, ...patch } as N) : n,
      );
      commit({ nodes, links: dataRef.current.links });
    },
    [commit],
  );

  const removeNode = useCallback(
    (id: string | number) => {
      const cur = dataRef.current;
      const nodes = cur.nodes.filter((n) => n.id !== id);
      const links = cur.links.filter((l) => {
        const s = typeof l.source === "object" ? l.source?.id : l.source;
        const t = typeof l.target === "object" ? l.target?.id : l.target;
        return s !== id && t !== id;
      });
      commit({ nodes, links });
    },
    [commit],
  );

  const removeNodes = useCallback(
    (ids: Array<string | number>) => {
      const idSet = new Set(ids);
      const cur = dataRef.current;
      const nodes = cur.nodes.filter((n) => !idSet.has(n.id));
      const links = cur.links.filter((l) => {
        const s = typeof l.source === "object" ? l.source?.id : l.source;
        const t = typeof l.target === "object" ? l.target?.id : l.target;
        return (
          s !== undefined &&
          t !== undefined &&
          !idSet.has(s) &&
          !idSet.has(t)
        );
      });
      commit({ nodes, links });
    },
    [commit],
  );

  const addLink = useCallback(
    (link: {
      source: string | number;
      target: string | number;
      id?: string | number;
    } & Partial<L>): L => {
      const created = { id: createId("l"), ...link } as unknown as L;
      commit({
        nodes: dataRef.current.nodes,
        links: [...dataRef.current.links, created],
      });
      return created;
    },
    [commit],
  );

  const addLinks = useCallback(
    (
      links: Array<
        { source: string | number; target: string | number } & Partial<L>
      >,
    ): L[] => {
      const created = links.map(
        (l) => ({ id: createId("l"), ...l }) as unknown as L,
      );
      commit({
        nodes: dataRef.current.nodes,
        links: [...dataRef.current.links, ...created],
      });
      return created;
    },
    [commit],
  );

  const removeLink = useCallback(
    (predicate: (l: L) => boolean) => {
      const links = dataRef.current.links.filter((l) => !predicate(l));
      commit({ nodes: dataRef.current.nodes, links });
    },
    [commit],
  );

  const clear = useCallback(() => {
    commit({ nodes: [], links: [] } as GraphData<N, L>);
  }, [commit]);

  return {
    data: internal,
    setData,
    addNode,
    addNodes,
    updateNode,
    removeNode,
    removeNodes,
    addLink,
    addLinks,
    removeLink,
    clear,
  };
}
