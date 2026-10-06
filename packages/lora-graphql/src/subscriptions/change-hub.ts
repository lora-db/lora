// Where committed writes go: the `onWrite` listeners, the `changes()`
// iterators and the subscriptions' dispatcher, fed by this instance's
// mutations or, with `changeFeed`, by the engine's change feed.

import type { LoraDriver } from "../driver.js";
import { requestError } from "../errors.js";
import type { WriteChange } from "../execute/changes.js";
import { EngineFeed } from "../execute/feed.js";
import type { GraphModel } from "../model/types.js";
import type { LoraGraphQLOptions } from "../options.js";

export interface ChangeHubOptions {
  driver: LoraDriver;
  model: GraphModel;
  /** The `changeFeed` option. */
  changeFeed: boolean | undefined;
  /** The `maxQueuedChanges` option, defaulted. */
  maxQueued: number;
  onError: LoraGraphQLOptions["onError"];
  /**
   * The owning instance's public `onWrite`: `changes()` and the
   * subscriptions register through it, like any other listener.
   */
  onWrite: (listener: (change: WriteChange) => void) => () => void;
}

export class ChangeHub {
  readonly #maxQueued: number;
  readonly #onWrite: ChangeHubOptions["onWrite"];
  readonly #listeners = new Set<(change: WriteChange) => void>();
  /** Consumers of `changes()` when the engine feed is on. */
  readonly #feedListeners = new Set<(change: WriteChange) => void>();
  readonly #feed: EngineFeed | undefined;
  #feedReady: Promise<void> | undefined;

  constructor(options: ChangeHubOptions) {
    this.#maxQueued = options.maxQueued;
    this.#onWrite = options.onWrite;
    if (options.changeFeed) {
      if (!options.driver.changes) {
        throw new Error(
          "changeFeed needs a driver with changes() (@loradb/lora-node)",
        );
      }
      this.#feed = new EngineFeed(
        options.driver,
        options.model,
        (change) => {
          change.timestamp ??= new Date().toISOString();
          for (const l of this.#feedListeners) {
            try {
              l(change);
            } catch {
              // A consumer's failure must not stop the feed.
            }
          }
        },
        (err) =>
          options.onError?.({
            id: globalThis.crypto.randomUUID(),
            field: "changeFeed",
            message: err instanceof Error ? err.message : String(err),
            error: err,
          }),
      );
    }
  }

  /** Called after every committed mutation with its exact write-set. */
  onWrite(listener: (change: WriteChange) => void): () => void {
    this.#listeners.add(listener);
    return () => this.#listeners.delete(listener);
  }

  /**
   * Register a consumer of committed writes: of the engine feed when it
   * is on, else of this instance's mutations. Returns its removal.
   */
  listen(listener: (change: WriteChange) => void): () => unknown {
    if (this.#feed) {
      this.#feedListeners.add(listener);
      return () => this.#feedListeners.delete(listener);
    }
    return this.#onWrite(listener);
  }

  /**
   * Committed writes as an async iterator, e.g. for a subscription
   * resolver. Only writes made through this instance are seen. `return()`
   * or aborting `signal` (already aborted included) ends it at once.
   */
  changes(
    options: { signal?: AbortSignal; maxQueued?: number } = {},
  ): AsyncIterableIterator<WriteChange> {
    return this.queue<WriteChange>(
      (listener) => this.listen(listener),
      options,
    );
  }

  /**
   * An async iterator over the items a `register`ed listener receives,
   * queued up to `maxQueued`; `register` returns the unregister call.
   */
  queue<T>(
    register: (listener: (item: T) => void) => () => void,
    options: { signal?: AbortSignal; maxQueued?: number },
  ): AsyncIterableIterator<T> {
    const limit = options.maxQueued ?? this.#maxQueued;
    const queue: T[] = [];
    let wake: (() => void) | undefined;
    let done = false;
    let overflow = false;
    const listener = (change: T) => {
      // A consumer that falls this far behind is ended with an error,
      // instead of holding every write in memory.
      if (queue.length >= limit) {
        overflow = true;
        stop();
      } else {
        queue.push(change);
      }
      wake?.();
    };
    const unregister = register(listener);
    const stop = () => void unregister();
    let ready: Promise<void> | undefined;
    if (this.#feed) {
      this.#feedReady ??= this.#feed.start();
      ready = this.#feedReady;
    }
    const finish = () => {
      done = true;
      stop();
      options.signal?.removeEventListener("abort", finish);
      wake?.();
    };
    if (options.signal?.aborted) finish();
    else options.signal?.addEventListener("abort", finish, { once: true });
    const iterator: AsyncIterableIterator<T> = {
      [Symbol.asyncIterator]: () => iterator,
      next: async () => {
        if (ready) await ready;
        while (queue.length === 0 && !done && !overflow) {
          await new Promise<void>((resolve) => (wake = resolve));
          wake = undefined;
        }
        if (queue.length > 0) return { value: queue.shift()!, done: false };
        if (overflow && !done) {
          done = true;
          throw requestError(
            "LIMIT_EXCEEDED",
            `the subscriber fell ${limit} changes behind; resubscribe and reload`,
          );
        }
        return { value: undefined, done: true };
      },
      return: async () => {
        finish();
        return { value: undefined, done: true };
      },
    };
    return iterator;
  }

  /** Hand a committed mutation's write-set to the `onWrite` listeners. */
  emit(change: WriteChange): void {
    change.timestamp ??= new Date().toISOString();
    for (const listener of this.#listeners) {
      try {
        listener(change);
      } catch {
        // A listener's failure must not fail a committed write.
      }
    }
  }

  /** Stop the engine change feed (with `changeFeed: true`). */
  close(): void {
    this.#feed?.stop();
    this.#feedReady = undefined;
  }
}
