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
 * `createDatabase(...)`, optionally with a WAL directory path. There is no
 * synchronous constructor. See the docs at
 * https://loradb.com/docs/getting-started/node for the full rationale.
 */

import type {
  LoraParams,
  LoraValue,
  QueryResult,
} from "./types.js";
import { wrapError } from "./types.js";

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

/**
 * Lora graph database instance.
 *
 * Obtained exclusively via `createDatabase()`. There is no public
 * constructor and no synchronous factory. With no args the instance is
 * purely in-memory; with a WAL directory path it replays committed WAL
 * state from disk before serving queries.
 *
 * Instances are independent — each owns its own in-memory graph. Multiple
 * concurrent `execute()` calls against one instance run one at a time
 * (serialised on the store mutex) but none of them block the event loop.
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

  /** Drop every node and relationship. Constant-time under the hood. */
  async clear(): Promise<void> {
    try {
      this.#inner.clear();
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
   * Call this when a WAL-backed database needs to be reopened in the same
   * process. New operations after disposal reject with `database is closed`.
   */
  dispose(): void {
    try {
      this.#inner.dispose();
    } catch (err) {
      throw wrapError(err);
    }
  }

  /**
   * Save the graph to a snapshot file. Writes atomically via a `.tmp` +
   * rename dance — the target path is only replaced once the full payload
   * has been written and fsync'd.
   *
   * Synchronous in the native layer (point-in-time consistency requires
   * holding the store mutex for the duration of the save); the returned
   * Promise resolves immediately once the save returns.
   */
  async saveSnapshot(path: string): Promise<SnapshotMeta> {
    try {
      return this.#inner.saveSnapshot(path) as SnapshotMeta;
    } catch (err) {
      throw wrapError(err);
    }
  }

  /**
   * Replace the current graph state with a snapshot loaded from `path`.
   * Concurrent `execute()` calls block on the store mutex until the load
   * completes.
   */
  async loadSnapshot(path: string): Promise<SnapshotMeta> {
    try {
      return this.#inner.loadSnapshot(path) as SnapshotMeta;
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
 * const inMemory = await createDatabase();            // in-memory
 * const persistent = await createDatabase("./app");  // persistent: pass a directory string
 * ```
 *
 * If you want persistence, pass a directory string. The string is treated
 * as a WAL directory path verbatim. Relative paths resolve from the
 * current working directory.
 *
 * Each call returns an independent graph — no shared state between instances.
 */
export async function createDatabase(walDir?: string): Promise<Database> {
  try {
    return new DatabaseImpl(
      walDir === undefined ? new NativeDatabase() : new NativeDatabase(walDir),
    );
  } catch (err) {
    throw wrapError(err);
  }
}
