import {
  buildASTSchema,
  concatAST,
  getDirectiveValues,
  getNamedType,
  isEnumType,
  isInterfaceType,
  isListType,
  isNonNullType,
  isObjectType,
  isScalarType,
  isUnionType,
  parse,
  type DocumentNode,
  type GraphQLDirective,
  type GraphQLField,
  type GraphQLInputType,
  type GraphQLInterfaceType,
  type GraphQLUnionType,
  type GraphQLObjectType,
  type GraphQLOutputType,
  type GraphQLSchema,
} from "graphql";
import { RANGE_UNINDEXABLE } from "../analyze/indexes.js";
import { ModelError, type ModelProblem } from "../errors.js";
import { directiveTypeDefs, PRELUDE_TYPES } from "./directives.js";
import { codeOnly, scanParams } from "./cypher-lexer.js";
import type {
  AbstractType,
  SearchIndex,
  AuthOperation,
  Authorization,
  AuthorizationWhere,
  CypherArgument,
  CypherField,
  EnumType,
  Field,
  ModelWarning,
  MutationOperation,
  TypeShape,
  FilterOperator,
  GraphModel,
  IndexKind,
  DeclaredRelationship,
  NestedOperation,
  NodeType,
  PageLimit,
  RelationshipField,
  RelationshipPropertiesType,
  ScalarField,
  ScalarType,
} from "./types.js";

export interface ModelOptions {
  /** Page size used when a list or connection gets no `limit` / `first`. */
  defaultLimit?: number;
  /** Hard cap on any page size; `@limit(max:)` may only lower it. */
  maxLimit?: number;
  /**
   * Sign cursors with HMAC-SHA-256 under this secret, and reject cursors
   * whose signature does not match. Without it cursors are only tagged
   * with their sort. Changing the secret invalidates every cursor.
   */
  cursorSecret?: string;
}

export const DEFAULT_LIMIT = 25;
export const MAX_LIMIT = 100;

const BUILTIN_SCALARS: Record<string, ScalarType> = {
  String: "String",
  ID: "ID",
  Int: "Int",
  Float: "Float",
  Boolean: "Boolean",
  BigInt: "BigInt",
  Date: "Date",
  Time: "Time",
  LocalTime: "LocalTime",
  DateTime: "DateTime",
  LocalDateTime: "LocalDateTime",
  Duration: "Duration",
  Point: "Point",
  CartesianPoint: "CartesianPoint",
};

const TEXT_OPS: FilterOperator[] = ["CONTAINS", "STARTS_WITH", "ENDS_WITH"];
const RANGE_OPS: FilterOperator[] = ["LT", "LTE", "GT", "GTE"];

/** Which filter operators each stored type supports. */
const ALLOWED_OPS: Record<ScalarType, readonly FilterOperator[]> = {
  String: ["EQ", "IN", ...RANGE_OPS, ...TEXT_OPS],
  ID: ["EQ", "IN", ...RANGE_OPS, ...TEXT_OPS],
  Int: ["EQ", "IN", ...RANGE_OPS],
  Float: ["EQ", "IN", ...RANGE_OPS],
  BigInt: ["EQ", "IN", ...RANGE_OPS],
  Boolean: ["EQ"],
  Date: ["EQ", "IN", ...RANGE_OPS],
  Time: ["EQ", "IN", ...RANGE_OPS],
  LocalTime: ["EQ", "IN", ...RANGE_OPS],
  DateTime: ["EQ", "IN", ...RANGE_OPS],
  LocalDateTime: ["EQ", "IN", ...RANGE_OPS],
  Duration: ["EQ", "IN"],
  Enum: ["EQ", "IN"],
  Point: ["WITHIN_BBOX", "DISTANCE"],
  CartesianPoint: ["WITHIN_BBOX", "DISTANCE"],
};

const KEY_TYPES = new Set<ScalarType>(["String", "ID", "Int", "BigInt"]);
const UNSORTABLE = new Set<ScalarType>(["Point", "CartesianPoint", "Duration"]);
const RELATIONSHIP_TYPE = /^[A-Z][A-Z0-9_]*$/;
const ROOT_TYPES = new Set(["Query", "Mutation", "Subscription"]);

/**
 * Parse and validate annotated type definitions into a `GraphModel`.
 * Throws a `ModelError` listing every problem found.
 */
