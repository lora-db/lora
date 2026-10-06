export { LoraGraphQL } from "./lora-graphql.js";
export {
  type AssertSchemaOptions,
  type CheckOptions,
  type CostEvent,
  type DatabaseErrorEvent,
  type ExecuteArgs,
  type LoraGraphQLContext,
  type LoraGraphQLOptions,
  type StatementEvent,
} from "./options.js";
export {
  type CheckReport,
  type ExecutionTiming,
  type LoraExecutionResult,
  type SchemaAssertion,
} from "./results.js";
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
export { buildModel } from "./model/build.js";
export { type ModelOptions } from "./model/build/limits.js";
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
export type { AccessEntry, AccessVerdict } from "./analyze/verdicts.js";
export type { OperationAccess, RootFieldAccess } from "./analyze/access.js";
export type { CypherFinding } from "./analyze/cypher-check.js";
export {
  diffSchemas,
  type ApiChange,
  type SchemaDiff,
} from "./analyze/diff.js";
export type {
  CompiledRead,
  ReadSet,
  SeekExpectation,
} from "./compile/read/types.js";
export type {
  EntityRef,
  RelationshipRef,
  WriteChange,
} from "./execute/changes.js";
export type {
  MutationInfo,
  PopulatedByCallback,
} from "./execute/mutate/env.js";
export type { MutationKind } from "./schema/mutations.js";
export type { ChangeEvent } from "./schema/hooks.js";
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
