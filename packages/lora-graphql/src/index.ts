export {
  LoraGraphQL,
  type AssertSchemaOptions,
  type CheckOptions,
  type CheckReport,
  type CostEvent,
  type DatabaseErrorEvent,
  type ExecuteArgs,
  type ExecutionTiming,
  type LoraGraphQLContext,
  type LoraGraphQLOptions,
  type LoraExecutionResult,
  type SchemaAssertion,
  type StatementEvent,
} from "./lora-graphql.js";
export {
  loraDriver,
  type DriverTransaction,
  type LoraDatabaseLike,
  type LoraDriver,
  type QueryPlan,
  type QueryResult,
  type RunOptions,
  type Statement,
} from "./driver.js";
export { directiveTypeDefs } from "./model/directives.js";
export { buildModel, type ModelOptions } from "./model/build.js";
export type * from "./model/types.js";
export {
  inferRequirements,
  requirementDdl,
  type SchemaRequirement,
} from "./analyze/indexes.js";
export {
  checkPlans,
  scanExpands,
  type PlanFinding,
  type PlanReport,
} from "./analyze/plans.js";
export type { DegreeStats, Statistics } from "./analyze/statistics.js";
export type { AccessEntry, AccessVerdict } from "./analyze/access.js";
export type { CypherFinding } from "./analyze/cypher-check.js";
export {
  diffSchemas,
  type ApiChange,
  type SchemaDiff,
} from "./analyze/diff.js";
export type { CompiledRead, ReadSet, SeekExpectation } from "./compile/read.js";
export type {
  EntityRef,
  RelationshipRef,
  WriteChange,
} from "./execute/changes.js";
export type { MutationInfo, PopulatedByCallback } from "./execute/mutate.js";
export type { MutationKind } from "./schema/mutations.js";
export type { ChangeEvent } from "./schema/build.js";
export { LoraTransaction } from "./execute/transaction.js";
export {
  ModelError,
  formatProblem,
  isLoraGraphQLError,
  LORA_GRAPHQL_ERROR_CODES,
  type LoraGraphQLErrorCode,
  type ModelProblem,
} from "./errors.js";
export { toGlobalId, fromGlobalId } from "./schema/global-id.js";
export type {
  Attributes,
  MetricsLike,
  ObservabilityOptions,
  SpanLike,
  StatementEndEvent,
  TracerLike,
} from "./observe.js";
export {
  schemaHash,
  type ManifestOperation,
  type OperationManifest,
} from "./codegen.js";
export {
  envelopPlugin,
  parseOptions,
  validationRules,
  DEFAULT_GUARDS,
  type DocumentGuards,
} from "./guards.js";
