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
  /** The GraphQL context, for `"$context.path"` values in rules. */
  requestContext: unknown;
}

export function newContext(
  base: SelectionContext,
  model: GraphModel,
  options: {
    jwt?: Record<string, unknown> | undefined;
    degrees?: ReadonlyMap<string, number>;
    requestContext?: unknown;
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
    requestContext: options.requestContext,
  };
}

/** Bind a value as the next positional parameter `$pN`. */
export function bind(ctx: CompileContext, value: unknown): Expr {
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
