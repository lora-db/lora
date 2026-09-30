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
  RelationshipField,
  RelationshipOperation,
  RelationshipPropertiesType,
} from "../model/types.js";
import { PLACEHOLDER } from "../model/types.js";
import { and, bin, fn, lit, not, or, type Expr } from "./cypher.js";
import { bind, freshVar, noteClaim, type CompileContext } from "./context.js";
import { compileNodeWhere, compilePropsWhere } from "./filter.js";

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
  if (ctx.inAuth || bypassed(ctx, node)) return undefined;
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

/**
 * The per-row condition for reading a field with field-level READ rules
 * (`@authorization(validate:)`, `@authentication`), for projecting it. It
 * never throws: a request without the token a rule needs gets a condition
 * that is false on every row, so the statement has the same shape with or
 * without a token and its plan can be checked anonymously. `code` is the
 * error a failing row reads as. Undefined when nothing guards the field.
 */
export function fieldReadGuard(
  ctx: CompileContext,
  node: NodeType,
  field: {
    authentication: ReadonlySet<AuthOperation> | undefined;
    authenticationJwt?: AuthorizationWhere | undefined;
    authorization?: Authorization | undefined;
  },
  variable: string,
): { when: Expr; code: "FORBIDDEN" | "UNAUTHENTICATED" } | undefined {
  if (ctx.inAuth) return undefined;
  if (
    field.authentication?.has("READ") &&
    (!ctx.jwt || !claimsSatisfy(ctx, field.authenticationJwt))
  ) {
    return { when: lit(false), code: "UNAUTHENTICATED" };
  }
  const rules = (field.authorization?.validate ?? []).filter(
    (r) => r.operations.has("READ") && r.when.has("BEFORE"),
  );
  if (rules.length === 0 || bypassed(ctx, node)) return undefined;
  if (rules.every((r) => r.requireAuthentication) && !ctx.jwt) {
    return { when: lit(false), code: "UNAUTHENTICATED" };
  }
  const folded = foldOr(
    rules.map((r) =>
      r.requireAuthentication && !ctx.jwt
        ? false
        : compileRule(ctx, node, variable, r.where),
    ),
  );
  if (folded === true) return undefined;
  return { when: folded === false ? lit(false) : folded, code: "FORBIDDEN" };
}

