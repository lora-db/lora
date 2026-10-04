// The time and parallelism one operation may use. `timeoutMs` bounds each
// statement; an operation with many root fields (aliases) could still run
// for timeoutMs × fields and occupy every libuv worker at once. The
// budget gives the operation one deadline and a bounded number of
// statements in flight; past the deadline its statements are aborted and
// its fields fail with TIMEOUT.

import { requestError } from "../errors.js";

/** Default `maxConcurrentStatements`. */
export const MAX_CONCURRENT_STATEMENTS = 2;
/** Default `operationTimeoutMs`, as a multiple of `timeoutMs`. */
export const OPERATION_TIMEOUT_FACTOR = 2;

export class OperationBudget {
  readonly #controller = new AbortController();
  readonly #deadline: number;
  readonly #timeoutMs: number;
  readonly #timer: ReturnType<typeof setTimeout> | undefined;
  readonly #maxConcurrent: number;
  readonly #waiting: Array<() => void> = [];
  readonly #expired: Promise<never>;
  #running = 0;
  /** Root fields of the operation in flight. */
  active = 0;

  constructor(timeoutMs: number, maxConcurrent: number) {
    this.#timeoutMs = timeoutMs;
    this.#maxConcurrent = Math.max(1, maxConcurrent);
    this.#deadline =
      timeoutMs > 0 && Number.isFinite(timeoutMs)
        ? performance.now() + timeoutMs
        : Infinity;
    let expire!: (err: unknown) => void;
    this.#expired = new Promise<never>((_, reject) => (expire = reject));
    // A budget nobody races against must not report an unhandled rejection.
    this.#expired.catch(() => undefined);
    if (this.#deadline !== Infinity) {
      this.#timer = setTimeout(() => {
        const error = this.#timeoutError();
        this.#controller.abort(error);
        expire(error);
        for (const wake of this.#waiting.splice(0)) wake();
      }, timeoutMs);
      (this.#timer as { unref?: () => void }).unref?.();
    }
  }

  get signal(): AbortSignal {
    return this.#controller.signal;
  }

  /** Milliseconds left before the deadline (Infinity without one). */
  remaining(): number {
    return Math.max(0, this.#deadline - performance.now());
  }

  /**
   * Run `fn` once a statement slot is free, failing with TIMEOUT when the
   * deadline passes first (the statement keeps its own abort signal).
   */
  async run<T>(fn: (signal: AbortSignal) => Promise<T>): Promise<T> {
    while (this.#running >= this.#maxConcurrent && !this.signal.aborted) {
      await new Promise<void>((wake) => this.#waiting.push(wake));
    }
    if (this.signal.aborted) throw this.#timeoutError();
    this.#running++;
    try {
      return await Promise.race([fn(this.signal), this.#expired]);
    } finally {
      this.#running--;
      this.#waiting.shift()?.();
    }
  }

  /** Stop the deadline timer: the operation is over. */
  dispose(): void {
    if (this.#timer !== undefined) clearTimeout(this.#timer);
  }

  #timeoutError() {
    return requestError(
      "TIMEOUT",
      `the operation ran longer than ${this.#timeoutMs} ms; ask for less, or split it into several requests`,
      undefined,
      { operationTimeoutMs: this.#timeoutMs },
    );
  }
}

/** One signal aborted when either is; `b` alone when `a` is absent. */
export function anySignal(
  a: AbortSignal | undefined,
  b: AbortSignal,
): AbortSignal {
  if (!a) return b;
  if (typeof AbortSignal.any === "function") return AbortSignal.any([a, b]);
  // Node before 20.3.
  const both = new AbortController();
  const abort = (s: AbortSignal) => () => both.abort(s.reason);
  if (a.aborted) both.abort(a.reason);
  else if (b.aborted) both.abort(b.reason);
  else {
    a.addEventListener("abort", abort(a), { once: true });
    b.addEventListener("abort", abort(b), { once: true });
  }
  return both.signal;
}
