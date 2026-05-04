/**
 * Wire protocol for the worker-backed Database.
 *
 * Messages are plain objects (structured-clone safe). Each request carries a
 * numeric `id` which the worker echoes back, letting the client correlate
 * asynchronous responses.
 */

import type { LoraParams, QueryResult, LoraErrorCode } from "./types.js";
import type { TransactionMode, TransactionStatement } from "./index.js";
import type { WasmSnapshotByteOptions, WasmSnapshotLoadOptions } from "./snapshot.js";

export type RequestBody =
  | { op: "execute"; query: string; params?: LoraParams | null }
  | { op: "streamOpen"; query: string; params?: LoraParams | null }
  | { op: "streamNext"; streamId: number }
  | { op: "streamClose"; streamId: number }
  | { op: "transaction"; statements: TransactionStatement[]; mode?: TransactionMode }
  | { op: "saveSnapshot"; options?: WasmSnapshotByteOptions | null }
  | { op: "loadSnapshot"; bytes: Uint8Array; options?: WasmSnapshotLoadOptions | null }
  | { op: "clear" }
  | { op: "nodeCount" }
  | { op: "relationshipCount" }
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
        | Record<string, unknown>;
    }
  | { ok: false; error: { message: string; code: LoraErrorCode } };

export interface Response {
  id: number;
  body: ResponseBody;
}
