/**
 * lora-node — typed Node.js binding for the Lora graph engine.
 *
 * Thin wrapper over the N-API module. Query execution is delegated to the
 * native layer, which runs each query on the libuv threadpool so the JS
 * event loop stays responsive. This file only narrows the `unknown`-valued
 * native surface into the strongly-typed `QueryResult<T>` / `LoraParams`
 * shapes defined in the shared TS contract (`crates/shared-ts/types.ts`).
 *
 * **Initialization is async-only.** The canonical entry point is
 * `createDatabase(...)`, optionally with archive-backed persistence. There is no
 * synchronous constructor. See the docs at
 * https://loradb.com/docs/getting-started/node for the full rationale.
 */

import { Buffer } from "node:buffer";
import { Readable } from "node:stream";
import { fileURLToPath } from "node:url";

import type {
  LoraParams,
  LoraValue,
  QueryResult,
} from "./types.js";
import { LoraError, wrapError } from "./types.js";

// eslint-disable-next-line @typescript-eslint/ban-ts-comment
// @ts-ignore - resolved at runtime via native.js loader
import native from "./native.js";

const NativeDatabase: typeof import("./native.js").Database = native.Database;

export * from "./types.js";

/**
 * Metadata returned by `saveSnapshot` / `loadSnapshot`. Mirrors the Rust
 * `SnapshotMeta` struct and matches the shape used by every other binding
 * (Python, WASM, Go, FFI) so snapshots can be described in the same way
 * regardless of language.
 */
export interface SnapshotMeta {
  formatVersion: number;
  nodeCount: number;
  relationshipCount: number;
  walLsn: number | null;
}

export type NodeSnapshotChunk = string | Uint8Array | ArrayBuffer | Buffer;
export type NodeSnapshotSource =
  | string
  | URL
  | Uint8Array
  | ArrayBuffer
  | Buffer
  | Readable
  | ReadableStream<NodeSnapshotChunk>
  | AsyncIterable<NodeSnapshotChunk>;
export type NodeSnapshotSaveFormat = "binary" | "base64";
export type NodeSnapshotSaveOptions =
  | { format: "path"; path: string | URL }
  | { format: NodeSnapshotSaveFormat };
export type NodeSnapshotSaveTarget =
  | string
  | URL
  | NodeSnapshotSaveFormat
  | NodeSnapshotSaveOptions;

export interface CreateDatabaseOptions {
  databaseDir?: string;
  /**
   * Durability mode for archive-backed databases.
   *
   * - `group` writes WAL bytes before `execute()` resolves and batches fsyncs
   *   on a short background cadence. This is the default.
   * - `perCommit` fsyncs every committed write before `execute()` resolves.
   */
  syncMode?: "group" | "perCommit";
  /**
   * Background fsync cadence for `syncMode: "group"`, in milliseconds.
   *
   * Defaults to 1000. Must be greater than zero.
   */
  groupSyncIntervalMs?: number;
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
  columns(): string[];
  toArray(): Promise<T[]>;
  close(): void;
}

class NativeRowStream<
  T extends Record<string, LoraValue> = Record<string, LoraValue>,
> implements RowStream<T> {
  readonly #inner: InstanceType<typeof NativeDatabase>;
  readonly #streamId: number;
  #closed = false;

  constructor(inner: InstanceType<typeof NativeDatabase>, streamId: number) {
    this.#inner = inner;
    this.#streamId = streamId;
  }

  [Symbol.asyncIterator](): AsyncIterableIterator<T> {
    return this;
  }

  columns(): string[] {
    try {
      return this.#inner.streamColumns(this.#streamId);
    } catch (err) {
      throw wrapError(err);
    }
  }

  async next(): Promise<IteratorResult<T>> {
    if (this.#closed) {
      return { done: true, value: undefined };
    }
    try {
      const row = this.#inner.streamNext(this.#streamId) as T | null;
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
      this.#inner.streamClose(this.#streamId);
    } catch (err) {
      throw wrapError(err);
    }
  }
}

function isFetchUrl(url: URL): boolean {
  return url.protocol === "http:" || url.protocol === "https:" || url.protocol === "data:";
}

function stringToUrl(value: string): URL | null {
  try {
    return new URL(value);
  } catch {
    return null;
  }
}

