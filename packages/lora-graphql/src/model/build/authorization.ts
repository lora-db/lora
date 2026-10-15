// Reading `@authentication` and `@authorization` into rules, and the
// questions the build asks of a rule's shape: whether it tests claims
// only, whether an anonymous request could pass it, which relationship
// operations it covers.

import type {
  AuthOperation,
  Authorization,
  AuthorizationWhere,
  NodeType,
  RelationshipField,
} from "../types.js";
import { RELATIONSHIP_OPERATIONS } from "../types.js";
import { VIEWER_TEST } from "../desugar.js";

export function authOps(
  args: Record<string, unknown> | undefined,
): Set<AuthOperation> | undefined {
  return args ? new Set(args["operations"] as AuthOperation[]) : undefined;
}

/**
 * Field-level rules on a relationship or @cypher field: READ validate rules
 * only, checked for each row that reads (or filters on) the field.
 */
export function readOnlyRules(
  args: Record<string, unknown> | undefined,
  at: (message: string) => void,
  relationship = false,
): Authorization | undefined {
  const rules = readAuthorization(args);
  if (!rules) return undefined;
  for (const r of rules.validate) {
    const ops = [...r.operations];
    const rel = ops.filter((op) => RELATIONSHIP_OPERATIONS.has(op));
    if (rel.length > 0 && !relationship) {
      at(`${rel.join(", ")} rules belong on a relationship field`);
    } else if (rel.length > 0 && rel.length < ops.length) {
      at(
        `a rule on ${rel.join(", ")} tests source, target and edge; give READ its own rule`,
      );
    } else if (rel.length === 0 && ops.some((op) => op !== "READ")) {
      at(
        relationship
          ? "field-level @authorization on a relationship field takes READ rules, or CONNECT, DISCONNECT, UPDATE_EDGE and READ_EDGE rules; write rules for the nodes belong on the types"
          : "field-level @authorization on a relationship or @cypher field takes READ rules only; write rules belong on the types",
      );
    }
  }
  return rules;
}

/**
 * `@authorization` on a Query or Mutation @cypher field: validate rules
 * guarding the call, tested before the statement runs. There is no node,
 * so they test claims (`jwt`) and the caller's node (`viewer`); their
 * `operations` and `when` do not matter.
 */
export function rootFieldRules(
  args: Record<string, unknown> | undefined,
  at: (message: string) => void,
): Authorization | undefined {
  const rules = readAuthorization(args);
  if (!rules) return undefined;
  if (rules.filter.length > 0) {
    at(
      "a root @cypher field takes validate rules: there are no rows to filter",
    );
  }
  if (rules.mask?.length) {
    at("a root @cypher field takes validate rules: there is no row to mask");
  }
  return rules;
}

export function readAuthorization(
  args: Record<string, unknown> | undefined,
): Authorization | undefined {
  if (!args) return undefined;
  type Raw = {
    operations: AuthOperation[];
    when?: Array<"BEFORE" | "AFTER">;
    where: AuthorizationWhere;
  };
  const filter = ((args["filter"] as Raw[] | undefined) ?? []).map((r) => ({
    operations: new Set(r.operations),
    // Settled with the schema's default once the rule is desugared.
    requireAuthentication: true,
    where: r.where,
  }));
  const validate = ((args["validate"] as Raw[] | undefined) ?? []).map((r) => ({
    operations: new Set(r.operations),
    when: new Set<"BEFORE" | "AFTER">(r.when ?? ["BEFORE", "AFTER"]),
    // Settled with the schema's default once the rule is desugared.
    requireAuthentication: true,
    where: r.where,
  }));
  return {
    filter,
    validate,
    ...(args["bypass"] != null ? { bypass: args["bypass"] as boolean } : {}),
    ...(args["public"] != null
      ? { public: new Set(args["public"] as AuthOperation[]) }
      : {}),
    ...(args["mask"] != null
      ? {
          mask: (
            args["mask"] as Array<{
              unless: AuthorizationWhere;
              value?: unknown;
            }>
          ).map((m) => ({
            unless: m.unless,
            value: m.value ?? null,
            ...("value" in m ? {} : { missingValue: true }),
          })),
        }
      : {}),
  };
}

export const PROPERTY_OPS = new Set<AuthOperation>([
  "READ",
  "CREATE",
  "UPDATE",
]);

