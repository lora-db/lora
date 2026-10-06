// What `LoraGraphQL` hands back: execution results and their timing, and
// the reports of `assertSchema()` and `check()`.

import type { ExecutionResult } from "graphql";
import type { CypherFinding } from "./analyze/cypher-check.js";
import type { SchemaRequirement } from "./analyze/indexes.js";
import type { PlanReport } from "./analyze/plans.js";
import type { ReadSet } from "./compile/read.js";
import type { ModelWarning } from "./model/types.js";

/** `extensions.timing` of an `execute()` result (see the `timing` option). */
export interface ExecutionTiming {
  /** The whole `execute()` call: parse, validation, execution. */
  totalMs: number;
  /** Time spent running statements in LoraDB. */
  databaseMs: number;
  /** Per root field, by response key (alias or name). */
  fields: Record<string, { totalMs: number; databaseMs: number }>;
}

/** `execute()`'s result; `readSet` is there when `ExecuteArgs.readSet` asked. */
export type LoraExecutionResult = ExecutionResult & {
  readonly readSet?: ReadSet;
};

export interface SchemaAssertion {
  required: SchemaRequirement[];
  missing: SchemaRequirement[];
  created: SchemaRequirement[];
  /**
   * Full-text and vector indexes present under their name but defined
   * differently (labels, properties, kind): search would keep using the
   * old definition. Without `create`; with it they are re-created.
   */
  mismatched: SchemaRequirement[];
  /** Indexes `create` dropped and created again with the model's definition. */
  recreated: SchemaRequirement[];
}

export interface CheckReport {
  /** True when nothing below is a failure. */
  ok: boolean;
  /** Model warnings, e.g. an unused @cypher argument. */
  warnings: readonly ModelWarning[];
  /** @cypher statements the engine rejects or that write from a query. */
  cypher: CypherFinding[];
  /** Constraints and indexes the database lacks. */
  missing: SchemaRequirement[];
  /** Indexes the database has that no part of the API needs. */
  unused: Array<{
    name: string;
    type: string;
    labels: string[];
    properties: string[];
  }>;
  /** Schema lint: valid but costly or risky choices (not failures). */
  lint: readonly ModelWarning[];
  /**
   * `@mutation` types with writes no rule guards (failures): declare them
   * with `@authorization(public: [...])` when that is intended.
   */
  security: readonly ModelWarning[];
  /** Plan findings per operation and root field. */
  plans: Array<{ operation: string; field: string; reports: PlanReport[] }>;
  /** Operations that failed to compile. */
  errors: Array<{ operation: string; message: string }>;
}
