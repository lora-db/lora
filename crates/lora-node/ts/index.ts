/**
 * lora-node — typed Node.js binding for the Lora graph engine.
 *
 * Thin wrapper over the N-API module. Query execution is delegated to the
 * native layer, which runs each query on the libuv threadpool so the JS
 * event loop stays responsive. This file only narrows the `unknown`-valued
 * native surface into the strongly-typed `QueryResult<T>` / `LoraParams`
 * shapes defined in the shared TS contract (`crates/shared-ts/types.ts`).
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
 * In-memory Lora graph database.
 *
 * ```ts
 * const db = await Database.create();
 * const res = await db.execute(
 *   "CREATE (n:Person {name: $name}) RETURN n",
 *   { name: "Alice" },
 * );
 * for (const row of res.rows) {
 *   if (isNode(row.n)) console.log(row.n.properties.name);
 * }
 * ```
 *
 * Instances are independent — each owns its own in-memory graph. Multiple
 * concurrent `execute()` calls against one instance run one at a time
 * (serialised on the store mutex) but none of them block the event loop.
 */
export class Database {
  readonly #inner: InstanceType<typeof NativeDatabase>;

  /** Synchronous constructor. Prefer `Database.create()` for API symmetry. */
  constructor() {
    this.#inner = new NativeDatabase();
  }

  /**
   * Async factory. Matches the `lora-wasm` API shape so consumers can
   * swap backends by changing only the import.
   */
  static async create(): Promise<Database> {
    return new Database();
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
