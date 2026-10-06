// Who may do what, read off the model: for every type and guarded field,
// each operation as each kind of caller (anonymous, authenticated, and each
// role the rules test). The claims part of every rule is decided the way a
// request decides it; what is left depends on the rows. Stable output, so
// consumers can snapshot it and review changes as diffs.

import {
  Kind,
  valueFromASTUntyped,
  visit,
  type FragmentDefinitionNode,
  type GraphQLSchema,
  type OperationDefinitionNode,
  type SelectionSetNode,
} from "graphql";
import { newContext } from "../compile/context.js";
import type {
  AuthOperation,
  CypherField,
  Field,
  GraphModel,
  NodeType,
  RelationshipOperation,
} from "../model/types.js";
import { lowerFirst } from "../model/build/shapes.js";
import { names } from "../schema/names.js";
import { mutationNames } from "../schema/mutations.js";
import {
  fieldAs,
  isRecord,
  nodeAs,
  principalsOf,
  propertyAs,
  type Principal,
} from "./principals.js";
import {
  PROPERTY_OPS,
  fieldVerdict,
  fieldWriteGuarded,
  fieldWriteVerdict,
  propertyVerdict,
  relationshipAuthentication,
  relationshipVerdict,
  rootFieldVerdict,
  typeVerdict,
  type AccessEntry,
  type AccessVerdict,
} from "./verdicts.js";

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
  /**
   * Variables in the field's arguments that were given no value: what
   * they would write through relationship fields is not in `access`.
   */
  unresolved?: string[];
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

/** One write a mutation's input makes below its root field. */
type NestedWrite =
  | {
      kind: "relationship";
      node: NodeType;
      field: Field & { kind: "relationship" };
      op: RelationshipOperation;
    }
  | { kind: "type"; node: NodeType; op: AuthOperation };

/** `x` when a list, `[x]` when one value, nothing when absent. */
const asList = (x: unknown): unknown[] =>
  Array.isArray(x) ? x : x === undefined || x === null ? [] : [x];

/** A type has `@authentication` or a rule for `op`. */
const ruled = (node: NodeType, op: AuthOperation): boolean =>
  (node.authentication?.has(op) ?? false) ||
  [
    ...(node.authorization?.filter ?? []),
    ...(node.authorization?.validate ?? []),
  ].some((r) => r.operations.has(op));

/**
 * The relationship writes in `input` (a create or update input of `node`),
 * as the mutation planner reads them: a connect or nested create is a
 * CONNECT, a disconnect or nested delete a DISCONNECT, as is re-pointing a
 * single relationship in an update; `update: { edge }` is an UPDATE_EDGE;
 * nested creates, updates and deletes are that operation on the target,
 * and are read on in turn.
 */
function nestedWrites(
  model: GraphModel,
  node: NodeType,
  input: unknown,
  update: boolean,
  out: NestedWrite[],
): void {
  if (!isRecord(input)) return;
  for (const field of node.fields.values()) {
    if (field.kind !== "relationship") continue;
    const value = input[field.name];
    if (!isRecord(value)) continue;
    const relationship = (op: RelationshipOperation) =>
      out.push({ kind: "relationship", node, field, op });
    // Unions and interfaces take their writes keyed by member type.
    const targets = model.abstracts.has(field.target)
      ? (model.abstracts.get(field.target)?.members ?? [])
      : [field.target];
    const parts = (key: string): Array<[NodeType, unknown]> =>
      targets.flatMap((name) => {
        const target = model.nodes.get(name);
        if (!target) return [];
        const given = model.abstracts.has(field.target)
          ? isRecord(value[key])
            ? (value[key] as Record<string, unknown>)[name]
            : undefined
          : value[key];
        return asList(given).map((v): [NodeType, unknown] => [target, v]);
      });
    const connect = parts("connect");
    const create = parts("create");
    const adds = connect.length + create.length > 0;
    if (adds) {
      relationship("CONNECT");
      for (const end of new Set([
        node,
        ...connect.concat(create).map(([t]) => t),
      ])) {
        if (ruled(end, "CREATE_RELATIONSHIP")) {
          out.push({ kind: "type", node: end, op: "CREATE_RELATIONSHIP" });
        }
      }
    }
    const given = (key: string) =>
      value[key] !== undefined && value[key] !== null && value[key] !== false;
    if (update) {
      const removes =
        given("disconnect") || given("delete") || (!field.list && adds);
      if (removes) {
        relationship("DISCONNECT");
        if (ruled(node, "DELETE_RELATIONSHIP")) {
          out.push({ kind: "type", node, op: "DELETE_RELATIONSHIP" });
        }
      }
      if (given("delete")) {
        for (const name of targets) {
          const target = model.nodes.get(name);
          if (target) out.push({ kind: "type", node: target, op: "DELETE" });
        }
      }
    }
    for (const [target, item] of create) {
      out.push({ kind: "type", node: target, op: "CREATE" });
      nestedWrites(
        model,
        target,
        isRecord(item) ? item["node"] : undefined,
        false,
        out,
      );
    }
    for (const [target, item] of parts("update")) {
      if (!isRecord(item)) continue;
      if (isRecord(item["edge"])) relationship("UPDATE_EDGE");
      if (isRecord(item["node"])) {
        out.push({ kind: "type", node: target, op: "UPDATE" });
        nestedWrites(model, target, item["node"], true, out);
      }
    }
  }
}

