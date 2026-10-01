// Who may do what, read off the model: for every type and guarded field,
// each operation as each kind of caller (anonymous, authenticated, and each
// role the rules test). The claims part of every rule is decided the way a
// request decides it; what is left depends on the rows. Stable output, so
// consumers can snapshot it and review changes as diffs.

import type { GraphQLSchema } from "graphql";
import {
  authFilter,
  authValidate,
  bypassed,
  checkAuthentication,
  fieldReadGuard,
  maskSettled,
  relationshipRules,
} from "../compile/auth.js";
import { newContext, type CompileContext } from "../compile/context.js";
import { printExpr, type Expr } from "../compile/cypher.js";
import type {
  AuthOperation,
  AuthorizationWhere,
  GraphModel,
  ModelWarning,
  NodeType,
  RelationshipOperation,
} from "../model/types.js";

export type AccessVerdict =
  /** Every row, for this caller. */
  | "allowed"
  /** Only rows a filter rule admits; others are invisible. */
  | "filtered"
  /** Checked per row; a failing row is FORBIDDEN. */
  | "validated"
  /** Rows failing a mask read a substitute value. */
  | "masked"
  /** Never, for this caller. */
  | "denied"
  /** Needs a token this caller does not have. */
  | "unauthenticated";

export interface AccessEntry {
  type: string;
  /** The field, for field-level and relationship rules. */
  field?: string;
  operation: AuthOperation;
  principal: string;
  verdict: AccessVerdict;
  /** What decides it: `filter[0]`, `validate[1]`, `@authentication`, `bypass`, … */
  by: string[];
}

interface Principal {
  name: string;
  jwt: Record<string, unknown> | undefined;
}

/** The access matrix of `model`, in a stable order. */
export function accessMatrix(
  model: GraphModel,
  schema: GraphQLSchema,
): AccessEntry[] {
  const principals = principalsOf(model);
  const out: AccessEntry[] = [];
  const context = (p: Principal) =>
    newContext({ schema, fragments: {}, variables: {} }, model, {
      jwt: p.jwt,
    });
  for (const node of [...model.nodes.values()].sort(byName)) {
    const ops: AuthOperation[] = [
      ...(node.read ? (["READ"] as const) : []),
      ...(["CREATE", "UPDATE", "DELETE"] as const).filter((op) =>
        node.mutations.has(op),
      ),
    ];
    for (const op of ops) {
      for (const p of principals) {
        out.push({
          type: node.name,
          operation: op,
          principal: p.name,
          ...typeVerdict(context(p), node, op),
        });
      }
    }
    for (const f of [...node.fields.values()].sort(byName)) {
      if (f.kind === "relationship") {
        const ruleOps = new Set<RelationshipOperation>();
        for (const r of f.authorization?.validate ?? []) {
          for (const op of r.operations) {
            if (
              op === "CONNECT" ||
              op === "DISCONNECT" ||
              op === "UPDATE_EDGE" ||
              op === "READ_EDGE"
            ) {
              ruleOps.add(op);
            }
          }
        }
        for (const op of [...ruleOps].sort()) {
          for (const p of principals) {
            out.push({
              type: node.name,
              field: f.name,
              operation: op,
              principal: p.name,
              ...relationshipVerdict(context(p), node, f, op),
            });
          }
        }
      }
      const guarded =
        f.kind !== "custom" &&
        (f.authentication?.has("READ") ||
          f.authorization?.validate.some((r) => r.operations.has("READ")) ||
          (f.authorization?.mask?.length ?? 0) > 0);
      if (!guarded) continue;
      for (const p of principals) {
        out.push({
          type: node.name,
          field: f.name,
          operation: "READ",
          principal: p.name,
          ...fieldVerdict(context(p), node, f),
        });
      }
    }
  }
  return out;
}

function typeVerdict(
  ctx: CompileContext,
  node: NodeType,
  op: AuthOperation,
): Pick<AccessEntry, "verdict" | "by"> {
  const by = rulesFor(node, op);
  try {
    checkAuthentication(ctx, node, op);
    if (bypassed(ctx, node)) return { verdict: "allowed", by: ["bypass"] };
    const filter = op === "CREATE" ? undefined : authFilter(ctx, node, "n", op);
    if (filter && isFalse(filter)) return { verdict: "denied", by };
    const validate =
      authValidate(ctx, node, "n", op, "BEFORE") ??
      (op === "READ" ? undefined : authValidate(ctx, node, "n", op, "AFTER"));
    const verdict: AccessVerdict = filter
      ? "filtered"
      : validate
        ? "validated"
        : "allowed";
    return { verdict, by: verdict === "allowed" ? allowedBy(by) : by };
  } catch (err) {
    return failed(err, by);
  }
}

