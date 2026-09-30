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
  Authorization,
  AuthorizationValidateRule,
  AuthorizationWhere,
  NodeType,
} from "../model/types.js";
import { PLACEHOLDER } from "../model/types.js";
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
    authenticationJwt?: AuthorizationWhere | undefined;
  },
  op: AuthOperation = "READ",
): void {
  if (ctx.inAuth || !field.authentication?.has(op)) return;
  if (!ctx.jwt || !claimsSatisfy(ctx, field.authenticationJwt)) {
    throw requestError(
      "UNAUTHENTICATED",
      `${owner}.${field.name} needs an authenticated request`,
    );
  }
}

/** Claims `@authentication(jwt:)` demands; true when there is none. */
function claimsSatisfy(
  ctx: CompileContext,
  where: AuthorizationWhere | undefined,
): boolean {
  if (!where) return true;
  try {
    return compileRulePart(ctx, undefined, "", { jwt: where }) === true;
  } catch (err) {
    if (err instanceof MissingClaim) return false;
    throw err;
  }
}

/** Throws UNAUTHENTICATED when `op` on `node` needs a jwt the request lacks. */
export function checkAuthentication(
  ctx: CompileContext,
  node: NodeType,
  op: AuthOperation,
): void {
  if (
    !ctx.inAuth &&
    node.authentication?.has(op) &&
    (!ctx.jwt || !claimsSatisfy(ctx, node.authenticationJwt))
  ) {
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
  return validateRules(
    ctx,
    node,
    variable,
    op,
    (node.authorization?.validate ?? []).filter(
      (r) => r.operations.has(op) && r.when.has(when),
    ),
  );
}

/**
 * Field-level validate rules for `op` on `field` of `variable`: the
 * condition reading (or writing) the field needs. Undefined when none.
 */
export function fieldValidate(
  ctx: CompileContext,
  node: NodeType,
  field: { authorization?: Authorization | undefined },
  variable: string,
  op: AuthOperation,
  when: "BEFORE" | "AFTER" = "BEFORE",
): Expr | undefined {
  return validateRules(
    ctx,
    node,
    variable,
    op,
    (field.authorization?.validate ?? []).filter(
      (r) => r.operations.has(op) && r.when.has(when),
    ),
  );
}

function validateRules(
  ctx: CompileContext,
  node: NodeType,
  variable: string,
  op: AuthOperation,
  rules: readonly AuthorizationValidateRule[],
): Expr | undefined {
  if (ctx.inAuth || rules.length === 0) return undefined;
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

/**
 * Whether `op` may touch a relationship property with field-level rules.
 * A @relationshipProperties type can sit under several relationship
 * fields, from either end, so its rules test claims only (checked at
 * startup) and fold to a yes or no here. `"unauthenticated"` when every
 * rule needs a token and the request has none.
 */
export function propertyAccess(
  ctx: CompileContext,
  field: {
    authentication: ReadonlySet<AuthOperation> | undefined;
    authenticationJwt?: AuthorizationWhere | undefined;
    authorization?: Authorization | undefined;
  },
  op: AuthOperation,
): "allowed" | "forbidden" | "unauthenticated" {
  if (ctx.inAuth) return "allowed";
  if (
    field.authentication?.has(op) &&
    (!ctx.jwt || !claimsSatisfy(ctx, field.authenticationJwt))
  ) {
    return "unauthenticated";
  }
  const rules = (field.authorization?.validate ?? []).filter((r) =>
    r.operations.has(op),
  );
  if (rules.length === 0) return "allowed";
  if (rules.every((r) => r.requireAuthentication) && !ctx.jwt) {
    return "unauthenticated";
  }
  const granted = rules.some(
    (r) =>
      !(r.requireAuthentication && !ctx.jwt) &&
      compileRule(ctx, undefined, "", r.where) === true,
  );
  return granted ? "allowed" : "forbidden";
}

/** Throws unless `op` may write (or filter by) the relationship property. */
export function checkPropertyAccess(
  ctx: CompileContext,
  type: string,
  field: Parameters<typeof propertyAccess>[1] & { name: string },
  op: AuthOperation,
): void {
  const access = propertyAccess(ctx, field, op);
  if (access === "unauthenticated") {
    throw requestError(
      "UNAUTHENTICATED",
      `${type}.${field.name} needs an authenticated request`,
    );
  }
  if (access === "forbidden") {
    throw requestError(
      "FORBIDDEN",
      `not allowed to ${op.toLowerCase()} ${type}.${field.name}`,
    );
  }
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
  node: NodeType | undefined,
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

/**
 * A test that needs a claim (or context value) the request lacks is
 * unknown. Unknown is false where it stands, but a NOT cannot turn it into
 * true: under a NOT, it makes the whole negated branch false, up to the
 * nearest AND / OR outside every NOT (or the rule itself).
 */
function compileRulePart(
  ctx: CompileContext,
  node: NodeType | undefined,
  variable: string,
  where: AuthorizationWhere,
  underNot = false,
): Folded {
  const child = (w: Where, negated = underNot): Folded => {
    if (negated) return compileRulePart(ctx, node, variable, w, true);
    try {
      return compileRulePart(ctx, node, variable, w, false);
    } catch (err) {
      if (err instanceof MissingClaim) return false;
      throw err;
    }
  };
  const parts: Folded[] = [];
  for (const [key, value] of Object.entries(where)) {
    if (value === null || value === undefined) continue;
    if (key === "AND") {
      parts.push(foldAnd((value as Where[]).map((w) => child(w))));
    } else if (key === "OR") {
      parts.push(foldOr((value as Where[]).map((w) => child(w))));
    } else if (key === "NOT") {
      const inner = compileRulePart(ctx, node, variable, value as Where, true);
      parts.push(typeof inner === "boolean" ? !inner : not(inner));
    } else if (key === "jwt") {
      parts.push(matchClaims(ctx, value as Where));
    } else if (key === "node" && node) {
      const bound = substitute(value, ctx);
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

/**
 * A claim by name. With a `@jwt` type, the name's first segment maps to its
 * `@jwtClaim(path:)`, whether the token arrived decoded or not.
 */
export function claim(ctx: CompileContext, path: string): unknown {
  const [head, ...rest] = path.split(".");
  const mapped = ctx.model.jwt?.get(head!) ?? head!;
  return lookupPath(ctx.jwt, [mapped, ...rest].join("."));
}

/**
 * A value by dotted path, through own properties only: `$context.x` must
 * not reach `constructor`, `__proto__` or anything else inherited.
 */
export function lookupPath(root: unknown, path: string): unknown {
  let cur: unknown = root;
  for (const part of path.split(".")) {
    if (cur === null || typeof cur !== "object" || !Object.hasOwn(cur, part)) {
      return undefined;
    }
    cur = (cur as Record<string, unknown>)[part];
  }
  return cur;
}

/**
 * Structural equality for claim values: arrays in order, objects by own
 * keys in any order, bigints and numbers by value.
 */
export function sameValue(a: unknown, b: unknown): boolean {
  if (typeof a === "bigint" || typeof b === "bigint") {
    return (
      (typeof a === "bigint" || typeof a === "number") &&
      (typeof b === "bigint" || typeof b === "number") &&
      a == b
    );
  }
  if (a === b) return true;
  if (
    a === null ||
    b === null ||
    typeof a !== "object" ||
    typeof b !== "object"
  ) {
    return false;
  }
  if (Array.isArray(a) || Array.isArray(b)) {
    return (
      Array.isArray(a) &&
      Array.isArray(b) &&
      a.length === b.length &&
      a.every((x, i) => sameValue(x, b[i]))
    );
  }
  const ka = Object.keys(a);
  const kb = Object.keys(b);
  return (
    ka.length === kb.length &&
    ka.every(
      (k) =>
        Object.hasOwn(b, k) &&
        sameValue(
          (a as Record<string, unknown>)[k],
          (b as Record<string, unknown>)[k],
        ),
    )
  );
}

/**
 * Replace `"$jwt.path"` and `"$context.path"` strings, and the
 * `${jwt.path}` / `${context.path}` placeholders inside strings; not ok
 * when a referenced value is absent (or, in a placeholder, not a scalar).
 */
function substitute(
  value: unknown,
  ctx: CompileContext,
): { ok: boolean; value: unknown } {
  if (typeof value === "string" && value.includes("${")) {
    let ok = true;
    const built = value.replace(
      PLACEHOLDER,
      (_, source: string, path: string) => {
        let v: unknown;
        if (source === "jwt") v = claim(ctx, path);
        else {
          v = lookupPath(ctx.requestContext, path);
          ctx.contextReads.push([path, v]);
        }
        if (
          typeof v === "string" ||
          typeof v === "number" ||
          typeof v === "bigint" ||
          typeof v === "boolean"
        ) {
          return String(v);
        }
        ok = false;
        return "";
      },
    );
    return ok ? { ok, value: built } : { ok, value: undefined };
  }
  if (
    typeof value === "string" &&
    (value.startsWith("$jwt.") || value.startsWith("$context."))
  ) {
    let v: unknown;
    if (value.startsWith("$jwt.")) v = claim(ctx, value.slice(5));
    else {
      v = lookupPath(ctx.requestContext, value.slice(9));
      ctx.contextReads.push([value.slice(9), v]);
    }
    return v === undefined || v === null
      ? { ok: false, value: undefined }
      : { ok: true, value: v };
  }
  if (Array.isArray(value)) {
    const items = value.map((x) => substitute(x, ctx));
    return {
      ok: items.every((i) => i.ok),
      value: items.map((i) => i.value),
    };
  }
  if (value !== null && typeof value === "object") {
    const out: Record<string, unknown> = {};
    for (const [k, v] of Object.entries(value)) {
      const r = substitute(v, ctx);
      if (!r.ok) return { ok: false, value: undefined };
      out[k] = r.value;
    }
    return { ok: true, value: out };
  }
  return { ok: true, value };
}

function matchClaims(ctx: CompileContext, where: Where): boolean {
  if (!ctx.jwt) throw new MissingClaim();
  return Object.entries(where).every(([path, ops]) => {
    const v = claim(ctx, path);
    return Object.entries(ops as Where).every(([op, expected]) => {
      // A test of a claim the token lacks cannot pass or fail: the whole
      // rule denies, even under NOT. Only `exists` asks about absence.
      if (op !== "exists" && (v === undefined || v === null)) {
        throw new MissingClaim();
      }
      return claimOp(v, op, expected);
    });
  });
}

function claimOp(v: unknown, op: string, expected: unknown): boolean {
  switch (op) {
    case "exists":
      return (v !== undefined && v !== null) === expected;
    case "eq":
      return v !== undefined && sameValue(v, expected);
    case "in":
      return Array.isArray(expected) && expected.some((e) => sameValue(e, v));
    case "includes":
      return Array.isArray(v) && v.some((x) => sameValue(x, expected));
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
