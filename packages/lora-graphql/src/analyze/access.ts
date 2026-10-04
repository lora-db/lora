// Who may do what, read off the model: for every type and guarded field,
// each operation as each kind of caller (anonymous, authenticated, and each
// role the rules test). The claims part of every rule is decided the way a
// request decides it; what is left depends on the rows. Stable output, so
// consumers can snapshot it and review changes as diffs.

import {
  Kind,
  type FragmentDefinitionNode,
  type GraphQLSchema,
  type OperationDefinitionNode,
  type SelectionSetNode,
} from "graphql";
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
import { newContext, type CompileContext } from "../compile/context.js";
import { printExpr, type Expr } from "../compile/cypher.js";
import type {
  AuthOperation,
  AuthorizationWhere,
  CypherField,
  Field,
  GraphModel,
  ModelWarning,
  NestedOperation,
  NodeType,
  RelationshipOperation,
} from "../model/types.js";
import { PLACEHOLDER } from "../model/types.js";
import { lowerFirst } from "../model/build.js";
import { names } from "../schema/names.js";
import { mutationNames } from "../schema/mutations.js";

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

interface Principal {
  name: string;
  jwt: Record<string, unknown> | undefined;
}

/** One root field of an operation, as each kind of caller. */
export interface RootFieldAccess {
  /** The root field. */
  field: string;
  /** Its response key, when aliased. */
  alias?: string;
  /**
   * What the field acts on: a node type, an interface or union, `Node`
   * (for `node` / `nodes`), or `Query` / `Mutation` for a `@cypher` field.
   */
  type: string;
  /** The operations it needs: `READ`, `CREATE`, `UPDATE` …, `EXECUTE`. */
  operations: Array<AuthOperation | "EXECUTE">;
  /**
   * Per principal (`anonymous`, `authenticated`, `roles:admin` …). A
   * field over several operations or types (an upsert, `node`, a union)
   * reports the most restrictive of them, `by` naming each part.
   */
  access: Record<string, { verdict: AccessVerdict; by: string[] }>;
}

export interface OperationAccess {
  operation: "query" | "mutation" | "subscription";
  name: string | undefined;
  /** The principals, in the order the matrix lists them. */
  principals: string[];
  /** Root fields in document order (fragments followed); `__typename` left out. */
  fields: RootFieldAccess[];
  /**
   * Per principal, the most restrictive verdict over the root fields:
   * `denied` or `unauthenticated` means the operation cannot succeed.
   */
  verdicts: Record<string, AccessVerdict>;
}

/** Most restrictive first. */
const RESTRICTIVENESS: readonly AccessVerdict[] = [
  "denied",
  "unauthenticated",
  "validated",
  "filtered",
  "masked",
  "allowed",
];

function strictest(verdicts: AccessVerdict[]): AccessVerdict {
  let best = RESTRICTIVENESS.length - 1;
  for (const v of verdicts) best = Math.min(best, RESTRICTIVENESS.indexOf(v));
  return RESTRICTIVENESS[best]!;
}

type RootTarget =
  | { kind: "node"; node: NodeType; ops: AuthOperation[] }
  | { kind: "abstract"; type: string; members: NodeType[] }
  | { kind: "relay"; members: NodeType[] }
  | { kind: "cypher"; field: CypherField };

