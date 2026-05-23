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
  RowExportStats,
  RowFormat,
  RowImportStats,
  RowMapping,
} from "./types.js";
import { wrapError } from "./types.js";
import {
  WasmDatabase,
  init as wasmInit,
  snapshotInfo as nativeSnapshotInfo,
} from "./loader-node.js";
import { decodeResult } from "./decode.js";
import { createWorkerDatabase } from "./worker-client.js";
import type { WorkerDatabase, WorkerLike } from "./worker-client.js";
import type {
  GraphStatsSnapshot,
  MemoryReportSnapshot,
} from "./worker-protocol.js";
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
  DistinctValueRecord,
  GraphStatsSnapshot,
  IndexScope,
  LabelCount,
  MemoryReportSnapshot,
} from "./worker-protocol.js";
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

/**
 * Codec used for the snapshot body. Mirrors the Rust `Compression` enum.
 * `gzip` carries the encoder level recorded in the manifest.
 */
export type SnapshotCompressionInfo =
  | { format: "none" }
  | { format: "gzip"; level: number };

/**
 * Header context decoded from a snapshot binary. Extends {@link SnapshotMeta}
 * with envelope-level fields (`compression`, `encrypted`, `keyId`) that are
 * stored in the manifest but absent from `loadSnapshot`'s return value.
 *
 * Returned by {@link snapshotInfo}, which inspects only the envelope and so
 * works on encrypted snapshots without credentials.
 */
export interface SnapshotInfo extends SnapshotMeta {
  compression: SnapshotCompressionInfo;
  encrypted: boolean;
  keyId: string | null;
}

/**
 * Read header metadata from a snapshot binary without loading it into a
 * database. The envelope is decoded synchronously — no decryption or body
 * decompression — so this works on encrypted snapshots too.
 *
 * **Node-only.** Browser callers must use `Database.snapshotInfo` instead,
 * because the WASM module lives inside the Web Worker and the main-thread
 * loader is shimmed away by the bundler. This standalone form bootstraps the
 * Node-target WASM directly.
 *
 * Use this to preview imported `.lorasnap` files or to persist richer
 * metadata alongside cached snapshot blobs.
 */
export function snapshotInfo(bytes: Uint8Array): SnapshotInfo {
  try {
    ensureBootstrapped();
    return nativeSnapshotInfo(bytes) as SnapshotInfo;
  } catch (err) {
    throw wrapError(err);
  }
}

/**
 * Progress payload emitted by [`Database.importStream`] callbacks.
 * Mirrors the JSON the WASM cursor's `feed` method returns: counts
 * are cumulative across the whole stream.
 */
export interface RowImportProgress {
  /** Total bytes accepted by the decoder. */
  bytesFed: number;
  /** Total records the decoder has parsed (regardless of commit state). */
  rowsSeen: number;
  /** Records that have been committed via a flushed batch. */
  rowsCommitted: number;
  /** Number of batches flushed so far. */
  batches: number;
  /**
   * Cumulative count of records the decoder skipped because they
   * failed to parse. Always 0 when permissive mode is off.
   */
  skipped: number;
}