function fieldVerdict(
  ctx: CompileContext,
  node: NodeType,
  field: Parameters<typeof fieldReadGuard>[2] & {
    name: string;
    kind: string;
    authorization?: NodeType["authorization"];
  },
): Pick<AccessEntry, "verdict" | "by"> {
  const by = [
    ...(field.authentication?.has("READ") ? ["@authentication"] : []),
    ...(field.authorization?.validate ?? []).flatMap((r, i) =>
      r.operations.has("READ") ? [`validate[${i}]`] : [],
    ),
    ...(field.authorization?.mask ?? []).map((_, i) => `mask[${i}]`),
  ];
  if (bypassed(ctx, node) && !field.authentication?.has("READ")) {
    return { verdict: "allowed", by: ["bypass"] };
  }
  const guard = fieldReadGuard(ctx, node, field, "n");
  if (guard?.code === "UNAUTHENTICATED") {
    return { verdict: "unauthenticated", by };
  }
  if (guard && isFalse(guard.when)) return { verdict: "denied", by };
  if (guard) return { verdict: "validated", by };
  if (field.kind === "scalar" && !maskSettled(ctx, node, field)) {
    return { verdict: "masked", by };
  }
  return { verdict: "allowed", by };
}

function relationshipVerdict(
  ctx: CompileContext,
  node: NodeType,
  field: Parameters<typeof relationshipRules>[1],
  op: RelationshipOperation,
): Pick<AccessEntry, "verdict" | "by"> {
  const by = (field.authorization?.validate ?? []).flatMap((r, i) =>
    r.operations.has(op) ? [`validate[${i}]`] : [],
  );
  if (bypassed(ctx, node)) return { verdict: "allowed", by: ["bypass"] };
  try {
    const rule = relationshipRules(ctx, field, op, {
      owner: "a",
      target: "b",
      rel: "r",
    });
    if (rule === false) return { verdict: "denied", by };
    return { verdict: rule ? "validated" : "allowed", by };
  } catch (err) {
    return failed(err, by);
  }
}

function rulesFor(node: NodeType, op: AuthOperation): string[] {
  return [
    ...(node.authentication?.has(op) ? ["@authentication"] : []),
    ...(node.authorization?.filter ?? []).flatMap((r, i) =>
      r.operations.has(op) ? [`filter[${i}]`] : [],
    ),
    ...(node.authorization?.validate ?? []).flatMap((r, i) =>
      r.operations.has(op) ? [`validate[${i}]`] : [],
    ),
    ...(node.authorization?.public?.has(op) ? ["public"] : []),
  ];
}

/** An allowed verdict names what granted it: rules that folded to true. */
function allowedBy(by: string[]): string[] {
  return by.length > 0 ? by : ["no rule"];
}

function failed(
  err: unknown,
  by: string[],
): Pick<AccessEntry, "verdict" | "by"> {
  const code = (err as { extensions?: { code?: unknown } }).extensions?.code;
  if (code === "UNAUTHENTICATED") return { verdict: "unauthenticated", by };
  if (code === "FORBIDDEN") return { verdict: "denied", by };
  throw err;
}

const isFalse = (e: Expr) => printExpr(e) === "false";

const byName = (a: { name: string }, b: { name: string }) =>
  a.name < b.name ? -1 : a.name > b.name ? 1 : 0;

/**
 * Anonymous, authenticated (a token with no roles), and one principal per
 * claim value the rules test with `includes`, `eq` or `in` (`roles:admin`).
 */
function principalsOf(model: GraphModel): Principal[] {
  const subject = model.viewer?.claim ?? "sub";
  const base = token(model, { [subject]: "someone" });
  const tested = new Map<
    string,
    { claim: string; value: unknown; list: boolean }
  >();
  const visit = (where: unknown) => {
    if (Array.isArray(where)) return where.forEach(visit);
    if (where === null || typeof where !== "object") return;
    for (const [key, value] of Object.entries(where)) {
      if (key === "jwt" && value && typeof value === "object") {
        for (const [claim, ops] of Object.entries(value)) {
          for (const [op, operand] of Object.entries(
            (ops ?? {}) as Record<string, unknown>,
          )) {
            const values =
              op === "in" && Array.isArray(operand)
                ? operand
                : op === "includes" || op === "eq"
                  ? [operand]
                  : [];
            for (const v of values) {
              if (typeof v !== "string" || claim === subject) continue;
              tested.set(`${claim}:${v}`, {
                claim,
                value: v,
                list: op === "includes",
              });
            }
          }
        }
      } else visit(value);
    }
  };
  const rules = (a: NodeType["authorization"]) => [
    ...(a?.filter ?? []).map((r) => r.where),
    ...(a?.validate ?? []).map((r) => r.where),
    ...(a?.mask ?? []).map((m) => m.unless),
  ];
  visit(model.bypass);
  for (const node of model.nodes.values()) {
    visit(rules(node.authorization));
    visit(node.authenticationJwt);
    for (const f of node.fields.values()) {
      visit(rules(f.authorization));
      visit(f.authenticationJwt);
    }
  }
  for (const props of model.relationshipProperties.values()) {
    for (const f of props.fields.values()) visit(rules(f.authorization));
  }
  return [
    { name: "anonymous", jwt: undefined },
    { name: "authenticated", jwt: base },
    ...[...tested.entries()]
      .sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))
      .map(([name, t]) => ({
        name,
        jwt: token(model, {
          [subject]: "someone",
          [t.claim]: t.list ? [t.value] : t.value,
        }),
      })),
  ];
}

