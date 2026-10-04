import { GraphQLError } from "graphql";

export interface ModelProblem {
  /** Type the problem is in, when it belongs to one. */
  type?: string;
  /** Field the problem is in, when it belongs to one. */
  field?: string;
  message: string;
}

/**
 * The annotated SDL is invalid. Carries every problem found, not just the
 * first, each located by type and field.
 */
export class ModelError extends Error {
  readonly problems: readonly ModelProblem[];

  constructor(problems: ModelProblem[]) {
    super(
      `Invalid LoraGraphQL type definitions (${problems.length} ${problems.length === 1 ? "problem" : "problems"}):\n` +
        problems.map((p) => "  - " + formatProblem(p)).join("\n"),
    );
    this.name = "ModelError";
    this.problems = problems;
  }
}

export function formatProblem(p: ModelProblem): string {
  const where =
    p.type && p.field ? `${p.type}.${p.field}` : (p.type ?? p.field);
  return where ? `${where}: ${p.message}` : p.message;
}

/**
 * Every `extensions.code` a request-time error of this library carries, in
 * a stable order. Use it to exhaustively map codes (HTTP status, client
 * copy) or to test membership at runtime.
 */
export const LORA_GRAPHQL_ERROR_CODES = Object.freeze([
  "BAD_USER_INPUT",
  "INVALID_CURSOR",
  "LIMIT_EXCEEDED",
  "COST_EXCEEDED",
  "UNAUTHENTICATED",
  "FORBIDDEN",
  "NOT_FOUND",
  "CONSTRAINT_VIOLATION",
  "DATABASE_ERROR",
  "PERSISTED_QUERY_ONLY",
  "WRONG_OPERATION_TYPE",
] as const);

export type LoraGraphQLErrorCode = (typeof LORA_GRAPHQL_ERROR_CODES)[number];

const ERROR_CODES: ReadonlySet<string> = new Set(LORA_GRAPHQL_ERROR_CODES);

/**
 * Whether `err` is a request-time error raised by this library: a
 * GraphQL error (or a plain `{ extensions: { code } }` from a serialized
 * response) whose `extensions.code` is one of `LORA_GRAPHQL_ERROR_CODES`.
 * Duck-typed, so it also recognizes errors built by another copy of
 * `graphql`.
 */
export function isLoraGraphQLError(
  err: unknown,
): err is { message: string; extensions: { code: LoraGraphQLErrorCode } } {
  if (typeof err !== "object" || err === null) return false;
  const ext = (err as { extensions?: unknown }).extensions;
  if (typeof ext !== "object" || ext === null) return false;
  const code = (ext as { code?: unknown }).code;
  return typeof code === "string" && ERROR_CODES.has(code);
}

/** A request-time error with a stable `extensions.code`. */
export function requestError(
  code: LoraGraphQLErrorCode,
  message: string,
  cause?: unknown,
  extensions: Record<string, unknown> = {},
): GraphQLError {
  return new GraphQLError(message, {
    extensions: { code, ...extensions },
    originalError: cause instanceof Error ? cause : undefined,
  });
}
