import {
  getArgumentValues,
  getDirectiveValues,
  GraphQLIncludeDirective,
  GraphQLSkipDirective,
  isAbstractType,
  isObjectType,
  Kind,
  type FieldNode,
  type FragmentDefinitionNode,
  type GraphQLField,
  type GraphQLObjectType,
  type GraphQLSchema,
  type SelectionSetNode,
  versionInfo,
} from "graphql";

export interface SelectionContext {
  schema: GraphQLSchema;
  fragments: Record<string, FragmentDefinitionNode>;
  /**
   * Variable values as graphql-js hands them to `getArgumentValues`: the
   * coerced map on graphql 16, `{ sources, coerced }` on 17. Read plain
   * values through `coercedVariables()`.
   */
  variables: Record<string, unknown>;
}

const GRAPHQL_17 = versionInfo.major >= 17;

/** The coerced variable values, on graphql 16 and 17 alike. */
export function coercedVariables(
  variables: unknown,
): Record<string, unknown> | undefined {
  const v = variables as Record<string, unknown> | null | undefined;
  if (v == null) return undefined;
  return GRAPHQL_17 ? (v["coerced"] as Record<string, unknown>) : v;
}

/**
 * The variable values of a `getVariableValues()` result, in the shape the
 * installed graphql's `getArgumentValues()` takes: 16 returns
 * `{ coerced }`, 17 returns `{ variableValues: { sources, coerced } }`.
 */
export function variableValuesOf(result: object): Record<string, unknown> {
  const r = result as Record<string, unknown>;
  return (GRAPHQL_17 ? r["variableValues"] : r["coerced"]) as Record<
    string,
    unknown
  >;
}

/** The fields selected on an object type, grouped by response key. */
export type FieldsByKey = Map<string, FieldNode[]>;

/**
 * Collect the fields that apply to `type` from one or more selection
 * sets: follows fragment spreads and inline fragments whose condition
 * matches, honours `@skip` / `@include`, groups by response key.
 */
export function collectFields(
  ctx: SelectionContext,
  type: GraphQLObjectType,
  selectionSets: ReadonlyArray<SelectionSetNode | undefined>,
): FieldsByKey {
  const out: FieldsByKey = new Map();
  const visited = new Set<string>();
  const visit = (set: SelectionSetNode | undefined) => {
    if (!set) return;
    for (const sel of set.selections) {
      if (!included(ctx, sel)) continue;
      if (sel.kind === Kind.FIELD) {
        const key = sel.alias?.value ?? sel.name.value;
        const list = out.get(key);
        if (list) list.push(sel);
        else out.set(key, [sel]);
      } else if (sel.kind === Kind.INLINE_FRAGMENT) {
        if (applies(ctx, type, sel.typeCondition?.name.value)) {
          visit(sel.selectionSet);
        }
      } else {
        const name = sel.name.value;
        if (visited.has(name)) continue;
        visited.add(name);
        const frag = ctx.fragments[name];
        if (frag && applies(ctx, type, frag.typeCondition.name.value)) {
          visit(frag.selectionSet);
        }
      }
    }
  };
  for (const set of selectionSets) visit(set);
  return out;
}

/** The merged sub-selections of every field node under one response key. */
export function subSelections(nodes: readonly FieldNode[]): SelectionSetNode[] {
  return nodes.flatMap((n) => (n.selectionSet ? [n.selectionSet] : []));
}

/** Coerced argument values of a field, variables substituted. */
export function fieldArgs(
  ctx: SelectionContext,
  def: GraphQLField<unknown, unknown>,
  node: FieldNode,
): Record<string, unknown> {
  return getArgumentValues(def, node, ctx.variables) as Record<string, unknown>;
}

function included(
  ctx: SelectionContext,
  node: Parameters<typeof getDirectiveValues>[1],
): boolean {
  const skip = getDirectiveValues(GraphQLSkipDirective, node, ctx.variables);
  if (skip?.["if"] === true) return false;
  const include = getDirectiveValues(
    GraphQLIncludeDirective,
    node,
    ctx.variables,
  );
  return include?.["if"] !== false;
}

function applies(
  ctx: SelectionContext,
  type: GraphQLObjectType,
  condition: string | undefined,
): boolean {
  if (condition === undefined || condition === type.name) return true;
  const cond = ctx.schema.getType(condition);
  if (cond && isAbstractType(cond)) {
    return ctx.schema.isSubType(cond, type);
  }
  return cond !== undefined && isObjectType(cond) && cond.name === type.name;
}
