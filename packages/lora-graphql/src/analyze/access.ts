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
import { newContext } from "../compile/context.js";
import type {
  AuthOperation,
  CypherField,
  GraphModel,
  NodeType,
  RelationshipOperation,
} from "../model/types.js";
import { lowerFirst } from "../model/build.js";
import { names } from "../schema/names.js";
import { mutationNames } from "../schema/mutations.js";
import {
  fieldAs,
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
  relationshipVerdict,
  rootFieldVerdict,
  typeVerdict,
  type AccessEntry,
  type AccessVerdict,
} from "./verdicts.js";

export { accessLints } from "./access-lint.js";
export type { AccessEntry, AccessVerdict } from "./verdicts.js";

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
