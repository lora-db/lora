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

export type LoraGraphQLErrorCode =
  | "BAD_USER_INPUT"
  | "INVALID_CURSOR"
  | "LIMIT_EXCEEDED"
  | "COST_EXCEEDED"
  | "UNAUTHENTICATED"
  | "FORBIDDEN"
  | "NOT_FOUND"
  | "CONSTRAINT_VIOLATION"
  | "DATABASE_ERROR";

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
