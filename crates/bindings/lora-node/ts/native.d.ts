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

export type NativeSnapshotOptions = unknown;

export declare class Database {
  constructor(
    databaseName?: string | null,
    databaseDir?: string | null,
    syncMode?: "groupSync" | null,
    groupSyncIntervalMs?: number | null,
    walDir?: string | null,
    snapshotDir?: string | null,
    snapshotEveryCommits?: number | null,
    snapshotKeepOld?: number | null,
    snapshotOptions?: NativeSnapshotOptions | null,
  );
  /**
   * Non-blocking: runs on the libuv threadpool, returns a Promise.
   * Resolves to the encoded result Buffer; the TS wrapper decodes it
   * into the public `{columns, rows}` shape.
   */
  execute(
    query: string,
    params?: Record<string, unknown> | null,
  ): Promise<Buffer>;
  /** Compile a query and return its plan without executing it. */
  explain(
    query: string,
    params?: Record<string, unknown> | null,
  ): Promise<unknown>;
  /** Execute a query and return the plan plus runtime metrics. PROFILE executes writes. */
  profile(
    query: string,
    params?: Record<string, unknown> | null,
  ): Promise<unknown>;
  openStream(
    query: string,
    params?: Record<string, unknown> | null,
  ): number;
  streamColumns(streamId: number): string[];
  streamNext(streamId: number): Record<string, unknown> | null;
  streamClose(streamId: number): void;
  transaction(
    statements: Array<{ query: string; params?: Record<string, unknown> | null }>,
    mode?: "read_write" | "read_only" | "readwrite" | "readonly" | "rw" | "ro" | null,
  ): Promise<Buffer[]>;
  /** Force pending WAL bytes and the portable container mirror to disk. */
  sync(): Promise<void>;
  clear(): Promise<void>;
  nodeCount(): number;
  relationshipCount(): number;
  dispose(): void;
  /** Atomic save. Synchronous under the store read lock. */
  saveSnapshot(path: string, options?: NativeSnapshotOptions | null): NativeSnapshotMeta;
  /** Serialize the current graph into snapshot bytes. */
  saveSnapshotBuffer(options?: NativeSnapshotOptions | null): Buffer;
  /** Replace the current graph with the snapshot at `path`. */
  loadSnapshot(path: string, options?: NativeSnapshotOptions | null): NativeSnapshotMeta;
  /** Replace the current graph with snapshot bytes. */
  loadSnapshotBuffer(
    bytes: Uint8Array | Buffer,
    options?: NativeSnapshotOptions | null,
  ): NativeSnapshotMeta;
}
