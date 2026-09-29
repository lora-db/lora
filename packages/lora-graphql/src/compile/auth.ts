// @authentication and @authorization, compiled into the statements.
//
// A rule is `{ node, jwt, AND, OR, NOT }`. The `jwt` half only reads the
// request's claims, so it is evaluated here, in JavaScript: role checks
// fold to true or false and never reach Cypher. The `node` half becomes a
// predicate, with `"$jwt.path"` strings bound as parameters. A claim a rule
// needs but the request lacks makes that rule deny.

import { requestError } from "../errors.js";
import type {
  AuthOperation,
  AuthorizationWhere,
  NodeType,
} from "../model/types.js";
import { and, lit, not, or, type Expr } from "./cypher.js";
import type { CompileContext } from "./context.js";
import { compileNodeWhere } from "./filter.js";

type Where = Record<string, unknown>;
type Folded = Expr | boolean;

/** Throws UNAUTHENTICATED when reading `field` needs a jwt the request lacks. */
export function checkFieldAuthentication(
  ctx: CompileContext,
  owner: string,
  field: {
    name: string;
    authentication: ReadonlySet<AuthOperation> | undefined;
  },
): void {
  if (!ctx.inAuth && field.authentication?.has("READ") && !ctx.jwt) {
    throw requestError(
      "UNAUTHENTICATED",
      `${owner}.${field.name} needs an authenticated request`,
    );
  }
}

/** Throws UNAUTHENTICATED when `op` on `node` needs a jwt the request lacks. */
export function checkAuthentication(
  ctx: CompileContext,
  node: NodeType,
  op: AuthOperation,
): void {
  if (!ctx.inAuth && node.authentication?.has(op) && !ctx.jwt) {
    throw requestError(
      "UNAUTHENTICATED",
      `${op.toLowerCase()} on ${node.name} needs an authenticated request`,
    );
  }
}

/**
 * The filter `op` applies to `variable` (a `node` of that type): nodes
 * failing it are invisible. Rules grant: any passing rule admits the node.
 * Undefined when the type has no filter rule for `op`.
 */
export function authFilter(
  ctx: CompileContext,
  node: NodeType,
  variable: string,
  op: AuthOperation,
): Expr | undefined {
  if (ctx.inAuth) return undefined;
  const rules = (node.authorization?.filter ?? []).filter((r) =>
    r.operations.has(op),
  );
  if (rules.length === 0) return undefined;
  const folded = rules.map((r) =>
    r.requireAuthentication && !ctx.jwt
      ? false
      : compileRule(ctx, node, variable, r.where),
  );
  return toExpr(foldOr(folded));
}

/**
 * The condition `op` must satisfy at `when`, or the request fails with
 * FORBIDDEN. Undefined when no rule applies; throws right away when the
 * claims alone already decide against the request.
 */
export function authValidate(
  ctx: CompileContext,
  node: NodeType,
  variable: string,
  op: AuthOperation,
  when: "BEFORE" | "AFTER",
): Expr | undefined {
  if (ctx.inAuth) return undefined;
  const rules = (node.authorization?.validate ?? []).filter(
    (r) => r.operations.has(op) && r.when.has(when),
  );
  if (rules.length === 0) return undefined;
  if (rules.every((r) => r.requireAuthentication) && !ctx.jwt) {
    throw requestError(
      "UNAUTHENTICATED",
      `${op.toLowerCase()} on ${node.name} needs an authenticated request`,
    );
  }
  const folded = foldOr(
    rules.map((r) =>
      r.requireAuthentication && !ctx.jwt
        ? false
        : compileRule(ctx, node, variable, r.where),
    ),
  );
  if (folded === false) throw forbidden(node, op);
  return toExpr(folded);
}

export function forbidden(node: NodeType, op: AuthOperation) {
  return requestError(
    "FORBIDDEN",
    `not allowed to ${op.toLowerCase()} this ${node.name}`,
  );
}

/** A rule needs a claim the request lacks: the whole rule denies. */
class MissingClaim extends Error {}

function compileRule(
  ctx: CompileContext,
  node: NodeType,
  variable: string,
  where: AuthorizationWhere,
): Folded {
  try {
    return compileRulePart(ctx, node, variable, where);
  } catch (err) {
    // Caught at the rule, not inside it: a NOT cannot turn a missing
    // claim into a grant.
    if (err instanceof MissingClaim) return false;
    throw err;
  }
}

