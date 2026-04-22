/**
 * lora-wasm — typed WebAssembly bindings for the Lora graph engine.
 *
 * This entry targets Node.js (ESM) and any bundler host whose resolver maps
 * `node:module` through. Browser consumers should prefer the Worker-backed
 * API exposed in `worker-client.ts` to keep the main thread responsive.
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

export * from "./types.js";
export { createWorkerDatabase, type WorkerDatabase } from "./worker-client.js";

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

  async clear(): Promise<void> {
    this.#inner.clear();
  }

  async nodeCount(): Promise<number> {
    return this.#inner.nodeCount();
  }

  async relationshipCount(): Promise<number> {
    return this.#inner.relationshipCount();
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
export type Database = DatabaseImpl;

/**
 * Create and initialize a new in-memory LoraDB instance on the WASM engine.
 *
 * **This is the only supported initialization pattern** for `lora-wasm`.
 * Synchronous construction is not available — the async factory bootstraps
 * the WASM module on first call and guarantees the engine is ready before
 * the first query.
 *
 * ```ts
 * import { createDatabase } from "lora-wasm";
 *
 * const db = await createDatabase();
 * const res = await db.execute("MATCH (n) RETURN count(n) AS n");
 * ```
 *
 * For heavy browser workloads, use `createWorkerDatabase()` from
 * `lora-wasm/worker-client` instead — it keeps the main thread responsive
 * by running the engine in a Web Worker.
 */
export async function createDatabase(): Promise<Database> {
  ensureBootstrapped();
  return new DatabaseImpl(new WasmDatabase());
}
