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
import type {
  RowStream,
  SnapshotMeta,
  WasmSnapshotSaveOptions,
  TransactionMode,
  TransactionStatement,
} from "./index.js";
import {
  downloadSnapshotBytes,
  resolveSnapshotSaveFormat,
  snapshotBytesToBase64,
  snapshotBytesToBlob,
  snapshotSourceToBytes,
} from "./snapshot.js";
import type { WasmSnapshotSource } from "./snapshot.js";

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
  stream<T extends Record<string, LoraValue> = Record<string, LoraValue>>(
    query: string,
    params?: LoraParams,
  ): RowStream<T>;
  rows<T extends Record<string, LoraValue> = Record<string, LoraValue>>(
    query: string,
    params?: LoraParams,
  ): RowStream<T>;
  transaction<T extends Record<string, LoraValue> = Record<string, LoraValue>>(
    statements: TransactionStatement[],
    mode?: TransactionMode,
  ): Promise<Array<QueryResult<T>>>;
  saveSnapshotToBytes(): Promise<Uint8Array>;
  saveSnapshot(): Promise<Uint8Array>;
  saveSnapshot(format: "binary"): Promise<Uint8Array>;
  saveSnapshot(format: "base64"): Promise<string>;
  saveSnapshot(format: "blob"): Promise<Blob>;
  saveSnapshot(format: "download"): Promise<void>;
  saveSnapshot(options: { format: "binary" }): Promise<Uint8Array>;
  saveSnapshot(options: { format: "base64" }): Promise<string>;
  saveSnapshot(options: { format: "blob"; mimeType?: string }): Promise<Blob>;
  saveSnapshot(options: { format: "download"; filename?: string; mimeType?: string }): Promise<void>;
  loadSnapshotFromBytes(bytes: Uint8Array): Promise<SnapshotMeta>;
  loadSnapshot(source: WasmSnapshotSource): Promise<SnapshotMeta>;
  clear(): Promise<void>;
  nodeCount(): Promise<number>;
  relationshipCount(): Promise<number>;
  dispose(): Promise<void>;
}

interface Pending {
  resolve(value: unknown): void;
  reject(err: Error): void;
}

class WorkerRowStream<
  T extends Record<string, LoraValue> = Record<string, LoraValue>,
> implements RowStream<T> {
  readonly #open: () => Promise<{ streamId: number; columns: string[] }>;
  readonly #call: <R>(body: Request["body"]) => Promise<R>;
  #state: Promise<{ streamId: number; columns: string[] }> | null = null;
  #closed = false;

  constructor(
    open: () => Promise<{ streamId: number; columns: string[] }>,
    call: <R>(body: Request["body"]) => Promise<R>,
  ) {
    this.#open = open;
    this.#call = call;
  }

  [Symbol.asyncIterator](): AsyncIterableIterator<T> {
    return this;
  }

  columns(): Promise<string[]> {
    return this.#ensureOpen().then((state) => state.columns);
  }

  async next(): Promise<IteratorResult<T>> {
    if (this.#closed) {
      return { done: true, value: undefined };
    }
    const { streamId } = await this.#ensureOpen();
    const row = await this.#call<T | null>({ op: "streamNext", streamId });
    if (row === null) {
      this.#closed = true;
      return { done: true, value: undefined };
    }
    return { done: false, value: row };
  }

  async return(): Promise<IteratorResult<T>> {
    this.close();
    return { done: true, value: undefined };
  }

  async toArray(): Promise<T[]> {
    const rows: T[] = [];
    for (;;) {
      const next = await this.next();
      if (next.done) return rows;
      rows.push(next.value);
    }
  }

  close(): void {
    if (this.#closed) return;
    this.#closed = true;
    if (this.#state) {
      void this.#state.then(({ streamId }) => {
        return this.#call<null>({ op: "streamClose", streamId });
      });
    }
  }

  #ensureOpen(): Promise<{ streamId: number; columns: string[] }> {
    if (!this.#state) {
      this.#state = this.#open();
    }
    return this.#state;
  }
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

  async function saveSnapshot(
    target?: WasmSnapshotSaveOptions["format"] | WasmSnapshotSaveOptions,
  ): Promise<Uint8Array | string | Blob | void> {
    const options = resolveSnapshotSaveFormat(target);
    const bytes = await call<Uint8Array>({ op: "saveSnapshotToBytes" });

    switch (options.format) {
      case "binary":
        return bytes;
      case "base64":
        return snapshotBytesToBase64(bytes);
      case "blob":
        return snapshotBytesToBlob(bytes, options.mimeType);
      case "download":
        return downloadSnapshotBytes(bytes, options.filename, options.mimeType);
    }
  }

  return {
    execute<T extends Record<string, LoraValue> = Record<string, LoraValue>>(
      query: string,
      params?: LoraParams,
    ): Promise<QueryResult<T>> {
      return call<QueryResult<T>>({ op: "execute", query, params: params ?? null });
    },
    stream<T extends Record<string, LoraValue> = Record<string, LoraValue>>(
      query: string,
      params?: LoraParams,
    ): RowStream<T> {
      return new WorkerRowStream<T>(
        () => {
          return call<{ streamId: number; columns: string[] }>({
            op: "streamOpen",
            query,
            params: params ?? null,
          });
        },
        call,
      );
    },
    rows<T extends Record<string, LoraValue> = Record<string, LoraValue>>(
      query: string,
      params?: LoraParams,
    ): RowStream<T> {
      return this.stream<T>(query, params);
    },
    transaction<T extends Record<string, LoraValue> = Record<string, LoraValue>>(
      statements: TransactionStatement[],
      mode: TransactionMode = "read_write",
    ): Promise<Array<QueryResult<T>>> {
      return call<Array<QueryResult<T>>>({ op: "transaction", statements, mode });
    },
    saveSnapshotToBytes(): Promise<Uint8Array> {
      return call<Uint8Array>({ op: "saveSnapshotToBytes" });
    },
    saveSnapshot: saveSnapshot as WorkerDatabase["saveSnapshot"],
    loadSnapshotFromBytes(bytes: Uint8Array): Promise<SnapshotMeta> {
      return call<SnapshotMeta>({ op: "loadSnapshotFromBytes", bytes });
    },
    async loadSnapshot(source: WasmSnapshotSource): Promise<SnapshotMeta> {
      return call<SnapshotMeta>({
        op: "loadSnapshotFromBytes",
        bytes: await snapshotSourceToBytes(source),
      });
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
