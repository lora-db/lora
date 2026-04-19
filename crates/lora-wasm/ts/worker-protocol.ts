/**
 * Wire protocol for the worker-backed Database.
 *
 * Messages are plain objects (structured-clone safe). Each request carries a
 * numeric `id` which the worker echoes back, letting the client correlate
 * asynchronous responses.
 */

import type { LoraParams, QueryResult, LoraErrorCode } from "./types.js";

export type RequestBody =
  | { op: "execute"; query: string; params?: LoraParams | null }
  | { op: "clear" }
  | { op: "nodeCount" }
  | { op: "relationshipCount" }
  | { op: "dispose" };

export interface Request {
  id: number;
  body: RequestBody;
}

export type ResponseBody =
  | { ok: true; result: QueryResult | number | null }
  | { ok: false; error: { message: string; code: LoraErrorCode } };

export interface Response {
  id: number;
  body: ResponseBody;
}