/** Every generated or declared root field, by operation type and name. */
function rootTargets(
  model: GraphModel,
): Record<OperationAccess["operation"], Map<string, RootTarget>> {
  const query = new Map<string, RootTarget>();
  const mutation = new Map<string, RootTarget>();
  const subscription = new Map<string, RootTarget>();
  const read = (node: NodeType): RootTarget => ({
    kind: "node",
    node,
    ops: ["READ"],
  });
  for (const node of model.nodes.values()) {
    if (node.read) {
      for (const name of [
        names.listRoot(node),
        names.connectionRoot(node),
        names.singleRoot(node),
        names.aggregateRoot(node),
        names.groupedRoot(node),
      ]) {
        query.set(name, read(node));
      }
      for (const index of node.search) {
        query.set(index.queryName, read(node));
        query.set(`${index.queryName}Connection`, read(node));
      }
    }
    const write = (ops: AuthOperation[]): RootTarget => ({
      kind: "node",
      node,
      ops,
    });
    mutation.set(mutationNames.create(node), write(["CREATE"]));
    mutation.set(mutationNames.upsert(node), write(["CREATE", "UPDATE"]));
    mutation.set(mutationNames.update(node), write(["UPDATE"]));
    mutation.set(mutationNames.updateMany(node), write(["UPDATE"]));
    mutation.set(mutationNames.delete(node), write(["DELETE"]));
    mutation.set(mutationNames.deleteMany(node), write(["DELETE"]));
    subscription.set(`${lowerFirst(node.name)}Changed`, write(["SUBSCRIBE"]));
  }
  for (const abstract of model.abstracts.values()) {
    query.set(abstract.plural, {
      kind: "abstract",
      type: abstract.name,
      members: abstract.members
        .map((m) => model.nodes.get(m))
        .filter((n): n is NodeType => n !== undefined)
        .sort(byName),
    });
  }
  const relay: RootTarget = {
    kind: "relay",
    members: [...model.nodes.values()]
      .filter((n) => n.key.relayId && n.read)
      .sort(byName),
  };
  query.set("node", relay);
  query.set("nodes", relay);
  for (const field of model.queries)
    query.set(field.name, { kind: "cypher", field });
  for (const field of model.mutations)
    mutation.set(field.name, { kind: "cypher", field });
  return { query, mutation, subscription };
}

/**
 * Who may run `operation`: each root field's verdict per principal, read
 * off the same rules as `accessMatrix`. Root fields only: nested
 * selections answer to their own types' READ rules (see the matrix).
 */
export function operationAccess(
  model: GraphModel,
  schema: GraphQLSchema,
  operation: OperationDefinitionNode,
  fragments: ReadonlyMap<string, FragmentDefinitionNode>,
): OperationAccess {
  const principals = principalsOf(model);
  const context = (p: Principal) =>
    newContext({ schema, fragments: {}, variables: {} }, model, {
      jwt: p.jwt,
    });
  const targets = rootTargets(model)[operation.operation];
  const fields: RootFieldAccess[] = [];
  for (const sel of rootSelections(operation.selectionSet, fragments)) {
    const field = sel.name.value;
    if (field === "__typename") continue;
    const target = targets.get(field);
    if (!target) {
      throw new Error(
        `operationAccess: ${operation.operation} has no root field ${field}`,
      );
    }
    const access: RootFieldAccess["access"] = {};
    let type: string;
    let operations: RootFieldAccess["operations"];
    for (const p of principals) {
      const parts: Array<
        { label?: string } & Pick<AccessEntry, "verdict" | "by">
      > = [];
      if (target.kind === "cypher") {
        parts.push(rootFieldVerdict(context(p), target.field));
      } else if (target.kind === "node") {
        for (const op of target.ops) {
          parts.push({
            ...(target.ops.length > 1 ? { label: op } : {}),
            ...typeVerdict(context(p), target.node, op),
          });
        }
      } else {
        for (const member of target.members) {
          parts.push({
            label: member.name,
            ...typeVerdict(context(p), member, "READ"),
          });
        }
      }
      access[p.name] = {
        verdict:
          parts.length === 0
            ? "denied"
            : strictest(parts.map((x) => x.verdict)),
        by: parts.flatMap((x) =>
          x.label ? x.by.map((b) => `${x.label}: ${b}`) : x.by,
        ),
      };
    }
    if (target.kind === "cypher") {
      type = target.field.owner;
      operations = [target.field.owner === "Mutation" ? "EXECUTE" : "READ"];
    } else if (target.kind === "node") {
      type = target.node.name;
      operations = [...target.ops];
    } else {
      type = target.kind === "relay" ? "Node" : target.type;
      operations = ["READ"];
    }
    fields.push({
      field,
      ...(sel.alias ? { alias: sel.alias.value } : {}),
      type,
      operations,
      access,
    });
  }
  const verdicts: Record<string, AccessVerdict> = {};
  for (const p of principals) {
    verdicts[p.name] = strictest(fields.map((f) => f.access[p.name]!.verdict));
  }
  return {
    operation: operation.operation,
    name: operation.name?.value,
    principals: principals.map((p) => p.name),
    fields,
    verdicts,
  };
}

function rootSelections(
  set: SelectionSetNode,
  fragments: ReadonlyMap<string, FragmentDefinitionNode>,
  seen = new Set<string>(),
): Array<
  Extract<SelectionSetNode["selections"][number], { kind: Kind.FIELD }>
