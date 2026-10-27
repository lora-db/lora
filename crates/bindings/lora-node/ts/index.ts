/**
 * lora-node — typed Node.js binding for the Lora graph engine.
 *
 * Thin wrapper over the N-API module. Query execution is delegated to the
 * native layer, which runs each query on the libuv threadpool so the JS
 * event loop stays responsive. This file only narrows the `unknown`-valued
 * native surface into the strongly-typed `QueryResult<T>` / `LoraParams`
 * shapes defined in the shared TS contract (`crates/bindings/shared-ts/types.ts`).
 *
 * **Initialization is async-only.** The canonical entry point is
 * `createDatabase(...)`, optionally with container-backed persistence. There is no
 * synchronous constructor. See the docs at
 * https://loradb.com/docs/getting-started/node for the full rationale.
 */

import { Buffer } from "node:buffer";
import { Readable } from "node:stream";
import { fileURLToPath } from "node:url";

import type {
  LoraChangeBatch,
  LoraParams,
  LoraQueryPlan,
  LoraQueryProfile,
  LoraValue,
  QueryResult,
} from "./types.js";
import { LoraError, wrapError } from "./types.js";

import { decodeResult } from "./decode.js";

// eslint-disable-next-line @typescript-eslint/ban-ts-comment
// @ts-ignore - resolved at runtime via native.js loader
import native from "./native.js";

type NativeDatabase = import("./native.js").Database;
const openNativeDatabase: typeof import("./native.js").openDatabase =
  native.openDatabase;

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

/**
 * What `db.snapshot(targetDir)` wrote: an online copy of a named database.
 */
export interface DatabaseSnapshot {
  /** The `.loradb` written under the target directory. */
  path: string;
  /** Its size in bytes. */
  bytes: number;
  /**
   * The WAL position the copy is taken at: it holds every transaction
   * committed at or below this LSN and none above it. The change feed
   * batch with this `lsn` is the last write it contains.
   */
  lsn: number;
  nodeCount: number;
  relationshipCount: number;
}

export type SnapshotCompression =
  | "none"
  | "gzip"
  | { format: "none" }
  | { format: "gzip"; level?: number };

export interface SnapshotPasswordParams {
  memoryCostKib?: number;
  timeCost?: number;
  parallelism?: number;
}

export type SnapshotEncryption =
  | {
      type?: "password" | "passphrase";
      keyId?: string;
      password: string;
      params?: SnapshotPasswordParams;
    }
  | {
      type: "key" | "rawKey" | "raw_key";
      keyId?: string;
      key: number[];
    };

export interface SnapshotCodecOptions {
  compression?: SnapshotCompression;
  encryption?: SnapshotEncryption | null;
}

