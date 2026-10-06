// What a mutation runs with: the environment the server hands it, the
// statement runner every write goes through, and the counts it reports.

import type { Statistics } from "../../analyze/statistics.js";
import type { SelectionContext } from "../../compile/selection.js";
import type {
  DriverTransaction,
  LoraDriver,
  QueryResult,
  Statement,
} from "../../driver.js";
import type { GraphModel, NodeType } from "../../model/types.js";
import type { WriteChange } from "../changes.js";

/** Computes a `@populatedBy` field. */
export type PopulatedByCallback = (args: {
  operation: "CREATE" | "UPDATE";
  /** Node type and field being populated. */
  type: string;
  field: string;
  key: unknown;
  /** The node's input in this mutation. */
  input: Record<string, unknown>;
  /** The GraphQL context. */
  context: unknown;
}) => unknown;

export interface MutationEnv {
  model: GraphModel;
  driver: LoraDriver;
  /** The caller's node's @key, for @key(scope: VIEWER) (see executeMutation). */
  viewerKey?: unknown;
  selection: SelectionContext;
  jwt: Record<string, unknown> | undefined;
  /** The GraphQL context, for `$context` in rules and for callbacks. */
  requestContext: unknown;
  timeoutMs: number;
  signal: AbortSignal | undefined;
  degrees: ReadonlyMap<string, number>;
  /**
   * Most nodes one mutation may create, update or delete, nested ones
   * included; relationships written, ten times as many.
   */
  maxBatch: number;
  /** Most items a @cypher list argument takes without `@size(max:)`. */
  maxListArgument?: number | undefined;
  /** Relationship levels one `where` may nest. */
  maxFilterDepth?: number | undefined;
  /** Items an `in` filter operand may hold. */
  maxListFilter?: number | undefined;
  /** Characters a string filter operand may hold. */
  maxStringFilter?: number | undefined;
  /** Statistics from `analyze()`, for filter costs. */
  statistics?: Statistics | undefined;
  callbacks: Readonly<Record<string, PopulatedByCallback>>;
  /**
   * A caller-owned transaction: run inside it and leave the commit to the
   * caller. A failed mutation rolls it back, so it is never half-applied.
   */
  transaction?: DriverTransaction | undefined;
  /** Observes every statement, for logging and tests. */
  onStatement?: ((statement: Statement) => void) | undefined;
  /** Runs a statement under timing, tracing and metrics, when configured. */
  observe?:
    | ((
        statement: Statement,
        run: () => Promise<QueryResult>,
      ) => Promise<QueryResult>)
    | undefined;
  /**
   * Called inside the transaction once a delete knows every node it
   * removes, before any is removed: subscribers' checks of the nodes as
   * they were (a deleted node cannot be checked after the commit).
   */
  beforeDelete?:
    | ((
        change: WriteChange,
        doomed: ReadonlyArray<{ node: NodeType; keys: unknown[] }>,
        run: (statement: Statement) => Promise<QueryResult>,
      ) => Promise<void>)
    | undefined;
}

/** Run `statement` in `tx`, reporting it before and observing it during. */
export function runStatement(
  env: MutationEnv,
  tx: DriverTransaction,
  statement: Statement,
): Promise<QueryResult> {
  env.onStatement?.(statement);
  return env.observe
    ? env.observe(statement, () => tx.execute(statement))
    : tx.execute(statement);
}

export interface MutationInfo {
  nodesCreated: number;
  nodesUpdated: number;
  nodesDeleted: number;
  relationshipsCreated: number;
  relationshipsDeleted: number;
}