/**
 * A token carrying `claims` where the `@jwt` type says they live
 * (`@jwtClaim(path: "app_metadata.roles")`).
 */
function token(
  model: GraphModel,
  claims: Record<string, unknown>,
): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  for (const [claim, value] of Object.entries(claims)) {
    const path = (model.jwt?.get(claim) ?? claim).split(".");
    let at = out;
    for (const part of path.slice(0, -1)) {
      at = (at[part] ??= {}) as Record<string, unknown>;
    }
    at[path[path.length - 1]!] = value;
  }
  return out;
}

/**
 * Authorization lints for `check`: valid rules that are probably not what
 * was meant.
 */
export function accessLints(
  model: GraphModel,
  schema: GraphQLSchema,
): ModelWarning[] {
  const out: ModelWarning[] = [];
  const subject = model.viewer?.claim ?? "sub";
  const authenticated = newContext(
    { schema, fragments: {}, variables: {} },
    model,
    { jwt: token(model, { [subject]: "someone" }) },
  );
  for (const node of model.nodes.values()) {
    const filters = node.authorization?.filter ?? [];
    filters.forEach((rule, i) => {
      // A filter any signed-in caller passes filters nothing for them.
      const probe = { ...authenticated, params: {}, vars: new Set(["n"]) };
      const alone = {
        ...node,
        authorization: { filter: [rule], validate: [] },
      };
      if (
        authFilter(probe, alone, "n", [...rule.operations][0]!) === undefined
      ) {
        out.push({
          type: node.name,
          message: `filter[${i}] holds for every authenticated caller: it only keeps out anonymous ones (use @authentication for that)`,
        });
      }
    });
    const rules = [
      ...filters.map((r, i) => ({ r, label: `filter[${i}]` })),
      ...(node.authorization?.validate ?? []).map((r, i) => ({
        r,
        label: `validate[${i}]`,
      })),
    ];
    for (const { r, label } of rules) {
      if (!r.requireAuthenticationDefaulted) continue;
      const branch = claimFreeBranch(r.where);
      if (branch !== undefined) {
        out.push({
          type: node.name,
          message: `${label} has a branch that needs no claims (${branch}), but requireAuthentication defaults to true, so anonymous callers are refused by the whole rule; set requireAuthentication: false if anonymous callers should get that branch`,
        });
      }
    }
    if (model.bypass && node.authorization?.bypass !== false) {
      for (const f of node.fields.values()) {
        if (
          f.kind !== "custom" &&
          ((f.authorization?.validate.length ?? 0) > 0 ||
            (f.authorization?.mask?.length ?? 0) > 0)
        ) {
          out.push({
            type: node.name,
            field: f.name,
            message:
              "the schema's bypass skips this field's rules for callers passing it; add @authorization(bypass: false) to the type if they must hold for everyone",
          });
        }
      }
    }
  }
  return out;
}

/** The path of an OR branch that tests no claim, if the rule has one. */
function claimFreeBranch(where: AuthorizationWhere): string | undefined {
  const or = where["OR"];
  if (!Array.isArray(or)) return undefined;
  const i = or.findIndex((b) => !usesClaims(b));
  return i >= 0 ? `OR[${i}]` : undefined;
}

function usesClaims(where: unknown): boolean {
  if (typeof where === "string") {
    return where.startsWith("$jwt.") || where.includes("${jwt.");
  }
  if (Array.isArray(where)) return where.some(usesClaims);
  if (where === null || typeof where !== "object") return false;
  return Object.entries(where).some(
    ([k, v]) =>
      k === "jwt" || k === "viewer" || k === "isViewer" || usesClaims(v),
  );
}
