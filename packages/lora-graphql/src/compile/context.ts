import type { Statistics } from "../analyze/statistics.js";
import type { GraphModel } from "../model/types.js";
import { param, type Expr } from "./cypher.js";
import type { SelectionContext } from "./selection.js";

/** State shared while compiling one root field into one statement. */
export interface CompileContext extends SelectionContext {
  model: GraphModel;
  params: Record<string, unknown>;
  vars: Set<string>;
  /** Labels and relationship types the statement reads (S5 read-set). */
  reads: { labels: Set<string>; relationships: Set<string> };
  /** The request's claims, or undefined when unauthenticated. */
  jwt: Record<string, unknown> | undefined;
  /** Compiling an authorization rule: nested rules are not applied. */
  inAuth: boolean;
  /** Estimated rows the statement touches (for the cost limit). */
  cost: number;
  /** Degree statistics from `analyze()`, by `Owner.field`. */
  degrees: ReadonlyMap<string, number>;
  /** Node counts and degrees from `analyze()`, for filter costs. */
  statistics?: Statistics | undefined;
  /** Relationship levels one `where` may nest (`maxFilterDepth`). */
  maxFilterDepth?: number | undefined;
  /** Items an `in` filter operand may hold (`maxListFilter`). */
  maxListFilter?: number | undefined;
  /** Characters a string filter operand may hold (`maxStringFilter`). */
  maxStringFilter?: number | undefined;
  /** Relationship levels of the `where` being compiled, so far. */
  filterDepth?: number | undefined;
  /** The GraphQL context, for `"$context.path"` values in rules. */
  requestContext: unknown;
  /**
   * Variables holding computed (`@cypher`) field values in the current
   * statement, by `variable + "\0" + field`: set by the root match.
   */
  computed: Map<string, string>;
  /**
   * `$context` paths the compile read, with their values: a cached
   * compile is reused only for a context that agrees on them.
   */
  contextReads: Array<[string, unknown]>;
  /**
   * Claims the compile read, by the path looked up in the token (`""` is
   * the whole token). `bound` stays true while every read only substituted
   * the value into a rule's `node` filter, so a cached compile may rebind
   * it as a parameter; otherwise the value may shape the statement text.
   */
  claimReads: Map<string, { value: unknown; bound: boolean }>;
  /** The most items a @cypher list argument takes without `@size(max:)`. */
  maxListArgument?: number | undefined;
}

export function newContext(
  base: SelectionContext,
  model: GraphModel,
  options: {
    jwt?: Record<string, unknown> | undefined;
    degrees?: ReadonlyMap<string, number>;
    statistics?: Statistics | undefined;
    requestContext?: unknown;
    maxListArgument?: number | undefined;
    maxFilterDepth?: number | undefined;
    maxListFilter?: number | undefined;
    maxStringFilter?: number | undefined;
  } = {},
): CompileContext {
  return {
    ...base,
    model,
    params: {},
    vars: new Set(["this"]),
    reads: { labels: new Set(), relationships: new Set() },
    jwt: options.jwt,
    inAuth: false,
    cost: 0,
    degrees: options.degrees ?? new Map(),
    statistics: options.statistics,
    maxFilterDepth: options.maxFilterDepth,
    maxListFilter: options.maxListFilter,
    maxStringFilter: options.maxStringFilter,
    requestContext: options.requestContext,
    computed: new Map(),
    contextReads: [],
    claimReads: new Map(),
    maxListArgument: options.maxListArgument,
  };
}

/** Record that the compile read the claim at `path` (see `claimReads`). */
export function noteClaim(
  ctx: CompileContext,
  path: string,
  value: unknown,
  bound: boolean,
): void {
  const seen = ctx.claimReads.get(path);
  ctx.claimReads.set(path, { value, bound: bound && (seen?.bound ?? true) });
}

/** Bind a value as the next positional parameter `$pN`. */
/**
 * A rule value that is an expression, not a parameter: a string with
 * `${viewer.field}` placeholders, read from the caller's node in the
 * statement. `bind` returns the expression as is.
 */
export const EXPR = Symbol("expr");

export function exprValue(expr: Expr): { [EXPR]: Expr } {
  return { [EXPR]: expr };
}

export function bind(ctx: CompileContext, value: unknown): Expr {
  if (value !== null && typeof value === "object" && EXPR in value) {
    return (value as { [EXPR]: Expr })[EXPR];
  }
  const name = `p${Object.keys(ctx.params).length}`;
  ctx.params[name] = value;
  return param(name);
}

/** A variable name derived from `base`, unique within the statement. */
export function freshVar(ctx: CompileContext, base: string): string {
  let candidate = base;
  for (let i = 1; ctx.vars.has(candidate); i++) candidate = `${base}${i}`;
  ctx.vars.add(candidate);
  return candidate;
}
