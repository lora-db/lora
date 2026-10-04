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
  | "TIMEOUT"
  | "UNAUTHENTICATED"
  | "FORBIDDEN"
  | "NOT_FOUND"
  | "CONSTRAINT_VIOLATION"
  | "DATABASE_ERROR"
  | "PERSISTED_QUERY_ONLY"
  | "WRONG_OPERATION_TYPE";

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

/**
 * One error per field path pattern: errors with the same message and code
 * at paths that differ only in list indices (a field every row of a list
 * refuses, such as an @authentication field read anonymously) become one
 * error at the first path, with `extensions.count` and
 * `extensions.pathPattern` (indices as `"*"`). A page of 100 x 100 rows
 * otherwise answers with 10 000 copies of one error.
 */
export function collapseErrors(
  errors: readonly GraphQLError[],
): GraphQLError[] {
  const groups = new Map<string, { first: GraphQLError; count: number }>();
  const out: Array<GraphQLError | { group: string }> = [];
  for (const error of errors) {
    const path = error.path;
    if (!path?.some((p) => typeof p === "number")) {
      out.push(error);
      continue;
    }
    const pattern = path.map((p) => (typeof p === "number" ? "*" : p));
    const key = JSON.stringify([
      error.message,
      error.extensions["code"] ?? null,
      pattern,
    ]);
    const group = groups.get(key);
    if (group) group.count++;
    else {
      groups.set(key, { first: error, count: 1 });
      out.push({ group: key });
    }
  }
  return out.map((item) => {
    if (item instanceof GraphQLError) return item;
    const { first, count } = groups.get(item.group)!;
    if (count === 1) return first;
    return new GraphQLError(first.message, {
      ...(first.nodes ? { nodes: first.nodes } : {}),
      ...(first.source ? { source: first.source } : {}),
      ...(first.positions ? { positions: first.positions } : {}),
      path: first.path!,
      ...(first.originalError ? { originalError: first.originalError } : {}),
      extensions: {
        ...first.extensions,
        count,
        pathPattern: first.path!.map((p) => (typeof p === "number" ? "*" : p)),
      },
    });
  });
}