/**
 * Who may run `operation`: each root field's verdict per principal, read
 * off the same rules as `accessMatrix`. A mutation's inputs are read for
 * the relationship writes they make, each a part of its root field's
 * verdict; an input given as a variable is read when `variables` has its
 * value and listed in `unresolved` when it does not. Selections below a
 * root field answer to their own types' READ rules (see the matrix).
 */
export function operationAccess(
  model: GraphModel,
  schema: GraphQLSchema,
  operation: OperationDefinitionNode,
  fragments: ReadonlyMap<string, FragmentDefinitionNode>,
  variables: Readonly<Record<string, unknown>> = {},
): OperationAccess {
  const principals = principalsOf(model);
  const context = (p: Principal) =>
    newContext({ schema, fragments: {}, variables: {} }, p.model ?? model, {
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
    // What the field's inputs write through relationship fields.
    const nested: NestedWrite[] = [];
    const unresolved = new Set<string>();
    if (operation.operation === "mutation" && target.kind === "node") {
      const creates = target.ops.includes("CREATE");
      const updates = target.ops.includes("UPDATE");
      for (const arg of sel.arguments ?? []) {
        if (arg.name.value !== "input" && arg.name.value !== "update") continue;
        visit(arg.value, {
          Variable(v) {
            if (!(v.name.value in variables)) unresolved.add(v.name.value);
          },
        });
        const value = valueFromASTUntyped(
          arg.value,
          variables as Record<string, unknown>,
        );
        for (const item of asList(value)) {
          // An upsert may do either; an update re-points and removes.
          nestedWrites(model, target.node, item, updates || !creates, nested);
        }
      }
    }
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
            ...typeVerdict(context(p), nodeAs(p, target.node), op),
          });
        }
        const seen = new Set<string>();
        for (const w of nested) {
          const label =
            w.kind === "relationship"
              ? `${w.node.name}.${w.field.name} ${w.op}`
              : `${w.node.name} ${w.op}`;
          if (seen.has(label)) continue;
          seen.add(label);
          parts.push({
            label,
            ...(w.kind === "relationship"
              ? relationshipVerdict(
                  context(p),
                  nodeAs(p, w.node),
                  fieldAs(p, w.node, w.field),
                  w.op,
                )
              : typeVerdict(context(p), nodeAs(p, w.node), w.op)),
          });
        }
      } else {
        for (const member of target.members) {
          parts.push({
            label: member.name,
            ...typeVerdict(context(p), nodeAs(p, member), "READ"),
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
      ...(unresolved.size > 0 ? { unresolved: [...unresolved].sort() } : {}),
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
    newContext({ schema, fragments: {}, variables: {} }, p.model ?? model, {
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
          ...typeVerdict(context(p), nodeAs(p, node), op),
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
        // `@authentication` on the field guards connecting and
        // disconnecting through it, with or without a rule beside it.
        for (const op of ["CONNECT", "DISCONNECT"] as const) {
          const guard = relationshipAuthentication(op);
          if (guard && f.authentication?.has(guard)) ruleOps.add(op);
        }
        for (const op of [...ruleOps].sort()) {
          for (const p of principals) {
            out.push({
              type: node.name,
              field: f.name,
              operation: op,
              principal: p.name,
              ...relationshipVerdict(
                context(p),
                nodeAs(p, node),
                fieldAs(p, node, f),
                op,
              ),
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
            ...fieldVerdict(context(p), nodeAs(p, node), fieldAs(p, node, f)),
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
            ...fieldWriteVerdict(
              context(p),
              nodeAs(p, node),
              fieldAs(p, node, f),
              op,
            ),
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
            ...propertyVerdict(context(p), propertyAs(p, props, f), op),
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

const byName = (a: { name: string }, b: { name: string }) =>
  a.name < b.name ? -1 : a.name > b.name ? 1 : 0;