export function buildModel(
  typeDefs: string | DocumentNode,
  options: ModelOptions = {},
): GraphModel {
  const globalLimit = resolveGlobalLimit(options);
  let schema: GraphQLSchema;
  try {
    const user = typeof typeDefs === "string" ? parse(typeDefs) : typeDefs;
    schema = buildASTSchema(concatAST([parse(directiveTypeDefs), user]));
  } catch (err) {
    throw new ModelError(
      String(err instanceof Error ? err.message : err)
        .split("\n\n")
        .map((message) => ({ message })),
    );
  }

  const problems: ModelProblem[] = [];
  const d = (n: string) => schema.getDirective(n)!;
  const atType = (type: string) => (message: string) =>
    problems.push({ type, message });

  const userTypes = Object.values(schema.getTypeMap()).filter(
    (t) => !t.name.startsWith("__") && !PRELUDE_TYPES.has(t.name),
  );

  const enums = new Map<string, EnumType>();
  const nodeTypes: GraphQLObjectType[] = [];
  const propsTypes: GraphQLObjectType[] = [];
  const rootTypes: GraphQLObjectType[] = [];
  const interfaceTypes: GraphQLInterfaceType[] = [];
  const unionTypes: GraphQLUnionType[] = [];
  let jwtShape: Map<string, string> | undefined;
  const warnings: ModelWarning[] = [];
  for (const t of userTypes) {
    if (isScalarType(t)) {
      if (!(t.name in BUILTIN_SCALARS)) {
        problems.push({
          type: t.name,
          message: "custom scalars are not supported; use a built-in scalar",
        });
      }
      continue;
    }
    if (isEnumType(t)) {
      enums.set(t.name, {
        name: t.name,
        values: t.getValues().map((val) => ({
          name: val.name,
          description: val.description ?? undefined,
        })),
        description: t.description ?? undefined,
      });
      continue;
    }
    if (isInterfaceType(t)) {
      interfaceTypes.push(t);
      continue;
    }
    if (isUnionType(t)) {
      unionTypes.push(t);
      continue;
    }
    if (!isObjectType(t)) {
      problems.push({
        type: t.name,
        message:
          "only object types, interfaces, unions, enums and scalars are supported",
      });
      continue;
    }
    if (ROOT_TYPES.has(t.name)) {
      if (t.name === "Subscription") {
        problems.push({
          type: t.name,
          message:
            "custom subscriptions are not supported; use LoraGraphQL.onWrite / changes()",
        });
      } else {
        rootTypes.push(t);
      }
      continue;
    }
    if (directive(d("jwt"), t, atType(t.name)) !== undefined) {
      if (jwtShape) {
        problems.push({
          type: t.name,
          message: "only one @jwt type is allowed",
        });
      }
      jwtShape = new Map();
      for (const f of Object.values(t.getFields())) {
        const claim = directive(d("jwtClaim"), f, atType(t.name));
        jwtShape.set(f.name, (claim?.["path"] as string | undefined) ?? f.name);
      }
      continue;
    }
    const isNode = directive(d("node"), t, atType(t.name)) !== undefined;
    const isProps =
      directive(d("relationshipProperties"), t, atType(t.name)) !== undefined;
    if (isNode && isProps) {
      problems.push({
        type: t.name,
        message: "@node and @relationshipProperties are mutually exclusive",
      });
    } else if (isNode) {
      nodeTypes.push(t);
    } else if (isProps) {
      propsTypes.push(t);
    } else {
      problems.push({
        type: t.name,
        message: "object types need @node or @relationshipProperties",
      });
    }
  }

  const nodeNames = new Set(nodeTypes.map((t) => t.name));
  // Relationships may target an interface or union over @node types.
  const targetNames = new Set([
    ...nodeNames,
    ...interfaceTypes.map((t) => t.name),
    ...unionTypes.map((t) => t.name),
  ]);
  const propsNames = new Set(propsTypes.map((t) => t.name));

  const relationshipProperties = new Map<string, RelationshipPropertiesType>();
  for (const t of propsTypes) {
    const fields = new Map<string, ScalarField>();
    for (const f of Object.values(t.getFields())) {
      const field = buildScalarField(t, f, d, problems, {
        allowKey: false,
      });
      if (field) fields.set(field.name, field);
      else if (!isScalarLike(getNamedType(f.type).name, schema)) {
        problems.push({
          type: t.name,
          field: f.name,
          message: "relationship properties must be scalars",
        });
      }
    }
    checkPropertyCollisions(t.name, [...fields.values()], problems);
    relationshipProperties.set(t.name, {
      name: t.name,
      fields,
      description: t.description ?? undefined,
    });
  }

  const nodes = new Map<string, NodeType>();
  const primaryLabels = new Map<string, string>();
  const rootFieldOwners = new Map<string, string>();
  for (const t of nodeTypes) {
    const nodeArgs = directive(d("node"), t, atType(t.name))!;
    const labels = (nodeArgs["labels"] as string[] | undefined) ?? [t.name];
    if (labels.length === 0) {
      problems.push({ type: t.name, message: "@node(labels:) is empty" });
      continue;
    }
    const primary = labels[0]!;
    const owner = primaryLabels.get(primary);
    if (owner) {
      problems.push({
        type: t.name,
        message: `primary label \`${primary}\` is already used by ${owner}`,
      });
    }
    primaryLabels.set(primary, t.name);

    const fields = new Map<string, Field>();
    for (const f of Object.values(t.getFields())) {
      const named = getNamedType(f.type);
      if (f.name.startsWith("__")) {
        problems.push({
          type: t.name,
          field: f.name,
          message: "names starting with `__` are reserved",
        });
        continue;
      }
      if (f.astNode?.directives?.some((x) => x.name.value === "cypher")) {
        const cypher = buildCypherField(t.name, f, d, problems, warnings, {
          nodeNames,
          schema,
          root: undefined,
        });
        if (cypher) fields.set(cypher.name, cypher);
        continue;
      }
      if (targetNames.has(named.name)) {
        const rel = buildRelationshipField(
          t,
          f,
          d,
          problems,
          propsNames,
          globalLimit,
        );
        if (rel) fields.set(rel.name, rel);
        continue;
      }
      if (propsNames.has(named.name)) {
        problems.push({
          type: t.name,
          field: f.name,
          message: `${named.name} is a @relationshipProperties type; reference it from @relationship(properties:)`,
        });
        continue;
      }
      if (directive(d("relationship"), f, atType(t.name)) !== undefined) {
        problems.push({
          type: t.name,
          field: f.name,
          message: "@relationship target must be a @node type",
        });
        continue;
      }
      const field = buildScalarField(t, f, d, problems, { allowKey: true });
      if (field) fields.set(field.name, field);
    }

    const scalars = [...fields.values()].filter(
      (f): f is ScalarField => f.kind === "scalar",
    );
    checkPropertyCollisions(t.name, scalars, problems);

    const keys = scalars.filter((f) => f.key);
    if (keys.length !== 1) {
      problems.push({
        type: t.name,
        message:
          keys.length === 0
            ? "a @node type needs exactly one @key field"
            : `only one @key field is allowed (found ${keys.map((k) => k.name).join(", ")})`,
      });
      continue;
    }
    const key = keys[0]!;
    if (scalars.some((f) => f.relayId) && fields.has("id")) {
      problems.push({
        type: t.name,
        field: "id",
        message:
          "@relayId adds a global `id` field; rename this field or map it with @alias",
      });
    }

    const search = readSearch(
      t.name,
      labels[0]!,
      defaultPlural(t.name),
      nodeArgs["plural"] as string | undefined,
      scalars,
      directive(d("fulltext"), t, atType(t.name)),
      problems,
    );
    const query = directive(d("query"), t, atType(t.name)) ?? {};
    const mutation = directive(d("mutation"), t, atType(t.name));
    const mutations = new Set<MutationOperation>(
      (mutation?.["operations"] as MutationOperation[] | undefined) ?? [],
    );
    const subscription = directive(d("subscription"), t, atType(t.name));
    const authentication = directive(d("authentication"), t, atType(t.name));
    const authorization = readAuthorization(
      directive(d("authorization"), t, atType(t.name)),
    );
    if (mutations.has("CREATE")) {
      for (const f of scalars) {
        if (
          f.required &&
          f.readonly &&
          !f.defaultValue &&
          !f.timestamp?.has("CREATE") &&
          !(f.key && f.generate)
        ) {
          problems.push({
            type: t.name,
            field: f.name,
            message:
              "is required but cannot be set on create; add @default, @timestamp or @key(generate: true), or drop @mutation(operations: CREATE)",
          });
        }
      }
    }
    const plural =
      (nodeArgs["plural"] as string | undefined) ?? defaultPlural(t.name);
    for (const root of [
      plural,
      `${plural}Connection`,
      `${plural}Aggregate`,
      lowerFirst(t.name),
      ...search.map((x) => x.queryName),
      `${lowerFirst(t.name)}Changed`,
    ]) {
      const other = rootFieldOwners.get(root);
      if (other) {
        problems.push({
          type: t.name,
          message: `root field \`${root}\` collides with ${other}; set @node(plural:)`,
        });
      }
      rootFieldOwners.set(root, t.name);
    }

    nodes.set(t.name, {
      name: t.name,
      labels,
      plural,
      fields,
      key,
      read: (query["read"] as boolean | undefined) ?? true,
      aggregate: (query["aggregate"] as boolean | undefined) ?? false,
      mutations,
      authentication: authentication
        ? new Set(authentication["operations"] as AuthOperation[])
        : undefined,
      authenticationJwt: authentication?.["jwt"] as
        | AuthorizationWhere
        | undefined,
      authorization,
      search,
      interfaces: t.getInterfaces().map((i) => i.name),
      subscriptions: new Set<MutationOperation>(
        (subscription?.["operations"] as MutationOperation[] | undefined) ?? [],
      ),
      limit: resolveLimit(
        directive(d("limit"), t, atType(t.name)),
        globalLimit,
        t.name,
        undefined,
        problems,
      ),
      description: t.description ?? undefined,
    });
  }

  // --- Interfaces and unions ----------------------------------------------
  const abstracts = new Map<string, AbstractType>();
  for (const t of [...interfaceTypes, ...unionTypes]) {
    const at = atType(t.name);
    const members = (
      isInterfaceType(t) ? schema.getPossibleTypes(t) : t.getTypes()
    ).map((m) => m.name);
    for (const m of members) {
      if (!nodes.has(m)) {
        at(
          `${m} is not a @node type; interfaces and unions range over @node types`,
        );
      }
    }
    if (members.length === 0) at("has no @node implementations");
    const fields = new Map<string, ScalarField>();
    const relationships = new Map<string, DeclaredRelationship>();
    if (isInterfaceType(t)) {
      for (const f of Object.values(t.getFields())) {
        if (directive(d("declareRelationship"), f, at) !== undefined) {
          const declaredRel = declareRelationship(t.name, f, members, nodes);
          if (typeof declaredRel === "string") {
            problems.push({
              type: t.name,
              field: f.name,
              message: declaredRel,
            });
          } else {
            relationships.set(f.name, declaredRel);
          }
          continue;
        }
        const field = buildScalarField(
          t as unknown as GraphQLObjectType,
          f as GraphQLField<unknown, unknown>,
          d,
          problems,
          { allowKey: false },
        );
        if (!field) continue;
        fields.set(field.name, field);
        // What the interface lets clients filter and sort by, every
        // implementation supports (and indexes).
        for (const m of members) {
          const mf = nodes.get(m)?.fields.get(f.name);
          if (mf?.kind !== "scalar") continue;
          (mf as unknown as { filters: ReadonlySet<FilterOperator> }).filters =
            new Set([...mf.filters, ...field.filters]);
          if (field.sortable && !mf.list)
            (mf as { sortable: boolean }).sortable = true;
        }
      }
    }
    const query = directive(d("query"), t, at) ?? {};
    const plural =
      (directive(d("plural"), t, at)?.["value"] as string | undefined) ??
      defaultPlural(t.name);
    const other = rootFieldOwners.get(plural);
    if (other)
      at(`root field \`${plural}\` collides with ${other}; set @plural`);
    rootFieldOwners.set(plural, t.name);
    abstracts.set(t.name, {
      kind: isInterfaceType(t) ? "interface" : "union",
      name: t.name,
      members,
      fields,
      relationships,
      plural,
      read: (query["read"] as boolean | undefined) ?? true,
      limit: resolveLimit(
        directive(d("limit"), t, at),
        globalLimit,
        t.name,
        undefined,
        problems,
      ),
      description: t.description ?? undefined,
    });
  }
  for (const node of nodes.values()) {
    for (const f of node.fields.values()) {
      if (f.kind !== "relationship") continue;
      (f as { members: readonly string[] }).members = abstracts.get(f.target)
        ?.members ?? [f.target];
      if (
        node.mutations.has("CREATE") &&
        f.required &&
        !f.list &&
        !f.nestedOperations.has("CONNECT") &&
        !f.nestedOperations.has("CREATE")
      ) {
        problems.push({
          type: node.name,
          field: f.name,
          message:
            "a required relationship needs CONNECT or CREATE in nestedOperations, or creates could never set it",
        });
      }
    }
  }

  const queries: CypherField[] = [];
  const mutationFields: CypherField[] = [];
  for (const t of rootTypes) {
    for (const f of Object.values(t.getFields())) {
      if (!f.astNode?.directives?.some((x) => x.name.value === "cypher")) {
        problems.push({
          type: t.name,
          field: f.name,
          message: `${t.name} fields are generated; a custom one needs @cypher`,
        });
        continue;
      }
      const field = buildCypherField(t.name, f, d, problems, warnings, {
        nodeNames,
        schema,
        root: t.name as "Query" | "Mutation",
      });
      if (!field) continue;
      const other = rootFieldOwners.get(field.name);
      if (t.name === "Query" && other) {
        problems.push({
          type: t.name,
          field: field.name,
          message: `collides with a generated field of ${other}`,
        });
      }
      (t.name === "Query" ? queries : mutationFields).push(field);
    }
  }

  for (const node of nodes.values()) {
    const rules = [
      ...(node.authorization?.filter ?? []),
      ...(node.authorization?.validate ?? []),
    ];
    for (const f of node.fields.values()) {
      if (!f.authorization) continue;
      if (f.authorization.filter.length > 0) {
        problems.push({
          type: node.name,
          field: f.name,
          message:
            "field-level @authorization takes validate rules; filter rules belong on the type",
        });
      }
      rules.push(...f.authorization.validate);
    }
    for (const rule of rules) {
      checkAuthorizationWhere(nodes, node, rule.where, problems, jwtShape);
    }
    for (const where of [
      node.authenticationJwt,
      ...[...node.fields.values()].map((f) => f.authenticationJwt),
    ]) {
      if (where) {
        checkAuthorizationWhere(
          nodes,
          node,
          { jwt: where },
          problems,
          jwtShape,
        );
      }
    }
  }

  if (problems.length > 0) throw new ModelError(dedupeProblems(problems));
  return {
    nodes,
    abstracts,
    enums,
    relationshipProperties,
    queries,
    mutations: mutationFields,
    warnings,
    jwt: jwtShape,
    cursorSecret: options.cursorSecret,
  };
}

