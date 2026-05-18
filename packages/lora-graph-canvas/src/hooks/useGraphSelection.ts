import { useCallback, useEffect, useMemo, useRef, useState } from "react";

export interface UseGraphSelectionOptions {
  mode?: "none" | "single" | "multi";
  onChange?: (selectedIds: Array<string | number>) => void;
}

export interface SelectionApi {
  /** Selected ids as a stable array. Identity flips on every mutation
   *  so it can drive React effect dep arrays. Backed by the same `Set`
   *  as `selectedSet`. */
  selected: ReadonlyArray<string | number>;
  /** O(1)-membership view of the same selection. Wrappers that test
   *  `selectedSet.has(id)` on every node/link must read this instead
   *  of the array — `selected.includes(id)` is O(N) and on a 10k
   *  selection the kapsule digest paid that cost per node per
   *  frame. */
  selectedSet: ReadonlySet<string | number>;
  isSelected(id: string | number): boolean;
  toggle(id: string | number, opts?: { additive?: boolean }): void;
  set(ids: ReadonlyArray<string | number> | ReadonlySet<string | number>): void;
  clear(): void;
}

const EMPTY_SET: ReadonlySet<string | number> = new Set();
const EMPTY_ARR: ReadonlyArray<string | number> = Object.freeze([]);

/** Read-only view that pretends to be a Set/array pair backed by one
 *  underlying mutation. We expose ids both as a Set (for the per-node
 *  `has` lookup in accessor wrappers) and as an array (for everything
 *  else: keybindings select-all, context menu enumeration, clipboard
 *  snapshot, fit-on-selection). Both views must flip identity together
 *  on every mutation so consumers' useEffect deps see the change. */
interface SelectionSnapshot {
  set: ReadonlySet<string | number>;
  arr: ReadonlyArray<string | number>;
}

function snapshotFrom(ids: Iterable<string | number>): SelectionSnapshot {
  const set = new Set<string | number>(ids);
  if (set.size === 0) return { set: EMPTY_SET, arr: EMPTY_ARR };
  // Spread once so the array shares the Set's iteration order and
  // remains structurally stable until the next snapshot.
  return { set, arr: Object.freeze([...set]) };
}

const EMPTY_SNAPSHOT: SelectionSnapshot = { set: EMPTY_SET, arr: EMPTY_ARR };

export function useGraphSelection(
  opts: UseGraphSelectionOptions,
): SelectionApi {
  const mode = opts.mode ?? "single";
  const [snapshot, setSnapshot] = useState<SelectionSnapshot>(EMPTY_SNAPSHOT);

  const onChangeRef = useRef(opts.onChange);
  onChangeRef.current = opts.onChange;

  // Skip the initial-mount onChange — empty selection on mount is
  // trivially known to the host.
  const firstRender = useRef(true);
  useEffect(() => {
    if (firstRender.current) {
      firstRender.current = false;
      return;
    }
    // `onChange` historically received an Array; preserve that shape.
    // `snapshot.arr` is already frozen, but consumers may mutate, so
    // slice into a fresh array.
    onChangeRef.current?.([...snapshot.arr]);
  }, [snapshot]);

  const toggle = useCallback(
    (id: string | number, toggleOpts?: { additive?: boolean }) => {
      if (mode === "none") return;
      setSnapshot((cur) => {
        const has = cur.set.has(id);
        if (mode === "single" || !toggleOpts?.additive) {
          // Plain click: collapse to just this id, unless we're
          // clicking the only selected node (deselect).
          if (has && cur.set.size === 1) return EMPTY_SNAPSHOT;
          return snapshotFrom([id]);
        }
        // Additive toggle. Build the new Set in O(1) — copy current,
        // add/remove the one id.
        const next = new Set(cur.set);
        if (has) next.delete(id);
        else next.add(id);
        return snapshotFrom(next);
      });
    },
    [mode],
  );

  const set = useCallback(
    (ids: ReadonlyArray<string | number> | ReadonlySet<string | number>) => {
      if (mode === "none") return;
      setSnapshot(() => {
        const seq =
          mode === "single"
            ? // Iterables don't have `.slice`; take the first id
              // generically.
              (() => {
                for (const id of ids) return [id];
                return [] as Array<string | number>;
              })()
            : ids;
        return snapshotFrom(seq);
      });
    },
    [mode],
  );

  const clear = useCallback(() => setSnapshot(EMPTY_SNAPSHOT), []);

  // Refs into the latest snapshot let `isSelected` keep a stable
  // function identity — without the ref, putting it in a useCallback
  // dep would defeat downstream memoisation every time the selection
  // changed (which is the entire point of the optimization).
  const snapshotRef = useRef(snapshot);
  snapshotRef.current = snapshot;
  const isSelected = useCallback(
    (id: string | number) => snapshotRef.current.set.has(id),
    [],
  );

  return useMemo<SelectionApi>(
    () => ({
      selected: snapshot.arr,
      selectedSet: snapshot.set,
      toggle,
      set,
      clear,
      isSelected,
    }),
    [snapshot, toggle, set, clear, isSelected],
  );
}
