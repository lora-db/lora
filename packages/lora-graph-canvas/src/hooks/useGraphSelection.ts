import { useCallback, useEffect, useRef, useState } from "react";

export interface UseGraphSelectionOptions {
  mode?: "none" | "single" | "multi";
  onChange?: (selectedIds: Array<string | number>) => void;
}

export interface SelectionApi {
  selected: Array<string | number>;
  isSelected(id: string | number): boolean;
  toggle(id: string | number, opts?: { additive?: boolean }): void;
  set(ids: Array<string | number>): void;
  clear(): void;
}

export function useGraphSelection(
  opts: UseGraphSelectionOptions,
): SelectionApi {
  const mode = opts.mode ?? "single";
  const [selected, setSelected] = useState<Array<string | number>>([]);

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
    onChangeRef.current?.(selected);
  }, [selected]);

  const toggle = useCallback(
    (id: string | number, toggleOpts?: { additive?: boolean }) => {
      if (mode === "none") return;
      setSelected((cur) => {
        const has = cur.includes(id);
        if (mode === "single" || !toggleOpts?.additive) {
          if (has && cur.length === 1) return [];
          return [id];
        }
        return has ? cur.filter((x) => x !== id) : [...cur, id];
      });
    },
    [mode],
  );

  const set = useCallback(
    (ids: Array<string | number>) => {
      if (mode === "none") return;
      setSelected(mode === "single" ? ids.slice(0, 1) : ids);
    },
    [mode],
  );

  const clear = useCallback(() => setSelected([]), []);

  const isSelected = useCallback(
    (id: string | number) => selected.includes(id),
    [selected],
  );

  return { selected, toggle, set, clear, isSelected };
}
