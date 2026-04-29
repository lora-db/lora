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
import type { Request, Response as WorkerResponse } from "./worker-protocol.js";
import type {
  WasmSnapshotByteOptions,
  WasmSnapshotLoadOptions,
  WasmSnapshotSaveOptions,
  WasmSnapshotSource,
  RowStream,
  SnapshotMeta,
  TransactionMode,
  TransactionStatement,
} from "./index.js";
import {
  snapshotAsArrayBuffer,
  snapshotAsBlob,
  snapshotAsObjectUrl,
  snapshotAsReadableStream,
  snapshotAsResponse,
  readSnapshotSource,
} from "./snapshot.js";

export interface WorkerLike {
  postMessage(message: unknown): void;
  terminate(): void;
  addEventListener(type: "message", listener: (event: { data: WorkerResponse }) => void): void;
  addEventListener(type: "error", listener: (event: { message?: string }) => void): void;
  removeEventListener(type: "message", listener: (event: { data: WorkerResponse }) => void): void;
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
  saveSnapshot(): Promise<Uint8Array>;
  saveSnapshot(options: WasmSnapshotByteOptions): Promise<Uint8Array>;
  saveSnapshot(options: { format?: "bytes" } & WasmSnapshotByteOptions): Promise<Uint8Array>;
  saveSnapshot(options: { format: "arrayBuffer" } & WasmSnapshotByteOptions): Promise<ArrayBuffer>;
  saveSnapshot(options: { format: "blob"; mimeType?: string } & WasmSnapshotByteOptions): Promise<Blob>;
  saveSnapshot(options: { format: "response"; mimeType?: string } & WasmSnapshotByteOptions): Promise<Response>;
  saveSnapshot(options: { format: "stream" } & WasmSnapshotByteOptions): Promise<ReadableStream<Uint8Array>>;
  saveSnapshot(options: { format: "url"; mimeType?: string } & WasmSnapshotByteOptions): Promise<URL>;
  loadSnapshot(source: WasmSnapshotSource, options?: WasmSnapshotLoadOptions): Promise<SnapshotMeta>;
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

  function saveSnapshot(): Promise<Uint8Array>;
  function saveSnapshot(options: WasmSnapshotByteOptions): Promise<Uint8Array>;
  function saveSnapshot(options: { format?: "bytes" } & WasmSnapshotByteOptions): Promise<Uint8Array>;
  function saveSnapshot(options: { format: "arrayBuffer" } & WasmSnapshotByteOptions): Promise<ArrayBuffer>;
  function saveSnapshot(options: { format: "blob"; mimeType?: string } & WasmSnapshotByteOptions): Promise<Blob>;
  function saveSnapshot(options: { format: "response"; mimeType?: string } & WasmSnapshotByteOptions): Promise<Response>;
  function saveSnapshot(options: { format: "stream" } & WasmSnapshotByteOptions): Promise<ReadableStream<Uint8Array>>;
  function saveSnapshot(options: { format: "url"; mimeType?: string } & WasmSnapshotByteOptions): Promise<URL>;
  async function saveSnapshot(
    options?: WasmSnapshotSaveOptions | WasmSnapshotByteOptions,
  ): Promise<Uint8Array | ArrayBuffer | Blob | Response | ReadableStream<Uint8Array> | URL> {
    const bytes = await call<Uint8Array>({ op: "saveSnapshot", options: options ?? null });
    const format = options && "format" in options ? options.format ?? "bytes" : "bytes";
    const mimeType = options && "mimeType" in options ? options.mimeType : undefined;
    switch (format) {
      case "bytes":
        return bytes;
      case "arrayBuffer":
        return snapshotAsArrayBuffer(bytes);
      case "blob":
        return snapshotAsBlob(bytes, mimeType);
      case "response":
        return snapshotAsResponse(bytes, mimeType);
      case "stream":
        return snapshotAsReadableStream(bytes);
      case "url":
        return snapshotAsObjectUrl(bytes, mimeType);
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
    saveSnapshot,
    async loadSnapshot(
      source: WasmSnapshotSource,
      options?: WasmSnapshotLoadOptions,
    ): Promise<SnapshotMeta> {
      return call<SnapshotMeta>({
        op: "loadSnapshot",
        bytes: await readSnapshotSource(source),
        options: options ?? null,
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