/**
 * An interface field under `@declareRelationship`: every implementation
 * must declare a relationship field of that name with the same target and
 * shape (the relationship type and direction may differ). Returns the
 * problem when one does not.
 */
function declareRelationship(
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

function buildScalarField(
  t: GraphQLObjectType,
  f: GraphQLField<unknown, unknown>,
  d: (n: string) => GraphQLDirective,
  problems: ModelProblem[],
  opts: { allowKey: boolean },
): ScalarField | undefined {
  const at = (message: string) =>
    problems.push({ type: t.name, field: f.name, message });
  const shape = unwrap(f.type);
  const named = getNamedType(f.type);
  let type: ScalarType;
  let enumName: string | undefined;
  if (isEnumType(named)) {
    type = "Enum";
    enumName = named.name;
  } else if (named.name in BUILTIN_SCALARS) {
    type = BUILTIN_SCALARS[named.name]!;
  } else {
    return undefined;
  }
  if (shape.list && !shape.itemRequired) {
    at("list items must be non-null, e.g. [String!]");
  }
  if (shape.depth > 1) at("nested lists are not supported");

  const key = directive(d("key"), f, at) !== undefined;
  const unique = directive(d("unique"), f, at) !== undefined;
  const isPrivate = directive(d("private"), f, at) !== undefined;
  const relayId = directive(d("relayId"), f, at) !== undefined;
  const sortable = directive(d("sortable"), f, at) !== undefined;
  const filterable = directive(d("filterable"), f, at);
  const alias = directive(d("alias"), f, at);
  const index = directive(d("index"), f, at);
  const defaultArgs = directive(d("default"), f, at);
  const timestampArgs = directive(d("timestamp"), f, at);
  const readonlyFlag = directive(d("readonly"), f, at) !== undefined;
  const authentication = directive(d("authentication"), f, at);
  const settableArgs = directive(d("settable"), f, at);
  const selectableArgs = directive(d("selectable"), f, at);
  const populatedArgs = directive(d("populatedBy"), f, at);
  const fieldAuthorization = readAuthorization(
    directive(d("authorization"), f, at),
  );
  if (key && (settableArgs || selectableArgs || populatedArgs)) {
    at("@key cannot take @settable, @selectable or @populatedBy");
  }
  const vectorArgs = directive(d("vector"), f, at);
  let vector: ScalarField["vector"];
  if (vectorArgs) {
    const dimensions = vectorArgs["dimensions"] as number;
    if (type !== "Float" || !shape.list) at("@vector needs a [Float!] field");
    if (!opts.allowKey) at("@vector is not allowed on relationship properties");
    if (!(dimensions >= 1 && dimensions <= 4096)) {
      at("@vector(dimensions:) must be between 1 and 4096");
    }
    vector = {
      dimensions,
      similarity: vectorArgs["similarity"] as "COSINE" | "EUCLIDEAN",
    };
  }
  const vectorQuery = vectorArgs?.["queryName"] as string | undefined;
  const generate = key && (keyArgs(d, f, at)?.["generate"] as boolean) === true;
  if (generate && type !== "ID" && type !== "String") {
    at("@key(generate: true) needs an ID or String field");
  }
  const timestamp = timestampArgs
    ? new Set(timestampArgs["operations"] as Array<"CREATE" | "UPDATE">)
    : undefined;
  if (
    timestamp &&
    (shape.list ||
      (type !== "DateTime" && type !== "LocalDateTime" && type !== "Date"))
  ) {
    at("@timestamp needs a DateTime, LocalDateTime or Date field");
  }
  if (timestamp && defaultArgs) at("@timestamp and @default are exclusive");
  let defaultValue: { value: unknown } | undefined;
  if (defaultArgs) {
    const value = defaultArgs["value"];
    if (!defaultMatches(type, shape.list, value)) {
      at(`@default(value:) does not fit ${shape.list ? `[${type}]` : type}`);
    }
    if (key) at("@key cannot have a @default; use @key(generate: true)");
    if (type === "Point" || type === "CartesianPoint") {
      at("Point fields cannot have a @default");
    }
    defaultValue = { value };
  }

  if (!opts.allowKey && (key || unique || relayId || index)) {
    at("@key, @unique, @relayId and @index are not allowed here");
  }
  if (key) {
    if (!KEY_TYPES.has(type)) at("@key must be String, ID, Int or BigInt");
    if (!shape.required) at("@key fields must be non-null");
    if (shape.list) at("@key cannot be a list");
    if (isPrivate) at("@key cannot be @private");
  }
  if (relayId && !key) at("@relayId belongs on the @key field");
  if (unique && shape.list) at("@unique cannot be a list");

  let filters = new Set<FilterOperator>();
  if (filterable) {
    // Lists: membership and presence. Scalars: the type's operators, plus
    // IS_NULL when nullable and CASE_INSENSITIVE on strings.
    const allowed: FilterOperator[] = shape.list
      ? type === "Point" || type === "CartesianPoint" || vectorArgs
        ? []
        : ["INCLUDES", ...(shape.required ? [] : (["IS_NULL"] as const))]
      : [
          ...ALLOWED_OPS[type],
          ...(shape.required ? [] : (["IS_NULL"] as const)),
          ...(type === "String" || type === "ID"
            ? (["CASE_INSENSITIVE"] as const)
            : []),
        ];
    // Without byValue: equality, and membership where the type has it.
    const requested =
      (filterable["byValue"] as FilterOperator[] | undefined) ??
      (shape.list
        ? (["INCLUDES"] as FilterOperator[])
        : (["EQ", "IN"] as FilterOperator[])
      ).filter((op) => allowed.includes(op));
    const bad = requested.filter((op) => !allowed.includes(op));
    if (bad.length > 0) {
      at(
        `${type} does not support ${bad.join(", ")}` +
          (allowed.length > 0 ? ` (allowed: ${allowed.join(", ")})` : ""),
      );
    }
    filters = new Set(requested.filter((op) => allowed.includes(op)));
  }
  // A key is always addressable by value.
  if (key) {
    filters.add("EQ");
    filters.add("IN");
  }
  if (isPrivate && (filterable || sortable)) {
    at("@private fields cannot be @filterable or @sortable");
  }
  if (sortable && (shape.list || UNSORTABLE.has(type))) {
    at(`${shape.list ? "lists" : type} cannot be @sortable`);
  }

  const property = (alias?.["property"] as string | undefined) ?? f.name;
  if (property.length === 0) at("@alias(property:) is empty");

  const indexes: IndexKind[] = [];
  if (index) {
    const kind = index["kind"] as IndexKind;
    if (kind === "TEXT" && type !== "String" && type !== "ID") {
      at("a TEXT index needs a String or ID field");
    } else if (
      kind === "POINT" &&
      type !== "Point" &&
      type !== "CartesianPoint"
    ) {
      at("a POINT index needs a Point field");
    } else if (
      kind === "RANGE" &&
      (type === "Point" || type === "CartesianPoint")
    ) {
      at("use a POINT index for Point fields");
    } else if (kind === "RANGE" && RANGE_UNINDEXABLE.has(type)) {
      at(
        `LoraDB cannot RANGE-index ${type} values yet: range filters through such an index return no rows`,
      );
    } else {
      indexes.push(kind);
    }
  }

  const field: ScalarField = {
    kind: "scalar",
    name: f.name,
    property,
    type,
    enumName,
    list: shape.list,
    required: shape.required,
    key,
    unique,
    private: isPrivate,
    relayId,
    filters,
    sortable: sortable && !isPrivate,
    indexes,
    generate,
    defaultValue,
    timestamp,
    readonly:
      readonlyFlag || isPrivate || timestamp !== undefined || !!populatedArgs,
    settableOn: {
      create:
        !(readonlyFlag || isPrivate || timestamp || populatedArgs) &&
        ((settableArgs?.["onCreate"] as boolean | undefined) ?? true),
      update:
        !key &&
        !(readonlyFlag || isPrivate || timestamp || populatedArgs) &&
        ((settableArgs?.["onUpdate"] as boolean | undefined) ?? true),
    },
    selectableOn: {
      read:
        !isPrivate &&
        ((selectableArgs?.["onRead"] as boolean | undefined) ?? true),
      aggregate:
        (selectableArgs?.["onAggregate"] as boolean | undefined) ?? true,
    },
    populatedBy: populatedArgs
      ? {
          callback: populatedArgs["callback"] as string,
          operations: new Set(
            populatedArgs["operations"] as Array<"CREATE" | "UPDATE">,
          ),
        }
      : undefined,
    vector,
    authentication: authentication
      ? new Set(authentication["operations"] as AuthOperation[])
      : undefined,
    authenticationJwt: authentication?.["jwt"] as
      | AuthorizationWhere
      | undefined,
    authorization: fieldAuthorization,
    description: f.description ?? undefined,
  };
  if (populatedArgs && (timestamp || defaultArgs)) {
    at("@populatedBy cannot be combined with @timestamp or @default");
  }
  if (
    !isPrivate &&
    !field.selectableOn.read &&
    (field.filters.size > 0 || field.sortable)
  ) {
    at(
      "a field with @selectable(onRead: false) cannot be @filterable or @sortable",
    );
  }
  if (vectorQuery) vectorQueryNames.set(field, vectorQuery);
  return field;
}

function keyArgs(
  d: (n: string) => GraphQLDirective,
  f: GraphQLField<unknown, unknown>,
  at: (message: string) => void,
): Record<string, unknown> | undefined {
  return directive(d("key"), f, at);
}

function defaultMatches(
  type: ScalarType,
  list: boolean,
  value: unknown,
): boolean {
  if (list) {
    return (
      Array.isArray(value) && value.every((v) => defaultMatches(type, false, v))
    );
  }
  switch (type) {
    case "Int":
    case "BigInt":
      return Number.isInteger(value);
    case "Float":
      return typeof value === "number";
    case "Boolean":
      return typeof value === "boolean";
    case "Point":
    case "CartesianPoint":
      return false;
    default:
      return typeof value === "string";
  }
}

function buildRelationshipField(
  t: GraphQLObjectType,
  f: GraphQLField<unknown, unknown>,
  d: (n: string) => GraphQLDirective,
  problems: ModelProblem[],
  propsNames: Set<string>,
  globalLimit: PageLimit,
): RelationshipField | undefined {
  const at = (message: string) =>
    problems.push({ type: t.name, field: f.name, message });
  if (directive(d("authorization"), f, at) !== undefined) {
    at(
      "field-level @authorization is supported on scalar fields only; put the rule on the related type",
    );
  }
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
  for (const forbidden of ["key", "unique", "sortable", "alias", "index"]) {
    if (directive(d(forbidden), f, at)) {
      at(`@${forbidden} is not allowed on a relationship field`);
    }
  }
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

function dedupeProblems(problems: ModelProblem[]): ModelProblem[] {
  const seen = new Set<string>();
  return problems.filter((p) => {
    const k = `${p.type}\0${p.field}\0${p.message}`;
    if (seen.has(k)) return false;
    seen.add(k);
    return true;
  });
}

function checkPropertyCollisions(
  typeName: string,
  fields: ScalarField[],
  problems: ModelProblem[],
) {
  const seen = new Map<string, string>();
  for (const f of fields) {
    const other = seen.get(f.property);
    if (other) {
      problems.push({
        type: typeName,
        field: f.name,
        message: `stores property \`${f.property}\`, already stored by ${other}`,
      });
    }
    seen.set(f.property, f.name);
  }
}

function resolveGlobalLimit(options: ModelOptions): PageLimit {
  const max = options.maxLimit ?? MAX_LIMIT;
  const def = Math.min(options.defaultLimit ?? DEFAULT_LIMIT, max);
  if (!(max >= 1) || !(def >= 1)) {
    throw new ModelError([
      { message: "defaultLimit and maxLimit must be at least 1" },
    ]);
  }
  return { default: def, max };
}

function resolveLimit(
  args: Record<string, unknown> | undefined,
  global: PageLimit,
  type: string,
  field: string | undefined,
  problems: ModelProblem[],
): PageLimit {
  if (!args) return global;
  const max = (args["max"] as number | undefined) ?? global.max;
  const def =
    (args["default"] as number | undefined) ?? Math.min(global.default, max);
  const at = (message: string) =>
    problems.push(field ? { type, field, message } : { type, message });
  if (max < 1 || def < 1) at("@limit values must be at least 1");
  if (max > global.max)
    at(`@limit(max: ${max}) exceeds the global maximum ${global.max}`);
  if (def > max) at(`@limit(default: ${def}) exceeds max ${max}`);
  return {
    default: Math.min(def, max, global.max),
    max: Math.min(max, global.max),
  };
}

function directive(
  def: GraphQLDirective,
  node: { astNode?: unknown },
  at: (message: string) => void,
): Record<string, unknown> | undefined {
  const ast = node.astNode as Parameters<typeof getDirectiveValues>[1] | null;
  if (!ast) return undefined;
  try {
    return getDirectiveValues(def, ast);
  } catch (err) {
    at(`@${def.name}: ${err instanceof Error ? err.message : String(err)}`);
    return undefined;
  }
}

function unwrap(type: GraphQLOutputType): {
  required: boolean;
  list: boolean;
  itemRequired: boolean;
  depth: number;
} {
  let required = false;
  let t: GraphQLOutputType = type;
  if (isNonNullType(t)) {
    required = true;
    t = t.ofType;
  }
  let depth = 0;
  let itemRequired = false;
  while (isListType(t)) {
    depth++;
    t = t.ofType as GraphQLOutputType;
    if (isNonNullType(t)) {
      itemRequired = true;
      t = t.ofType;
    } else {
      itemRequired = false;
    }
  }
  return { required, list: depth > 0, itemRequired, depth };
}

function isScalarLike(name: string, schema: GraphQLSchema): boolean {
  const t = schema.getType(name);
  return name in BUILTIN_SCALARS || (t !== undefined && isEnumType(t));
}

export function lowerFirst(s: string): string {
  return s.charAt(0).toLowerCase() + s.slice(1);
}

export function defaultPlural(typeName: string): string {
  const base = lowerFirst(typeName);
  if (/[^aeiou]y$/i.test(base)) return base.slice(0, -1) + "ies";
  if (/(s|x|z|ch|sh)$/i.test(base)) return base + "es";
  return base + "s";
}

// ---------------------------------------------------------------------------
// @cypher
// ---------------------------------------------------------------------------

const WRITE_CLAUSES = /\b(CREATE|MERGE|SET|DELETE|REMOVE|DETACH)\b/;

function buildCypherField(
  owner: string,
  f: GraphQLField<unknown, unknown>,
  d: (n: string) => GraphQLDirective,
  problems: ModelProblem[],
  warnings: ModelWarning[],
  ctx: {
    nodeNames: Set<string>;
    schema: GraphQLSchema;
    root: "Query" | "Mutation" | undefined;
  },
): CypherField | undefined {
  const at = (message: string) =>
    problems.push({ type: owner, field: f.name, message });
  const warn = (message: string) =>
    warnings.push({ type: owner, field: f.name, message });
  const args = directive(d("cypher"), f, at);
  if (!args) return undefined;
  if (directive(d("authorization"), f, at) !== undefined) {
    at(
      "field-level @authorization is supported on scalar fields only; guard a @cypher field with @authentication or inside its statement",
    );
  }
  const statement = args["statement"] as string;
  for (const other of [
    "relationship",
    "key",
    "unique",
    "alias",
    "index",
    "default",
    "timestamp",
  ]) {
    if (directive(d(other), f, at))
      at(`@${other} cannot be combined with @cypher`);
  }

  const shape = unwrap(f.type);
  const named = getNamedType(f.type).name;
  const node = ctx.nodeNames.has(named) ? named : undefined;
  if (!node && !isScalarLike(named, ctx.schema)) {
    at("@cypher fields return scalars, enums or @node types");
    return undefined;
  }
  if (shape.depth > 1) at("nested lists are not supported");
  if (shape.list && !shape.itemRequired) {
    at("list items must be non-null, e.g. [Festival!]");
  }

  const columnName =
    (args["columnName"] as string | undefined) ?? inferColumn(statement);
  if (!columnName) {
    at("cannot tell which column holds the value; set @cypher(columnName:)");
    return undefined;
  }

  const cypherArgs: CypherArgument[] = [];
  for (const a of f.args) {
    const argNamed = getNamedType(a.type).name;
    if (!isScalarLike(argNamed, ctx.schema)) {
      at(`argument ${a.name}: @cypher arguments must be scalars or enums`);
      continue;
    }
    if (a.name === "jwt")
      at("argument `jwt` is reserved for the request's claims");
    const argShape = unwrapInput(a.type);
    cypherArgs.push({
      name: a.name,
      type: { named: argNamed, ...argShape },
      defaultValue: a.defaultValue,
      description: a.description ?? undefined,
    });
  }

  const params = [...new Set(scanParams(statement).map((p) => p.name))];
  const known = new Set([...cypherArgs.map((a) => a.name), "jwt"]);
  for (const p of params) {
    if (!known.has(p)) {
      at(`the statement uses $${p}, which is neither an argument nor $jwt`);
    }
  }
  for (const a of cypherArgs) {
    if (!params.includes(a.name))
      warn(`argument ${a.name} is never used by the statement`);
  }

  const code = codeOnly(statement);
  if (ctx.root !== "Mutation" && WRITE_CLAUSES.test(code)) {
    at(
      ctx.root === "Query"
        ? "a Query field must not write; declare it on Mutation"
        : "an object field must not write",
    );
  }
  if (ctx.root === undefined && !/\bTHIS\b/.test(code)) {
    warn(
      "the statement never uses `this`, so it returns the same value for every parent",
    );
  }
  if (code.includes("OPTIONAL MATCH")) {
    warn(
      "OPTIONAL MATCH is slow on LoraDB; a pattern comprehension is usually 17-130x faster",
    );
  }

  // Opt-in filters and sorts: computed per row, so never through an index.
  let computed: ScalarField | undefined;
  if (
    directive(d("filterable"), f, at) !== undefined ||
    directive(d("sortable"), f, at) !== undefined
  ) {
    if (ctx.root !== undefined) {
      at("@filterable and @sortable on @cypher apply to @node type fields");
    } else if (node || shape.list) {
      at(
        "only scalar, non-list @cypher fields can be @filterable or @sortable",
      );
    } else if (cypherArgs.some((a) => a.defaultValue === undefined)) {
      at(
        "a @filterable or @sortable @cypher field needs a default for every argument",
      );
    } else {
      computed = buildScalarField(
        { name: owner } as GraphQLObjectType,
        f,
        d,
        problems,
        { allowKey: false },
      );
      warn(
        `filtering or sorting by ${f.name} runs its statement for every ${owner} considered: no index applies`,
      );
    }
  }

  const field: CypherField = {
    kind: "cypher",
    name: f.name,
    owner,
    statement,
    columnName,
    type: { named, ...shapeOf(shape) },
    node,
    args: cypherArgs,
    params,
    computed,
    authentication: authOps(directive(d("authentication"), f, at)),
    authenticationJwt: directive(d("authentication"), f, at)?.["jwt"] as
      | AuthorizationWhere
      | undefined,
    description: f.description ?? undefined,
  };
  if (computed) computed.computedBy = field;
  return field;
}

/** The column of a statement ending in `RETURN x` or `RETURN … AS x`. */
function inferColumn(statement: string): string | undefined {
  const matches = [...statement.matchAll(/\bRETURN\b/gi)];
  const last = matches[matches.length - 1];
  if (!last) return undefined;
  const tail = statement
    .slice(last.index! + last[0].length)
    .replace(/\s+(ORDER\s+BY|SKIP|LIMIT)\b[\s\S]*$/i, "")
    .replace(/^\s*DISTINCT\b/i, "")
    .trim();
  if (tail.includes(",")) return undefined;
  const alias = /\bAS\s+`?([A-Za-z_][A-Za-z0-9_]*)`?\s*$/i.exec(tail);
  if (alias) return alias[1];
  return /^[A-Za-z_][A-Za-z0-9_]*$/.test(tail) ? tail : undefined;
}

function shapeOf(shape: ReturnType<typeof unwrap>): Omit<TypeShape, "named"> {
  return {
    list: shape.list,
    required: shape.required,
    itemRequired: shape.itemRequired,
  };
}

function unwrapInput(type: GraphQLInputType): Omit<TypeShape, "named"> {
  let required = false;
  let t: GraphQLInputType = type;
  if (isNonNullType(t)) {
    required = true;
    t = t.ofType;
  }
  let list = false;
  let itemRequired = false;
  if (isListType(t)) {
    list = true;
    itemRequired = isNonNullType(t.ofType);
  }
  return { list, required, itemRequired };
}

// ---------------------------------------------------------------------------
// @authentication / @authorization
// ---------------------------------------------------------------------------

function authOps(
  args: Record<string, unknown> | undefined,
): Set<AuthOperation> | undefined {
  return args ? new Set(args["operations"] as AuthOperation[]) : undefined;
}

function readAuthorization(
  args: Record<string, unknown> | undefined,
): Authorization | undefined {
  if (!args) return undefined;
  type Raw = {
    operations: AuthOperation[];
    requireAuthentication?: boolean;
    when?: Array<"BEFORE" | "AFTER">;
    where: AuthorizationWhere;
  };
  const filter = ((args["filter"] as Raw[] | undefined) ?? []).map((r) => ({
    operations: new Set(r.operations),
    requireAuthentication: r.requireAuthentication ?? true,
    where: r.where,
  }));
  const validate = ((args["validate"] as Raw[] | undefined) ?? []).map((r) => ({
    operations: new Set(r.operations),
    when: new Set<"BEFORE" | "AFTER">(r.when ?? ["BEFORE", "AFTER"]),
    requireAuthentication: r.requireAuthentication ?? true,
    where: r.where,
  }));
  return { filter, validate };
}

const SCALAR_WHERE_OPS = new Set([
  "withinBBox",
  "distance",
  "eq",
  "in",
  "lt",
  "lte",
  "gt",
  "gte",
  "contains",
  "startsWith",
  "endsWith",
]);
const JWT_OPS = new Set([...SCALAR_WHERE_OPS, "includes", "exists"]);
const COUNT_WHERE_OPS = new Set(["eq", "lt", "lte", "gt", "gte"]);

const isRecord = (x: unknown): x is Record<string, unknown> =>
  typeof x === "object" && x !== null && !Array.isArray(x);

/** Check an `@authorization` where against the model, at startup. */
function checkAuthorizationWhere(
  nodes: ReadonlyMap<string, NodeType>,
  node: NodeType,
  where: unknown,
  problems: ModelProblem[],
  jwtShape?: ReadonlyMap<string, string>,
) {
  const at = (message: string) =>
    problems.push({ type: node.name, message: `@authorization: ${message}` });
  const visit = (w: unknown, path: string) => {
    if (!isRecord(w)) return at(`${path || "where"} must be an object`);
    for (const [k, value] of Object.entries(w)) {
      const here = path ? `${path}.${k}` : k;
      if (k === "AND" || k === "OR") {
        if (!Array.isArray(value)) at(`${here} must be a list`);
        else value.forEach((x, i) => visit(x, `${here}[${i}]`));
      } else if (k === "NOT") {
        visit(value, here);
      } else if (k === "node") {
        checkNodeWhere(nodes, node, value, here, at);
        if (jwtShape) {
          for (const ref of claimRefs(value)) {
            if (!jwtShape.has(ref))
              at(`${here}: $jwt.${ref} is not a claim of the @jwt type`);
          }
        }
      } else if (k === "jwt") {
        if (!isRecord(value)) at(`${here} must be an object`);
        else {
          for (const [claim, ops] of Object.entries(value)) {
            if (jwtShape && !jwtShape.has(claim)) {
              at(`${here}.${claim}: not a claim of the @jwt type`);
            }
            if (!isRecord(ops))
              at(`${here}.${claim} must be an operator object`);
            else {
              for (const op of Object.keys(ops)) {
                if (!JWT_OPS.has(op))
                  at(`${here}.${claim}: unknown operator ${op}`);
              }
            }
          }
        }
      } else {
        at(`${here}: expected node, jwt, AND, OR or NOT`);
      }
    }
  };
  visit(where, "");
}

function checkNodeWhere(
  nodes: ReadonlyMap<string, NodeType>,
  node: NodeType,
  where: unknown,
  path: string,
  at: (message: string) => void,
) {
  if (!isRecord(where)) return at(`${path} must be an object`);
  // An empty or null test would compile to nothing and grant everyone.
  if (Object.keys(where).length === 0) {
    return at(`${path} is empty: a rule must test something`);
  }
  for (const [k, value] of Object.entries(where)) {
    const here = `${path}.${k}`;
    if (value === null) {
      at(`${here} is null: a rule must test something`);
      continue;
    }
    if (k === "AND" || k === "OR") {
      if (!Array.isArray(value)) at(`${here} must be a list`);
      else
        value.forEach((x, i) =>
          checkNodeWhere(nodes, node, x, `${here}[${i}]`, at),
        );
      continue;
    }
    if (k === "NOT") {
      checkNodeWhere(nodes, node, value, here, at);
      continue;
    }
    const field = node.fields.get(k);
    if (!field) {
      at(`${here}: ${node.name} has no field ${k}`);
    } else if (field.kind === "cypher") {
      at(`${here}: @cypher fields cannot be used in rules`);
    } else if (field.kind === "scalar") {
      if (!isRecord(value) || Object.keys(value).length === 0) {
        at(`${here} must be a non-empty operator object`);
      } else {
        for (const [op, operand] of Object.entries(value)) {
          if (!SCALAR_WHERE_OPS.has(op)) at(`${here}: unknown operator ${op}`);
          else if (operand === null) at(`${here}.${op} is null`);
        }
      }
    } else {
      const target = nodes.get(field.target);
      if (!target) continue;
      if (!field.list) {
        checkNodeWhere(nodes, target, value, here, at);
        continue;
      }
      if (!isRecord(value)) {
        at(`${here} must be an object`);
        continue;
      }
      for (const [q, inner] of Object.entries(value)) {
        if (q === "count") {
          if (!isRecord(inner)) at(`${here}.count must be an operator object`);
          else {
            for (const op of Object.keys(inner)) {
              if (!COUNT_WHERE_OPS.has(op))
                at(`${here}.count: unknown operator ${op}`);
            }
          }
        } else if (["some", "all", "none", "single"].includes(q)) {
          checkNodeWhere(nodes, target, inner, `${here}.${q}`, at);
        } else {
          at(`${here}: expected some, all, none, single or count`);
        }
      }
    }
  }
}

// ---------------------------------------------------------------------------
// @fulltext / @vector
// ---------------------------------------------------------------------------

const vectorQueryNames = new WeakMap<ScalarField, string>();

function readSearch(
  typeName: string,
  label: string,
  defaultName: string,
  plural: string | undefined,
  scalars: ScalarField[],
  fulltext: Record<string, unknown> | undefined,
  problems: ModelProblem[],
): SearchIndex[] {
  const at = (message: string) => problems.push({ type: typeName, message });
  const Plural = upperFirst(plural ?? defaultName);
  const out: SearchIndex[] = [];
  type Raw = {
    name?: string;
    fields: string[];
    analyzer: "STANDARD" | "SIMPLE";
    queryName?: string;
  };
  const raws = (fulltext?.["indexes"] as Raw[] | undefined) ?? [];
  raws.forEach((raw, i) => {
    if (i > 0 && !raw.name) {
      at("@fulltext: every index after the first needs a name");
    }
    const name = raw.name ?? `${snakeCase(label)}_search`;
    const fields: ScalarField[] = [];
    for (const f of raw.fields) {
      const field = scalars.find((x) => x.name === f);
      if (!field) at(`@fulltext: ${typeName} has no field ${f}`);
      else if ((field.type !== "String" && field.type !== "ID") || field.list) {
        at(`@fulltext: ${f} is not a String field`);
      } else fields.push(field);
    }
    if (raw.fields.length === 0) at("@fulltext: an index needs fields");
    out.push({
      kind: "fulltext",
      name,
      fields,
      analyzer: raw.analyzer,
      queryName:
        raw.queryName ??
        (i === 0 ? `search${Plural}` : `search${Plural}By${upperFirst(name)}`),
    });
  });
  const vectors = scalars.filter((f) => f.vector);
  for (const f of vectors) {
    out.push({
      kind: "vector",
      name: `${snakeCase(label)}_${snakeCase(f.property)}_vector`,
      field: f,
      dimensions: f.vector!.dimensions,
      similarity: f.vector!.similarity,
      queryName:
        vectorQueryNames.get(f) ??
        (vectors.length === 1
          ? `similar${Plural}`
          : `similar${Plural}By${upperFirst(f.name)}`),
    });
  }
  const names = new Set<string>();
  for (const x of out) {
    if (names.has(x.name)) at(`search index name ${x.name} is used twice`);
    names.add(x.name);
  }
  return out;
}

function upperFirst(s: string): string {
  return s.charAt(0).toUpperCase() + s.slice(1);
}

function snakeCase(s: string): string {
  return s
    .replace(/([a-z0-9])([A-Z])/g, "$1_$2")
    .replace(/[^A-Za-z0-9]+/g, "_")
    .toLowerCase();
}

/** Claim names referenced as "$jwt.name…" strings anywhere in a value. */
function claimRefs(value: unknown): string[] {
  if (typeof value === "string") {
    return value.startsWith("$jwt.") ? [value.slice(5).split(".")[0]!] : [];
  }
  if (Array.isArray(value)) return value.flatMap(claimRefs);
  if (value !== null && typeof value === "object") {
    return Object.values(value).flatMap(claimRefs);
  }
  return [];
}
