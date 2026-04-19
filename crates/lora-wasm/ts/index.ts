/**
 * lora-wasm — typed WebAssembly bindings for the Lora graph engine.
 *
 * This entry targets Node.js (ESM) and any bundler host whose resolver maps
 * `node:module` through. Browser consumers should prefer the Worker-backed
 * API exposed in `worker-client.ts` to keep the main thread responsive.
 *
 *   const db = await Database.create();
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
 * Main-thread wrapper for the WASM engine. Construct via `Database.create()`
 * so WASM bootstrapping completes before the first query runs.
 *
 * All methods return Promises for API symmetry and forward-compatibility with
 * the Worker-backed variant; queries still execute synchronously inside WASM,
 * so for heavy queries in the browser prefer `createWorkerDatabase()`.
 */
export class Database {
  readonly #inner: InstanceType<typeof WasmDatabase>;

  private constructor(inner: InstanceType<typeof WasmDatabase>) {
    this.#inner = inner;
  }

  static async create(): Promise<Database> {
    ensureBootstrapped();
    return new Database(new WasmDatabase());
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
