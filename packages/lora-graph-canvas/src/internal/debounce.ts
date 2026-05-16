// Trailing-edge debounce — calls inside the wait window reset the
// timer; the wrapped function fires once after the last call.
// Used internally by our `kapsule` port (kapsule.ts) for the
// digest queue. Tiny standalone implementation so we don't have to
// pull `lodash-es` for one helper.

export interface Debounced<F extends (...args: never[]) => unknown> {
  (...args: Parameters<F>): void;
  cancel: () => void;
}

export function debounce<F extends (...args: never[]) => unknown>(
  fn: F,
  wait: number,
): Debounced<F> {
  let timeoutId: ReturnType<typeof setTimeout> | null = null;
  let lastArgs: Parameters<F> | null = null;

  const debounced = ((...args: Parameters<F>): void => {
    lastArgs = args;
    if (timeoutId !== null) clearTimeout(timeoutId);
    timeoutId = setTimeout(() => {
      timeoutId = null;
      const a = lastArgs!;
      lastArgs = null;
      fn(...a);
    }, wait);
  }) as Debounced<F>;

  debounced.cancel = () => {
    if (timeoutId !== null) {
      clearTimeout(timeoutId);
      timeoutId = null;
    }
    lastArgs = null;
  };

  return debounced;
}