export interface SnapshotLoadOptions {
  credentials?: SnapshotEncryption | null;
  encryption?: SnapshotEncryption | null;
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
export type NodeSnapshotSaveFormat =
  | "buffer"
  | "binary"
  | "uint8Array"
  | "arrayBuffer"
  | "base64"
  | "stream"
  | "path";
export type NodeSnapshotSaveOptions =
  | ({ format?: "buffer" | "binary" } & SnapshotCodecOptions)
  | ({ format: "uint8Array" } & SnapshotCodecOptions)
  | ({ format: "arrayBuffer" } & SnapshotCodecOptions)
  | ({ format: "base64" } & SnapshotCodecOptions)
  | ({ format: "stream" } & SnapshotCodecOptions)
  | ({ format: "path"; path: string | URL } & SnapshotCodecOptions);
export type NodeSnapshotSaveTarget =
  | string
  | URL
  | NodeSnapshotSaveOptions
  | SnapshotCodecOptions;

export interface CreateDatabaseOptions {
  databaseDir?: string;
  /**
   * Durability mode for container-backed databases.
   *
   * - `groupSync` writes WAL bytes before `execute()` resolves and batches
   *   fsyncs on a short background cadence. This is the default.
   */
  syncMode?: "groupSync";
  /**
   * Background fsync cadence for `syncMode: "groupSync"`, in milliseconds.
   *
   * Defaults to 1000. Must be greater than zero.
   */
  groupSyncIntervalMs?: number;
  /**
   * Default timeout, in milliseconds, for every `execute()`, `stream()`
   * and `transaction()` call on this database that does not pass its own
   * `timeoutMs`. A query that runs past it rejects with `LORA_TIMEOUT`,
   * rolls back, and releases its locks. Omit or pass 0 for no default.
   */
  queryTimeoutMs?: number;
  /**
   * Most transactions and write streams this database keeps open at once.
   * Each open `begin()` transaction and each write stream that waits for
   * the writer lock (see `stream()`) runs on a thread of its own; past
   * this many, the next one rejects at once with `LORA_TOO_MANY_OPEN`
   * instead of starting yet another thread. A slot frees up when its
   * transaction commits or rolls back, or its stream ends or is closed.
   * Read-write `transaction()` batches have a budget of the same size of
   * their own: past it they wait their turn (first in, first out), and
   * the wait counts against their `timeoutMs` and `signal`. Defaults to
   * 1024.
   */
  maxOpenTransactions?: number;
}

/**
 * Per-call limits for `execute()`, `stream()` and `transaction()`.
 */
export interface QueryOptions {
  /**
   * Abort the query after this many milliseconds. It rejects with a
   * `LoraError` coded `LORA_TIMEOUT`; writes are rolled back and locks
   * released. Overrides the database-wide `queryTimeoutMs`; 0 disables it.
   */
  timeoutMs?: number;
  /**
   * Cancel the query when this signal aborts. The query stops at its next
   * check point, rolls back, releases its locks, and the call rejects with
   * the signal's reason (an `AbortError` by default).
   */
  signal?: AbortSignal;
}

export interface WalDatabaseOptions {
  /**
   * Explicit WAL directory. Use `openWalDatabase` for this lower-level
   * persistent storage shape.
   * Pair with `snapshotDir` for managed snapshots.
   */
  walDir: string;
  /** Directory for managed checkpoint snapshots. Requires `walDir`. */
  snapshotDir?: string;
  /**
   * Automatically checkpoint after this many committed WAL transactions.
   * Omit or pass 0 to keep checkpoints manual.
   */
  snapshotEveryCommits?: number;
  /** Number of older managed snapshot files to retain. Defaults to 1. */
  snapshotKeepOld?: number;
  /** Compression/encryption options used for managed snapshots. */
  snapshotOptions?: SnapshotCodecOptions;
  /** Durability mode for WAL commits. */
  syncMode?: "groupSync";
  /** Background fsync cadence for `syncMode: "groupSync"`, in milliseconds. */
  groupSyncIntervalMs?: number;
  /** See `CreateDatabaseOptions.maxOpenTransactions`. Defaults to 1024. */
  maxOpenTransactions?: number;
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

/**
 * Native cleanup for handles a program drops without closing them: a
 * stream never drained or closed, a transaction never committed or rolled
 * back, a change feed never closed. A mutating stream or a read-write
 * transaction holds the database's writer lock, so before this an
 * abandoned one blocked every later write for good.
 *
 * Each cleanup is built by a module-level function so that it captures
 * only what it needs, never the handle it cleans up after (a closure that
 * kept the handle alive would keep it from being collected).
 */
const finalizers = new FinalizationRegistry<() => void>((cleanup) => {
  try {
    cleanup();
  } catch {
    // Already gone: the database was disposed.
  }
});

function streamCleanup(
  inner: NativeDatabase,
  streamId: number,
  release: (() => void) | undefined,
): () => void {
  return () => {
    release?.();
    inner.streamClose(streamId);
  };
}

function transactionCleanup(inner: NativeDatabase, id: number): () => void {
  return () => {
    inner.txFinish(id, false).catch(() => undefined);
  };
}

function feedCleanup(native: {
  inner: NativeDatabase;
  id: number | undefined;
}): () => void {
  return () => {
    if (native.id !== undefined) native.inner.changesClose(native.id);
  };
}

/**
 * Native handles still alive, so that their transactions and streams can be
 * closed when the process exits. Otherwise a write still waiting for the
 * writer lock on a libuv worker would wait forever, and libuv joins its
 * workers on exit: `process.exit()` hung.
 */
const liveDatabases = new Set<WeakRef<NativeDatabase>>();
const forgetDatabase = new FinalizationRegistry<WeakRef<NativeDatabase>>(
  (ref) => liveDatabases.delete(ref),
);
let exitHookInstalled = false;

function trackDatabase(inner: NativeDatabase): void {
  const ref = new WeakRef(inner);
  liveDatabases.add(ref);
  forgetDatabase.register(inner, ref);
  if (exitHookInstalled) return;
  exitHookInstalled = true;
  process.once("exit", () => {
    for (const live of liveDatabases) {
      try {
        live.deref()?.closeWriters();
      } catch {
        // Disposed.
      }
    }
  });
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
  readonly #inner: NativeDatabase;
  readonly #streamId: number;
  readonly #signal: AbortSignal | undefined;
  /**
   * A mutating stream opens (waiting for the writer lock) and is pulled on
   * its own native thread, so the event loop never waits for the lock.
   */
  readonly #async: boolean;
  #release: (() => void) | undefined;
  #closed = false;

  constructor(
    inner: NativeDatabase,
    streamId: number,
    signal?: AbortSignal,
    release?: () => void,
  ) {
    this.#inner = inner;
    this.#streamId = streamId;
    this.#signal = signal;
    this.#release = release;
    this.#async = inner.streamIsAsync(streamId);
    finalizers.register(this, streamCleanup(inner, streamId, release), this);
  }

  #finish(): void {
    this.#closed = true;
    finalizers.unregister(this);
    const release = this.#release;
    this.#release = undefined;
    release?.();
    // A read-only stream is already gone once it ended or failed; a
    // mutating one is dropped here, after its writes committed or rolled
    // back.
    if (this.#async) this.#inner.streamClose(this.#streamId);
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
      const row = (
        this.#async
          ? await this.#inner.streamNextAsync(this.#streamId)
          : this.#inner.streamNext(this.#streamId)
      ) as T | null;
      if (row === null) {
        this.#finish();
        return { done: true, value: undefined };
      }
      return { done: false, value: row };
    } catch (err) {
      // A pull still pending when the stream was closed (or ended by
      // another pull) settles as closed: that is the end, not a failure.
      if (this.#closed) return { done: true, value: undefined };
      this.#finish();
      if (this.#signal?.aborted) throw this.#signal.reason;
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
    this.#finish();
    try {
      this.#inner.streamClose(this.#streamId);
    } catch (err) {
      throw wrapError(err);
    }
  }
}

/** Options for `db.changes()`. */
export interface ChangesOptions {
  /**
   * Resume after this LSN: the feed first yields every retained batch with
   * a greater LSN, then live batches. Omit to start with the next commit.
   * An LSN the database no longer retains rejects with
   * `LORA_CHANGES_TRUNCATED`.
   */
  fromLsn?: number | bigint;
  /** Stop the feed when this signal aborts; `next()` rejects with its reason. */
  signal?: AbortSignal;
  /**
   * Undelivered batches the feed may hold before it ends with
   * `LORA_CHANGES_LAGGED`. Defaults to 1024.
   */
  bufferSize?: number;
}

/**
 * Committed changes, one batch per committed write, in commit order.
 * Iterate with `for await`; `break` (or `close()`) unsubscribes.
 */
export interface ChangeFeed extends AsyncIterableIterator<LoraChangeBatch> {
  /**
   * Resolves once the feed is subscribed: every commit that finishes after
   * this point is delivered. Rejects with the open error (for example
   * `LORA_CHANGES_TRUNCATED`), which `next()` also surfaces.
   */
  readonly ready: Promise<void>;
  /** LSN of the last batch delivered, or `fromLsn` before the first one. */
  readonly lastLsn: number | undefined;
  close(): void;
}

const CHANGES_POLL_MAX = 256;

const WAKE = Symbol("wake");

/**
 * Feeds whose consumer waits in `next()` for new batches. Only the native
 * waker can resume such a consumer, and it holds its feed weakly (see
 * `weakWaker`), so the feed is kept here meanwhile: otherwise a consumer
 * loop parked on a quiet database could be collected with its feed.
 */
const parkedFeeds = new Set<NativeChangeFeed>();

/**
 * The callback the native feed calls when it may have something new. It
 * holds the feed weakly: the native side keeps the callback alive for as
 * long as the feed is open, and a strong reference would keep the feed,
 * and through it the whole database handle, from ever being collected.
 */
function weakWaker(feed: WeakRef<NativeChangeFeed>): () => void {
  return () => feed.deref()?.[WAKE]();
}

class NativeChangeFeed implements ChangeFeed {
  readonly #inner: NativeDatabase;
  readonly #signal: AbortSignal | undefined;
  readonly #onDone: (feed: WeakRef<NativeChangeFeed>) => void;
  readonly #opened: Promise<number>;
  readonly ready: Promise<void>;
  /** @internal How the database handle refers to this feed. */
  readonly self = new WeakRef<NativeChangeFeed>(this);
  /** The native feed, shared with the cleanup that closes a dropped feed. */
  readonly #native: { inner: NativeDatabase; id: number | undefined };
  #buffer: LoraChangeBatch[] = [];
  #done = false;
  #error: unknown = undefined;
  #lastLsn: number | undefined;
  #wakeSeq = 0;
  #wake: (() => void) | undefined;
  #queue: Promise<unknown> = Promise.resolve();
  #onAbort: (() => void) | undefined;

  constructor(
    inner: NativeDatabase,
    options: ChangesOptions | undefined,
    onDone: (feed: WeakRef<NativeChangeFeed>) => void,
  ) {
    this.#inner = inner;
    this.#native = { inner, id: undefined };
    this.#signal = options?.signal;
    this.#onDone = onDone;
    const fromLsn = normalizeLsn(options?.fromLsn);
    this.#lastLsn = fromLsn ?? undefined;
    const bufferSize = normalizeBufferSize(options?.bufferSize);
    if (this.#signal?.aborted) {
      this.#done = true;
      this.#error = this.#signal.reason;
      this.#opened = Promise.reject(this.#signal.reason);
    } else {
      // Subscribes synchronously when capture is already on, so writes
      // issued after `changes()` returns are never missed.
      let opening: Promise<number>;
      try {
        opening = inner.openChanges(fromLsn, bufferSize, weakWaker(this.self));
      } catch (err) {
        opening = Promise.reject(err);
      }
      this.#opened = opening.then((id) => {
        this.#native.id = id;
        if (this.#done) this.#closeNative();
        return id;
      });
      finalizers.register(this, feedCleanup(this.#native), this);
      if (this.#signal) {
        const signal = this.#signal;
        this.#onAbort = () => {
          this.#buffer = [];
          this.#end(signal.reason);
        };
        signal.addEventListener("abort", this.#onAbort, { once: true });
      }
    }
    this.ready = this.#opened.then(
      () => undefined,
      (err) => {
        throw wrapError(err);
      },
    );
    // `ready` is optional to await; its rejection also reaches `next()`.
    this.ready.catch(() => undefined);
  }

  get lastLsn(): number | undefined {
    return this.#lastLsn;
  }

  [Symbol.asyncIterator](): AsyncIterableIterator<LoraChangeBatch> {
    return this;
  }

  next(): Promise<IteratorResult<LoraChangeBatch>> {
    // Serialize concurrent `next()` calls so batches stay in order.
    const result = this.#queue.then(() => this.#next());
    this.#queue = result.catch(() => undefined);
    return result;
  }

  async return(): Promise<IteratorResult<LoraChangeBatch>> {
    this.close();
    return { done: true, value: undefined };
  }

  close(): void {
    this.#buffer = [];
    this.#end(undefined);
  }

  /** Called by `dispose()`: the native side already dropped the feed. */
  disposed(): void {
    this.#native.id = undefined;
    this.#end(undefined);
  }

  async #next(): Promise<IteratorResult<LoraChangeBatch>> {
    for (;;) {
      const buffered = this.#buffer.shift();
      if (buffered) {
        this.#lastLsn = buffered.lsn;
        return { done: false, value: buffered };
      }
      if (this.#done) {
        const err = this.#error;
        this.#error = undefined;
        if (err !== undefined) throw err;
        return { done: true, value: undefined };
      }
      let id: number;
      try {
        id = await this.#opened;
      } catch (err) {
        this.#end(wrapError(err));
        continue;
      }
      if (this.#done) continue;
      const seq = this.#wakeSeq;
      let polled: Awaited<ReturnType<NativeDatabase["changesPoll"]>>;
      try {
        polled = await this.#inner.changesPoll(id, CHANGES_POLL_MAX);
      } catch (err) {
        this.#end(this.#done ? undefined : wrapError(err));
        continue;
      }
      this.#buffer.push(...(polled.batches as LoraChangeBatch[]));
      if (polled.closed) {
        this.#end(undefined);
        continue;
      }
      if (polled.batches.length === 0 && seq === this.#wakeSeq && !this.#done) {
        parkedFeeds.add(this);
        await new Promise<void>((resolve) => {
          this.#wake = resolve;
        });
      }
    }
  }

  /** @internal Called by the native waker. */
  [WAKE](): void {
    this.#onWake();
  }

  #onWake(): void {
    parkedFeeds.delete(this);
    this.#wakeSeq += 1;
    const wake = this.#wake;
    this.#wake = undefined;
    wake?.();
  }

  #end(error: unknown): void {
    if (this.#done) {
      if (error !== undefined && this.#error === undefined) this.#error = error;
      return;
    }
    this.#done = true;
    this.#error = error;
    if (this.#onAbort && this.#signal) {
      this.#signal.removeEventListener("abort", this.#onAbort);
      this.#onAbort = undefined;
    }
    this.#closeNative();
    finalizers.unregister(this);
    this.#onDone(this.self);
    this.#onWake();
  }

  #closeNative(): void {
    const id = this.#native.id;
    if (id === undefined) return;
    this.#native.id = undefined;
    try {
      this.#inner.changesClose(id);
    } catch {
      // Already gone (database disposed).
    }
  }
}

function normalizeLsn(lsn: number | bigint | undefined): number | null {
  if (lsn === undefined) return null;
  const value = typeof lsn === "bigint" ? Number(lsn) : lsn;
  if (
    !Number.isSafeInteger(value) ||
    value < 0 ||
    (typeof lsn === "bigint" && BigInt(value) !== lsn)
  ) {
    throw new LoraError(
      `\`fromLsn\` must be a non-negative safe integer, got ${String(lsn)}`,
      "LORA_INVALID_PARAMS",
    );
  }
  return value;
}

function normalizeBufferSize(size: number | undefined): number | null {
  if (size === undefined) return null;
  if (!Number.isInteger(size) || size < 1 || size > 0xffff_ffff) {
    throw new LoraError(
      `\`bufferSize\` must be a positive integer, got ${String(size)}`,
      "LORA_INVALID_PARAMS",
    );
  }
  return size;
}

function isFetchUrl(url: URL): boolean {
  return (
    url.protocol === "http:" ||
    url.protocol === "https:" ||
    url.protocol === "data:"
  );
}

function stringToUrl(value: string): URL | null {
  try {
    return new URL(value);
  } catch {
    return null;
  }
}

function bytesToUint8Array(
  bytes: Uint8Array | ArrayBuffer | Buffer,
): Uint8Array {
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

function snapshotBufferToUint8Array(buffer: Buffer): Uint8Array {
  const bytes = new Uint8Array(buffer.byteLength);
  bytes.set(buffer);
  return bytes;
}

function snapshotBufferToArrayBuffer(buffer: Buffer): ArrayBuffer {
  const arrayBuffer = new ArrayBuffer(buffer.byteLength);
  new Uint8Array(arrayBuffer).set(buffer);
  return arrayBuffer;
}

function resolveNodeSnapshotPath(path: string | URL): string {
  if (path instanceof URL) {
    if (path.protocol !== "file:") {
      throw new Error(
        `LORA_INVALID_PARAMS: unsupported snapshot save URL protocol '${path.protocol}'`,
      );
    }
    return fileURLToPath(path);
  }
  return path;
}

async function fetchSnapshotBytes(url: URL): Promise<Buffer> {
  const res = await fetch(url);
  if (!res.ok) {
    throw new Error(
      `LORA_IO: snapshot fetch failed (${res.status} ${res.statusText})`,
    );
  }
  return Buffer.from(await res.arrayBuffer());
}

function isReadableStream(
  source: unknown,
): source is ReadableStream<NodeSnapshotChunk> {
  return typeof (source as { getReader?: unknown }).getReader === "function";
}

function isAsyncIterable(
  source: unknown,
): source is AsyncIterable<NodeSnapshotChunk> {
  return (
    typeof (source as { [Symbol.asyncIterator]?: unknown })[
      Symbol.asyncIterator
    ] === "function"
  );
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
    throw new Error(
      `LORA_INVALID_PARAMS: unsupported snapshot URL protocol '${source.protocol}'`,
    );
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

function normalizeSnapshotCodecOptions(
  options?: SnapshotCodecOptions | null,
): SnapshotCodecOptions | null {
  if (!options) return null;
  const out: SnapshotCodecOptions = {};
  if ("compression" in options) out.compression = options.compression;
  if ("encryption" in options) out.encryption = options.encryption;
  return Object.keys(out).length === 0 ? null : out;
}

function normalizeSnapshotLoadOptions(
  options?: SnapshotLoadOptions | null,
): SnapshotLoadOptions | null {
  if (!options) return null;
  const out: SnapshotLoadOptions = {};
  if ("credentials" in options) out.credentials = options.credentials;
  if ("encryption" in options) out.encryption = options.encryption;
  return Object.keys(out).length === 0 ? null : out;
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
/**
 * FIFO admission of read-write `transaction()` batches. Each running batch
 * has a native thread of its own, so at most `max` run at once and the
 * rest wait their turn here. Unlike `begin()` and write streams, a batch
 * can wait: nothing in JS holds one open, so every running batch finishes
 * by itself and lets the next one in. A waiting batch is a plain pending
 * promise (plus an unref'd timer for its timeout): it does not keep the
 * process alive.
 */
class BatchGate {
  readonly #max: number;
  #running = 0;
  readonly #waiting: Array<() => void> = [];

  constructor(max: number) {
    this.#max = max;
  }

  /**
   * Take a place, or `null` if one was free: the batch may start now.
   * Otherwise a promise that resolves once the batch may start, or
   * rejects (leaving the queue) when `timeoutMs` passes or `signal`
   * aborts first. Call `leave()` once an admitted batch is done.
   */
  enter(timeoutMs: number | null, signal: AbortSignal | undefined) {
    if (this.#running < this.#max && this.#waiting.length === 0) {
      this.#running++;
      return null;
    }
    return new Promise<void>((resolve, reject) => {
      let timer: ReturnType<typeof setTimeout> | undefined;
      const quit = (err: unknown) => {
        const at = this.#waiting.indexOf(admit);
        if (at !== -1) this.#waiting.splice(at, 1);
        done();
        reject(err);
      };
      const onAbort = () => quit(signal?.reason);
      const done = () => {
        if (timer !== undefined) clearTimeout(timer);
        signal?.removeEventListener("abort", onAbort);
      };
      // `leave()` hands its place over: `#running` stays as it is.
      const admit = () => {
        done();
        resolve();
      };
      if (signal?.aborted) {
        reject(signal.reason);
        return;
      }
      this.#waiting.push(admit);
      signal?.addEventListener("abort", onAbort, { once: true });
      if (timeoutMs !== null) {
        timer = setTimeout(
          () =>
            quit(
              new LoraError(
                "query deadline exceeded waiting for a free transaction slot",
                "LORA_TIMEOUT",
              ),
            ),
          timeoutMs,
        );
        timer.unref?.();
      }
    });
  }

  leave(): void {
    const next = this.#waiting.shift();
    if (next) next();
    else this.#running--;
  }
}

class DatabaseImpl {
  readonly #inner: NativeDatabase;
  /** Open feeds, held weakly so a dropped feed can be collected. */
  readonly #feeds = new Set<WeakRef<NativeChangeFeed>>();
  /** Database-wide `queryTimeoutMs`; 0 for none. */
  readonly #defaultTimeoutMs: number;
  /** Queues read-write `transaction()` batches past `maxOpenTransactions`. */
  readonly #batches: BatchGate;

  constructor(
    inner: NativeDatabase,
    defaultTimeoutMs: number | null,
    maxOpen: number | null,
  ) {
    this.#inner = inner;
    this.#defaultTimeoutMs = defaultTimeoutMs ?? 0;
    this.#batches = new BatchGate(maxOpen ?? DEFAULT_MAX_OPEN_TRANSACTIONS);
    trackDatabase(inner);
  }

  /**
   * Execute a Lora query on the libuv threadpool. Returns a Promise that
   * resolves with `{ columns, rows }`; errors surface as `LoraError`
   * with a narrowed `code` from the `LoraErrorCode` union (e.g.
   * `LORA_PARSE`, `LORA_INVALID_PARAMS`, `LORA_INTERNAL`).
   */
  async execute<
    T extends Record<string, LoraValue> = Record<string, LoraValue>,
  >(
    query: string,
    params?: LoraParams,
    options?: QueryOptions,
  ): Promise<QueryResult<T>> {
    try {
      const encoded = encodeParams(params);
      const buf = await this.#limited(options, (timeoutMs, token) =>
        this.#inner.execute(query, encoded, timeoutMs, token),
      );
      return decodeResult(buf) as QueryResult<T>;
    } catch (err) {
      throw wrapError(err);
    }
  }

  /**
   * Run `start` under `options`. Without a signal the timeout goes to the
   * native call directly. With one, a native cancellation token carries
   * the timeout, `abort` cancels the query server-side, and a cancelled
   * call rejects with the signal's reason.
   */
  #limited<R>(
    options: QueryOptions | undefined,
    start: (timeoutMs: number | null, token: number | null) => Promise<R>,
  ): Promise<R> {
    return runLimited(this.#inner, options, start);
  }

  /**
   * Compile a query and return its execution plan without running it.
   *
   * This is a separate API method from `execute()`; it never invokes the
   * executor, so calling `explain()` on a mutating query (`CREATE`,
   * `MERGE`, `SET`, `DELETE`, `REMOVE`) leaves the graph untouched.
   * Errors surface with the same `LoraError` shape as `execute()`.
   */
  async explain(query: string, params?: LoraParams): Promise<LoraQueryPlan> {
    try {
      return (await this.#inner.explain(
        query,
        encodeParams(params),
      )) as LoraQueryPlan;
    } catch (err) {
      throw wrapError(err);
    }
  }

  /**
   * Execute a query and return the plan plus runtime metrics.
   *
   * **`profile()` runs the query for real.** Mutating queries
   * (`CREATE`, `MERGE`, `SET`, `DELETE`, `REMOVE`) produce the same
   * side effects as `execute()`: the WAL is written, snapshots observe
   * the commit, and the live store advances. Use `explain()` if you
   * want to inspect a mutating plan without running it.
   */
  async profile(query: string, params?: LoraParams): Promise<LoraQueryProfile> {
    try {
      return (await this.#inner.profile(
        query,
        encodeParams(params),
      )) as LoraQueryProfile;
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
    options?: QueryOptions,
  ): RowStream<T> {
    try {
      const encoded = encodeParams(params);
      const timeoutMs = normalizeTimeout(options?.timeoutMs);
      const signal = options?.signal;
      if (signal == null) {
        const streamId = this.#inner.openStream(
          query,
          encoded,
          timeoutMs,
          null,
        );
        return new NativeRowStream<T>(this.#inner, streamId);
      }
      if (signal.aborted) throw signal.reason;
      const token = this.#inner.createCancelToken(timeoutMs);
      const onAbort = () => this.#inner.cancelQuery(token);
      signal.addEventListener("abort", onAbort, { once: true });
      const release = () => {
        signal.removeEventListener("abort", onAbort);
        this.#inner.releaseCancelToken(token);
      };
      try {
        const streamId = this.#inner.openStream(query, encoded, null, token);
        return new NativeRowStream<T>(this.#inner, streamId, signal, release);
      } catch (err) {
        release();
        throw err;
      }
    } catch (err) {
      throw wrapError(err);
    }
  }

  /** Alias for `stream()`, useful when naming the row-level API explicitly. */
  rows<T extends Record<string, LoraValue> = Record<string, LoraValue>>(
    query: string,
    params?: LoraParams,
    options?: QueryOptions,
  ): RowStream<T> {
    return this.stream<T>(query, params, options);
  }

  /**
   * Begin an interactive transaction: run statements, decide in JS, then
   * `commit()` or `rollback()`.
   *
   * A `read_write` transaction holds the database's writer lock from
   * `begin()` until it finishes, so no other write can interleave with it
   * (other writers wait). Reads outside the transaction keep seeing the
   * last committed state. A statement that fails rolls the transaction
   * back. An unfinished transaction rolls back when the handle is disposed
   * (`await using tx = await db.begin()`), when the database is disposed,
   * or when the process exits. Keep transactions short: while one is open,
   * writers queue behind it.
   *
   * `options.timeoutMs` bounds the wait for the writer lock: past it
   * `begin()` rejects with `LORA_TIMEOUT` and nothing is held. Aborting
   * `options.signal` rejects `begin()` at once; a transaction that opens
   * afterwards is rolled back, so it never holds the lock.
   *
   * Each open transaction runs on a thread of its own. With
   * `maxOpenTransactions` (default 1024) transactions and write streams
   * already open, `begin()` rejects at once with `LORA_TOO_MANY_OPEN`.
   */
  async begin(
    mode: TransactionMode = "read_write",
    options: { timeoutMs?: number; signal?: AbortSignal } = {},
  ): Promise<Transaction> {
    const { timeoutMs, signal } = options;
    if (signal?.aborted) throw signal.reason;
    let opening: Promise<number>;
    try {
      opening = this.#inner.beginTransaction(mode, timeoutMs ?? null);
    } catch (err) {
      throw wrapError(err);
    }
    try {
      const id = await (signal
        ? new Promise<number>((resolve, reject) => {
            const onAbort = () => {
              // Whenever it opens, it is rolled back at once.
              void opening.then(
                (late) =>
                  new Transaction(this.#inner, late)
                    .rollback()
                    .catch(() => undefined),
                () => undefined,
              );
              reject(signal.reason);
            };
            signal.addEventListener("abort", onAbort, { once: true });
            opening
              .then(resolve, reject)
              .finally(() => signal.removeEventListener("abort", onAbort));
          })
        : opening);
      return new Transaction(this.#inner, id);
    } catch (err) {
      if (signal?.aborted) throw signal.reason;
      throw wrapError(err);
    }
  }

  /**
   * Execute a statement batch inside one native transaction.
   *
   * Results are returned in statement order. If any statement fails, the
   * native transaction is dropped before commit and all prior writes in the
   * batch are rolled back; the error of a failing statement of a
   * read-write batch ends with `(statement i of n)`. `options.timeoutMs`
   * bounds a read-write batch's wait for the writer lock, and then its
   * statements.
   */
  async transaction<
    T extends Record<string, LoraValue> = Record<string, LoraValue>,
  >(
    statements: TransactionStatement[],
    mode: TransactionMode = "read_write",
    options?: QueryOptions,
  ): Promise<Array<QueryResult<T>>> {
    try {
      const encoded = statements.map((st) =>
        st.params == null ? st : { ...st, params: encodeParams(st.params) },
      );
      // A write waits for the writer lock on a thread of its own: on a
      // libuv worker, a few queued writes would take the whole pool and
      // every read would wait behind them.
      const write =
        mode === "read_write" || mode === "readwrite" || mode === "rw";
      const buffers = write
        ? await this.#writeBatch(encoded, options)
        : await this.#limited(options, (timeoutMs, token) =>
            this.#inner.transaction(encoded, mode, timeoutMs, token),
          );
      return buffers.map((buf) => decodeResult(buf)) as Array<QueryResult<T>>;
    } catch (err) {
      throw wrapError(err);
    }
  }

  /**
   * Run a read-write batch once the gate lets it in. Time spent waiting
   * counts against its timeout, and its signal can abandon the wait.
   */
  async #writeBatch(
    encoded: TransactionStatement[],
    options: QueryOptions | undefined,
  ): Promise<Buffer[]> {
    const timeoutMs =
      normalizeTimeout(options?.timeoutMs) ?? this.#defaultTimeoutMs;
    const waiting = this.#batches.enter(timeoutMs || null, options?.signal);
    let limits = options;
    if (waiting !== null) {
      const since = Date.now();
      await waiting;
      if (timeoutMs) {
        const left = Math.max(1, timeoutMs - (Date.now() - since));
        limits = { ...options, timeoutMs: left };
      }
    }
    try {
      return await this.#limited(limits, (ms, token) =>
        this.#inner.writeTransaction(encoded, ms, token),
      );
    } finally {
      this.#batches.leave();
    }
  }

  /**
   * Force pending WAL bytes and container updates to disk.
   *
   * The default `groupSync` mode writes WAL bytes before `execute()` resolves
   * and batches fsyncs for speed; call `sync()` when you need an immediate
   * fsync point and a current portable `.loradb` archive, for example before
   * copying it elsewhere.
   */
  async sync(): Promise<void> {
    try {
      await this.#inner.sync();
    } catch (err) {
      throw wrapError(err);
    }
  }

  /**
   * Make every write committed before the call durable: one fsync of the
   * WAL (none when the group sync already covered it), and no checkpoint,
   * so it costs the same on any graph size. A reopen after a crash, even
   * `kill -9`, replays what it covered. Resolves with the WAL's durable
   * LSN, `null` for an in-memory database.
   *
   * ```ts
   * await db.execute("CREATE (:Batch {id: $id})", { id });
   * await db.flush(); // now tell the sender it is durable
   * ```
   *
   * `sync()` also writes a snapshot (a checkpoint); use it before copying
   * the `.loradb` file, not per write.
   */
  async flush(): Promise<{ lsn: number | null }> {
    try {
      return await this.#inner.flush();
    } catch (err) {
      throw wrapError(err);
    }
  }

  /**
   * Write a self-contained copy of this database into `targetDir` while
   * reads and writes keep running, for an off-host backup.
   *
   * The copy is the graph as of one committed transaction (the last one
   * when the call starts), written as `<targetDir>/<name>.loradb`; it
   * opens with `createDatabase(name, { databaseDir: targetDir })`.
   * `targetDir` must be missing or empty and may be on another
   * filesystem. Writers wait only while the graph is taken, not while it
   * is written; the copy runs off the JS thread in bounded memory. The
   * `.loradb` appears only once it is complete and fsynced: a crash or
   * `kill -9` mid-copy leaves the target without one.
   *
   * Only for a database opened with a name and `databaseDir`; rejects
   * with `LORA_CONFIG` otherwise, and with `LORA_VALIDATION` when
   * `targetDir` is not empty.
   */
  async snapshot(targetDir: string | URL): Promise<DatabaseSnapshot> {
    try {
      return await this.#inner.snapshot(
        targetDir instanceof URL ? fileURLToPath(targetDir) : targetDir,
      );
    } catch (err) {
      throw wrapError(err);
    }
  }

  /**
   * Take a checkpoint now and report the snapshot it wrote. For a named
   * database this is what `sync()` does: the `.loradb` is rewritten as a
   * snapshot of the graph plus the WAL since. With `walDir` and
   * `snapshotDir` it writes a managed snapshot. Rejects with
   * `LORA_CONFIG` on an in-memory database.
   */
  async checkpoint(): Promise<SnapshotMeta> {
    try {
      return await this.#inner.checkpoint();
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
   * New operations after disposal reject with `database is closed`.
   * Queries already running finish on their own; the directory of a named
   * database is unlocked when the last of them does. Opening the same
   * database again in this process waits for that; another process may
   * find it locked meanwhile. Use `close()` to wait for it.
   */
  dispose(): void {
    try {
      this.#inner.dispose();
    } catch (err) {
      throw wrapError(err);
    } finally {
      this.#feedsDisposed();
    }
  }

  /**
   * Dispose the handle and resolve once it is closed: every operation
   * started on it has settled and, unless another handle in this process
   * has the same database open, its directory is unlocked. Queued writes
   * and open transactions reject as closed; queries already running
   * finish. Rejects with `LORA_TIMEOUT` if they still run after 60 s.
   * Idempotent. `await using db = await createDatabase(...)` calls it.
   *
   * ```ts
   * await db.close();
   * const reopened = await createDatabase("app", { databaseDir });
   * ```
   */
  async close(): Promise<void> {
    let closed: Promise<void>;
    try {
      closed = this.#inner.close();
    } catch (err) {
      throw wrapError(err);
    } finally {
      this.#feedsDisposed();
    }
    try {
      await closed;
    } catch (err) {
      throw wrapError(err);
    }
  }

  async [Symbol.asyncDispose](): Promise<void> {
    await this.close();
  }

  #feedsDisposed(): void {
    for (const feed of [...this.#feeds]) feed.deref()?.disposed();
    this.#feeds.clear();
  }

  /**
   * Subscribe to committed changes: one `LoraChangeBatch` per committed
   * write (auto-commit query, transaction, streamed write, `clear()`,
   * snapshot restore), in commit order. Rolled-back work never appears.
   *
   * ```ts
   * for await (const batch of db.changes({ fromLsn: saved })) {
   *   for (const change of batch.changes) handle(change);
   *   saved = batch.lsn;
   * }
   * ```
   *
   * Writers never wait for the feed. A consumer that falls more than
   * `bufferSize` batches behind gets `LORA_CHANGES_LAGGED` after the
   * buffered batches and should resume with `fromLsn: feed.lastLsn`.
   * `dispose()` ends every feed. An open feed does not keep the process
   * alive on its own.
   */
  changes(options?: ChangesOptions): ChangeFeed {
    const feeds = this.#feeds;
    // Forget feeds collected without being closed.
    for (const ref of feeds) if (ref.deref() === undefined) feeds.delete(ref);
    const feed = new NativeChangeFeed(this.#inner, options, (done) =>
      feeds.delete(done),
    );
    feeds.add(feed.self);
    return feed;
  }

  saveSnapshot(): Promise<Buffer>;
  saveSnapshot(options: SnapshotCodecOptions): Promise<Buffer>;
  saveSnapshot(path: string | URL): Promise<SnapshotMeta>;
  saveSnapshot(
    path: string | URL,
    options: SnapshotCodecOptions,
  ): Promise<SnapshotMeta>;
  saveSnapshot(
    options: { format?: "buffer" | "binary" } & SnapshotCodecOptions,
  ): Promise<Buffer>;
  saveSnapshot(
    options: { format: "uint8Array" } & SnapshotCodecOptions,
  ): Promise<Uint8Array>;
  saveSnapshot(
    options: { format: "arrayBuffer" } & SnapshotCodecOptions,
  ): Promise<ArrayBuffer>;
  saveSnapshot(
    options: { format: "base64" } & SnapshotCodecOptions,
  ): Promise<string>;
  saveSnapshot(
    options: { format: "stream" } & SnapshotCodecOptions,
  ): Promise<Readable>;
  saveSnapshot(
    options: { format: "path"; path: string | URL } & SnapshotCodecOptions,
  ): Promise<SnapshotMeta>;
  /**
   * Save the graph as a snapshot.
   *
   * - `saveSnapshot()` / `{ format: "buffer" }` return a Node `Buffer`.
   * - `saveSnapshot(path)` and `{ format: "path", path }` write atomically
   *   to a local file and return `SnapshotMeta`.
   * - `{ format: "uint8Array" | "arrayBuffer" | "base64" | "stream" }`
   *   return in-memory snapshot data in that shape.
   */
  async saveSnapshot(
    target?: NodeSnapshotSaveTarget,
    options?: SnapshotCodecOptions,
  ): Promise<
    SnapshotMeta | Buffer | Uint8Array | ArrayBuffer | string | Readable
  > {
    try {
      if (typeof target === "string" || target instanceof URL) {
        return this.#inner.saveSnapshot(
          resolveNodeSnapshotPath(target),
          normalizeSnapshotCodecOptions(options),
        ) as SnapshotMeta;
      }

      const saveOptions = target ?? {};
      if ("format" in saveOptions && saveOptions.format === "path") {
        const path = "path" in saveOptions ? saveOptions.path : undefined;
        if (!(typeof path === "string" || path instanceof URL)) {
          throw new Error(
            "LORA_INVALID_PARAMS: snapshot path format requires a path",
          );
        }
        return this.#inner.saveSnapshot(
          resolveNodeSnapshotPath(path),
          normalizeSnapshotCodecOptions({ ...options, ...saveOptions }),
        ) as SnapshotMeta;
      }

      const format =
        ("format" in saveOptions ? saveOptions.format : "buffer") ?? "buffer";
      const bytes = this.#inner.saveSnapshotBuffer(
        normalizeSnapshotCodecOptions({ ...options, ...saveOptions }),
      );
      switch (format) {
        case "buffer":
        case "binary":
          return bytes;
        case "uint8Array":
          return snapshotBufferToUint8Array(bytes);
        case "arrayBuffer":
          return snapshotBufferToArrayBuffer(bytes);
        case "base64":
          return bytes.toString("base64");
        case "stream":
          return Readable.from([bytes]);
        default:
          throw new Error(
            `LORA_INVALID_PARAMS: unsupported snapshot save format '${format}'`,
          );
      }
    } catch (err) {
      throw wrapError(err);
    }
  }

  /**
   * Replace the current graph state with a snapshot loaded from `source`.
   * The load runs off the event loop and waits for the writer lock like
   * any write: while an interactive transaction or a mutating stream is
   * open, it resolves once that finishes. Concurrent writes wait for the
   * load.
   */
  async loadSnapshot(
    source: NodeSnapshotSource,
    options?: SnapshotLoadOptions,
  ): Promise<SnapshotMeta> {
    try {
      const resolved = await resolveNodeSnapshotSource(source);
      const loadOptions = normalizeSnapshotLoadOptions(options);
      // Off the JS thread: the load takes the writer lock, which an open
      // transaction or write stream may hold until this thread lets go.
      if (typeof resolved === "string") {
        return (await this.#inner.loadSnapshot(
          resolved,
          loadOptions,
        )) as SnapshotMeta;
      }
      return (await this.#inner.loadSnapshotBuffer(
        resolved,
        loadOptions,
      )) as SnapshotMeta;
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
 * An open interactive transaction from `Database.begin()`.
 *
 * A read-write transaction holds the database's writer lock from `begin()`
 * until it ends. It ends on `commit()`, on `rollback()`, and when a statement
 * fails, which rolls it back. `isOpen === false` therefore guarantees that
 * the transaction no longer holds the lock and that nothing it wrote
 * uncommitted remains: there is nothing left to release, and `commit()` and
 * `rollback()` reject. That holds however the statement failed, including one
 * the engine never ran (a signal that was already aborted, a timeout that is
 * not a number).
 */
export class Transaction {
  readonly #inner: NativeDatabase;
  readonly #id: number;
  #open = true;

  /** @internal */
  constructor(inner: NativeDatabase, id: number) {
    this.#inner = inner;
    this.#id = id;
    // Dropped while open: roll back and release the writer lock.
    finalizers.register(this, transactionCleanup(inner, id), this);
  }

  /** Whether the transaction can still run statements. */
  get isOpen(): boolean {
    return this.#open;
  }

  /**
   * Run one statement inside the transaction. It sees the transaction's
   * own uncommitted writes. If it fails, the transaction is rolled back,
   * and the writer lock is free by the time the promise rejects.
   */
  async execute<
    T extends Record<string, LoraValue> = Record<string, LoraValue>,
  >(
    query: string,
    params?: LoraParams,
    options?: QueryOptions,
  ): Promise<QueryResult<T>> {
    this.#assertOpen();
    try {
      const encoded = encodeParams(params);
      const buf = await runLimited(this.#inner, options, (timeoutMs, token) =>
        this.#inner.txExecute(this.#id, query, encoded, timeoutMs, token),
      );
      return decodeResult(buf) as QueryResult<T>;
    } catch (err) {
      await this.#abandon();
      throw wrapError(err);
    }
  }

  /**
   * Run several statements inside the transaction in one native call and
   * return their results in order. Each statement sees the writes of the
   * ones before it. This saves the per-call round trip of calling
   * `execute()` once per statement.
   *
   * The batch stops at the first failing statement: the transaction is
   * rolled back (as with a failed `execute()`) and the promise rejects
   * with that statement's error, whose message ends with
   * `(statement i of n)`. `options.timeoutMs` and `options.signal` bound
   * the whole batch.
   */
  async executeMany<
    T extends Record<string, LoraValue> = Record<string, LoraValue>,
  >(
    statements: TransactionStatement[],
    options?: QueryOptions,
  ): Promise<Array<QueryResult<T>>> {
    this.#assertOpen();
    let encoded: TransactionStatement[];
    try {
      encoded = statements.map((st) =>
        st.params == null ? st : { ...st, params: encodeParams(st.params) },
      );
    } catch (err) {
      // Invalid params: nothing ran, the transaction stays open.
      throw wrapError(err);
    }
    try {
      const buffers = await runLimited(
        this.#inner,
        options,
        (timeoutMs, token) =>
          this.#inner.txExecuteMany(
            this.#id,
            encoded as Array<{
              query: string;
              params?: Record<string, unknown> | null;
            }>,
            timeoutMs,
            token,
          ),
      );
      return buffers.map((buf) => decodeResult(buf)) as Array<QueryResult<T>>;
    } catch (err) {
      await this.#abandon();
      throw wrapError(err);
    }
  }

  /** Commit every write made in the transaction and release the writer lock. */
  async commit(): Promise<void> {
    await this.#finish(true);
  }

  /** Discard every write made in the transaction and release the writer lock. */
  async rollback(): Promise<void> {
    await this.#finish(false);
  }

  /** Roll back if still open. Lets `await using tx = await db.begin()` clean up. */
  async [Symbol.asyncDispose](): Promise<void> {
    if (this.#open) await this.rollback();
  }

  async #finish(commit: boolean): Promise<void> {
    this.#assertOpen();
    this.#open = false;
    finalizers.unregister(this);
    try {
      await this.#inner.txFinish(this.#id, commit);
    } catch (err) {
      throw wrapError(err);
    }
  }

  /**
   * Close the handle after a failed statement and make sure the native
   * transaction went with it. The engine rolls back a statement that fails
   * while it runs, but not one that never reached it (a signal already
   * aborted between two statements, invalid options): that transaction
   * would keep the writer lock with no handle left to release it.
   */
  async #abandon(): Promise<void> {
    this.#open = false;
    finalizers.unregister(this);
    try {
      await this.#inner.txFinish(this.#id, false);
    } catch {
      // Already rolled back by the engine.
    }
  }