> {
  const out: Array<
    Extract<SelectionSetNode["selections"][number], { kind: Kind.FIELD }>
  > = [];
  for (const sel of set.selections) {
    if (sel.kind === Kind.FIELD) out.push(sel);
    else if (sel.kind === Kind.INLINE_FRAGMENT) {
      out.push(...rootSelections(sel.selectionSet, fragments, seen));
    } else {
      const frag = fragments.get(sel.name.value);
      if (!frag || seen.has(sel.name.value)) continue;
      seen.add(sel.name.value);
      out.push(...rootSelections(frag.selectionSet, fragments, seen));
    }
  }
  return out;
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
      if (guarded) {
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
      // Field-level CREATE / UPDATE rules: who may write this field.
      if (f.kind !== "scalar") continue;
      for (const op of ["CREATE", "UPDATE"] as const) {
        if (!node.mutations.has(op) || !fieldWriteGuarded(f, op)) continue;
        for (const p of principals) {
          out.push({
            type: node.name,
            field: f.name,
            operation: op,
            principal: p.name,
            ...fieldWriteVerdict(context(p), node, f, op),
          });
        }
      }
    }
  }
  // Rules on relationship properties, per property and operation.
  for (const props of [...model.relationshipProperties.values()].sort(byName)) {
    for (const f of [...props.fields.values()].sort(byName)) {
      for (const op of PROPERTY_OPS) {
        if (!fieldWriteGuarded(f, op)) continue;
        for (const p of principals) {
          out.push({
            type: props.name,
            field: f.name,
            operation: op,
            principal: p.name,
            ...propertyVerdict(context(p), f, op),
          });
        }
      }
    }
  }
  // Root @cypher fields, guarded or not: a hand-written write is exactly
  // what an audit wants listed.
  for (const field of [
    ...[...model.queries].sort(byName),
    ...[...model.mutations].sort(byName),
  ]) {
    for (const p of principals) {
      out.push({
        type: field.owner,
        field: field.name,
        operation: field.owner === "Mutation" ? "EXECUTE" : "READ",
        principal: p.name,
        ...rootFieldVerdict(context(p), field),
      });
    }
  }
  return out;
}

