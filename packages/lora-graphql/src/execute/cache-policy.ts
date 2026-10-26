// An operation's cache policy from the `@cacheControl` hints: what
// `execute()` reports as `cacheControl` for a response cache or a
// `Cache-Control` header to use.

import {
  getNamedType,
  getOperationAST,
  isCompositeType,
  isInterfaceType,
  isObjectType,
  Kind,
  type DocumentNode,
  type FragmentDefinitionNode,
  type GraphQLCompositeType,
  type GraphQLSchema,
  type SelectionSetNode,
} from "graphql";
import type { CacheHint, GraphModel } from "../model/types.js";
import { names } from "../schema/names.js";

/** How long an operation's result may be cached, and by whom. */
export interface CachePolicy {
  /** Seconds: the smallest `maxAge` among the fields it selects. */
  readonly maxAge: number;
  /** `PRIVATE` when any field it selects is. */
  readonly scope: "PUBLIC" | "PRIVATE";
}

/**
 * The policy of a query: every selected field takes its own hint, else
 * its type's (a generated wrapper, such as `<T>Connection`, `<T>Match` or
 * `<T>Aggregate`, takes T's), and the smallest `maxAge` wins. A scalar
 * takes its parent's. There is no policy, so nothing may be cached by
 * hint, when the operation is not a query, selects a declared type with no
 * hint, or a root field contributes none (an unhinted scalar `@cypher`
 * root, say).
 */
export function operationCachePolicy(
  schema: GraphQLSchema,
  model: GraphModel,
  document: DocumentNode,
  operationName: string | null | undefined,
): CachePolicy | undefined {
  const hints = model.cacheHints;
  if (!hints || (hints.types.size === 0 && hints.fields.size === 0)) {
    return undefined;
  }
  const operation = getOperationAST(document, operationName);
  const root = schema.getQueryType();
  if (operation?.operation !== "query" || !root) return undefined;

  const fragments = new Map<string, FragmentDefinitionNode>();
  for (const d of document.definitions) {
    if (d.kind === Kind.FRAGMENT_DEFINITION) fragments.set(d.name.value, d);
  }
  const declared = new Set<string>([
    ...model.nodes.keys(),
    ...model.abstracts.keys(),
    ...model.objects.keys(),
  ]);
  const wrapped = wrapperTypes(model);
  const typeHint = (name: string): CacheHint | undefined =>
    hints.types.get(wrapped.get(name) ?? name);

  let maxAge = Infinity;
  let scope: CachePolicy["scope"] = "PUBLIC";
  let hinted = 0;
  const take = (hint: CacheHint) => {
    hinted++;
    maxAge = Math.min(maxAge, hint.maxAge);
    if (hint.scope === "PRIVATE") scope = "PRIVATE";
  };

  // False as soon as something is selected that no hint covers.
  const visit = (
    parent: GraphQLCompositeType,
    set: SelectionSetNode,
    isRoot: boolean,
  ): boolean => {
    for (const selection of set.selections) {
      if (selection.kind === Kind.FIELD) {
        const name = selection.name.value;
        if (name.startsWith("__")) continue;
        if (!isObjectType(parent) && !isInterfaceType(parent)) continue;
        const field = parent.getFields()[name];
        if (!field) continue;
        const type = getNamedType(field.type);
        const before = hinted;
        const hint =
          model.cacheHints!.fields.get(`${parent.name}.${name}`) ??
          (isCompositeType(type) ? typeHint(type.name) : undefined);
        if (hint) take(hint);
        else if (isCompositeType(type) && declared.has(type.name)) return false;
        if (
          selection.selectionSet &&
          isCompositeType(type) &&
          !visit(type, selection.selectionSet, false)
        ) {
          return false;
        }
        if (isRoot && hinted === before) return false;
        continue;
      }
      const condition =
        selection.kind === Kind.INLINE_FRAGMENT
          ? selection.typeCondition
          : fragments.get(selection.name.value)?.typeCondition;
      const target = condition ? schema.getType(condition.name.value) : parent;
      const body =
        selection.kind === Kind.INLINE_FRAGMENT
          ? selection.selectionSet
          : fragments.get(selection.name.value)?.selectionSet;
      if (!body || !isCompositeType(target)) continue;
      if (!visit(target, body, isRoot)) return false;
    }
    return true;
  };
  if (!visit(root, operation.selectionSet, true) || hinted === 0) {
    return undefined;
  }
  return { maxAge, scope };
}

/** The generated types that stand for a node or abstract type: name → its type. */
function wrapperTypes(model: GraphModel): Map<string, string> {
  const out = new Map<string, string>();
  for (const type of [...model.nodes.keys(), ...model.abstracts.keys()]) {
    for (const wrap of [
      names.connection,
      names.edge,
      names.aggregate,
      names.match,
      names.searchConnection,
      names.searchEdge,
    ]) {
      out.set(wrap(type), type);
    }
  }
  return out;
}