function bytesToUint8Array(bytes: Uint8Array | ArrayBuffer | Buffer): Uint8Array {
  if (bytes instanceof Uint8Array) {
    return bytes;
  }
  return new Uint8Array(bytes);
}

function snapshotChunkToBuffer(chunk: NodeSnapshotChunk): Buffer {
  return typeof chunk === "string"
    ? Buffer.from(chunk)
    : Buffer.from(bytesToUint8Array(chunk));
}

function resolveNodeSnapshotPath(path: string | URL): string {
  if (path instanceof URL) {
    if (path.protocol !== "file:") {
      throw new Error(`LORA_ERROR: unsupported snapshot save URL protocol '${path.protocol}'`);
    }
    return fileURLToPath(path);
  }
  return path;
}

async function fetchSnapshotBytes(url: URL): Promise<Buffer> {
  const res = await fetch(url);
  if (!res.ok) {
    throw new Error(`LORA_ERROR: snapshot fetch failed (${res.status} ${res.statusText})`);
  }
  return Buffer.from(await res.arrayBuffer());
}

function isReadableStream(source: unknown): source is ReadableStream<NodeSnapshotChunk> {
  return typeof (source as { getReader?: unknown }).getReader === "function";
}

function isAsyncIterable(source: unknown): source is AsyncIterable<NodeSnapshotChunk> {
  return typeof (source as { [Symbol.asyncIterator]?: unknown })[Symbol.asyncIterator] === "function";
}

async function readableStreamToBuffer(
  stream: ReadableStream<NodeSnapshotChunk>,
): Promise<Buffer> {
  const reader = stream.getReader();
  const chunks: Buffer[] = [];
  try {
    for (;;) {
      const { done, value } = await reader.read();
      if (done) {
        return Buffer.concat(chunks);
      }
      chunks.push(snapshotChunkToBuffer(value));
    }
  } finally {
    reader.releaseLock();
  }
}

async function asyncIterableToBuffer(
  stream: AsyncIterable<NodeSnapshotChunk>,
): Promise<Buffer> {
  const chunks: Buffer[] = [];
  for await (const chunk of stream) {
    chunks.push(snapshotChunkToBuffer(chunk));
  }
  return Buffer.concat(chunks);
}

async function resolveNodeSnapshotSource(
  source: NodeSnapshotSource,
): Promise<string | Buffer> {
  if (source instanceof URL) {
    if (source.protocol === "file:") {
      return fileURLToPath(source);
    }
    if (isFetchUrl(source)) {
      return fetchSnapshotBytes(source);
    }
    throw new Error(`LORA_ERROR: unsupported snapshot URL protocol '${source.protocol}'`);
  }

  if (typeof source === "string") {
    const url = stringToUrl(source);
    if (!url) {
      return source;
    }
    if (url.protocol === "file:") {
      return fileURLToPath(url);
    }
    if (isFetchUrl(url)) {
      return fetchSnapshotBytes(url);
    }
    return source;
  }

  if (isReadableStream(source)) {
    return readableStreamToBuffer(source);
  }

  if (source instanceof Readable) {
    return asyncIterableToBuffer(source as AsyncIterable<NodeSnapshotChunk>);
  }

  if (isAsyncIterable(source)) {
    return asyncIterableToBuffer(source);
  }

  return Buffer.from(bytesToUint8Array(source));
}

/**
 * Lora graph database instance.
 *
 * Obtained exclusively via `createDatabase()`. There is no public
 * constructor and no synchronous factory. With no args the instance is
 * purely in-memory; with a database name and `databaseDir` it replays committed
 * WAL state from the serialized `.loradb` path under `databaseDir` before
 * serving queries.
 *
 * Instances are independent — each owns its own in-memory graph. Multiple
 * concurrent read-only `execute()` calls against one instance can share
 * the store read lock; writes serialize without blocking the event loop.
 */
class DatabaseImpl {
  readonly #inner: InstanceType<typeof NativeDatabase>;

  constructor(inner: InstanceType<typeof NativeDatabase>) {
    this.#inner = inner;
  }

  /**
   * Execute a Lora query on the libuv threadpool. Returns a Promise that
   * resolves with `{ columns, rows }`; errors surface as `LoraError`
   * with a narrowed `code` (`LORA_ERROR` / `INVALID_PARAMS`).
   */
  async execute<
    T extends Record<string, LoraValue> = Record<string, LoraValue>,
  >(query: string, params?: LoraParams): Promise<QueryResult<T>> {
    try {
      const raw = await this.#inner.execute(query, params ?? null);
      return raw as QueryResult<T>;
    } catch (err) {
      throw wrapError(err);
    }
  }

