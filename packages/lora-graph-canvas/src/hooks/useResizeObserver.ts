import { useEffect, useState, type RefObject } from "react";

/** Reports the content rect of `ref.current`. Returns null on the
 *  initial render. Re-fires whenever the observed element resizes. */
export function useResizeObserver(
  ref: RefObject<HTMLElement | null>,
): { width: number; height: number } | null {
  const [size, setSize] = useState<{ width: number; height: number } | null>(
    null,
  );

  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    // ResizeObserver fires synchronously after layout; we capture the
    // initial size on first fire so callers don't need a separate
    // measurement pass.
    const ro = new ResizeObserver((entries) => {
      const entry = entries[0];
      if (!entry) return;
      const cr = entry.contentRect;
      setSize({ width: cr.width, height: cr.height });
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, [ref]);

  return size;
}
