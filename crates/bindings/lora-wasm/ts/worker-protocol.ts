/**
 * Wire protocol for the worker-backed Database.
 *
 * Messages are plain objects (structured-clone safe). Each request carries a
 * numeric `id` which the worker echoes back, letting the client correlate
 * asynchronous responses.
 */

import type { LoraParams, QueryResult, LoraErrorCode } from "./types.js";
import type { RowFormat } from "./types.js";
import type { TransactionMode, TransactionStatement } from "./index.js";
import type {
  WasmSnapshotByteOptions,
  WasmSnapshotLoadOptions,
} from "./snapshot.js";

export type RequestBody =
  | { op: "execute"; query: string; params?: LoraParams | null }
  | { op: "streamOpen"; query: string; params?: LoraParams | null }
  | { op: "streamNext"; streamId: number }
  | { op: "streamClose"; streamId: number }
  | {
      op: "transaction";
      statements: TransactionStatement[];
      mode?: TransactionMode;
    }
  | { op: "saveSnapshot"; options?: WasmSnapshotByteOptions | null }
  | {
      op: "loadSnapshot";
      bytes: Uint8Array;
      options?: WasmSnapshotLoadOptions | null;
    }
  | { op: "snapshotInfo"; bytes: Uint8Array }
  | {
      op: "exportRows";
      query: string;
      params?: LoraParams | null;
      format: RowFormat;
    }
  | {
      op: "exportOpen";
      query: string;
      params?: LoraParams | null;
      format: RowFormat;
    }
  | { op: "exportNext"; exportId: number }
  | { op: "exportClose"; exportId: number }
  | {
      op: "importRows";
      bytes: Uint8Array;
      format: RowFormat;
      mapping: unknown;
      batchSize?: number | null;
    }
  | {
      op: "importRowsWithCypher";
      bytes: Uint8Array;
      format: RowFormat;
      template: string;
      batchSize?: number | null;
    }
  | {
      op: "importOpen";
      format: RowFormat;
      /** Either a `RowMapping` object or a Cypher template string. */
      mappingOrTemplate: unknown;
      batchSize?: number | null;
      /** When true, the cursor parses + counts rows but skips Cypher
       * execution. Used by the playground's Preview button. */
      dryRun?: boolean | null;
      /** When true, per-record parse failures are skipped and reported
       * in the final stats instead of aborting the stream. */
      permissive?: boolean | null;
    }
  | { op: "importFeed"; importId: number; chunk: Uint8Array }
  | { op: "importFinish"; importId: number }
  | { op: "importClose"; importId: number }
  | { op: "clear" }
  | { op: "nodeCount" }
  | { op: "relationshipCount" }
  | { op: "graphStats" }
  | { op: "memoryReport" }
  | { op: "dispose" };

export interface Request {
  id: number;
  body: RequestBody;
}

export type ResponseBody =
  | {
      ok: true;
      result:
        | QueryResult
        | QueryResult[]
        | number
        | Uint8Array
        | null
        | { streamId: number; columns: string[] }
        | { exportId: number; columns: string[] }
        | { importId: number }
        | Record<string, unknown>;
    }
  | { ok: false; error: { message: string; code: LoraErrorCode } };

export interface Response {
  id: number;
  body: ResponseBody;
}

/**
 * `(label, property)` scope of an active secondary index.
 */
export interface IndexScope {
  label: string;
  property: string;
}

/**
 * Per-`(label, property)` distinct-value count surfaced by `GraphStats`.
 * Populated for properties that have an active hash index; missing
 * entries mean "fall back to assume-all-distinct".
 */
export interface DistinctValueRecord {
  label: string;
  property: string;
  count: number;
}

/**
 * Per-label cardinality count from `GraphStats.nodes_by_label` and the
 * mirrored `relationships_by_type`.
 */
export interface LabelCount {
  label: string;
  count: number;
}

/**
 * Cardinality snapshot mirroring the Rust `lora_store::GraphStats`.
 * Returned by `WorkerDatabase.graphStats()`.
 */
export interface GraphStatsSnapshot {
  nodeCount: number;
  relationshipCount: number;
  nodesByLabel: LabelCount[];
  relationshipsByType: LabelCount[];
  nodeDistinctValues: DistinctValueRecord[];
  relationshipDistinctValues: DistinctValueRecord[];
  nodeRangeIndexes: IndexScope[];
  relationshipRangeIndexes: IndexScope[];
  nodeTextIndexes: IndexScope[];
  relationshipTextIndexes: IndexScope[];
  nodePointIndexes: IndexScope[];
  relationshipPointIndexes: IndexScope[];
  nodeVectorIndexes: IndexScope[];
  relationshipVectorIndexes: IndexScope[];
}

/**
 * Approximate retained-heap breakdown mirroring the Rust
 * `lora_store::MemoryReport`. See that struct's rustdoc for the
 * methodology — every field is in *bytes* and uses fixed amortised
 * overhead constants for `BTreeMap`/`HashMap` so two reports can be
 * diffed without per-run allocator noise.
 */
export interface MemoryReportSnapshot {
  liveNodeCount: number;
  liveRelationshipCount: number;
  nodeTombstoneCount: number;
  relationshipTombstoneCount: number;
  nodesBytes: number;
  relationshipsBytes: number;
  outgoingBytes: number;
  incomingBytes: number;
  labelIndexBytes: number;
  typeIndexBytes: number;
  propertyIndexBytes: number;
  sortedIndexBytes: number;
  textIndexBytes: number;
  pointIndexBytes: number;
  fulltextIndexBytes: number;
  vectorIndexBytes: number;
  indexCatalogBytes: number;
  constraintCatalogBytes: number;
}
