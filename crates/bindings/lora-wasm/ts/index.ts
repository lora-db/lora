/**
 * lora-wasm — typed WebAssembly bindings for the Lora graph engine.
 *
 * This entry targets Node.js (ESM) and browser bundlers. In browser-like
 * hosts, `createDatabase()` tries to start the packaged Web Worker first so
 * query work stays off the main thread. If that fails, it warns once and
 * falls back to the in-process WASM engine.
 *
 * **Initialization is async-only.** The one canonical entry point is
 * `createDatabase()`; the WASM module is bootstrapped inside it before the
 * first query runs. There is no synchronous constructor.
 *
 *   import { createDatabase } from "lora-wasm";
 *
 *   const db = await createDatabase();
 *   const res = await db.execute("CREATE (:N {n: $v}) RETURN 1 AS one", { v: 1 });
 */

import type {
  LoraParams,
  LoraValue,
  QueryResult,
} from "./types.js";
import { wrapError } from "./types.js";
import { WasmDatabase, init as wasmInit } from "./loader-node.js";
import { createWorkerDatabase } from "./worker-client.js";
import type { WorkerDatabase, WorkerLike } from "./worker-client.js";
import {
  snapshotAsArrayBuffer,
  snapshotAsBlob,
  snapshotAsObjectUrl,
  snapshotAsReadableStream,
  snapshotAsResponse,
  readSnapshotSource,
} from "./snapshot.js";
import type {
  WasmSnapshotByteOptions,
  WasmSnapshotLoadOptions,
  WasmSnapshotSaveOptions,
  WasmSnapshotSource,
} from "./snapshot.js";

export * from "./types.js";
export {
  createWorkerDatabase,
  type WorkerDatabase,
  type WorkerLike,
} from "./worker-client.js";
export type {
  WasmSnapshotByteOptions,
  WasmSnapshotCompression,
  WasmSnapshotEncryption,
  WasmSnapshotLoadOptions,
  WasmSnapshotPasswordParams,
  WasmSnapshotSaveFormat,
  WasmSnapshotSaveOptions,
  WasmSnapshotSource,
} from "./snapshot.js";

/**
 * Metadata returned by `loadSnapshot`.
 * Mirrors the Rust `SnapshotMeta` struct. WASM saves the database snapshot
 * codec and accepts legacy store snapshots on load for compatibility.
 */
export interface SnapshotMeta {
  formatVersion: number;
  nodeCount: number;
  relationshipCount: number;
  walLsn: number | null;
}

export interface CreateDatabaseOptions {
  /**
   * Select where the WASM engine runs.
   *
   * - `"auto"` tries a Web Worker first when available, then falls back to
   *   the main thread.
   * - `"worker"` requires a Web Worker and rejects if startup fails.
   * - `"main-thread"` skips Worker startup and runs the engine in-process.
   *
   * Defaults to `"auto"`.
   */
  runtime?: "auto" | "worker" | "main-thread";
  /**
   * Emit `console.warn` if worker startup fails and the factory falls back to
   * the main-thread WASM engine in `"auto"` mode. Defaults to `true`.
   */
  warnOnFallback?: boolean;
}

export type TransactionMode =
  | "read_write"
  | "read_only"
  | "readwrite"
  | "readonly"
  | "rw"
  | "ro";

export interface TransactionStatement {
  query: string;
  params?: LoraParams | null;
}

export interface RowStream<
  T extends Record<string, LoraValue> = Record<string, LoraValue>,
> extends AsyncIterableIterator<T> {
  columns(): string[] | Promise<string[]>;
  toArray(): Promise<T[]>;
  close(): void;
}

interface NativeQueryStream {
  columns(): unknown;
  next(): unknown;
  close(): void;
}

class NativeRowStream<
  T extends Record<string, LoraValue> = Record<string, LoraValue>,
