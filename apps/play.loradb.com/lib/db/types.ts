/**
 * Internal types shared across the playground UI for query results.
 *
 * These are derived from but distinct from `@loradb/lora-wasm`'s wire types
 * — adapter.ts converts a raw `QueryResult` into an `AdaptedResult` that the
 * UI (table, graph canvas, run history) can consume directly.
 */

import type { GraphData } from "@loradb/lora-graph-canvas";

export type CellType =
  | "node"
  | "relationship"
  | "path"
  | "array"
  | "object"
  | "string"
  | "number"
  | "boolean"
  | "null";

export interface QueryRow {
  /** Index-aligned with `AdaptedResult.columns[]`. */
  values: unknown[];
}

export interface AdaptedResult {
  columns: string[];
  /** Index-aligned with `columns`; per-column inferred dominant type. */
  cellTypes: CellType[];
  rows: QueryRow[];
  /** `null` if the result contains no nodes or relationships. */
  graph: GraphData | null;
  stats: {
    nodeCount: number;
    relCount: number;
    rowCount: number;
  };
}

export interface RunOk {
  state: "ok";
  runId: string;
  startedAt: number;
  endedAt: number;
  ms: number;
  result: AdaptedResult;
}

export interface RunErr {
  state: "error";
  runId: string;
  startedAt: number;
  endedAt: number;
  ms: number;
  message: string;
  position?: { line: number; col: number };
}

export type RunOutcome = RunOk | RunErr;