  #assertOpen(): void {
    if (!this.#open) {
      throw new LoraError(
        "transaction is no longer open (already committed, rolled back, or failed)",
        "LORA_TRANSACTION",
      );
    }
  }
}

/**
 * Run `start` under `options`. Without a signal the timeout goes to the
 * native call directly. With one, a native cancellation token carries the
 * timeout, `abort` cancels the query server-side, and a cancelled call
 * rejects with the signal's reason.
 */
async function runLimited<R>(
  inner: NativeDatabase,
  options: QueryOptions | undefined,
  start: (timeoutMs: number | null, token: number | null) => Promise<R>,
): Promise<R> {
  const timeoutMs = normalizeTimeout(options?.timeoutMs);
  const signal = options?.signal;
  if (signal == null) return start(timeoutMs, null);
  if (signal.aborted) throw signal.reason;
  const token = inner.createCancelToken(timeoutMs);
  const onAbort = () => inner.cancelQuery(token);
  signal.addEventListener("abort", onAbort, { once: true });
  try {
    return await start(null, token);
  } catch (err) {
    if (signal.aborted) throw signal.reason;
    throw err;
  } finally {
    signal.removeEventListener("abort", onAbort);
    inner.releaseCancelToken(token);
  }
}

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
 * const walPersistent = await openWalDatabase({
 *   walDir: "./data/wal",
 *   snapshotDir: "./data/snapshots",
 * });
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
    const rawOptions = options as Record<string, unknown>;
    const hasWalOptions =
      rawOptions.walDir !== undefined ||
      rawOptions.snapshotDir !== undefined ||
      rawOptions.snapshotEveryCommits !== undefined ||
      rawOptions.snapshotKeepOld !== undefined ||
      rawOptions.snapshotOptions !== undefined;
    if (hasWalOptions) {
      throw new LoraError(
        "walDir/snapshotDir are not valid for createDatabase(); use openWalDatabase()",
        "LORA_INVALID_PARAMS",
      );
    }
    if (databaseName == null && hasPersistenceOptions) {
      throw new LoraError(
        "databaseName is required when persistence options are provided",
        "LORA_INVALID_PARAMS",
      );
    }
    const syncMode = options.syncMode ?? null;
    const groupSyncIntervalMs = options.groupSyncIntervalMs ?? null;
    const maxOpen = maxOpenTransactions(options.maxOpenTransactions);
    // Opening reads the snapshot and replays the WAL on the libuv
    // threadpool, so a large graph does not hold the event loop.
    return new DatabaseImpl(
      await openNativeDatabase(
        databaseName ?? null,
        options.databaseDir ?? null,
        syncMode,
        groupSyncIntervalMs,
        null,
        null,
        null,
        null,
        null,
        options.queryTimeoutMs ?? null,
        maxOpen,
      ),
      options.queryTimeoutMs ?? null,
      maxOpen,
    );
  } catch (err) {
    throw wrapError(err);
  }
}