> implements RowStream<T> {
  readonly #inner: NativeQueryStream;
  #closed = false;

  constructor(inner: NativeQueryStream) {
    this.#inner = inner;
  }

  [Symbol.asyncIterator](): AsyncIterableIterator<T> {
    return this;
  }

  columns(): string[] {
    try {
      return this.#inner.columns() as string[];
    } catch (err) {
      throw wrapError(err);
    }
  }

  async next(): Promise<IteratorResult<T>> {
    if (this.#closed) {
      return { done: true, value: undefined };
    }
    try {
      const row = this.#inner.next() as T | null;
      if (row === null) {
        this.#closed = true;
        return { done: true, value: undefined };
      }
      return { done: false, value: row };
    } catch (err) {
      this.#closed = true;
      throw wrapError(err);
    }
  }

  async return(): Promise<IteratorResult<T>> {
    this.close();
    return { done: true, value: undefined };
  }

  async toArray(): Promise<T[]> {
    const rows: T[] = [];
    for (;;) {
      const next = await this.next();
      if (next.done) {
        return rows;
      }
      rows.push(next.value);
    }
  }

  close(): void {
    if (this.#closed) return;
    this.#closed = true;
    try {
      this.#inner.close();
    } catch (err) {
      throw wrapError(err);
    }
  }
}

let bootstrapped = false;
function ensureBootstrapped(): void {
  if (bootstrapped) return;
  wasmInit();
  bootstrapped = true;
}

/**
 * In-memory Lora graph database running on the WASM engine.
 *
 * Obtained exclusively via `createDatabase()`. Queries still execute
 * synchronously inside WASM, so for heavy queries in the browser prefer
 * `createWorkerDatabase()`; every method returns a Promise for API symmetry
 * with `lora-node` and the Worker variant.
 */
class DatabaseImpl {
  readonly #inner: InstanceType<typeof WasmDatabase>;

  constructor(inner: InstanceType<typeof WasmDatabase>) {
    this.#inner = inner;
  }

  async execute<
    T extends Record<string, LoraValue> = Record<string, LoraValue>,
  >(query: string, params?: LoraParams): Promise<QueryResult<T>> {
    try {
      const raw = this.#inner.execute(query, (params ?? null) as unknown);
      return raw as QueryResult<T>;
    } catch (err) {
      throw wrapError(err);
    }
  }

  stream<T extends Record<string, LoraValue> = Record<string, LoraValue>>(
    query: string,
    params?: LoraParams,
  ): RowStream<T> {
    try {
      const native = this.#inner as unknown as {
        openStream(query: string, params: unknown): NativeQueryStream;
      };
      return new NativeRowStream<T>(native.openStream(query, params ?? null));
    } catch (err) {
      throw wrapError(err);
    }
  }

  rows<T extends Record<string, LoraValue> = Record<string, LoraValue>>(
    query: string,
    params?: LoraParams,
  ): RowStream<T> {
    return this.stream<T>(query, params);
  }

  async transaction<
    T extends Record<string, LoraValue> = Record<string, LoraValue>,
  >(
    statements: TransactionStatement[],
    mode: TransactionMode = "read_write",
  ): Promise<Array<QueryResult<T>>> {
    try {
      const native = this.#inner as unknown as {
        transaction(statements: unknown, mode: TransactionMode): unknown;
      };
      return native.transaction(statements, mode) as Array<QueryResult<T>>;
    } catch (err) {
      throw wrapError(err);
    }
  }

  async clear(): Promise<void> {
    this.#inner.clear();
  }

  async nodeCount(): Promise<number> {
    return this.#inner.nodeCount();
  }

  async relationshipCount(): Promise<number> {
    return this.#inner.relationshipCount();
  }

  saveSnapshot(): Promise<Uint8Array>;
  saveSnapshot(options: WasmSnapshotByteOptions): Promise<Uint8Array>;
  saveSnapshot(options: { format?: "bytes" } & WasmSnapshotByteOptions): Promise<Uint8Array>;
  saveSnapshot(options: { format: "arrayBuffer" } & WasmSnapshotByteOptions): Promise<ArrayBuffer>;
  saveSnapshot(options: { format: "blob"; mimeType?: string } & WasmSnapshotByteOptions): Promise<Blob>;
  saveSnapshot(options: { format: "response"; mimeType?: string } & WasmSnapshotByteOptions): Promise<Response>;
  saveSnapshot(options: { format: "stream" } & WasmSnapshotByteOptions): Promise<ReadableStream<Uint8Array>>;
  saveSnapshot(options: { format: "url"; mimeType?: string } & WasmSnapshotByteOptions): Promise<URL>;
  /**
   * Serialize the current graph to a host-friendly snapshot object. WASM has
   * no filesystem access; callers persist the returned bytes/Blob/stream/URL
   * through host-provided storage.
   */
  async saveSnapshot(
    options?: WasmSnapshotSaveOptions | WasmSnapshotByteOptions,
  ): Promise<Uint8Array | ArrayBuffer | Blob | Response | ReadableStream<Uint8Array> | URL> {
    try {
      const native = this.#inner as unknown as {
        saveSnapshot(options?: unknown): Uint8Array;
      };
      const bytes = native.saveSnapshot(options ?? null);
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
    } catch (err) {
      throw wrapError(err);
    }
  }

  async loadSnapshot(
    source: WasmSnapshotSource,
    options?: WasmSnapshotLoadOptions,
  ): Promise<SnapshotMeta> {
    try {
      const bytes = await readSnapshotSource(source);
      const native = this.#inner as unknown as {
        loadSnapshot(bytes: Uint8Array, options?: unknown): unknown;
      };
      return native.loadSnapshot(bytes, options ?? null) as SnapshotMeta;
    } catch (err) {
      throw wrapError(err);
    }
  }

  /** Release the underlying wasm handle. Subsequent calls will throw. */
  dispose(): void {
    this.#inner.free();
  }
}

