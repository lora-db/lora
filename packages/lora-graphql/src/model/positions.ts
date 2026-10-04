// Where each directive applies. GraphQL's `on` locations already refuse a
// directive on the wrong kind of definition (a field directive on a type);
// this table refuses one on the wrong kind of *field or type*: a
// `@selectable` on a plain object field, an `@authorization` on an
// interface field. A directive the model would not apply is a model error,
// never silently ignored.

import {
  getNamedType,
  isInterfaceType,
  isObjectType,
  isUnionType,
  parse,
  type ConstDirectiveNode,
  type GraphQLField,
  type GraphQLNamedType,
  type GraphQLSchema,
} from "graphql";
import type { ModelProblem } from "../errors.js";
import { directiveTypeDefs } from "./directives.js";

export type Position =
  | "node type"
  | "relationship properties type"
  | "@jwt type"
  | "object type without @node"
  | "Query or Mutation type"
  | "interface"
  | "union"
  | "node field"
  | "relationship field"
  | "@cypher field"
  | "Query or Mutation field"
  | "@customResolver field"
  | "relationship property"
  | "interface field"
  | "interface relationship field"
  | "field of an object type without @node"
  | "@jwt claim";

/** Directives on arguments of @cypher fields. */
const ARGUMENT_DIRECTIVES: ReadonlySet<string> = new Set(["size", "range"]);

/**
 * The directives each position applies, plus those it refuses with a more
 * specific message elsewhere in the model build (listed so they are not
 * reported twice).
 */
export const DIRECTIVE_POSITIONS: Readonly<
  Record<Position, readonly string[]>
> = {
  "node type": [
    "node",
    "authorizationRule",
    "query",
    "mutation",
    "subscription",
    "authentication",
    "authorization",
    "fulltext",
    "limit",
    "uniqueTogether",
  ],
  "relationship properties type": ["relationshipProperties"],
  "@jwt type": ["jwt"],
  "object type without @node": [],
  "Query or Mutation type": [],
  interface: ["query", "plural", "limit"],
  union: ["query", "plural", "limit"],
  "node field": [
    "key",
    "unique",
    "private",
    "relayId",
    "sortable",
    "groupBy",
    "filterable",
    "alias",
    "index",
    "default",
    "timestamp",
    "readonly",
    "authentication",
    "settable",
    "selectable",
    "populatedBy",
    "authorization",
    "vector",
    // Refused with "@relationship target must be a @node type".
    "relationship",
  ],
  "relationship field": [
    "relationship",
    "authorization",
    "authentication",
    "settable",
    "readonly",
    "filterable",
    "cardinality",
    "limit",
    // Refused with "@x is not allowed on a relationship field".
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
  ],
  "@cypher field": [
    "cypher",
    "authorization",
    "authentication",
    "filterable",
    "sortable",
    // Refused with "@x cannot be combined with @cypher".
    "relationship",
    "key",
    "unique",
    "alias",
    "index",
    "default",
    "timestamp",
    "groupBy",
  ],
  "Query or Mutation field": [
    "cypher",
    "authorization",
    "authentication",
    // Refused with a specific message.
    "filterable",
    "sortable",
    "relationship",
    "key",
    "unique",
    "alias",
    "index",
    "default",
    "timestamp",
    "groupBy",
  ],
  "@customResolver field": [
    "customResolver",
    "authentication",
    // Refused with "@x cannot be combined with @customResolver".
    "cypher",
    "relationship",
    "key",
    "filterable",
    "sortable",
    "groupBy",
  ],
  "relationship property": [
    "private",
    "sortable",
    "filterable",
    "alias",
    "default",
    "timestamp",
    "readonly",
    "authentication",
    "settable",
    "selectable",
    "authorization",
    // Refused with a specific message.
    "key",
    "unique",
    "relayId",
    "index",
    "vector",
    "groupBy",
  ],
  "interface field": [
    "filterable",
    "sortable",
    // Refused with a specific message.
    "key",
    "unique",
    "relayId",
    "index",
    "vector",
    "groupBy",
  ],
  "interface relationship field": ["declareRelationship"],
  "field of an object type without @node": [],
  "@jwt claim": ["jwtClaim", "viewer"],
};

const HINTS: Partial<Record<Position, string>> = {
  "interface field": "; put it on the field of each implementation",
  "@customResolver field": "; the resolver decides what it returns",
  "field of an object type without @node":
    "; this type only shapes @cypher results",
  "object type without @node": "; this type only shapes @cypher results",
};