export async function openWalDatabase(
  options: WalDatabaseOptions,
): Promise<Database> {
  try {
    const hasSnapshotTuningOptions =
      options.snapshotEveryCommits !== undefined ||
      options.snapshotKeepOld !== undefined ||
      options.snapshotOptions !== undefined;
    if (!options.walDir) {
      throw new LoraError(
        "walDir is required for openWalDatabase()",
        "LORA_INVALID_PARAMS",
      );
    }
    if (options.snapshotDir === undefined && hasSnapshotTuningOptions) {
      throw new LoraError(
        "snapshotDir is required when managed snapshot options are provided",
        "LORA_INVALID_PARAMS",
      );
    }
    const syncMode = options.syncMode ?? null;
    const groupSyncIntervalMs = options.groupSyncIntervalMs ?? null;
    const snapshotEveryCommits = options.snapshotEveryCommits ?? null;
    const snapshotKeepOld = options.snapshotKeepOld ?? null;
    const maxOpen = maxOpenTransactions(options.maxOpenTransactions);
    return new DatabaseImpl(
      await openNativeDatabase(
        null,
        null,
        syncMode,
        groupSyncIntervalMs,
        options.walDir,
        options.snapshotDir ?? null,
        snapshotEveryCommits,
        snapshotKeepOld,
        options.snapshotOptions ?? null,
        null,
        maxOpen,
      ),
      null,
      maxOpen,
    );
  } catch (err) {
    throw wrapError(err);
  }
}