  /**
   * Return an async row iterator for a query.
   *
   * The binding exposes the same `for await` shape as the Rust stream API.
   * Rows are materialized by the native `execute()` promise today, then
   * yielded one at a time to JS consumers.
   */
  stream<T extends Record<string, LoraValue> = Record<string, LoraValue>>(
    query: string,
    params?: LoraParams,
  ): RowStream<T> {
    try {
      const streamId = this.#inner.openStream(query, params ?? null);
      return new NativeRowStream<T>(this.#inner, streamId);
    } catch (err) {
      throw wrapError(err);
    }
  }

  /** Alias for `stream()`, useful when naming the row-level API explicitly. */
  rows<T extends Record<string, LoraValue> = Record<string, LoraValue>>(
    query: string,
    params?: LoraParams,
  ): RowStream<T> {
    return this.stream<T>(query, params);
  }

  /**
   * Execute a statement batch inside one native transaction.
   *
   * Results are returned in statement order. If any statement fails, the
   * native transaction is dropped before commit and all prior writes in the
   * batch are rolled back.
   */
  async transaction<
    T extends Record<string, LoraValue> = Record<string, LoraValue>,
  >(
    statements: TransactionStatement[],
    mode: TransactionMode = "read_write",
  ): Promise<Array<QueryResult<T>>> {
    try {
      return (await this.#inner.transaction(statements, mode)) as Array<QueryResult<T>>;
    } catch (err) {
      throw wrapError(err);
    }
  }

  /**
   * Force pending WAL bytes and archive updates to disk.
   *
   * `syncMode: "perCommit"` fsyncs each committed write before `execute()`
   * resolves. The default `group` mode writes WAL bytes before `execute()`
   * resolves and batches fsyncs for speed; call `sync()` when you need an
   * immediate fsync point and a current portable `.loradb` archive, for example
   * before copying it elsewhere.
   */
  async sync(): Promise<void> {
    try {
      await this.#inner.sync();
    } catch (err) {
      throw wrapError(err);
    }
  }

  /** Drop every node and relationship and persist the clear when WAL-backed. */
  async clear(): Promise<void> {
    try {
      await this.#inner.clear();
    } catch (err) {
      throw wrapError(err);
    }
  }

  /** Number of nodes currently in the graph. */
  async nodeCount(): Promise<number> {
    try {
      return this.#inner.nodeCount();
    } catch (err) {
      throw wrapError(err);
    }
  }

  /** Number of relationships currently in the graph. */
  async relationshipCount(): Promise<number> {
    try {
      return this.#inner.relationshipCount();
    } catch (err) {
      throw wrapError(err);
    }
  }

  /**
   * Release the native database handle. Idempotent.
   *
   * Call this when an archive-backed database needs to be reopened in the same
   * process. New operations after disposal reject with `database is closed`.
   */
  dispose(): void {
    try {
      this.#inner.dispose();
    } catch (err) {
      throw wrapError(err);
    }
  }

  saveSnapshot(format: "binary"): Promise<Buffer>;
  saveSnapshot(format: "base64"): Promise<string>;
  saveSnapshot(path: string | URL): Promise<SnapshotMeta>;
  saveSnapshot(options: { format: "binary" }): Promise<Buffer>;
  saveSnapshot(options: { format: "base64" }): Promise<string>;
  saveSnapshot(options: { format: "path"; path: string | URL }): Promise<SnapshotMeta>;
  /**
   * Save the graph as a snapshot.
   *
   * - `saveSnapshot(path)` and `saveSnapshot({ format: "path", path })`
   *   write atomically to a local file and return `SnapshotMeta`.
   * - `saveSnapshot("binary")` / `{ format: "binary" }` return a `Buffer`.
   * - `saveSnapshot("base64")` / `{ format: "base64" }` return base64 text.
   */
  async saveSnapshot(
    target: NodeSnapshotSaveTarget,
  ): Promise<SnapshotMeta | Buffer | string> {
    try {
      if (target === "binary" || (typeof target === "object"
        && !(target instanceof URL)
        && target.format === "binary")) {
        return this.saveSnapshotToBytes();
      }

      if (target === "base64" || (typeof target === "object"
        && !(target instanceof URL)
        && target.format === "base64")) {
        return (await this.saveSnapshotToBytes()).toString("base64");
      }

      let path: string | URL;
      if (typeof target === "object" && !(target instanceof URL)) {
        if (target.format !== "path") {
          throw new Error(`LORA_ERROR: unsupported snapshot save format '${target.format}'`);
        }
        path = target.path;
      } else {
        path = target;
      }
      return this.#inner.saveSnapshot(resolveNodeSnapshotPath(path)) as SnapshotMeta;
    } catch (err) {
      throw wrapError(err);
    }
  }

  async saveSnapshotToBytes(): Promise<Buffer> {
    try {
      return this.#inner.saveSnapshotToBytes();
    } catch (err) {
      throw wrapError(err);
    }
  }

  /**
   * Replace the current graph state with a snapshot loaded from `path`.
   * Concurrent `execute()` calls block on the store write lock until the
   * load completes.
   */
  async loadSnapshot(source: NodeSnapshotSource): Promise<SnapshotMeta> {
    try {
      const resolved = await resolveNodeSnapshotSource(source);
      if (typeof resolved === "string") {
        return this.#inner.loadSnapshot(resolved) as SnapshotMeta;
      }
      return this.#inner.loadSnapshotFromBytes(resolved) as SnapshotMeta;
    } catch (err) {
      throw wrapError(err);
    }
  }

  async loadSnapshotFromBytes(
    bytes: Uint8Array | ArrayBuffer | Buffer,
  ): Promise<SnapshotMeta> {
    try {
      return this.#inner.loadSnapshotFromBytes(
        Buffer.from(bytesToUint8Array(bytes)),
      ) as SnapshotMeta;
    } catch (err) {
      throw wrapError(err);
    }
  }
}

/**
 * Public type for a LoraDB instance.
 *
 * Exported as a type only — there is no runtime `Database` value. To obtain
 * an instance, always use `createDatabase()`:
 *
 * ```ts
 * import { createDatabase, type Database } from "lora-node";
 *
 * const db: Database = await createDatabase();
 * ```
 */
export type Database = DatabaseImpl;

/**
 * Create and initialize a new LoraDB instance.
 *
 * **This is the only supported initialization pattern** for `lora-node`.
 * Synchronous construction is not available — the async factory guarantees
 * the native layer is ready before the first query dispatches.
 *
 * ```ts
 * import { createDatabase } from "lora-node";
 *
 * const db = await createDatabase(); // in-memory by default
 * const res = await db.execute(
 *   "CREATE (n:Person {name: $name}) RETURN n",
 *   { name: "Alice" },
 * );
 * ```
 *
 * Optional Node-only persistence convenience:
 *
 * ```ts
 * import { createDatabase } from "lora-node";
 *
 * const inMemory = await createDatabase();          // in-memory
 * const defaultPersistent = await createDatabase("app"); // ./app.loradb
 * const nestedPersistent = await createDatabase("app", {
 *   databaseDir: "./data",
 * });                                               // ./data/app.loradb
 * ```
 *
 * Passing a database name enables persistence. The database name is validated
 * and resolved under `databaseDir`, or the current working directory when no
 * directory is supplied, appending `.loradb` to the basename when needed.
 * Relative paths resolve from the current working directory.
 *
 * Persistent opens for the same resolved archive path in one Node process share
 * a single live native engine. Cross-process opens are blocked by the archive
 * lock to prevent split-brain writers.
 */
export async function createDatabase(
  databaseName?: string,
  options: CreateDatabaseOptions = {},
): Promise<Database> {
  try {
    const hasPersistenceOptions =
      options.databaseDir !== undefined ||
      options.syncMode !== undefined ||
      options.groupSyncIntervalMs !== undefined;
    if (databaseName == null && hasPersistenceOptions) {
      throw new LoraError(
        "databaseName is required when persistence options are provided",
        "INVALID_PARAMS",
      );
    }
    const syncMode = options.syncMode ?? null;
    const groupSyncIntervalMs = options.groupSyncIntervalMs ?? null;
    return new DatabaseImpl(
      databaseName == null
        ? new NativeDatabase()
        : new NativeDatabase(
            databaseName,
            options.databaseDir ?? null,
            syncMode,
            groupSyncIntervalMs,
          ),
    );
  } catch (err) {
    throw wrapError(err);
  }
}