/** Keys of a rule that are not claim tests: `node`, `viewer`, … */
/**
 * Whether a request without a token could pass the rule: some branch
 * reads no claim. A claim read without a token denies its branch (and
 * the whole rule under NOT), so those branches never pass. An `isViewer`
 * test is the exception: without a token it is false for every node (see
 * `VIEWER_TEST`), so under `none`, `NOT` or `all` it can pass.
 */
export function passableWithoutClaims(where: unknown): boolean {
  if (!isRecord(where)) return false;
  return Object.entries(where).every(([k, value]) => {
    if (k === "AND") {
      return Array.isArray(value) && value.every(passableWithoutClaims);
    }
    if (k === "OR") {
      return Array.isArray(value) && value.some(passableWithoutClaims);
    }
    if (k === "jwt" || k === "viewer") return false;
    return !readsClaims(value, k === "NOT" ? "negated" : "plain");
  });
}

/**
 * Where a test stands: `plain` (it must hold), `negated` (under `none` or
 * `NOT`: it must fail), or `any` (under `all`: an empty list passes).
 */
type Polarity = "plain" | "negated" | "any";

function readsClaims(value: unknown, polarity: Polarity = "plain"): boolean {
  if (typeof value === "string") {
    return (
      value.startsWith("$jwt.") ||
      value.includes("${jwt.") ||
      value.includes("${viewer.")
    );
  }
  if (Array.isArray(value)) return value.some((v) => readsClaims(v, polarity));
  if (!isRecord(value)) return false;
  // A false `isViewer` passes a negated test, and `all` over no nodes.
  if (VIEWER_TEST in value) return polarity === "plain";
  return Object.entries(value).some(([k, v]) => {
    if (k === "jwt" || k === "viewer") return true;
    const inner: Polarity =
      polarity === "any"
        ? "any"
        : k === "all"
          ? "any"
          : k === "none" || k === "NOT"
            ? polarity === "plain"
              ? "negated"
              : "plain"
            : polarity;
    return readsClaims(v, inner);
  });
}

export function claimsOnlyViolations(where: unknown, path = ""): string[] {
  if (!isRecord(where)) return [];
  const out: string[] = [];
  for (const [k, value] of Object.entries(where)) {
    const here = path ? `${path}.${k}` : k;
    if (k === "AND" || k === "OR") {
      if (Array.isArray(value)) {
        value.forEach((w, i) =>
          out.push(...claimsOnlyViolations(w, `${here}[${i}]`)),
        );
      }
    } else if (k === "NOT") out.push(...claimsOnlyViolations(value, here));
    else if (k !== "jwt") out.push(here);
  }
  return out;
}

/** The relationship operations a field's rules cover. */
export function relationshipRuleOperations(
  f: RelationshipField,
): Set<AuthOperation> {
  return new Set(
    (f.authorization?.validate ?? []).flatMap((r) =>
      [...r.operations].filter((op) => RELATIONSHIP_OPERATIONS.has(op)),
    ),
  );
}

/** A rule over a relationship's source, target and edge. */
export const isRelationshipRule = (rule: {
  operations: ReadonlySet<AuthOperation>;
}): boolean =>
  [...rule.operations].some((op) => RELATIONSHIP_OPERATIONS.has(op));

/**
 * The authorization of a field to an interface or union, per member: its
 * relationship rules hold per member, each with that member as its
 * target. Sugar expands differently per member (`target: { isViewer:
 * true }` is only ever true of one), so each member gets its own copy of
 * the rules, which `desugar` expands in place.
 */
export function memberAuthorization(
  f: RelationshipField,
  nodes: ReadonlyMap<string, NodeType>,
  desugar: (
    rules: ReadonlyArray<{ where: AuthorizationWhere }>,
    target: NodeType,
  ) => void,
): Map<string, Authorization> {
  const perMember = new Map<string, Authorization>();
  const authorization = f.authorization;
  if (!authorization) return perMember;
  const rel = authorization.validate.filter(isRelationshipRule);
  const rest = authorization.validate.filter((r) => !isRelationshipRule(r));
  for (const member of f.members) {
    const target = nodes.get(member);
    if (!target) continue;
    const own = rel.map((r) => ({ ...r, where: structuredClone(r.where) }));
    desugar(own, target);
    perMember.set(member, { ...authorization, validate: [...rest, ...own] });
  }
  return perMember;
}

export const isRecord = (x: unknown): x is Record<string, unknown> =>
  typeof x === "object" && x !== null && !Array.isArray(x);