function compileRulePart(
  ctx: CompileContext,
  node: NodeType,
  variable: string,
  where: AuthorizationWhere,
): Folded {
  const parts: Folded[] = [];
  for (const [key, value] of Object.entries(where)) {
    if (value === null || value === undefined) continue;
    if (key === "AND") {
      parts.push(
        foldAnd(
          (value as Where[]).map((w) =>
            compileRulePart(ctx, node, variable, w),
          ),
        ),
      );
    } else if (key === "OR") {
      parts.push(
        foldOr(
          (value as Where[]).map((w) =>
            compileRulePart(ctx, node, variable, w),
          ),
        ),
      );
    } else if (key === "NOT") {
      const inner = compileRulePart(ctx, node, variable, value as Where);
      parts.push(typeof inner === "boolean" ? !inner : not(inner));
    } else if (key === "jwt") {
      parts.push(matchClaims(ctx.jwt, value as Where));
    } else if (key === "node") {
      const bound = substitute(value, ctx.jwt);
      if (!bound.ok) throw new MissingClaim();
      const wasInAuth = ctx.inAuth;
      ctx.inAuth = true;
      try {
        const pred = compileNodeWhere(
          ctx,
          node,
          variable,
          bound.value as Where,
        );
        // Rules are checked non-empty at startup; if one still compiles
        // to nothing, deny rather than grant.
        parts.push(pred ?? false);
      } finally {
        ctx.inAuth = wasInAuth;
      }
    }
  }
  return foldAnd(parts);
}

function foldAnd(parts: Folded[]): Folded {
  if (parts.some((p) => p === false)) return false;
  const exprs = parts.filter((p): p is Expr => typeof p !== "boolean");
  return and(...exprs) ?? true;
}

function foldOr(parts: Folded[]): Folded {
  if (parts.some((p) => p === true)) return true;
  const exprs = parts.filter((p): p is Expr => typeof p !== "boolean");
  return or(...exprs) ?? false;
}

function toExpr(f: Folded): Expr | undefined {
  if (f === true) return undefined;
  if (f === false) return lit(false);
  return f;
}

// ---------------------------------------------------------------------------
// Claims
// ---------------------------------------------------------------------------

export function claim(
  jwt: Record<string, unknown> | undefined,
  path: string,
): unknown {
  let cur: unknown = jwt;
  for (const part of path.split(".")) {
    if (cur === null || typeof cur !== "object") return undefined;
    cur = (cur as Record<string, unknown>)[part];
  }
  return cur;
}

/** Replace `"$jwt.path"` strings; not ok when a referenced claim is absent. */
function substitute(
  value: unknown,
  jwt: Record<string, unknown> | undefined,
): { ok: boolean; value: unknown } {
  if (typeof value === "string" && value.startsWith("$jwt.")) {
    const v = claim(jwt, value.slice(5));
    return v === undefined || v === null
      ? { ok: false, value: undefined }
      : { ok: true, value: v };
  }
  if (Array.isArray(value)) {
    const items = value.map((x) => substitute(x, jwt));
    return {
      ok: items.every((i) => i.ok),
      value: items.map((i) => i.value),
    };
  }
  if (value !== null && typeof value === "object") {
    const out: Record<string, unknown> = {};
    for (const [k, v] of Object.entries(value)) {
      const r = substitute(v, jwt);
      if (!r.ok) return { ok: false, value: undefined };
      out[k] = r.value;
    }
    return { ok: true, value: out };
  }
  return { ok: true, value };
}

function matchClaims(
  jwt: Record<string, unknown> | undefined,
  where: Where,
): boolean {
  if (!jwt) return false;
  return Object.entries(where).every(([path, ops]) => {
    const v = claim(jwt, path);
    return Object.entries(ops as Where).every(([op, expected]) =>
      claimOp(v, op, expected),
    );
  });
}

function claimOp(v: unknown, op: string, expected: unknown): boolean {
  switch (op) {
    case "exists":
      return (v !== undefined && v !== null) === expected;
    case "eq":
      return v !== undefined && JSON.stringify(v) === JSON.stringify(expected);
    case "in":
      return Array.isArray(expected) && expected.some((e) => e === v);
    case "includes":
      return Array.isArray(v) && v.includes(expected);
    case "contains":
      return typeof v === "string" && v.includes(String(expected));
    case "startsWith":
      return typeof v === "string" && v.startsWith(String(expected));
    case "endsWith":
      return typeof v === "string" && v.endsWith(String(expected));
    case "lt":
      return typeof v === "number" && v < (expected as number);
    case "lte":
      return typeof v === "number" && v <= (expected as number);
    case "gt":
      return typeof v === "number" && v > (expected as number);
    case "gte":
      return typeof v === "number" && v >= (expected as number);
    default:
      return false;
  }
}