let known: ReadonlySet<string> | undefined;

/** The directives this library defines (not graphql's built-ins). */
export function libraryDirectives(): ReadonlySet<string> {
  known ??= new Set(
    parse(directiveTypeDefs).definitions.flatMap((d) =>
      d.kind === "DirectiveDefinition" ? [d.name.value] : [],
    ),
  );
  return known;
}

export interface PositionContext {
  schema: GraphQLSchema;
  nodeNames: ReadonlySet<string>;
  propsNames: ReadonlySet<string>;
  jwtType: string | undefined;
  /** @node types, interfaces and unions: what a relationship may target. */
  targetNames: ReadonlySet<string>;
  userTypes: readonly GraphQLNamedType[];
}

/** Report every library directive used where the model would ignore it. */
export function checkDirectivePositions(
  ctx: PositionContext,
  problems: ModelProblem[],
): void {
  const ours = libraryDirectives();
  const check = (
    directives: readonly ConstDirectiveNode[],
    position: Position,
    type: string,
    field?: string,
  ) => {
    const allowed = DIRECTIVE_POSITIONS[position];
    for (const dir of directives) {
      const name = dir.name.value;
      if (!ours.has(name) || allowed.includes(name)) continue;
      problems.push({
        type,
        ...(field ? { field } : {}),
        message: `@${name} is not supported on ${/^[aeiou]/.test(position) ? "an" : "a"} ${position}${HINTS[position] ?? ""}`,
      });
    }
  };
  const typeDirectives = (t: GraphQLNamedType) => [
    ...(t.astNode?.directives ?? []),
    ...t.extensionASTNodes.flatMap((n) => n.directives ?? []),
  ];
  const has = (f: GraphQLField<unknown, unknown>, name: string) =>
    f.astNode?.directives?.some((x) => x.name.value === name) ?? false;

  for (const t of ctx.userTypes) {
    if (isUnionType(t)) {
      check(typeDirectives(t), "union", t.name);
      continue;
    }
    if (isInterfaceType(t)) {
      check(typeDirectives(t), "interface", t.name);
      for (const f of Object.values(t.getFields())) {
        check(
          f.astNode?.directives ?? [],
          has(f as GraphQLField<unknown, unknown>, "declareRelationship")
            ? "interface relationship field"
            : "interface field",
          t.name,
          f.name,
        );
      }
      continue;
    }
    if (!isObjectType(t)) continue;
    let typePosition: Position;
    let fieldPosition: (f: GraphQLField<unknown, unknown>) => Position;
    if (t.name === "Query" || t.name === "Mutation") {
      typePosition = "Query or Mutation type";
      fieldPosition = () => "Query or Mutation field";
    } else if (t.name === ctx.jwtType) {
      typePosition = "@jwt type";
      fieldPosition = () => "@jwt claim";
    } else if (ctx.nodeNames.has(t.name)) {
      typePosition = "node type";
      fieldPosition = (f) =>
        has(f, "customResolver")
          ? "@customResolver field"
          : has(f, "cypher")
            ? "@cypher field"
            : ctx.targetNames.has(getNamedType(f.type).name)
              ? "relationship field"
              : "node field";
    } else if (ctx.propsNames.has(t.name)) {
      typePosition = "relationship properties type";
      fieldPosition = () => "relationship property";
    } else if (t.name === "Subscription") {
      continue;
    } else {
      typePosition = "object type without @node";
      fieldPosition = () => "field of an object type without @node";
    }
    check(typeDirectives(t), typePosition, t.name);
    for (const f of Object.values(t.getFields())) {
      check(f.astNode?.directives ?? [], fieldPosition(f), t.name, f.name);
    }
  }
  // Argument directives (`@size`, `@range`) apply to arguments of @cypher fields:
  // every other argument is generated, or passed to a resolver as is.
  for (const t of ctx.userTypes) {
    if (!isObjectType(t) && !isInterfaceType(t)) continue;
    for (const f of Object.values(t.getFields())) {
      const cypher = has(f as GraphQLField<unknown, unknown>, "cypher");
      for (const a of f.args) {
        for (const dir of a.astNode?.directives ?? []) {
          const name = dir.name.value;
          if (!ours.has(name) || (cypher && ARGUMENT_DIRECTIVES.has(name)))
            continue;
          problems.push({
            type: t.name,
            field: f.name,
            message: `argument ${a.name}: @${name} applies to arguments of @cypher fields`,
          });
        }
      }
    }
  }
}