function validateRules(
  ctx: CompileContext,
  node: NodeType,
  variable: string,
  op: AuthOperation,
  rules: readonly AuthorizationValidateRule[],
): Expr | undefined {
  if (ctx.inAuth || rules.length === 0 || bypassed(ctx, node)) {
    return undefined;
  }
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
  if (rules.length === 0 || bypassed(ctx, undefined)) return "allowed";
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

/**
 * The schema's bypass (`@authorizationDefaults(bypass:)`) grants this
 * request: it skips every filter and validate rule, except on a type with
 * `@authorization(bypass: false)`. Claims only, so it is decided here.
 */
function bypassed(ctx: CompileContext, node: NodeType | undefined): boolean {
  const bypass = ctx.model.bypass;
  if (!bypass || !ctx.jwt || node?.authorization?.bypass === false) {
    return false;
  }
  try {
    return compileRulePart(ctx, undefined, "", bypass) === true;
  } catch (err) {
    if (err instanceof MissingClaim) return false;
    throw err;
  }
}

/**
 * A scalar field's value as the reader may see it: under
 * `@authorization(mask:)`, a row failing `unless` reads the mask's value.
 * Rules see the stored value, and so does a request the bypass lets
 * through. `raw` is the stored value's expression.
 */
export function maskedValue(
  ctx: CompileContext,
  node: NodeType,
  field: { authorization?: Authorization | undefined },
  variable: string,
  raw: Expr,
): Expr {
  const masks = field.authorization?.mask;
  if (!masks?.length || ctx.inAuth || bypassed(ctx, node)) return raw;
  let value = raw;
  // The first mask that fails decides: build from the last one out.
  for (const m of [...masks].reverse()) {
    const unless = compileRule(ctx, node, variable, m.unless);
    if (unless === true) continue;
    const substitute = bind(ctx, m.value);
    value =
      unless === false
        ? substitute
        : {
            kind: "case",
            when: fn("coalesce", unless, lit(false)),
            then: value,
            else: substitute,
          };
  }
  return value;
}

/**
 * Whether a masked field reads its stored value on every row for this
 * request (no mask, or every `unless` settled true by the claims). Sorting,
 * grouping and aggregating need it: a per-row mask there would reveal or
 * reorder by the hidden values.
 */
export function maskSettled(
  ctx: CompileContext,
  node: NodeType,
  field: { authorization?: Authorization | undefined },
): boolean {
  const masks = field.authorization?.mask;
  if (!masks?.length || ctx.inAuth || bypassed(ctx, node)) return true;
  const probe = { ...ctx, params: {}, vars: new Set(ctx.vars) };
  return masks.every(
    (m) => compileRule(probe, node, "probe", m.unless) === true,
  );
}

/**
 * `@key(scope: VIEWER)`: a created key must start with the caller's
 * `@viewer` claim and the separator. Checked in JavaScript before any
 * statement runs, so the answer never depends on whether the key exists.
 * A claim containing the separator is refused: it could reach into
 * another caller's key space (`a` taking `a:b:…` from `a:b`).
 */
export function checkKeyScope(
  ctx: CompileContext,
  node: NodeType,
  key: unknown,
): void {
  const scope = node.key.keyScope;
  const mapping = ctx.model.viewer;
  if (!scope || !mapping || ctx.inAuth || bypassed(ctx, node)) return;
  if (!ctx.jwt) {
    throw requestError(
      "UNAUTHENTICATED",
      `create on ${node.name} needs an authenticated request`,
    );
  }
  const owner = claim(ctx, mapping.claim);
  const prefix =
    typeof owner === "string" || typeof owner === "number"
      ? `${owner}${scope.separator}`
      : undefined;
  if (
    prefix === undefined ||
    String(owner).includes(scope.separator) ||
    typeof key !== "string" ||
    !key.startsWith(prefix) ||
    key.length === prefix.length
  ) {
    throw forbidden(node, "CREATE");
  }
}

/** A rule needs a claim the request lacks: the whole rule denies. */
class MissingClaim extends Error {}

function compileRule(
  ctx: CompileContext,
  node: NodeType | undefined,
  variable: string,
  where: AuthorizationWhere,
  ends?: RuleEnds,
): Folded {
  try {
    return compileRulePart(ctx, node, variable, where, false, ends);
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
  ends?: RuleEnds,
): Folded {
  const child = (w: Where, negated = underNot): Folded => {
    if (negated) return compileRulePart(ctx, node, variable, w, true, ends);
    try {
      return compileRulePart(ctx, node, variable, w, false, ends);
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
      const inner = compileRulePart(
        ctx,
        node,
        variable,
        value as Where,
        true,
        ends,
      );
      parts.push(typeof inner === "boolean" ? !inner : not(inner));
    } else if (key === "viewer") {
      parts.push(viewerTest(ctx, value));
    } else if (ends && (key === "source" || key === "target")) {
      const end = ends[key];
      parts.push(
        inRule(ctx, value, (w) =>
          compileNodeWhere(ctx, end.node, end.variable, w),
        ),
      );
    } else if (ends && key === "edge") {
      const edge = ends.edge;
      parts.push(
        edge
          ? inRule(ctx, value, (w) =>
              compilePropsWhere(ctx, edge.props, edge.variable, w),
            )
          : false,
      );
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

/**
 * A filter part of a rule, with `$jwt` / `$context` strings substituted,
 * compiled as a rule (no nested rules applied). Empty compiles deny.
 */
function inRule(
  ctx: CompileContext,
  value: unknown,
  compile: (where: Where) => Expr | undefined,
): Folded {
  const bound = substitute(value, ctx);
  if (!bound.ok) throw new MissingClaim();
  const wasInAuth = ctx.inAuth;
  ctx.inAuth = true;
  try {
    return compile(bound.value as Where) ?? false;
  } finally {
    ctx.inAuth = wasInAuth;
  }
}

/** The statement variables a relationship rule's parts test. */
interface RuleEnds {
  source: { node: NodeType; variable: string };
  target: { node: NodeType; variable: string };
  edge: { props: RelationshipPropertiesType; variable: string } | undefined;
}

/**
 * Rules on relationship fields for `op` on relationships of `rel`, with
 * the statement's owner-side, target-side and relationship variables. The
 * rules of `rel` itself apply, and so do those of a field on the other
 * side declaring the same relationship type (a connect from
 * `Person.trips` answers to `Trip.members`' rules), with source and
 * target swapped. Within a field any rule grants; across fields all must.
 *
 * `"write"` throws UNAUTHENTICATED / FORBIDDEN when the claims already
 * decide against the request; `"read"` never throws. Undefined when no
 * rule applies or the claims alone grant.
 */
export function relationshipRules(
  ctx: CompileContext,
  rel: RelationshipField,
  op: RelationshipOperation,
  vars: { owner: string; target: string; rel: string },
  mode: "write" | "read" = "write",
): Expr | false | undefined {
  if (ctx.inAuth) return undefined;
  const parts: Folded[] = [];
  for (const { field, flipped } of relationshipRuleFields(ctx, rel)) {
    const rules = (field.authorization?.validate ?? []).filter((r) =>
      r.operations.has(op),
    );
    if (rules.length === 0) continue;
    if (bypassed(ctx, ctx.model.nodes.get(field.owner))) continue;
    const label = `${op.toLowerCase().replace("_", " ")} ${field.owner}.${field.name}`;
    if (rules.every((r) => r.requireAuthentication) && !ctx.jwt) {
      if (mode === "read") return false;
      throw requestError(
        "UNAUTHENTICATED",
        `${label} needs an authenticated request`,
      );
    }
    const source = ctx.model.nodes.get(field.owner)!;
    const target = ctx.model.nodes.get(field.target)!;
    const props = field.properties
      ? ctx.model.relationshipProperties.get(field.properties)
      : undefined;
    const ends: RuleEnds = {
      source: { node: source, variable: flipped ? vars.target : vars.owner },
      target: { node: target, variable: flipped ? vars.owner : vars.target },
      edge: props ? { props, variable: vars.rel } : undefined,
    };
    const folded = foldOr(
      rules.map((r) =>
        r.requireAuthentication && !ctx.jwt
          ? false
          : compileRule(ctx, undefined, "", r.where, ends),
      ),
    );
    if (folded === false && mode === "write") {
      throw requestError("FORBIDDEN", `not allowed to ${label}`);
    }
    parts.push(folded);
  }
  const all = foldAnd(parts);
  return all === true ? undefined : all === false ? false : all;
}

/** FORBIDDEN: using a relationship's properties its READ_EDGE rules guard. */
export function refusedEdgeRead(rel: RelationshipField): Error {
  return requestError(
    "FORBIDDEN",
    `${rel.owner}.${rel.name}: its READ_EDGE rules decide per relationship, so its properties cannot be filtered, sorted or aggregated by`,
  );
}

/**
 * Throws when `rel`'s properties have READ_EDGE rules that are not settled
 * by the claims alone: filtering, sorting or aggregating by them would
 * reveal what the rules hide per relationship.
 */
export function refuseEdgeRowRules(
  ctx: CompileContext,
  rel: RelationshipField,
): void {
  if (ctx.inAuth) return;
  const probe = { ...ctx, params: {}, vars: new Set(ctx.vars) };
  const rule = relationshipRules(
    probe,
    rel,
    "READ_EDGE",
    { owner: "a", target: "b", rel: "r" },
    "read",
  );
  if (rule !== undefined) throw refusedEdgeRead(rel);
}

/** Fields whose relationship rules cover `rel`'s relationships. */
export function relationshipRuleFields(
  ctx: CompileContext,
  rel: RelationshipField,
): Array<{ field: RelationshipField; flipped: boolean }> {
  const out: Array<{ field: RelationshipField; flipped: boolean }> = [];
  const hasRules = (f: RelationshipField) =>
    f.authorization?.validate.some((r) =>
      [...r.operations].some((op) =>
        ["CONNECT", "DISCONNECT", "UPDATE_EDGE", "READ_EDGE"].includes(op),
      ),
    ) ?? false;
  if (hasRules(rel)) out.push({ field: rel, flipped: false });
  const target = ctx.model.nodes.get(rel.target);
  for (const f of target?.fields.values() ?? []) {
    if (
      f.kind === "relationship" &&
      f !== rel &&
      f.type === rel.type &&
      f.target === rel.owner &&
      f.direction !== rel.direction &&
      hasRules(f)
    ) {
      out.push({ field: f, flipped: true });
    }
  }
  return out;
}

/**
 * `viewer: { … }`: the caller's own node (found by the `@viewer` claim)
 * passes the filter. One seek by the claim, as a pattern comprehension so
 * it composes with the rest of the rule:
 * `size([(v:Person {subject: $claim}) WHERE <filter> | 1]) > 0`. Without
 * the claim it is unknown, like any test on a missing claim.
 */
function viewerTest(ctx: CompileContext, where: unknown): Folded {
  const mapping = ctx.model.viewer;
  const node = mapping && ctx.model.nodes.get(mapping.type);
  // The model build refuses `viewer` without a @viewer claim.
  if (!mapping || !node) return false;
  const id = substitute(`$jwt.${mapping.claim}`, ctx);
  const bound = substitute(where, ctx);
  if (!id.ok || !bound.ok) throw new MissingClaim();
  const field = node.fields.get(mapping.field);
  const property = field?.kind === "scalar" ? field.property : mapping.field;
  const x = freshVar(ctx, "viewer");
  const wasInAuth = ctx.inAuth;
  ctx.inAuth = true;
  try {
    const pred = compileNodeWhere(ctx, node, x, bound.value as Where);
    return bin(
      ">",
      fn("size", {
        kind: "comprehension",
        pattern: {
          start: {
            variable: x,
            labels: [node.labels[0]!],
            properties: [{ key: property, value: bind(ctx, id.value) }],
          },
          hops: [],
        },
        where: pred,
        projection: lit(1),
      }),
      lit(0),
    );
  } finally {
    ctx.inAuth = wasInAuth;
  }
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
export function claim(
  ctx: CompileContext,
  path: string,
  bound = false,
): unknown {
  const [head, ...rest] = path.split(".");
  const mapped = ctx.model.jwt?.get(head!) ?? head!;
  const full = [mapped, ...rest].join(".");
  const value = lookupPath(ctx.jwt, full);
  noteClaim(ctx, full, value, bound);
  return value;
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
    // Only bound as a filter value: a cached compile may rebind it.
    if (value.startsWith("$jwt.")) v = claim(ctx, value.slice(5), true);
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
