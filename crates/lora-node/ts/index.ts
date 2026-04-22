/**
 * lora-node — typed Node.js binding for the Lora graph engine.
 *
 * Thin wrapper over the N-API module. Query execution is delegated to the
 * native layer, which runs each query on the libuv threadpool so the JS
 * event loop stays responsive. This file only narrows the `unknown`-valued
 * native surface into the strongly-typed `QueryResult<T>` / `LoraParams`
 * shapes defined in the shared TS contract (`crates/shared-ts/types.ts`).
 *
 * **Initialization is async-only.** The one canonical entry point is
 * `createDatabase()`; there is no synchronous constructor. See the docs at
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
 * In-memory Lora graph database instance.
 *
 * Obtained exclusively via `createDatabase()`. There is no public constructor
 * and no synchronous factory — the async entry point lets the binding
 * extend initialization later (lazy native loading, warmup, schema
 * preflight) without breaking callers.
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
    this.#inner.clear();
  }

  /** Number of nodes currently in the graph. */
  async nodeCount(): Promise<number> {
    return this.#inner.nodeCount();
  }

  /** Number of relationships currently in the graph. */
  async relationshipCount(): Promise<number> {
    return this.#inner.relationshipCount();
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
 * Create and initialize a new in-memory LoraDB instance.
 *
 * **This is the only supported initialization pattern** for `lora-node`.
 * Synchronous construction is not available — the async factory guarantees
 * the native layer is ready before the first query dispatches and keeps
 * the Node and WASM surfaces symmetric.
 *
 * ```ts
 * import { createDatabase } from "lora-node";
 *
 * const db = await createDatabase();
 * const res = await db.execute(
 *   "CREATE (n:Person {name: $name}) RETURN n",
 *   { name: "Alice" },
 * );
 * ```
 *
 * Each call returns an independent graph — no shared state between instances.
 */
export async function createDatabase(): Promise<Database> {
  return new DatabaseImpl(new NativeDatabase());
}
