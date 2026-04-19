/**
 * Main-thread client for the WASM-in-a-Worker architecture.
 *
 * All queries are posted as messages and awaited as promises; the heavy work
 * runs off the main thread. The API mirrors the in-process `Database` class
 * so consumers can choose the execution model without rewriting their code.
 */

import type {
  LoraParams,
  LoraValue,
  QueryResult,
} from "./types.js";
import { LoraError } from "./types.js";
import type { Request, Response } from "./worker-protocol.js";

export interface WorkerLike {
  postMessage(message: unknown): void;
  terminate(): void;
  addEventListener(type: "message", listener: (event: { data: Response }) => void): void;
  addEventListener(type: "error", listener: (event: { message?: string }) => void): void;
  removeEventListener(type: "message", listener: (event: { data: Response }) => void): void;
}

export interface WorkerDatabase {
  execute<T extends Record<string, LoraValue> = Record<string, LoraValue>>(
    query: string,
    params?: LoraParams,
  ): Promise<QueryResult<T>>;
  clear(): Promise<void>;
  nodeCount(): Promise<number>;
  relationshipCount(): Promise<number>;
  dispose(): Promise<void>;
}

interface Pending {
  resolve(value: unknown): void;
  reject(err: Error): void;
}

/**
 * Build a Database client that proxies to the supplied worker.
 *
 * Typical browser usage:
 * ```ts
 * const worker = new Worker(new URL("./worker.js", import.meta.url), { type: "module" });
 * const db = createWorkerDatabase(worker);
 * const result = await db.execute("MATCH (n) RETURN n");
 * ```
 */
export function createWorkerDatabase(worker: WorkerLike): WorkerDatabase {
  let nextId = 1;
  const pending = new Map<number, Pending>();

  worker.addEventListener("message", (event) => {
    const { id, body } = event.data;
    const p = pending.get(id);
    if (!p) return;
    pending.delete(id);
    if (body.ok) {
      p.resolve(body.result);
    } else {
      p.reject(new LoraError(body.error.message, body.error.code));
    }
  });

  worker.addEventListener("error", (event) => {
    const message = event.message ?? "worker errored";
    for (const p of pending.values()) {
      p.reject(new LoraError(message, "WORKER_ERROR"));
    }
    pending.clear();
  });

  function call<R>(body: Request["body"]): Promise<R> {
    const id = nextId++;
    return new Promise<R>((resolve, reject) => {
      pending.set(id, { resolve: resolve as (v: unknown) => void, reject });
      const msg: Request = { id, body };
      worker.postMessage(msg);
    });
  }

  return {
    execute<T extends Record<string, LoraValue> = Record<string, LoraValue>>(
      query: string,
      params?: LoraParams,
    ): Promise<QueryResult<T>> {
      return call<QueryResult<T>>({ op: "execute", query, params: params ?? null });
    },
    async clear(): Promise<void> {
      await call<null>({ op: "clear" });
    },
    nodeCount(): Promise<number> {
      return call<number>({ op: "nodeCount" });
    },
    relationshipCount(): Promise<number> {
      return call<number>({ op: "relationshipCount" });
    },
    async dispose(): Promise<void> {
      await call<null>({ op: "dispose" });
      worker.terminate();
    },
  };
}