export interface ImportStreamOptions {
  /** Rows per Cypher batch. Defaults to 1000 on the WASM side. */
  batchSize?: number;
  /** Called after each chunk is fed; useful for progress UIs. */
  onProgress?: (progress: RowImportProgress) => void;
  /**
   * When true, every row is parsed and counted but no Cypher executes.
   * Useful for validating a mapping + checking the file parses before
   * committing to a mutation. Stats reflect what *would have* been
   * imported.
   */
  dryRun?: boolean;
  /**
   * When true, records that fail to parse are skipped and reported in
   * the final stats (`skipped` + `errors`) instead of aborting the
   * import. Defaults to false — the first bad record terminates the
   * stream with a structured error.
   */
  permissive?: boolean;
  /**
   * Cancel the in-flight import. When the signal aborts mid-stream,
   * the native cursor is closed (releasing the WASM cursor + any
   * in-flight batch) and the returned promise rejects with a
   * `LoraError` carrying code `"LORA_INTERNAL"` and the abort reason
   * as its message. Already-committed batches are kept — there is no
   * transactional rollback across the whole import.
   */
  signal?: AbortSignal;
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

/**
 * Normalise an [`AbortSignal`] cancellation into a `DOMException`
 * whose `name === "AbortError"` (the standard browser shape). We
 * accept the signal's `reason` when it's already an `Error`,
 * otherwise wrap a generic message.
 */
function abortError(signal: AbortSignal): Error {
  const reason: unknown = (signal as AbortSignal & { reason?: unknown }).reason;
  if (reason instanceof Error) return reason;
  const message =
    typeof reason === "string" && reason.length > 0 ? reason : "import aborted";
  // DOMException is available in both browsers and modern Node.
  if (typeof DOMException === "function") {
    return new DOMException(message, "AbortError");
  }
  const err = new Error(message);
  err.name = "AbortError";
  return err;
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
      // executeBuffer ships a single binary buffer instead of building
      // the JS object tree inside Rust + serialising it through
      // serde_wasm_bindgen value-by-value. We decode once on the JS
      // side in JIT'd code, which is materially faster.
      const native = this.#inner as unknown as {
        executeBuffer(query: string, params: unknown): Uint8Array;
      };
      const buf = native.executeBuffer(query, (params ?? null) as unknown);
      return decodeResult(buf) as QueryResult<T>;
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
    try {
      this.#inner.clear();
    } catch (err) {
      throw wrapError(err);
    }
  }

  async nodeCount(): Promise<number> {
    return this.#inner.nodeCount();
  }

  async relationshipCount(): Promise<number> {
    return this.#inner.relationshipCount();
  }

  async graphStats(): Promise<GraphStatsSnapshot> {
    const native = this.#inner as unknown as {
      graphStats(): GraphStatsSnapshot;
    };
    return native.graphStats();
  }

  async memoryReport(): Promise<MemoryReportSnapshot> {
    const native = this.#inner as unknown as {
      memoryReport(): MemoryReportSnapshot;
    };
    return native.memoryReport();
  }

  saveSnapshot(): Promise<Uint8Array>;
  saveSnapshot(options: WasmSnapshotByteOptions): Promise<Uint8Array>;
  saveSnapshot(
    options: { format?: "bytes" } & WasmSnapshotByteOptions,
  ): Promise<Uint8Array>;
  saveSnapshot(
    options: { format: "arrayBuffer" } & WasmSnapshotByteOptions,
  ): Promise<ArrayBuffer>;
  saveSnapshot(
    options: { format: "blob"; mimeType?: string } & WasmSnapshotByteOptions,
  ): Promise<Blob>;
  saveSnapshot(
    options: {
      format: "response";
      mimeType?: string;
    } & WasmSnapshotByteOptions,
  ): Promise<Response>;
  saveSnapshot(
    options: { format: "stream" } & WasmSnapshotByteOptions,
  ): Promise<ReadableStream<Uint8Array>>;
  saveSnapshot(
    options: { format: "url"; mimeType?: string } & WasmSnapshotByteOptions,
  ): Promise<URL>;
  /**
   * Serialize the current graph to a host-friendly snapshot object. WASM has
   * no filesystem access; callers persist the returned bytes/Blob/stream/URL
   * through host-provided storage.
   */
  async saveSnapshot(
    options?: WasmSnapshotSaveOptions | WasmSnapshotByteOptions,
  ): Promise<
    | Uint8Array
    | ArrayBuffer
    | Blob
    | Response
    | ReadableStream<Uint8Array>
    | URL
  > {
    try {
      const native = this.#inner as unknown as {
        saveSnapshot(options?: unknown): Uint8Array;
      };
      const bytes = native.saveSnapshot(options ?? null);
      const format =
        options && "format" in options ? (options.format ?? "bytes") : "bytes";
      const mimeType =
        options && "mimeType" in options ? options.mimeType : undefined;
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

  /**
   * Decode header metadata from a snapshot binary without loading it.
   * Resolves with the same shape as the standalone {@link snapshotInfo}
   * function, but uses the WASM instance already attached to this database
   * — so it works in browser bundles where the main-thread loader is
   * shimmed out.
   */
  async snapshotInfo(bytes: Uint8Array): Promise<SnapshotInfo> {
    try {
      return nativeSnapshotInfo(bytes) as SnapshotInfo;
    } catch (err) {
      throw wrapError(err);
    }
  }

  /**
   * Run a query and serialise its result rows as the chosen format.
   * Returns the encoded bytes plus a row count.
   *
   * Materialises the full encoded payload in WASM memory before
   * returning. For large exports prefer {@link openExportStream},
   * which yields chunks row-at-a-time without ever holding the whole
   * dataset in memory.
   */
  async exportRows(
    query: string,
    params: LoraParams | null | undefined,
    format: RowFormat,
  ): Promise<{ bytes: Uint8Array; stats: RowExportStats }> {
    try {
      const native = this.#inner as unknown as {
        exportRows(query: string, params: unknown, format: string): unknown;
      };
      return native.exportRows(query, params ?? null, format) as {
        bytes: Uint8Array;
        stats: RowExportStats;
      };
    } catch (err) {
      throw wrapError(err);
    }
  }

  /**
   * Open a streaming row-export cursor and wrap it as a Web
   * {@link ReadableStream}. The engine pulls rows one chunk at a
   * time and writes encoded bytes through the encoder; the JS side
   * pulls chunks one at a time and pushes them to the stream
   * controller. **At no point is the full encoded payload held in
   * memory** — the bound is the per-chunk encoder buffer (≈32–256 KiB).
   *
   * Cancel-aware: cancelling the returned stream closes the native
   * cursor so the engine releases its snapshot reference.
   */
  openExportStream(
    query: string,
    params: LoraParams | null | undefined,
    format: RowFormat,
  ): ReadableStream<Uint8Array> {
    let cursor: {
      next(): unknown;
      close(): void;
    } | null = null;
    const inner = this.#inner;
    return new ReadableStream<Uint8Array>({
      start() {
        try {
          const native = inner as unknown as {
            openExport(
              query: string,
              params: unknown,
              format: string,
            ): { next(): unknown; close(): void };
          };
          cursor = native.openExport(query, params ?? null, format);
        } catch (err) {
          throw wrapError(err);
        }
      },
      pull(controller) {
        if (!cursor) {
          controller.close();
          return;
        }
        try {
          const chunk = cursor.next() as Uint8Array | null;
          if (chunk === null) {
            cursor.close();
            cursor = null;
            controller.close();
            return;
          }
          controller.enqueue(chunk);
        } catch (err) {
          cursor?.close();
          cursor = null;
          controller.error(wrapError(err));
        }
      },
      cancel() {
        cursor?.close();
        cursor = null;
      },
    });
  }

  /**
   * Decode rows from `bytes` and apply them to the graph using the
   * auto-mapping path. The mapping renders a parameterised
   * `UNWIND $rows AS r CREATE …` template internally; rows are batched
   * and each batch executes as a single auto-committed statement.
   */
  async importRows(
    bytes: Uint8Array,
    format: RowFormat,
    mapping: RowMapping,
    batchSize?: number | null,
  ): Promise<RowImportStats> {
    try {
      const native = this.#inner as unknown as {
        importRows(
          bytes: Uint8Array,
          format: string,
          mapping: unknown,
          batchSize?: number | null,
        ): unknown;
      };
      return native.importRows(
        bytes,
        format,
        mapping,
        batchSize ?? null,
      ) as RowImportStats;
    } catch (err) {
      throw wrapError(err);
    }
  }

  /**
   * Decode rows from `bytes` and execute `template` once per batch with
   * `$rows` bound to the batch payload. Escape hatch for the auto-mapping
   * path: anything Cypher accepts is fair game here.
   */
  async importRowsWithCypher(
    bytes: Uint8Array,
    format: RowFormat,
    template: string,
    batchSize?: number | null,
  ): Promise<RowImportStats> {
    try {
      const native = this.#inner as unknown as {
        importRowsWithCypher(
          bytes: Uint8Array,
          format: string,
          template: string,
          batchSize?: number | null,
        ): unknown;
      };
      return native.importRowsWithCypher(
        bytes,
        format,
        template,
        batchSize ?? null,
      ) as RowImportStats;
    } catch (err) {
      throw wrapError(err);
    }
  }

  /**
   * Stream rows from a `ReadableStream<Uint8Array>` into the graph,
   * one chunk at a time. The chunk source can be a `File.stream()`,
   * a `fetch` response body, or any other Web stream.
   *
   * Memory bound: peak resident set is one chunk + one batch of
   * decoded rows. Files of arbitrary size are supported without ever
   * being loaded fully into memory on either the JS or the WASM side.
   *
   * `mappingOrTemplate` is either a {@link RowMapping} (auto-mapping)
   * or a Cypher template string with a `$rows` parameter (escape
   * hatch). All three row formats stream chunk-by-chunk.
   */
  async importStream(
    source: ReadableStream<Uint8Array>,
    format: RowFormat,
    mappingOrTemplate: RowMapping | string,
    options?: ImportStreamOptions,
  ): Promise<RowImportStats> {
    try {
      const native = this.#inner as unknown as {
        openImport(
          format: string,
          mappingOrTemplate: unknown,
          batchSize: number | null,
          dryRun: boolean | null,
          permissive: boolean | null,
        ): {
          feed(chunk: Uint8Array): unknown;
          finish(): unknown;
          close(): void;
        };
      };
      const cursor = native.openImport(
        format,
        mappingOrTemplate,
        options?.batchSize ?? null,
        options?.dryRun ?? null,
        options?.permissive ?? null,
      );
      const signal = options?.signal;
      if (signal?.aborted) {
        cursor.close();
        throw abortError(signal);
      }
      try {
        const reader = source.getReader();
        try {
          while (true) {
            if (signal?.aborted) {
              await reader.cancel();
              cursor.close();
              throw abortError(signal);
            }
            const { value, done } = await reader.read();
            if (done) break;
            const progress = cursor.feed(value);
            options?.onProgress?.(progress as RowImportProgress);
          }
        } finally {
          reader.releaseLock();
        }
        const stats = cursor.finish() as RowImportStats;
        return stats;
      } catch (err) {
        cursor.close();
        throw err;
      }
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

function requestedRuntime(
  options?: CreateDatabaseOptions,
): "auto" | "worker" | "main-thread" {
  return options?.runtime ?? "auto";
}

function shouldTryDefaultWorker(options?: CreateDatabaseOptions): boolean {
  const runtime = requestedRuntime(options);
  return runtime !== "main-thread" && typeof Worker !== "undefined";
}

function shouldFallbackToMainThread(options?: CreateDatabaseOptions): boolean {
  return requestedRuntime(options) === "auto";
}

function warnWorkerFallback(
  err: unknown,
  options?: CreateDatabaseOptions,
): void {
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
