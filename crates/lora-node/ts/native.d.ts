/**
 * Low-level typings for the native Rust N-API module (`cypher.node`).
 *
 * These mirror what napi-derive would generate. The high-level TS wrapper
 * in `index.ts` narrows the return / param types to the strongly-typed
 * `QueryResult<T>` / `LoraParams` surface.
 */

export interface NativeQueryResult {
  columns: string[];
  rows: Array<Record<string, unknown>>;
}

export interface NativeSnapshotMeta {
  formatVersion: number;
  nodeCount: number;
  relationshipCount: number;
  walLsn: number | null;
}

export declare class Database {
  constructor(walDir?: string | null);
  /** Non-blocking: runs on the libuv threadpool, returns a Promise. */
  execute(
    query: string,
    params?: Record<string, unknown> | null,
  ): Promise<NativeQueryResult>;
  clear(): void;
  nodeCount(): number;
  relationshipCount(): number;
  dispose(): void;
  /** Atomic save. Synchronous under the store mutex. */
  saveSnapshot(path: string): NativeSnapshotMeta;
  /** Replace the current graph with the snapshot at `path`. */
  loadSnapshot(path: string): NativeSnapshotMeta;
}