function rootFieldVerdict(
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

const PROPERTY_OPS = ["READ", "CREATE", "UPDATE"] as const;

/** `field` has rules (or `@authentication`) for `op`. */
function fieldWriteGuarded(
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

function fieldWriteVerdict(
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

function propertyVerdict(
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

function typeVerdict(
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
  for (const f of [...model.queries, ...model.mutations]) {
    visit(rules(f.authorization));
    visit(f.authenticationJwt);
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
    // `bypass: true` (or `false`) on the type says it was decided.
    if (model.bypass && node.authorization?.bypass === undefined) {
      for (const f of node.fields.values()) {
        if (
          f.kind !== "custom" &&
          ((f.authorization?.validate.length ?? 0) > 0 ||
            (f.authorization?.mask?.length ?? 0) > 0)
        ) {
          out.push({
            type: node.name,
            field: f.name,
            message: `the schema's bypass skips the rules of ${node.name}.${f.name} for callers passing it; add @authorization(bypass: false) to ${node.name} if they must hold for everyone, or bypass: true to acknowledge it`,
          });
        }
      }
    }
  }
  out.push(
    ...rewritableRuleFields(model, authenticated),
    ...maskOracles(model, authenticated),
    ...refusedNestedWrites(model, () =>
      newContext({ schema, fragments: {}, variables: {} }, model, {
        jwt: token(model, { [subject]: "someone" }),
      }),
    ),
  );
  return out;
}

const isRecord = (x: unknown): x is Record<string, unknown> =>
  typeof x === "object" && x !== null && !Array.isArray(x);

/**
 * Fields of `node` a rule tests at the top level of its `part` (`node`,
 * `source` or `target`), through AND, OR and NOT. `pinned`: every test of
 * the field names one node by a claim or context value (`author: {
 * isViewer: true }` once expanded), so it passes for one node only.
 */
function testedFields(
  where: unknown,
  part: "node" | "source" | "target",
  node: NodeType,
  model: GraphModel,
  out = new Map<string, { pinned: boolean }>(),
): Map<string, { pinned: boolean }> {
  if (Array.isArray(where)) {
    for (const w of where) testedFields(w, part, node, model, out);
    return out;
  }
  if (!isRecord(where)) return out;
  for (const [key, value] of Object.entries(where)) {
    if (key === "AND" || key === "OR" || key === "NOT") {
      testedFields(value, part, node, model, out);
    } else if (key === part) nodeTests(value, node, model, out);
    // `${node.trip.key}` anywhere in the rule reads the node's `trip` too.
    for (const name of referencedFields(value, part)) {
      if (node.fields.has(name)) out.set(name, { pinned: false });
    }
  }
  return out;
}

/** The first field of every `${<part>.path}` placeholder in a rule value. */
function referencedFields(value: unknown, part: string): string[] {
  if (typeof value === "string") {
    return [...value.matchAll(PLACEHOLDER)]
      .filter((m) => m[1] === part)
      .map((m) => m[2]!.split(".")[0]!);
  }
  if (Array.isArray(value)) {
    return value.flatMap((v) => referencedFields(v, part));
  }
  if (isRecord(value)) {
    return Object.values(value).flatMap((v) => referencedFields(v, part));
  }
  return [];
}

function nodeTests(
  filter: unknown,
  node: NodeType,
  model: GraphModel,
  out: Map<string, { pinned: boolean }>,
): void {
  if (Array.isArray(filter)) {
    for (const f of filter) nodeTests(f, node, model, out);
    return;
  }
  if (!isRecord(filter)) return;
  for (const [key, value] of Object.entries(filter)) {
    if (key === "AND" || key === "OR" || key === "NOT") {
      nodeTests(value, node, model, out);
      continue;
    }
    const name = node.fields.has(key)
      ? key
      : key.replace(/(Connection|Aggregate|Exists)$/, "");
    const f = node.fields.get(name);
    if (!f || (f.kind !== "scalar" && f.kind !== "relationship")) continue;
    const pinned =
      f.kind === "relationship" &&
      key === name &&
      pinsOne(value, model.nodes.get(f.target));
    out.set(name, { pinned: (out.get(name)?.pinned ?? true) && pinned });
  }
}

/** `{ <key or unique field>: { eq: "$jwt.…" } }`: one node, by a claim. */
function pinsOne(value: unknown, target: NodeType | undefined): boolean {
  if (!target || !isRecord(value)) return false;
  const keys = Object.keys(value);
  if (keys.length !== 1) return false;
  const f = target.fields.get(keys[0]!);
  const test = value[keys[0]!];
  return (
    f?.kind === "scalar" &&
    (f.key || f.unique) &&
    !f.list &&
    isRecord(test) &&
    Object.keys(test).length === 1 &&
    typeof test["eq"] === "string" &&
    test["eq"].startsWith("$")
  );
}

const REPOINT: readonly NestedOperation[] = ["CONNECT", "DISCONNECT", "CREATE"];

/**
 * The nested writes an update input offers that re-point a single
 * relationship (`connect`, `disconnect`, `create`), or none.
 */
function repointable(node: NodeType, f: Field): NestedOperation[] {
  if (
    f.kind !== "relationship" ||
    f.list ||
    !node.mutations.has("UPDATE") ||
    !f.settableOn.update
  ) {
    return [];
  }
  return REPOINT.filter((op) => f.nestedOperations.has(op));
}

/** Whether an update can change the field's value. */
function updatable(node: NodeType, f: Field): boolean {
  if (f.kind === "scalar") {
    return node.mutations.has("UPDATE") && f.settableOn.update && !f.readonly;
  }
  return repointable(node, f).length > 0;
}

/**
 * A rule over a field the guarded operation can rewrite:
 * - an UPDATE rule testing a single relationship the update input can
 *   re-point: the rule sees the node before (and, validated AFTER, after)
 *   the write, so a caller passing it on two targets moves the node;
 * - a CREATE validate rule testing a field UPDATE can change while no
 *   UPDATE rule tests it: created as the rule demands, then changed.
 * Tests that name one node by a claim (`isViewer`) can only re-point to
 * the caller's own node, so the first check skips them. A type no
 * signed-in caller without roles may update has nothing to re-point, and a
 * scalar a CREATE rule tests is usually state an update moves on (a
 * request's status), so the second check looks at relationships only.
 */
function rewritableRuleFields(
  model: GraphModel,
  authenticated: CompileContext,
): ModelWarning[] {
  const out: ModelWarning[] = [];
  for (const node of model.nodes.values()) {
    if (!node.mutations.has("UPDATE")) continue;
    const probe = { ...authenticated, params: {}, vars: new Set(["n"]) };
    const { verdict } = typeVerdict(probe, node, "UPDATE");
    if (verdict === "denied" || verdict === "unauthenticated") continue;
    const rules = [
      ...(node.authorization?.filter ?? []).map((r, i) => ({
        r,
        label: `filter[${i}]`,
        after: false,
      })),
      ...(node.authorization?.validate ?? []).map((r, i) => ({
        r,
        label: `validate[${i}]`,
        after: r.when.has("AFTER"),
      })),
    ];
    const updateRules = rules.filter(({ r }) => r.operations.has("UPDATE"));
    const updateTests = new Map<
      string,
      { labels: string[]; after: boolean; pinned: boolean }
    >();
    for (const { r, label, after } of updateRules) {
      for (const [name, t] of testedFields(r.where, "node", node, model)) {
        const seen = updateTests.get(name);
        updateTests.set(name, {
          labels: [...(seen?.labels ?? []), label],
          after: (seen?.after ?? false) || after,
          pinned: (seen?.pinned ?? true) && t.pinned,
        });
      }
    }
    for (const [name, t] of updateTests) {
      const f = node.fields.get(name)!;
      const ops = repointable(node, f);
      if (ops.length === 0 || t.pinned || f.kind !== "relationship") continue;
      out.push({
        type: node.name,
        field: name,
        message: `${t.labels.join(", ")} ${t.labels.length === 1 ? "guards" : "guard"} UPDATE by testing ${name}, which the update input can re-point (${ops.map((o) => o.toLowerCase()).join(", ")}): ${t.after ? `the rule runs before and after the write, so a caller who passes it on both the old and the new ${f.target} moves the ${node.name}` : `the rule only sees the ${node.name} before the write, so a caller who passes it can move it to any ${f.target}`}; declare ${name} @settable(onCreate: true, onUpdate: false) if it is fixed once created`,
      });
    }
    const createTested = new Map<string, string[]>();
    (node.authorization?.validate ?? []).forEach((r, i) => {
      if (!r.operations.has("CREATE")) return;
      for (const name of testedFields(r.where, "node", node, model).keys()) {
        createTested.set(name, [
          ...(createTested.get(name) ?? []),
          `validate[${i}]`,
        ]);
      }
    });
    for (const [name, labels] of createTested) {
      const f = node.fields.get(name)!;
      if (updateTests.has(name) || !updatable(node, f)) continue;
      // A field-level UPDATE rule guards writing the field itself.
      if (f.authorization?.validate.some((r) => r.operations.has("UPDATE"))) {
        continue;
      }
      if (f.kind !== "relationship" || f.list) continue;
      out.push({
        type: node.name,
        field: name,
        message: `${labels.join(", ")} ${labels.length === 1 ? "tests" : "test"} ${name} on CREATE, but UPDATE can re-point it and no UPDATE rule tests it: a caller can create the ${node.name} as the rule demands, then change ${name}; declare ${name} @settable(onCreate: true, onUpdate: false), or add an UPDATE rule that tests it`,
      });
    }
  }
  return out;
}

/**
 * A validate rule (other than READ) or relationship rule testing a masked
 * field: rules see the stored value, so success versus FORBIDDEN tells a
 * caller what the mask hides from them. Skipped when the claims of an
 * authenticated caller already settle the mask.
 */
function maskOracles(
  model: GraphModel,
  authenticated: CompileContext,
): ModelWarning[] {
  const out: ModelWarning[] = [];
  const masked = (node: NodeType, name: string) => {
    const f = node.fields.get(name);
    return (
      f?.kind === "scalar" &&
      (f.authorization?.mask?.length ?? 0) > 0 &&
      !maskSettled(authenticated, node, f)
    );
  };
  const report = (owner: NodeType, name: string, rule: string, ops: string[]) =>
    out.push({
      type: owner.name,
      field: name,
      message: `${rule} (${ops.join(", ")}) tests ${owner.name}.${name}, which a mask hides from callers failing its unless; rules see the stored value, so whether the request succeeds or is FORBIDDEN tells such a caller the hidden value. Test what the caller can see, or make sure every caller the rule decides for passes the mask`,
    });
  for (const node of model.nodes.values()) {
    const seen = new Set<string>();
    const once = (...args: Parameters<typeof report>) => {
      const id = `${args[0].name}.${args[1]}:${args[2]}`;
      if (seen.has(id)) return;
      seen.add(id);
      report(...args);
    };
    // A create's rules test values the caller wrote: nothing hidden.
    const opsOf = (r: { operations: ReadonlySet<string> }) =>
      [...r.operations].filter((op) => op !== "READ" && op !== "CREATE");
    (node.authorization?.validate ?? []).forEach((r, i) => {
      const ops = opsOf(r);
      if (ops.length === 0) return;
      for (const name of testedFields(r.where, "node", node, model).keys()) {
        if (masked(node, name)) once(node, name, `validate[${i}]`, ops);
      }
    });
    for (const f of node.fields.values()) {
      (f.authorization?.validate ?? []).forEach((r, i) => {
        const ops = opsOf(r);
        if (ops.length === 0) return;
        const label = `${node.name}.${f.name} validate[${i}]`;
        for (const part of ["node", "source"] as const) {
          for (const name of testedFields(r.where, part, node, model).keys()) {
            if (masked(node, name)) once(node, name, label, ops);
          }
        }
        const target =
          f.kind === "relationship" ? model.nodes.get(f.target) : undefined;
        if (!target) return;
        for (const name of testedFields(
          r.where,
          "target",
          target,
          model,
        ).keys()) {
          if (masked(target, name)) once(target, name, label, ops);
        }
      });
    }
  }
  return out;
}

/**
 * Nested creates, updates and deletes an input offers into a type whose
 * rules for that write refuse every authenticated caller without a role
 * or claim beyond the subject: surface only an admin (or the bypass) can
 * use. Decided with the access matrix's verdicts; skipped on an input the
 * same caller cannot use at all (an admin-only type's links).
 */
function refusedNestedWrites(
  model: GraphModel,
  context: () => CompileContext,
): ModelWarning[] {
  const out: ModelWarning[] = [];
  const subject = model.viewer?.claim ?? "sub";
  const refuses = (t: NodeType, op: "CREATE" | "UPDATE" | "DELETE") => {
    const { verdict } = typeVerdict(context(), t, op);
    return verdict === "denied" || verdict === "unauthenticated";
  };
  for (const node of model.nodes.values()) {
    // An input the same caller cannot use advertises nothing to them.
    const createOpen = node.mutations.has("CREATE") && !refuses(node, "CREATE");
    const updateOpen = node.mutations.has("UPDATE") && !refuses(node, "UPDATE");
    if (!createOpen && !updateOpen) continue;
    for (const f of node.fields.values()) {
      if (f.kind !== "relationship" || f.via) continue;
      const offered = new Set<"CREATE" | "UPDATE" | "DELETE">();
      if (createOpen && f.settableOn.create) {
        if (f.nestedOperations.has("CREATE")) offered.add("CREATE");
      }
      if (updateOpen && f.settableOn.update) {
        for (const op of ["CREATE", "UPDATE", "DELETE"] as const) {
          if (f.nestedOperations.has(op)) offered.add(op);
        }
      }
      const targets = f.members
        .map((m) => model.nodes.get(m))
        .filter((t): t is NodeType => t !== undefined);
      if (targets.length === 0) continue;
      const refused = [...offered].filter((op) =>
        targets.every((t) => refuses(t, op)),
      );
      if (refused.length === 0) continue;
      const kept = [...f.nestedOperations].filter(
        (op) => !refused.includes(op as "CREATE"),
      );
      // Edge properties stay writable without the node: UPDATE_EDGE.
      if (
        refused.includes("UPDATE") &&
        f.properties &&
        !kept.includes("UPDATE_EDGE")
      ) {
        kept.push("UPDATE_EDGE");
      }
      out.push({
        type: node.name,
        field: f.name,
        message: `nested ${refused.map((o) => o.toLowerCase()).join(", ")} into ${f.target}: its ${refused.join(", ")} rules refuse every signed-in caller without a role or claim beyond ${subject}, so the input advertises writes only an admin (or the bypass) can make; declare nestedOperations: [${kept.join(", ")}] on ${f.name}`,
      });
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
