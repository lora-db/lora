// The verdict of one rule set for one caller: a type, a field, a relationship,
// a relationship property or a root @cypher field, for one operation. The
// claims part of every rule is decided the way a request decides it; what is
// left depends on the rows.

import {
  authFilter,
  authValidate,
  bypassed,
  checkAuthentication,
  checkFieldAuthentication,
  fieldReadGuard,
  fieldValidate,
  maskSettled,
  propertyAccess,
  propertyReadAccess,
  relationshipRules,
  rootFieldGuard,
} from "../compile/auth.js";
import type { CompileContext } from "../compile/context.js";
import { printExpr, type Expr } from "../compile/cypher.js";
import type {
  AuthOperation,
  CypherField,
  Field,
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
  /** `EXECUTE`: running a Mutation @cypher field. */
  operation: AuthOperation | "EXECUTE";
  principal: string;
  verdict: AccessVerdict;
  /** What decides it: `filter[0]`, `validate[1]`, `@authentication`, `bypass`, … */
  by: string[];
}

export function rootFieldVerdict(
  ctx: CompileContext,
  field: CypherField,
): Pick<AccessEntry, "verdict" | "by"> {
  const by = [
    ...(field.authentication?.size ? ["@authentication"] : []),
    ...(field.authorization?.validate ?? []).map((_, i) => `validate[${i}]`),
  ];
  // A root mutation's listed operations are its own: any one guards it.
  const op =
    field.owner === "Mutation"
      ? [...(field.authentication ?? [])][0]
      : ("READ" as const);
  try {
    if (op) checkFieldAuthentication(ctx, field.owner, field, op);
    if (field.authorization?.validate.length && bypassed(ctx, undefined)) {
      return { verdict: "allowed", by: ["bypass"] };
    }
    const rule = rootFieldGuard(ctx, field);
    if (rule) return { verdict: "validated", by };
    return { verdict: "allowed", by: allowedBy(by) };
  } catch (err) {
    return failed(err, by);
  }
}

export const PROPERTY_OPS = ["READ", "CREATE", "UPDATE"] as const;

/** `field` has rules (or `@authentication`) for `op`. */
export function fieldWriteGuarded(
  field: {
    authentication?: ReadonlySet<AuthOperation> | undefined;
    authorization?: NodeType["authorization"];
  },
  op: AuthOperation,
): boolean {
  return (
    (field.authentication?.has(op) ?? false) ||
    (field.authorization?.validate.some((r) => r.operations.has(op)) ?? false)
  );
}

function ruleLabels(
  field: {
    authentication?: ReadonlySet<AuthOperation> | undefined;
    authorization?: NodeType["authorization"];
  },
  op: AuthOperation,
): string[] {
  return [
    ...(field.authentication?.has(op) ? ["@authentication"] : []),
    ...(field.authorization?.validate ?? []).flatMap((r, i) =>
      r.operations.has(op) ? [`validate[${i}]`] : [],
    ),
  ];
}

export function fieldWriteVerdict(
  ctx: CompileContext,
  node: NodeType,
  field: Field & { kind: "scalar" },
  op: "CREATE" | "UPDATE",
): Pick<AccessEntry, "verdict" | "by"> {
  const by = ruleLabels(field, op);
  try {
    checkFieldAuthentication(ctx, node.name, field, op);
    if (
      field.authorization?.validate.some((r) => r.operations.has(op)) &&
      bypassed(ctx, node)
    ) {
      return { verdict: "allowed", by: ["bypass"] };
    }
    const rule =
      fieldValidate(ctx, node, field, "n", op, "BEFORE") ??
      fieldValidate(ctx, node, field, "n", op, "AFTER");
    return rule ? { verdict: "validated", by } : { verdict: "allowed", by };
  } catch (err) {
    return failed(err, by);
  }
}

export function propertyVerdict(
  ctx: CompileContext,
  field: Parameters<typeof propertyAccess>[1],
  op: AuthOperation,
): Pick<AccessEntry, "verdict" | "by"> {
  const by = ruleLabels(field, op);
  if (
    field.authorization?.validate.some((r) => r.operations.has(op)) &&
    bypassed(ctx, undefined) &&
    !field.authentication?.has(op)
  ) {
    return { verdict: "allowed", by: ["bypass"] };
  }
  // READ rules over the relationship's ends decide per relationship.
  const access: string =
    op === "READ"
      ? propertyReadAccess(ctx, field)
      : propertyAccess(ctx, field, op);
  const verdict: AccessVerdict =
    access === "allowed"
      ? "allowed"
      : access === "unauthenticated"
        ? "unauthenticated"
        : access === "forbidden"
          ? "denied"
          : "validated";
  return { verdict, by };
}

export function typeVerdict(
  ctx: CompileContext,
  node: NodeType,
  op: AuthOperation,
): Pick<AccessEntry, "verdict" | "by"> {
  const verdict = typeRuleVerdict(ctx, node, op);
  // `@key(scope: VIEWER)`: a create is checked against the caller's key
  // space before any statement runs, so it needs a token, and then it is
  // decided per key. The schema's bypass skips it.
  if (
    op !== "CREATE" ||
    !node.key.keyScope ||
    !ctx.model.viewer ||
    verdict.verdict === "denied" ||
    verdict.verdict === "unauthenticated" ||
    bypassed(ctx, node)
  ) {
    return verdict;
  }
  const by = [...verdict.by.filter((b) => b !== "no rule"), "key scope"];
  return { verdict: ctx.jwt ? "validated" : "unauthenticated", by };
}

function typeRuleVerdict(
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

export function fieldVerdict(
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

/**
 * The `@authentication` operation a relationship operation answers to: a
 * connect is checked as CREATE_RELATIONSHIP on the field, a disconnect as
 * DELETE_RELATIONSHIP. Edge reads and updates have none of their own.
 */
export function relationshipAuthentication(
  op: RelationshipOperation,
): AuthOperation | undefined {
  return op === "CONNECT"
    ? "CREATE_RELATIONSHIP"
    : op === "DISCONNECT"
      ? "DELETE_RELATIONSHIP"
      : undefined;
}

export function relationshipVerdict(
  ctx: CompileContext,
  node: NodeType,
  field: Parameters<typeof relationshipRules>[1],
  op: RelationshipOperation,
): Pick<AccessEntry, "verdict" | "by"> {
  const authentication = relationshipAuthentication(op);
  const authenticated =
    authentication !== undefined && field.authentication?.has(authentication);
  const by = [
    ...(authenticated ? ["@authentication"] : []),
    ...(field.authorization?.validate ?? []).flatMap((r, i) =>
      r.operations.has(op) ? [`validate[${i}]`] : [],
    ),
  ];
  try {
    // Like a field's READ: `@authentication` is asked before the bypass.
    if (authentication) {
      checkFieldAuthentication(ctx, node.name, field, authentication);
    }
    if (bypassed(ctx, node)) return { verdict: "allowed", by: ["bypass"] };
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
