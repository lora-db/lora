/**
 * Minimal async helpers used by the persistence and store layers.
 *
 * `debounce` is trailing-edge only with a manual `cancel()` so callers can
 * tear it down on unmount; `sleep` is a typed convenience wrapper around
 * `setTimeout`.
 */

/**
 * Returns a debounced wrapper around `fn` that defers invocation until `ms`
 * milliseconds have elapsed since the last call. The returned function has
 * a `cancel()` method that drops any pending invocation.
 */
export function debounce<T extends (...args: never[]) => void>(
  fn: T,
  ms: number,
): T & { cancel: () => void } {
  let timer: ReturnType<typeof setTimeout> | null = null;
  let lastArgs: Parameters<T> | null = null;

  const wrapped = ((...args: Parameters<T>) => {
    lastArgs = args;
    if (timer !== null) clearTimeout(timer);
    timer = setTimeout(() => {
      timer = null;
      const callArgs = lastArgs;
      lastArgs = null;
      if (callArgs) fn(...callArgs);
    }, ms);
  }) as T & { cancel: () => void };

  wrapped.cancel = () => {
    if (timer !== null) {
      clearTimeout(timer);
      timer = null;
    }
    lastArgs = null;
  };

  return wrapped;
}

/** Resolves after `ms` milliseconds. */
export function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}
