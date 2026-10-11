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
  "OVERLOADED",
  "TIMEOUT",
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

/** One value a mutation was given, or computed, that breaks a field's rule. */
export interface Violation {
  /** Where the value is in the mutation's arguments: keys and list indices. */
  path: Array<string | number>;
  /** The model field whose rule it is, as `Type.field`. */
  coordinate: string;
  /** The rule, named as its JSON Schema keyword (`maxLength`, `format`, ...). */
  rule: string;
  /** The rule's own numbers and names, enough to localise the message. */
  params: Record<string, unknown>;
  message: string;
}

/** The most violations one error lists; a batch can hold thousands. */
export const MAX_VIOLATIONS = 100;

const pathText = (path: ReadonlyArray<string | number>): string =>
  path.reduce<string>(
    (text, part) =>
      typeof part === "number"
        ? `${text}[${part}]`
        : text === ""
          ? part
          : `${text}.${part}`,
    "",
  );

/**
 * Every value a mutation got wrong, in one error: `extensions.violations`
 * lists each with its path, rule and parameters. The values themselves
 * are never included: a rule is often on a password or an address.
 */
export function validationError(
  code: "BAD_USER_INPUT" | "CONSTRAINT_VIOLATION",
  violations: readonly Violation[],
  total: number = violations.length,
): GraphQLError {
  const shown = violations
    .slice(0, 3)
    .map((v) => `${pathText(v.path) || v.coordinate} ${v.message}`);
  const more = total - shown.length;
  return requestError(
    code,
    `${total === 1 ? "1 value is" : `${total} values are`} not valid: ${shown.join("; ")}${more > 0 ? `; and ${more} more` : ""}`,
    undefined,
    {
      violations,
      ...(total > violations.length ? { violationCount: total } : {}),
    },
  );
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
