// Trailing-edge throttle with `.flush()` + `.cancel()`. Replaces our
// only use of `lodash-es` (`throttle` in the in-tree force-graph-2d
// kapsule), so we can drop the dep without losing behaviour.
//
// Contract:
//   - Calls inside the window enqueue the latest args; the wrapped
//     function then fires once at the trailing edge.
//   - `.flush()` invokes the pending call synchronously and clears
//     the timer. (Used to force an immediate shadow-canvas repaint
//     when pointer-area paints change.)
//   - `.cancel()` drops the pending call without invoking it.

export interface Throttled<F extends (...args: never[]) => unknown> {
  (...args: Parameters<F>): ReturnType<F> | undefined;
  flush: () => ReturnType<F> | undefined;
  cancel: () => void;
}

export function throttle<F extends (...args: never[]) => unknown>(
  fn: F,
  wait: number,
): Throttled<F> {
  let lastArgs: Parameters<F> | null = null;
  let lastResult: ReturnType<F> | undefined;
  let timeoutId: ReturnType<typeof setTimeout> | null = null;
  let lastInvokeTime = 0;

  const invoke = (time: number) => {
    if (!lastArgs) return undefined;
    const args = lastArgs;
    lastArgs = null;
    lastInvokeTime = time;
    lastResult = fn(...args) as ReturnType<F>;
    return lastResult;
  };

  const throttled = ((...args: Parameters<F>): ReturnType<F> | undefined => {
    const now = Date.now();
    lastArgs = args;
    const remaining = wait - (now - lastInvokeTime);
    if (remaining <= 0 || remaining > wait) {
      if (timeoutId !== null) {
        clearTimeout(timeoutId);
        timeoutId = null;
      }
      return invoke(now);
    }
    if (timeoutId === null) {
      timeoutId = setTimeout(() => {
        timeoutId = null;
        invoke(Date.now());
      }, remaining);
    }
    return lastResult;
  }) as Throttled<F>;

  throttled.flush = () => {
    if (timeoutId === null) return lastResult;
    clearTimeout(timeoutId);
    timeoutId = null;
    return invoke(Date.now());
  };

  throttled.cancel = () => {
    if (timeoutId !== null) {
      clearTimeout(timeoutId);
      timeoutId = null;
    }
    lastArgs = null;
    lastInvokeTime = 0;
  };

  return throttled;
}