/** Default of `maxOpenTransactions`; the native side has the same. */
const DEFAULT_MAX_OPEN_TRANSACTIONS = 1024;

/** Validate `maxOpenTransactions`: a whole number of at least 1. */
function maxOpenTransactions(value: number | undefined): number | null {
  if (value === undefined) return null;
  if (!Number.isInteger(value) || value < 1 || value > 0xffffffff) {
    throw new LoraError(
      `maxOpenTransactions must be a whole number of at least 1, got ${String(value)}`,
      "LORA_INVALID_PARAMS",
    );
  }
  return value;
}

/**
 * Prepare params for the native boundary. Params cross as JSON-shaped
 * values, which cannot carry a `bigint` or bytes, so every bigint is
 * rewritten to `{ kind: "integer", value: "<decimal>" }` and every
 * `Buffer`, `Uint8Array` or `ArrayBuffer` to `{ kind: "binary", base64 }`,
 * both decoded exactly on the Rust side. Returns the input unchanged (no
 * copy) when it holds neither, which is the common case.
 */
function encodeParams(
  params: LoraParams | null | undefined,
): LoraParams | null {
  if (params == null) return null;
  return needsEncoding(params) ? (encodeValue(params) as LoraParams) : params;
}

/**
 * An integer-valued `number` past 2^53 has usually already been rounded
 * by JavaScript (an id or timestamp parsed from JSON, say). Storing it
 * would make that corruption permanent, so refuse it and point at bigint.
 * Magnitudes of 2^63 and up cannot be 64-bit integers and stay floats.
 */