/**
 * Public type for a LoraDB instance backed by WASM.
 *
 * Exported as a type only — there is no runtime `Database` value. To obtain
 * an instance, always use `createDatabase()`.
 */
export type Database = DatabaseImpl | WorkerDatabase;

let warnedWorkerFallback = false;

function requestedRuntime(options?: CreateDatabaseOptions): "auto" | "worker" | "main-thread" {
  return options?.runtime ?? "auto";
}

function shouldTryDefaultWorker(options?: CreateDatabaseOptions): boolean {
  const runtime = requestedRuntime(options);
  return runtime !== "main-thread" && typeof Worker !== "undefined";
}

function shouldFallbackToMainThread(options?: CreateDatabaseOptions): boolean {
  return requestedRuntime(options) === "auto";
}

function warnWorkerFallback(err: unknown, options?: CreateDatabaseOptions): void {
  if (options?.warnOnFallback === false || warnedWorkerFallback) return;
  warnedWorkerFallback = true;
  const detail = err instanceof Error ? err.message : String(err);
  console.warn(
    `[lora-wasm] Web Worker startup failed; falling back to main-thread WASM. ${detail}`,
  );
}

function createDefaultWorker(): WorkerLike {
  return new Worker(new URL("./worker.js", import.meta.url), {
    type: "module",
  }) as WorkerLike;
}

/**
 * Create and initialize a new in-memory LoraDB instance on the WASM engine.
 *
 * In browser-like hosts this factory tries the packaged Web Worker first,
 * pings it, and returns the worker-backed database when startup succeeds.
 * If worker construction or bootstrap fails it warns once and falls back to
 * the main-thread WASM engine. Pass `{ runtime: "main-thread" }` to force the
 * in-process engine, or `{ runtime: "worker" }` to require a Worker.
 *
 * ```ts
 * import { createDatabase } from "lora-wasm";
 *
 * const db = await createDatabase();
 * const res = await db.execute("MATCH (n) RETURN count(n) AS n");
 * ```
 *
 * Use `createMainThreadDatabase()` when you explicitly want the in-process
 * WASM engine, or `createWorkerDatabase(worker)` when you need to supply a
 * custom Worker instance.
 */
export async function createMainThreadDatabase(): Promise<DatabaseImpl> {
  ensureBootstrapped();
  return new DatabaseImpl(new WasmDatabase());
}

export async function createDatabase(
  options: CreateDatabaseOptions = {},
): Promise<Database> {
  if (shouldTryDefaultWorker(options)) {
    let worker: WorkerLike | null = null;
    try {
      worker = createDefaultWorker();
      const db = createWorkerDatabase(worker);
      await db.nodeCount();
      return db;
    } catch (err) {
      try {
        worker?.terminate();
      } catch {
        // best-effort cleanup after a failed worker startup
      }
      if (!shouldFallbackToMainThread(options)) {
        throw wrapError(err);
      }
      warnWorkerFallback(err, options);
    }
  }
  if (requestedRuntime(options) === "worker") {
    throw wrapError(new Error("WORKER_ERROR: Web Worker is not available"));
  }
  return createMainThreadDatabase();
}
