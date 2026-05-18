import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { GraphData, LinkObject, NodeObject } from "../types";
import { createId } from "../utils/ids";

const EMPTY_DATA: GraphData = { nodes: [], links: [] };

/** Fields the d3-force / kapsule simulation writes onto each node
 *  every tick. Their presence means a prior canvas mount has chewed
 *  on this exact node object — typical when the host has a
 *  module-level data constant that's reused across remounts (story
 *  navigation, HMR, route changes). We clear them so the next mount
 *  re-derives a fresh layout instead of inheriting the previous one,
 *  which otherwise looks like the canvas has "remembered" positions
 *  across reloads.
 *
 *  Intentional positions (host passed `{ id, x, y }` for layout
 *  control) survive — we only strip when a velocity / index marker
 *  proves a previous simulation ran. Pinned coords (`fx`, `fy`,
 *  `fz`) are never touched: those are an explicit "stay here" pin
 *  the kapsule respects. */
const SIM_FIELDS = [
  "x",
  "y",
  "z",
  "vx",
  "vy",
  "vz",
  "index",
  "_neighbors",
  "_links",
] as const;

function stripStaleSimulationState<
  N extends NodeObject,
  L extends LinkObject,
>(data: GraphData<N, L>): GraphData<N, L> {
  let mutated = false;
  const nodes = data.nodes.map((n) => {
    const sim = n as unknown as Record<string, unknown>;
    // No simulation markers → host node, leave alone.
    if (
      sim.vx === undefined &&
      sim.vy === undefined &&
      sim.vz === undefined &&
      sim.index === undefined
    ) {
      return n;
    }
    mutated = true;
    const clone: Record<string, unknown> = {};
    for (const key in sim) {
      if (!(SIM_FIELDS as readonly string[]).includes(key)) {
        clone[key] = sim[key];
      }
    }
    return clone as N;
  });
  // Links may have had source/target resolved to node-object refs
  // by a prior kapsule mount. Reset back to ids so the next mount
  // re-resolves against the cleaned-up node array (and so id-based
  // selection lookups still work before the first tick).
  const links = data.links.map((l) => {
    const src = (l as unknown as { source?: unknown }).source;
    const tgt = (l as unknown as { target?: unknown }).target;
    const srcIsObj = src !== null && typeof src === "object";
    const tgtIsObj = tgt !== null && typeof tgt === "object";
    if (!srcIsObj && !tgtIsObj) return l;
    mutated = true;
    return {
      ...l,
      source: srcIsObj ? (src as { id: string | number }).id : src,
      target: tgtIsObj ? (tgt as { id: string | number }).id : tgt,
    } as L;
  });
  if (!mutated) return data;
  return { nodes, links };
}

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
  removeNodes(ids: ReadonlyArray<string | number>): void;
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
  // click. For uncontrolled data the kapsule already mutates these
  // objects for simulation state, so an in-place fill is fine; for
  // controlled data the host owns the array and an in-place mutation
  // would surprise them (and break shallow-equality state checks),
  // so we shallow-clone any link that's missing an id. Memoised so
  // it's stable across renders and safe to put in dependency arrays.
  const ensureLinkIdsInPlace = useCallback((data: GraphData<N, L>) => {
    for (const link of data.links) {
      if ((link as { id?: unknown }).id === undefined) {
        (link as { id?: string | number }).id = createId("l");
      }
    }
    return data;
  }, []);
  const ensureLinkIdsCopyOnWrite = useCallback(
    (data: GraphData<N, L>): GraphData<N, L> => {
      let dirty = false;
      const links = data.links.map((link) => {
        if ((link as { id?: unknown }).id !== undefined) return link;
        dirty = true;
        return { ...link, id: createId("l") } as L;
      });
      if (!dirty) return data;
      return { nodes: data.nodes, links };
    },
    [],
  );

  const [internal, setInternal] = useState<GraphData<N, L>>(() => {
    // Seed precedence: uncontrolled `defaultData`, then controlled
    // `controlled`, then empty. For the controlled seed we don't
    // mutate the host's links — copy-on-write so the host's array
    // stays untouched. Uncontrolled data is canvas-owned so we can
    // mutate freely.
    if (opts.defaultData !== undefined) {
      return ensureLinkIdsInPlace(stripStaleSimulationState(opts.defaultData));
    }
    if (opts.controlled !== undefined) {
      return ensureLinkIdsCopyOnWrite(stripStaleSimulationState(opts.controlled));
    }
    return EMPTY_DATA as GraphData<N, L>;
  });

  // Mirror controlled data into the internal slot so the mutators
  // always read from the same source. Copy-on-write to avoid mutating
  // the host's link objects.
  useEffect(() => {
    if (isControlled && opts.controlled) {
      setInternal(ensureLinkIdsCopyOnWrite(opts.controlled));
    }
  }, [isControlled, opts.controlled, ensureLinkIdsCopyOnWrite]);

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
    (next: GraphData<N, L>) =>
      commit(
        isControlled
          ? ensureLinkIdsCopyOnWrite(next)
          : ensureLinkIdsInPlace(next),
      ),
    [commit, ensureLinkIdsInPlace, ensureLinkIdsCopyOnWrite, isControlled],
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
    (ids: ReadonlyArray<string | number>) => {
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

  // Stable object identity: the methods only change identity when
  // `isControlled` flips (which the host can't change at runtime
  // anyway — controlled-vs-uncontrolled is decided on first render).
  // `data` is what consumers should depend on for "did the graph
  // change" semantics. Without this memo, every render produced a
  // fresh object literal and the dozen `useCallback`s in the main
  // component that list `dataApi` in their dep array would all
  // re-create — defeating their memoisation entirely.
  return useMemo<GraphDataApi<N, L>>(
    () => ({
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
    }),
    [
      internal,
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
    ],
  );
}
