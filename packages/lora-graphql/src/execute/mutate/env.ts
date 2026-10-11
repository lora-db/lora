// What a mutation runs with: the environment the server hands it, the
// statement runner every write goes through, and the counts it reports.

import type { TypeValidator } from "../../standard-schema.js";
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
import type { LoraTransaction } from "../transaction.js";

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

/** What a JavaScript trigger is called with. */
export interface TriggerEvent {
  operation: "CREATE" | "UPDATE" | "DELETE";
  /** The node type whose nodes were written. */
  type: string;
  /** Their @key values. For DELETE the nodes still exist. */
  keys: unknown[];
  /** The mutation's write-set so far. */
  change: WriteChange;
  /**
   * The mutation's transaction: `execute()` runs Cypher in it, and
   * `lora.execute({ context: { ...context, transaction } })` runs a
   * generated mutation in it, whose writes join the write-set. The
   * mutation commits it; throw to roll everything back.
   */
  transaction: LoraTransaction;
  /** The GraphQL context of the request. */
  context: unknown;
}

/** A trigger function; it may return a promise. Throwing vetoes the write. */
export type TriggerFunction = (event: TriggerEvent) => unknown;

/** The trigger functions of one node type. */
export interface TriggerFunctions {
  create?: TriggerFunction;
  update?: TriggerFunction;
  delete?: TriggerFunction;
}

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
  /** The `triggers` option: functions per node type and operation. */
  triggers?: Readonly<Record<string, TriggerFunctions>> | undefined;
  /** The `triggerTimeoutMs` option, defaulted. */
  triggerTimeoutMs?: number | undefined;
  /**
   * The mutation runs inside a trigger's transaction: it fires no trigger
   * itself, so what a trigger writes never cascades.
   */
  inTrigger?: boolean | undefined;
  /**
   * `@populatedBy` callbacks already started, by the input they compute
   * for and the field's name: those of the mutation's own inputs start
   * before the transaction opens, all at once, so they do not run under
   * the writer lock one after another. Each is awaited where it would
   * have been called, so a failure surfaces at the same point.
   */
  computed?: WeakMap<object, Map<string, Promise<unknown>>> | undefined;
  /** The `validators` option: a Standard Schema per node type. */
  validators?: Readonly<Record<string, TypeValidator>> | undefined;
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