function checkNumber(value: number): void {
  if (
    Number.isInteger(value) &&
    !Number.isSafeInteger(value) &&
    Math.abs(value) < 2 ** 63
  ) {
    throw new LoraError(
      `integer ${value} is outside the JavaScript safe-integer range and may already be rounded; pass it as a bigint`,
      "LORA_INVALID_PARAMS",
    );
  }
}

/**
 * The bytes of a binary param, or `null` for any other value. Other
 * typed arrays and `DataView` are refused: their bytes depend on the
 * platform's endianness, and a `Float32Array` is more likely meant as a
 * `vector()`.
 */
function paramBytes(value: object): Uint8Array | null {
  if (value instanceof Uint8Array) return value;
  if (value instanceof ArrayBuffer) return new Uint8Array(value);
  if (ArrayBuffer.isView(value)) {
    throw new LoraError(
      `${value.constructor.name} is not a supported parameter; pass bytes as a Buffer, Uint8Array or ArrayBuffer, or numbers as vector()`,
      "LORA_INVALID_PARAMS",
    );
  }
  return null;
}

function needsEncoding(value: unknown): boolean {
  if (typeof value === "bigint") return true;
  if (typeof value === "number") {
    checkNumber(value);
    return false;
  }
  if (value === null || typeof value !== "object") return false;
  if (paramBytes(value) !== null) return true;
  if (Array.isArray(value)) {
    let found = false;
    for (const item of value) found = needsEncoding(item) || found;
    return found;
  }
  let found = false;
  for (const key in value) {
    found = needsEncoding((value as Record<string, unknown>)[key]) || found;
  }
  return found;
}

function encodeValue(value: unknown): unknown {
  if (typeof value === "bigint")
    return { kind: "integer", value: value.toString() };
  if (value === null || typeof value !== "object") return value;
  const bytes = paramBytes(value);
  if (bytes !== null) {
    const base64 = Buffer.from(
      bytes.buffer,
      bytes.byteOffset,
      bytes.byteLength,
    ).toString("base64");
    return { kind: "binary", base64 };
  }
  if (Array.isArray(value)) return value.map(encodeValue);
  const out: Record<string, unknown> = {};
  for (const key in value)
    out[key] = encodeValue((value as Record<string, unknown>)[key]);
  return out;
}

function normalizeTimeout(timeoutMs: number | undefined): number | null {
  if (timeoutMs === undefined) return null;
  if (!Number.isFinite(timeoutMs) || timeoutMs < 0) {
    throw new LoraError(
      `timeoutMs must be a non-negative finite number, got ${timeoutMs}`,
      "LORA_INVALID_PARAMS",
    );
  }
  // The native side takes whole milliseconds as u32; 0 disables.
  return Math.min(Math.ceil(timeoutMs), 0xffff_ffff);
}
