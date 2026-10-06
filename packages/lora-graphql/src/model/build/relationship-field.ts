// A `@relationship` field of a @node type, and an interface's
// `@declareRelationship` checked against its implementations.

import {
  getNamedType,
  type GraphQLDirective,
  type GraphQLField,
  type GraphQLObjectType,
} from "graphql";
import type { ModelProblem } from "../../errors.js";
import type {
  AuthorizationWhere,
  DeclaredRelationship,
  NestedOperation,
  NodeType,
  PageLimit,
  RelationshipField,
} from "../types.js";
import { resolveLimit } from "./limits.js";
import { unwrap } from "./shapes.js";
import { directive } from "./directive-args.js";
import { authOps, readOnlyRules } from "./authorization.js";

const RELATIONSHIP_TYPE = /^[A-Z][A-Z0-9_]*$/;

/**
 * An interface field under `@declareRelationship`: every implementation
 * must declare a relationship field of that name with the same target and
 * shape (the relationship type and direction may differ). Returns the
 * problem when one does not.
 */
export function declareRelationship(
  iface: string,
  f: GraphQLField<unknown, unknown>,
  members: readonly string[],
  nodes: ReadonlyMap<string, NodeType>,
): DeclaredRelationship | string {
  let first: RelationshipField | undefined;
  for (const m of members) {
    const mf = nodes.get(m)?.fields.get(f.name);
    if (mf?.kind !== "relationship") {
      return `${m} implements ${iface} but ${f.name} on it is not a @relationship field`;
    }
    if (!first) first = mf;
    else if (
      mf.target !== first.target ||
      mf.list !== first.list ||
      mf.properties !== first.properties
    ) {
      return `every implementation must declare ${f.name} with the same target, list shape and properties (${first.owner} and ${m} differ)`;
    }
  }
  if (!first) return "has no @node implementations";
  return {
    name: f.name,
    target: first.target,
    list: first.list,
    description: f.description ?? undefined,
  };
}

export function buildRelationshipField(
  t: GraphQLObjectType,
  f: GraphQLField<unknown, unknown>,
  d: (n: string) => GraphQLDirective,
  problems: ModelProblem[],
  propsNames: Set<string>,
  globalLimit: PageLimit,
): RelationshipField | undefined {
  const at = (message: string) =>
    problems.push({ type: t.name, field: f.name, message });
  const authorization = readOnlyRules(
    directive(d("authorization"), f, at),
    at,
    true,
  );
  const rel = directive(d("relationship"), f, at);
  if (!rel) {
    at(
      `${getNamedType(f.type).name} is a @node type; add @relationship(type:, direction:)`,
    );
    return undefined;
  }
  const shape = unwrap(f.type);
  if (shape.list && (!shape.itemRequired || !shape.required)) {
    at("relationship lists must be written [T!]!");
  }
  if (shape.depth > 1) at("nested lists are not supported");

  const type = rel["type"] as string;
  if (!RELATIONSHIP_TYPE.test(type)) {
    at(`relationship type \`${type}\` must be SCREAMING_SNAKE_CASE`);
  }
  const properties = rel["properties"] as string | undefined;
  if (properties !== undefined && !propsNames.has(properties)) {
    at(`${properties} is not a @relationshipProperties type`);
  }
  // Directives about stored values have no meaning on a relationship:
  // refused, never silently ignored.
  for (const forbidden of [
    "key",
    "unique",
    "sortable",
    "alias",
    "index",
    "default",
    "timestamp",
    "populatedBy",
    "selectable",
    "private",
    "groupBy",
    "vector",
    "relayId",
    "storedAs",
  ]) {
    if (directive(d(forbidden), f, at)) {
      at(`@${forbidden} is not allowed on a relationship field`);
    }
  }
  const settableArgs = directive(d("settable"), f, at);
  const readonlyFlag = directive(d("readonly"), f, at) !== undefined;
  const filterable = directive(d("filterable"), f, at);
  if (filterable?.["byValue"] !== undefined) {
    at("@filterable on a relationship takes no byValue");
  }
  const cardinality = directive(d("cardinality"), f, at)?.["max"] as
    | number
    | undefined;
  if (cardinality !== undefined && cardinality < 1) {
    at("@cardinality(max:) must be at least 1");
  }
  if (cardinality !== undefined && !shape.list) {
    at("@cardinality only applies to list relationships");
  }
  const limitArgs = directive(d("limit"), f, at);
  if (limitArgs && !shape.list) at("@limit only applies to lists");

  return {
    kind: "relationship",
    authorization,
    name: f.name,
    owner: t.name,
    type,
    direction: rel["direction"] as "IN" | "OUT",
    target: getNamedType(f.type).name,
    list: shape.list,
    required: shape.required,
    properties,
    // Filled in once interfaces and unions are known.
    members: [],
    filterable: filterable !== undefined,
    queryDirection: rel["queryDirection"] as "DIRECTED" | "UNDIRECTED",
    onDelete: rel["onDelete"] as "DETACH" | "CASCADE" | "RESTRICT",
    nestedOperations: new Set(rel["nestedOperations"] as NestedOperation[]),
    settableOn: {
      create:
        !readonlyFlag &&
        ((settableArgs?.["onCreate"] as boolean | undefined) ?? true),
      update:
        !readonlyFlag &&
        ((settableArgs?.["onUpdate"] as boolean | undefined) ?? true),
    },
    aggregate: rel["aggregate"] as boolean,
    cardinality,
    limit: limitArgs
      ? resolveLimit(limitArgs, globalLimit, t.name, f.name, problems)
      : undefined,
    authentication: authOps(directive(d("authentication"), f, at)),
    authenticationJwt: directive(d("authentication"), f, at)?.["jwt"] as
      | AuthorizationWhere
      | undefined,
    description: f.description ?? undefined,
  };
}
