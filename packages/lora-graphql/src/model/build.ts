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
  Kind,
  parse,
  valueFromAST,
  type ConstValueNode,
  type DocumentNode,
  type GraphQLArgument,
  type GraphQLNamedType,
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
import { checkDirectivePositions } from "./positions.js";
import {
  UNEXPANDED,
  checkViewer,
  desugarRule,
  testsRelationshipEnds,
  type DesugarContext,
  type NamedRules,
  type ViewerMapping,
} from "./desugar.js";
import { codeOnly, maskLiterals, scanParams } from "./cypher-lexer.js";
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
  PlainObjectType,
  PageLimit,
  RelationshipField,
  RelationshipPropertiesType,
  ScalarField,
  ScalarType,
  UniqueTogether,
} from "./types.js";
import {
  PLACEHOLDER,
  RELATIONSHIP_OPERATIONS,
  RULE_REFERENCES,
} from "./types.js";
import { isUpdatable, outOfRange } from "./inputs.js";

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

  // `extend schema @authorizationDefaults(...)`.
  const defaults: {
    bypass?: AuthorizationWhere;
    mutations?: AuthorizationWhere;
  } = {};
  for (const node of [schema.astNode, ...schema.extensionASTNodes]) {
    if (!node) continue;
    const args = directive(
      d("authorizationDefaults"),
      { astNode: node },
      (message) => problems.push({ type: "schema", message }),
    );
    if (!args) continue;
    if (args["bypass"] != null)
      defaults.bypass = args["bypass"] as AuthorizationWhere;
    if (args["mutations"] != null)
      defaults.mutations = args["mutations"] as AuthorizationWhere;
  }
  // `extend schema @authorizationRules(rules: [...])`: claims-only rules.
  const schemaRules = new Map<string, AuthorizationWhere>();
  for (const node of [schema.astNode, ...schema.extensionASTNodes]) {
    for (const dir of node?.directives ?? []) {
      if (dir.name.value !== "authorizationRules") continue;
      const args = directive(
        d("authorizationRules"),
        { astNode: { directives: [dir] } },
        (message) => problems.push({ type: "schema", message }),
      );
      for (const r of (args?.["rules"] as
        | Array<{ name: string; where: AuthorizationWhere }>
        | undefined) ?? []) {
        if (schemaRules.has(r.name)) {
          problems.push({
            type: "schema",
            message: `@authorizationRules: rule "${r.name}" is defined twice`,
          });
        }
        schemaRules.set(r.name, r.where);
      }
    }
  }

  const enums = new Map<string, EnumType>();
  const scalars = new Map<string, ScalarType>();
  const scalarDescriptions = new Map<string, string>();
  const nodeTypes: GraphQLObjectType[] = [];
  const propsTypes: GraphQLObjectType[] = [];
  const plainTypes: GraphQLObjectType[] = [];
  const rootTypes: GraphQLObjectType[] = [];
  const interfaceTypes: GraphQLInterfaceType[] = [];
  const unionTypes: GraphQLUnionType[] = [];
  let jwtShape: Map<string, string> | undefined;
  let jwtType: string | undefined;
  let viewer: ViewerMapping | undefined;
  const warnings: ModelWarning[] = [];
  for (const t of userTypes) {
    if (isScalarType(t)) {
      const stored = storageOf(t);
      if (stored && !(t.name in BUILTIN_SCALARS)) {
        scalars.set(t.name, stored);
        if (t.description) scalarDescriptions.set(t.name, t.description);
      }
      if (!(t.name in BUILTIN_SCALARS) && !stored) {
        problems.push({
          type: t.name,
          message:
            "a custom scalar needs @storedAs(type:) to say how its values are stored",
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
      jwtType = t.name;
      for (const f of Object.values(t.getFields())) {
        const claim = directive(d("jwtClaim"), f, atType(t.name));
        jwtShape.set(f.name, (claim?.["path"] as string | undefined) ?? f.name);
        const viewerArgs = directive(d("viewer"), f, atType(t.name));
        if (viewerArgs) {
          if (viewer) {
            problems.push({
              type: t.name,
              field: f.name,
              message: `only one @viewer claim is allowed (${viewer.claim} has one)`,
            });
          }
          viewer = {
            claim: f.name,
            type: viewerArgs["type"] as string,
            field: viewerArgs["field"] as string,
          };
        }
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
      // A plain object type: valid when a @cypher field returns it.
      plainTypes.push(t);
    }
  }

  const nodeNames = new Set(nodeTypes.map((t) => t.name));
  const plainNames = new Set(plainTypes.map((t) => t.name));
  const objects = new Map<string, PlainObjectType>();
  for (const t of plainTypes) {
    const fields = new Map<string, { name: string; type: TypeShape }>();
    for (const f of Object.values(t.getFields())) {
      const named = getNamedType(f.type).name;
      if (!isScalarLike(named, schema)) {
        problems.push({
          type: t.name,
          field: f.name,
          message:
            "fields of an object type without @node are scalars or enums",
        });
        continue;
      }
      fields.set(f.name, {
        name: f.name,
        type: { named, ...shapeOf(unwrap(f.type)) },
      });
    }
    objects.set(t.name, {
      name: t.name,
      fields,
      description: t.description ?? undefined,
    });
  }
  // Relationships may target an interface or union over @node types.
  const targetNames = new Set([
    ...nodeNames,
    ...interfaceTypes.map((t) => t.name),
    ...unionTypes.map((t) => t.name),
  ]);
  const propsNames = new Set(propsTypes.map((t) => t.name));
  checkDirectivePositions(
    { schema, nodeNames, propsNames, jwtType, targetNames, userTypes },
    problems,
  );

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

  const at0 = (type: string, field: string) => (message: string) =>
    problems.push({ type, field, message });
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
      const custom = directive(d("customResolver"), f, (message) =>
        problems.push({ type: t.name, field: f.name, message }),
      );
      if (custom !== undefined) {
        for (const other of [
          "cypher",
          "relationship",
          "key",
          "filterable",
          "sortable",
          "groupBy",
        ]) {
          if (f.astNode?.directives?.some((x) => x.name.value === other)) {
            problems.push({
              type: t.name,
              field: f.name,
              message: `@${other} cannot be combined with @customResolver`,
            });
          }
        }
        fields.set(f.name, {
          kind: "custom",
          authentication: authOps(
            directive(d("authentication"), f, at0(t.name, f.name)),
          ),
          name: f.name,
          owner: t.name,
          requires: custom["requires"] as string | undefined,
          type: { named: named.name, ...shapeOf(unwrap(f.type)) },
          description: f.description ?? undefined,
        });
        continue;
      }
      if (f.astNode?.directives?.some((x) => x.name.value === "cypher")) {
        const cypher = buildCypherField(t.name, f, d, problems, warnings, {
          nodeNames,
          plainNames,
          schema,
          root: undefined,
          viewer: viewer !== undefined,
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
    // A single relationship's `<field>Exists` filter takes that name.
    for (const f of fields.values()) {
      if (
        f.kind === "relationship" &&
        !f.list &&
        fields.has(`${f.name}Exists`)
      ) {
        problems.push({
          type: t.name,
          field: `${f.name}Exists`,
          message: `collides with the ${f.name}Exists filter of the single relationship ${f.name}; rename one`,
        });
      }
    }

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
          !f.populatedBy?.operations.has("CREATE") &&
          !(f.key && f.generate)
        ) {
          problems.push({
            type: t.name,
            field: f.name,
            message:
              "is required but cannot be set on create; add @default, @timestamp, @populatedBy(operations: [CREATE]) or @key(generate: true), or drop @mutation(operations: CREATE)",
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
      uniqueTogether: [],
      subscriptions: new Set<MutationOperation>(
        (subscription?.["operations"] as MutationOperation[] | undefined) ?? [],
      ),
      subscriptionOptions: {
        relationships: subscription?.["relationships"] === true,
        previousState: subscription?.["previousState"] === true,
      },
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
      if (f.kind === "scalar" && f.groupBy && !node.aggregate) {
        problems.push({
          type: node.name,
          field: f.name,
          message: "@groupBy needs @query(aggregate: true) on the type",
        });
      }
      if (f.kind !== "relationship") continue;
      (f as { members: readonly string[] }).members = abstracts.get(f.target)
        ?.members ?? [f.target];
      if (
        node.mutations.has("CREATE") &&
        f.required &&
        !f.list &&
        !f.settableOn.create
      ) {
        problems.push({
          type: node.name,
          field: f.name,
          message:
            "a required relationship must be settable on create, or creates could never set it",
        });
      } else if (
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
      if (f.nestedOperations.has("UPDATE_EDGE") && !f.properties) {
        problems.push({
          type: node.name,
          field: f.name,
          message:
            "UPDATE_EDGE updates relationship properties, and this relationship has none",
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
        plainNames,
        schema,
        root: t.name as "Query" | "Mutation",
        viewer: viewer !== undefined,
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

  // `@authorizationRule(name:, where:)` on node types (repeatable).
  const typeRules = new Map<string, Map<string, AuthorizationWhere>>();
  for (const t of nodeTypes) {
    const own = new Map<string, AuthorizationWhere>();
    for (const dir of t.astNode?.directives ?? []) {
      if (dir.name.value !== "authorizationRule") continue;
      const args = directive(
        d("authorizationRule"),
        { astNode: { directives: [dir] } },
        atType(t.name),
      );
      if (!args) continue;
      const name = args["name"] as string;
      if (own.has(name)) {
        atType(t.name)(`@authorizationRule: rule "${name}" is defined twice`);
      }
      if (schemaRules.has(name)) {
        atType(t.name)(
          `@authorizationRule: rule "${name}" shadows the schema rule of that name; rename one`,
        );
      }
      own.set(name, args["where"] as AuthorizationWhere);
    }
    if (own.size > 0) typeRules.set(t.name, own);
  }
  const namedRules: NamedRules = { schema: schemaRules, byType: typeRules };
  const schemaDesugar = (where: AuthorizationWhere) =>
    desugarRule(
      {
        nodes,
        props: relationshipProperties,
        viewer,
        abstracts,
        rules: namedRules,
        at: (message) =>
          problems.push({
            type: "schema",
            message: `@authorization: ${message}`,
          }),
      },
      undefined,
      where,
    );
  // Schema rules test claims only, so they stay compile-time decisions.
  for (const [name, where] of schemaRules) {
    const expanded = schemaDesugar(where);
    const violations = claimsOnlyViolations(expanded);
    for (const key of violations) {
      problems.push({
        type: "schema",
        message: `@authorizationRules: rule "${name}" tests claims only (jwt, AND, OR, NOT); ${key} is not allowed (a node rule belongs on its type, with @authorizationRule)`,
      });
    }
    if (violations.length === 0) {
      checkRuleWhere(
        nodes,
        relationshipProperties,
        undefined,
        "schema",
        undefined,
        expanded,
        problems,
        jwtShape,
      );
    }
  }

  // @authorizationDefaults: a claims-only bypass, and the write rule of
  // every @mutation type without one of its own.
  if (defaults.bypass) {
    defaults.bypass = schemaDesugar(defaults.bypass);
    const violations = claimsOnlyViolations(defaults.bypass);
    for (const key of violations) {
      problems.push({
        type: "schema",
        message: `@authorizationDefaults(bypass:) tests claims only (jwt, AND, OR, NOT), so it stays a compile-time decision; ${key} is not allowed`,
      });
    }
    if (violations.length === 0) {
      checkRuleWhere(
        nodes,
        relationshipProperties,
        undefined,
        "schema",
        undefined,
        defaults.bypass,
        problems,
        jwtShape,
      );
    }
  }
  if (defaults.mutations) {
    const WRITES: AuthOperation[] = ["CREATE", "UPDATE", "DELETE"];
    for (const node of nodes.values()) {
      if (node.mutations.size === 0) continue;
      const own = [
        ...(node.authorization?.filter ?? []),
        ...(node.authorization?.validate ?? []),
      ].some((r) => WRITES.some((op) => r.operations.has(op)));
      if (own) continue;
      (node as { authorization: Authorization }).authorization = {
        ...(node.authorization ?? { filter: [] }),
        validate: [
          ...(node.authorization?.validate ?? []),
          {
            operations: new Set<AuthOperation>(WRITES),
            when: new Set<"BEFORE" | "AFTER">(["BEFORE", "AFTER"]),
            requireAuthentication: true,
            where: defaults.mutations,
          },
        ],
      };
    }
  }
  for (const node of nodes.values()) {
    if (node.authorization?.mask) {
      problems.push({
        type: node.name,
        message: "@authorization(mask:) belongs on a scalar field",
      });
    }
    for (const f of node.fields.values()) {
      const masks = f.authorization?.mask ?? [];
      const at = (message: string) =>
        problems.push({ type: node.name, field: f.name, message });
      if (masks.length > 0 && (f.kind !== "scalar" || f.key)) {
        at(
          "@authorization(mask:) belongs on a scalar field that is not the @key",
        );
      } else if (f.kind === "scalar") {
        for (const m of masks) {
          const missing = (m as { missingValue?: boolean }).missingValue;
          if (m.value === null && f.required) {
            at(
              missing
                ? "@authorization(mask:) on a non-null field needs a value"
                : "@authorization(mask:) value null does not fit a non-null field",
            );
          } else if (
            m.value !== null &&
            (!defaultMatches(f.type, f.list, m.value) ||
              (f.enumName !== undefined &&
                !(enums.get(f.enumName)?.values ?? []).some(
                  (v) => v.name === m.value,
                )))
          ) {
            at(
              `@authorization(mask:) value ${JSON.stringify(m.value)} does not fit ${f.list ? `[${f.enumName ?? f.type}]` : (f.enumName ?? f.type)}`,
            );
          }
        }
      }
      if (f.authorization?.bypass !== undefined || f.authorization?.public) {
        problems.push({
          type: node.name,
          field: f.name,
          message: "@authorization(bypass:, public:) belong on the type",
        });
      }
    }
  }

  abstractsOf.set(nodes, abstracts);
  for (const t of nodeTypes) {
    const node = nodes.get(t.name);
    if (!node) continue;
    (node as { uniqueTogether: readonly UniqueTogether[] }).uniqueTogether =
      readUniqueTogether(t, node, nodes, relationshipProperties, d, problems);
  }
  const ruleEnds = (f: RelationshipField): RuleEnds | undefined => {
    const source = nodes.get(f.owner);
    const target = nodes.get(f.target);
    if (!source || !target) return undefined;
    return {
      source,
      target,
      edge: f.properties ? relationshipProperties.get(f.properties) : undefined,
    };
  };
  // Rule sugar (isViewer, viewer) expands into the plain rule AST before
  // the rules are checked, so the checks and the compiler see one form.
  if (viewer && jwtType) checkViewer(viewer, nodes, jwtType, problems);
  for (const node of nodes.values()) {
    if (node.key.keyScope && !viewer) {
      problems.push({
        type: node.name,
        field: node.key.name,
        message:
          "@key(scope: VIEWER) needs a @viewer claim on the @jwt type: it names whose key space a create writes in",
      });
    }
  }
  const viewerNode = viewer ? nodes.get(viewer.type) : undefined;
  const desugar = (
    owner: NodeType | undefined,
    type: string,
    field: string | undefined,
    rules: ReadonlyArray<{ where: AuthorizationWhere }>,
    ends?: { source: NodeType; target: NodeType },
  ) => {
    const ctx: DesugarContext = {
      nodes,
      props: relationshipProperties,
      viewer,
      abstracts,
      rules: namedRules,
      at: (message) =>
        problems.push({
          type,
          ...(field ? { field } : {}),
          message: `@authorization: ${message}`,
        }),
    };
    for (const rule of rules) {
      (rule as { where: AuthorizationWhere }).where = desugarRule(
        ctx,
        owner,
        rule.where,
        ends,
      );
    }
  };
  for (const node of nodes.values()) {
    desugar(node, node.name, undefined, [
      ...(node.authorization?.filter ?? []),
      ...(node.authorization?.validate ?? []),
    ]);
    for (const f of node.fields.values()) {
      if (!f.authorization) continue;
      for (const m of f.authorization.mask ?? []) {
        const rule = { where: m.unless };
        desugar(node, node.name, f.name, [rule]);
        (m as { unless: AuthorizationWhere }).unless = rule.where;
      }
      const rel = f.authorization.validate.filter(isRelationshipRule);
      desugar(node, node.name, f.name, [
        ...f.authorization.filter,
        ...f.authorization.validate.filter((r) => !isRelationshipRule(r)),
      ]);
      const ends = f.kind === "relationship" ? ruleEnds(f) : undefined;
      if (ends) desugar(undefined, node.name, f.name, rel, ends);
    }
  }
  // A relationship property's rules may test the relationship's ends
  // when every relationship field using its type declares the same ones.
  const propertyEnds = new Map<string, RuleEnds | string>();
  for (const props of relationshipProperties.values()) {
    const users: RelationshipField[] = [];
    for (const node of nodes.values()) {
      for (const f of node.fields.values()) {
        if (f.kind === "relationship" && f.properties === props.name) {
          users.push(f);
        }
      }
    }
    const pairs = [...new Set(users.map((f) => `${f.owner} → ${f.target}`))];
    const first = users[0] && ruleEnds(users[0]);
    propertyEnds.set(
      props.name,
      users.length === 0
        ? `no relationship field uses ${props.name}`
        : pairs.length > 1
          ? `the relationship fields using ${props.name} declare different ends (${users.map((f) => `${f.owner}.${f.name}: ${f.owner} → ${f.target}`).join(", ")})`
          : (first ??
            `${pairs[0]} has an interface or union end; source and target need @node types`),
    );
  }
  for (const props of relationshipProperties.values()) {
    for (const f of props.fields.values()) {
      if (!f.authorization) continue;
      const ends = propertyEnds.get(props.name);
      const row = f.authorization.validate.filter((r) =>
        testsRelationshipEnds(r.where),
      );
      desugar(
        undefined,
        props.name,
        f.name,
        f.authorization.validate.filter((r) => !row.includes(r)),
      );
      if (typeof ends === "object") {
        desugar(undefined, props.name, f.name, row, ends);
      }
    }
  }
  // Named rules of a type are checked as rules of that type, used or not.
  for (const [typeName, own] of typeRules) {
    const node = nodes.get(typeName);
    if (!node) continue;
    for (const name of own.keys()) {
      // Expanded by reference, so a cycle's chain starts at this rule.
      const rule = { where: { rule: name } as AuthorizationWhere };
      desugar(node, typeName, undefined, [rule]);
      checkAuthorizationWhere(
        nodes,
        relationshipProperties,
        node,
        rule.where,
        problems,
        jwtShape,
        viewerNode,
      );
    }
  }

  for (const node of nodes.values()) {
    const rules = [
      ...(node.authorization?.filter ?? []),
      ...(node.authorization?.validate ?? []),
    ];
    for (const rule of rules) {
      const misplaced = [...rule.operations].filter((op) =>
        RELATIONSHIP_OPERATIONS.has(op),
      );
      if (misplaced.length > 0) {
        problems.push({
          type: node.name,
          message: `@authorization: ${misplaced.join(", ")} rules belong on a relationship field`,
        });
      }
    }
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
      for (const m of f.authorization.mask ?? []) {
        checkAuthorizationWhere(
          nodes,
          relationshipProperties,
          node,
          m.unless,
          problems,
          jwtShape,
          viewerNode,
        );
      }
      for (const rule of f.authorization.validate) {
        if (!isRelationshipRule(rule)) {
          rules.push(rule);
          continue;
        }
        const ends = f.kind === "relationship" ? ruleEnds(f) : undefined;
        if (!ends) {
          if (f.kind === "scalar") {
            problems.push({
              type: node.name,
              field: f.name,
              message:
                "@authorization: CONNECT, DISCONNECT, UPDATE_EDGE and READ_EDGE rules belong on a relationship field",
            });
          } else if (f.kind === "relationship") {
            problems.push({
              type: node.name,
              field: f.name,
              message:
                "@authorization: relationship rules need a @node target, not an interface or union",
            });
          }
          continue;
        }
        checkRuleWhere(
          nodes,
          relationshipProperties,
          undefined,
          node.name,
          f.name,
          rule.where,
          problems,
          jwtShape,
          viewerNode,
          ends,
        );
      }
    }
    for (const rule of rules) {
      checkAuthorizationWhere(
        nodes,
        relationshipProperties,
        node,
        rule.where,
        problems,
        jwtShape,
        viewerNode,
      );
    }
    for (const where of [
      node.authenticationJwt,
      ...[...node.fields.values()].map((f) => f.authenticationJwt),
    ]) {
      if (where) {
        checkAuthorizationWhere(
          nodes,
          relationshipProperties,
          node,
          { jwt: where },
          problems,
          jwtShape,
        );
      }
    }
  }

  // Rules on root @cypher fields: claims and the caller's node, no node.
  for (const f of [...queries, ...mutationFields]) {
    if (!f.authorization) continue;
    desugar(undefined, f.owner, f.name, f.authorization.validate);
    for (const rule of f.authorization.validate) {
      checkRuleWhere(
        nodes,
        relationshipProperties,
        undefined,
        f.owner,
        f.name,
        rule.where,
        problems,
        jwtShape,
        viewerNode,
        undefined,
        "a root @cypher field has no node: its rules test jwt and viewer",
      );
    }
  }

  // Rules on relationship properties: validate rules over claims, for
  // reading and writing the property.
  for (const props of relationshipProperties.values()) {
    for (const f of props.fields.values()) {
      if (!f.authorization && !f.authentication) continue;
      const at = (message: string) =>
        problems.push({ type: props.name, field: f.name, message });
      if (f.authorization?.filter.length) {
        at(
          "@authorization on a relationship property takes validate rules only",
        );
      }
      if (f.authorization?.mask) {
        at("@authorization(mask:) belongs on a scalar field of a @node type");
      }
      const ops = [
        ...(f.authorization?.validate ?? []).flatMap((r) => [...r.operations]),
        ...(f.authentication ?? []),
      ];
      const unsupported = [
        ...new Set(ops.filter((op) => !PROPERTY_OPS.has(op))),
      ];
      if (unsupported.length > 0) {
        at(
          `rules on a relationship property apply to READ, CREATE and UPDATE, not ${unsupported.join(", ")}`,
        );
      }
      for (const rule of f.authorization?.validate ?? []) {
        const ends = testsRelationshipEnds(rule.where)
          ? propertyEnds.get(props.name)
          : undefined;
        if (typeof ends === "string") {
          at(
            `@authorization: source, target and edge need every relationship field using ${props.name} to declare the same ends, but ${ends}`,
          );
          continue;
        }
        if (ends && [...rule.operations].some((op) => op !== "READ")) {
          at(
            "@authorization: source, target and edge decide per relationship, so they take READ rules only; CREATE and UPDATE rules on a relationship property test claims",
          );
          continue;
        }
        checkRuleWhere(
          nodes,
          relationshipProperties,
          undefined,
          props.name,
          f.name,
          rule.where,
          problems,
          jwtShape,
          ends ? viewerNode : undefined,
          ends,
        );
      }
      if (f.authenticationJwt) {
        checkRuleWhere(
          nodes,
          relationshipProperties,
          undefined,
          props.name,
          f.name,
          { jwt: f.authenticationJwt },
          problems,
          jwtShape,
        );
      }
    }
  }

  // A plain object type exists to shape @cypher results.
  const returned = new Set(
    [
      ...[...nodes.values()].flatMap((n) => [...n.fields.values()]),
      ...queries,
      ...mutationFields,
    ].flatMap((f) => (f.kind === "cypher" && f.object ? [f.object] : [])),
  );
  for (const name of objects.keys()) {
    if (!returned.has(name)) {
      problems.push({
        type: name,
        message:
          "object types need @node or @relationshipProperties, unless a @cypher field returns them",
      });
    }
  }

  if (problems.length > 0) throw new ModelError(dedupeProblems(problems));
  const model: GraphModel = {
    nodes,
    abstracts,
    enums,
    relationshipProperties,
    queries,
    mutations: mutationFields,
    warnings,
    objects,
    scalars,
    scalarDescriptions,
    jwt: jwtShape,
    viewer,
    bypass: defaults.bypass,
    cursorSecret: options.cursorSecret,
    maxLimit: globalLimit.max,
  };
  // An update input with nothing in it would break the schema: the
  // mutations that take it are left out, and the model says so.
  for (const node of nodes.values()) {
    if (node.mutations.has("UPDATE") && !isUpdatable(model, node)) {
      warnings.push({
        type: node.name,
        message:
          "@mutation(operations: [UPDATE]) but nothing is updatable (no field settable on update, no relationship with a nested operation): update mutations are left out",
      });
    }
  }
  return model;
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
  } else if (storageOf(named)) {
    type = storageOf(named)!;
  } else {
    return undefined;
  }
  const customScalar =
    isScalarType(named) && !(named.name in BUILTIN_SCALARS)
      ? named.name
      : undefined;
  if (shape.list && !shape.itemRequired) {
    at("list items must be non-null, e.g. [String!]");
  }
  if (shape.depth > 1) at("nested lists are not supported");

  const key = directive(d("key"), f, at) !== undefined;
  const unique = directive(d("unique"), f, at) !== undefined;
  const isPrivate = directive(d("private"), f, at) !== undefined;
  const relayId = directive(d("relayId"), f, at) !== undefined;
  const sortable = directive(d("sortable"), f, at) !== undefined;
  const groupBy = directive(d("groupBy"), f, at) !== undefined;
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
  const scopeArgs = key ? keyArgs(d, f, at) : undefined;
  const keyScope =
    scopeArgs?.["scope"] === "VIEWER"
      ? { separator: (scopeArgs["separator"] as string | undefined) ?? ":" }
      : undefined;
  if (keyScope) {
    if (generate) at("@key(scope:) and @key(generate: true) are exclusive");
    if (keyScope.separator.length === 0) {
      at("@key(separator:) cannot be empty");
    }
    if (type !== "ID" && type !== "String") {
      at("@key(scope:) needs an ID or String field");
    }
  }
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
  if (
    groupBy &&
    (shape.list ||
      type === "Point" ||
      type === "CartesianPoint" ||
      isPrivate ||
      !opts.allowKey)
  ) {
    at("@groupBy needs a readable, non-list, non-Point field of a @node type");
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
      at(`${type} values have no order a RANGE index can use`);
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
    customScalar,
    list: shape.list,
    required: shape.required,
    key,
    unique,
    private: isPrivate,
    relayId,
    filters,
    sortable: sortable && !isPrivate,
    groupBy,
    indexes,
    generate,
    ...(keyScope ? { keyScope } : {}),
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

/** Every `@uniqueTogether` of a node type (repeatable), checked. */
function readUniqueTogether(
  t: GraphQLObjectType,
  node: NodeType,
  nodes: ReadonlyMap<string, NodeType>,
  props: ReadonlyMap<string, RelationshipPropertiesType>,
  d: (name: string) => GraphQLDirective,
  problems: ModelProblem[],
): UniqueTogether[] {
  const out: UniqueTogether[] = [];
  const dirs = [
    ...(t.astNode?.directives ?? []),
    ...t.extensionASTNodes.flatMap((n) => n.directives ?? []),
  ].filter((x) => x.name.value === "uniqueTogether");
  for (const dir of dirs) {
    let ok = true;
    const at = (message: string) => {
      ok = false;
      problems.push({ type: t.name, message: `@uniqueTogether: ${message}` });
    };
    const args = directive(
      d("uniqueTogether"),
      { astNode: { directives: [dir] } },
      at,
    );
    if (!args) continue;
    const names = args["fields"] as string[];
    const where = (args["where"] ?? undefined) as
      | Record<string, unknown>
      | undefined;
    if (names.length === 0) at("fields is empty");
    if (new Set(names).size < names.length) at("fields names a field twice");
    const scalars: ScalarField[] = [];
    const singles: RelationshipField[] = [];
    const sets: RelationshipField[] = [];
    for (const n of names) {
      const f = node.fields.get(n);
      if (!f) at(`${t.name} has no field ${n}`);
      else if (f.kind === "cypher" || f.kind === "custom") {
        at(
          `${n} is ${f.kind === "cypher" ? "a @cypher" : "a @customResolver"} field; only stored fields and relationships can be unique`,
        );
      } else if (f.kind === "scalar") {
        if (f.list) at(`${n} is a list; list scalar fields cannot be unique`);
        else scalars.push(f);
      } else if (!nodes.has(f.target)) {
        at(
          `${n} reaches an interface or union; only relationships to a @node type can be unique`,
        );
      } else if (f.list) sets.push(f);
      else singles.push(f);
    }
    if (sets.length > 1) {
      at(
        `fields names ${sets.length} list relationships (${sets.map((f) => f.name).join(", ")}); at most one is compared as a set`,
      );
    }
    if (where !== undefined) {
      checkNodeWhere(nodes, props, node, where, "where", at);
    }
    if (!ok) continue;
    out.push({ fields: names, scalars, singles, set: sets[0], where });
  }
  return out;
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
  return (
    name in BUILTIN_SCALARS ||
    (t !== undefined && (isEnumType(t) || storageOf(t) !== undefined))
  );
}

const STORAGE_TYPES: Record<string, ScalarType> = {
  STRING: "String",
  INT: "Int",
  FLOAT: "Float",
  BOOLEAN: "Boolean",
  DATETIME: "DateTime",
  DATE: "Date",
};

/** A custom scalar's storage type, from its `@storedAs(type:)`. */
export function storageOf(t: GraphQLNamedType): ScalarType | undefined {
  if (!isScalarType(t)) return undefined;
  const d = t.astNode?.directives?.find((x) => x.name.value === "storedAs");
  const arg = d?.arguments?.find((a) => a.name.value === "type")?.value;
  return arg && arg.kind === Kind.ENUM ? STORAGE_TYPES[arg.value] : undefined;
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
    plainNames: Set<string>;
    schema: GraphQLSchema;
    root: "Query" | "Mutation" | undefined;
    /** Whether the @jwt type has a @viewer claim (enables `$viewer`). */
    viewer: boolean;
  },
): CypherField | undefined {
  const at = (message: string) =>
    problems.push({ type: owner, field: f.name, message });
  const warn = (message: string) =>
    warnings.push({ type: owner, field: f.name, message });
  const args = directive(d("cypher"), f, at);
  if (!args) return undefined;
  const authorization = ctx.root
    ? rootFieldRules(directive(d("authorization"), f, at), at)
    : readOnlyRules(directive(d("authorization"), f, at), at);
  const statement = args["statement"] as string;
  for (const other of [
    "relationship",
    "key",
    "unique",
    "alias",
    "index",
    "default",
    "timestamp",
    "groupBy",
  ]) {
    if (directive(d(other), f, at))
      at(`@${other} cannot be combined with @cypher`);
  }

  const shape = unwrap(f.type);
  const named = getNamedType(f.type).name;
  const node = ctx.nodeNames.has(named) ? named : undefined;
  const namedType = ctx.schema.getType(named);
  const abstract =
    namedType && (isInterfaceType(namedType) || isUnionType(namedType))
      ? named
      : undefined;
  const object = ctx.plainNames.has(named) ? named : undefined;
  if (!node && !abstract && !object && !isScalarLike(named, ctx.schema)) {
    at(
      "@cypher fields return scalars, enums, @node types, interfaces or unions over them, or object types without @node",
    );
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
    if (a.name === "viewer")
      at("argument `viewer` is reserved for the caller's @viewer key");
    const argShape = unwrapInput(a.type);
    const size = directive(d("size"), a, at)?.["max"] as number | undefined;
    if (size !== undefined && !argShape.list) {
      at(`argument ${a.name}: @size applies to list arguments`);
    } else if (size !== undefined && size < 1) {
      at(`argument ${a.name}: @size(max:) must be at least 1`);
    }
    const range = cypherArgumentRange(a, argNamed, d("range"), at);
    const defaultValue = argumentDefault(a);
    if (range && outOfRange(defaultValue, range) !== undefined) {
      at(`argument ${a.name}: its default is outside @range`);
    }
    cypherArgs.push({
      name: a.name,
      type: { named: argNamed, ...argShape },
      defaultValue,
      description: a.description ?? undefined,
      ...(size !== undefined ? { maxItems: size } : {}),
      ...(range ? { range } : {}),
    });
  }

  const params = [...new Set(scanParams(statement).map((p) => p.name))];
  const known = new Set([...cypherArgs.map((a) => a.name), "jwt", "viewer"]);
  for (const p of params) {
    if (p === "viewer" && !ctx.viewer) {
      at(
        "the statement uses $viewer, which needs a @viewer claim on the @jwt type",
      );
    } else if (!known.has(p)) {
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
    } else if (node || abstract || object || shape.list) {
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
    authorization,
    name: f.name,
    owner,
    statement,
    columnName,
    type: { named, ...shapeOf(shape) },
    node,
    abstract,
    object,
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
/**
 * The column a statement returns, when it returns exactly one: the alias
 * of the single item of its last top-level RETURN (or the item, when it
 * is a bare variable). Brackets, strings and comments are respected, so
 * `RETURN coalesce(a, b) AS n` and a RETURN inside `CALL { }` do not
 * confuse it.
 */
/**
 * An SDL argument's default, as a value. graphql 16 keeps it on
 * `defaultValue`; graphql 17 builds `default: { literal }` from SDL and
 * leaves `defaultValue` undefined, so reading only `defaultValue` drops
 * `peers(limit: Int = 2)` to `peers(limit: Int)` there.
 */
/** `@range(min:, max:)` of a @cypher argument, checked against its type. */
function cypherArgumentRange(
  a: GraphQLArgument,
  named: string,
  def: GraphQLDirective,
  at: (message: string) => void,
): { min?: number; max?: number } | undefined {
  const args = directive(def, a, at);
  if (!args) return undefined;
  const min = args["min"] as number | null | undefined;
  const max = args["max"] as number | null | undefined;
  if (named !== "Int" && named !== "Float") {
    at(`argument ${a.name}: @range applies to Int and Float arguments`);
    return undefined;
  }
  if (min == null && max == null) {
    at(`argument ${a.name}: @range needs min, max or both`);
    return undefined;
  }
  if (min != null && max != null && min > max) {
    at(`argument ${a.name}: @range(min:) is greater than max`);
    return undefined;
  }
  return {
    ...(min != null ? { min } : {}),
    ...(max != null ? { max } : {}),
  };
}

function argumentDefault(a: GraphQLArgument): unknown {
  if (a.defaultValue !== undefined) return a.defaultValue;
  const d = (a as { default?: { value?: unknown; literal?: ConstValueNode } })
    .default;
  if (!d) return undefined;
  if ("value" in d) return d.value;
  return d.literal ? valueFromAST(d.literal, a.type) : undefined;
}

function inferColumn(statement: string): string | undefined {
  const code = maskLiterals(statement);
  const depths: number[] = [];
  let depth = 0;
  for (const c of code) {
    if (c === ")" || c === "]" || c === "}") depth--;
    depths.push(depth);
    if (c === "(" || c === "[" || c === "{") depth++;
  }
  const topLevel = (re: RegExp, from = 0) =>
    [...code.slice(from).matchAll(re)]
      .map((m) => ({ index: m.index! + from, length: m[0].length }))
      .filter((m) => depths[m.index] === 0);
  const last = topLevel(/\bRETURN\b/gi).at(-1);
  if (!last) return undefined;
  const start = last.index + last.length;
  const stop =
    topLevel(/\b(ORDER\s+BY|SKIP|LIMIT|UNION)\b/gi, start)[0]?.index ??
    code.length;
  const commas = topLevel(/,/g, start).filter((m) => m.index < stop);
  if (commas.length > 0) return undefined;
  const item = code.slice(start, stop).replace(/^\s*DISTINCT\b/i, "");
  const offset = stop - item.length;
  const alias = /\bAS\s+(`[^`]*`|[A-Za-z_][A-Za-z0-9_]*)\s*$/i.exec(item);
  if (alias) {
    const at = offset + alias.index + alias[0].indexOf(alias[1]!);
    const name = statement.slice(at, at + alias[1]!.length);
    return name.startsWith("`") ? name.slice(1, -1) : name;
  }
  const bare = item.trim();
  return /^[A-Za-z_][A-Za-z0-9_]*$/.test(bare) ? bare : undefined;
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

/**
 * Field-level rules on a relationship or @cypher field: READ validate rules
 * only, checked for each row that reads (or filters on) the field.
 */
function readOnlyRules(
  args: Record<string, unknown> | undefined,
  at: (message: string) => void,
  relationship = false,
): Authorization | undefined {
  const rules = readAuthorization(args);
  if (!rules) return undefined;
  for (const r of rules.validate) {
    const ops = [...r.operations];
    const rel = ops.filter((op) => RELATIONSHIP_OPERATIONS.has(op));
    if (rel.length > 0 && !relationship) {
      at(`${rel.join(", ")} rules belong on a relationship field`);
    } else if (rel.length > 0 && rel.length < ops.length) {
      at(
        `a rule on ${rel.join(", ")} tests source, target and edge; give READ its own rule`,
      );
    } else if (rel.length === 0 && ops.some((op) => op !== "READ")) {
      at(
        relationship
          ? "field-level @authorization on a relationship field takes READ rules, or CONNECT, DISCONNECT, UPDATE_EDGE and READ_EDGE rules; write rules for the nodes belong on the types"
          : "field-level @authorization on a relationship or @cypher field takes READ rules only; write rules belong on the types",
      );
    }
  }
  return rules;
}

/**
 * `@authorization` on a Query or Mutation @cypher field: validate rules
 * guarding the call, tested before the statement runs. There is no node,
 * so they test claims (`jwt`) and the caller's node (`viewer`); their
 * `operations` and `when` do not matter.
 */
function rootFieldRules(
  args: Record<string, unknown> | undefined,
  at: (message: string) => void,
): Authorization | undefined {
  const rules = readAuthorization(args);
  if (!rules) return undefined;
  if (rules.filter.length > 0) {
    at(
      "a root @cypher field takes validate rules: there are no rows to filter",
    );
  }
  if (rules.mask?.length) {
    at("a root @cypher field takes validate rules: there is no row to mask");
  }
  return rules;
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
    requireAuthenticationDefaulted: r.requireAuthentication == null,
    where: r.where,
  }));
  const validate = ((args["validate"] as Raw[] | undefined) ?? []).map((r) => ({
    operations: new Set(r.operations),
    when: new Set<"BEFORE" | "AFTER">(r.when ?? ["BEFORE", "AFTER"]),
    requireAuthentication: r.requireAuthentication ?? true,
    requireAuthenticationDefaulted: r.requireAuthentication == null,
    where: r.where,
  }));
  return {
    filter,
    validate,
    ...(args["bypass"] != null ? { bypass: args["bypass"] as boolean } : {}),
    ...(args["public"] != null
      ? { public: new Set(args["public"] as AuthOperation[]) }
      : {}),
    ...(args["mask"] != null
      ? {
          mask: (
            args["mask"] as Array<{
              unless: AuthorizationWhere;
              value?: unknown;
            }>
          ).map((m) => ({
            unless: m.unless,
            value: m.value ?? null,
            ...("value" in m ? {} : { missingValue: true }),
          })),
        }
      : {}),
  };
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
const PROPERTY_OPS = new Set<AuthOperation>(["READ", "CREATE", "UPDATE"]);
const JWT_OPS = new Set([...SCALAR_WHERE_OPS, "includes", "exists"]);
const COUNT_WHERE_OPS = new Set(["eq", "lt", "lte", "gt", "gte"]);

/** Keys of a rule that are not claim tests: `node`, `viewer`, … */
function claimsOnlyViolations(where: unknown, path = ""): string[] {
  if (!isRecord(where)) return [];
  const out: string[] = [];
  for (const [k, value] of Object.entries(where)) {
    const here = path ? `${path}.${k}` : k;
    if (k === "AND" || k === "OR") {
      if (Array.isArray(value)) {
        value.forEach((w, i) =>
          out.push(...claimsOnlyViolations(w, `${here}[${i}]`)),
        );
      }
    } else if (k === "NOT") out.push(...claimsOnlyViolations(value, here));
    else if (k !== "jwt") out.push(here);
  }
  return out;
}

/** A rule over a relationship's source, target and edge. */
const isRelationshipRule = (rule: {
  operations: ReadonlySet<AuthOperation>;
}): boolean =>
  [...rule.operations].some((op) => RELATIONSHIP_OPERATIONS.has(op));

const isRecord = (x: unknown): x is Record<string, unknown> =>
  typeof x === "object" && x !== null && !Array.isArray(x);

/** Check an `@authorization` where against the model, at startup. */
function checkAuthorizationWhere(
  nodes: ReadonlyMap<string, NodeType>,
  props: ReadonlyMap<string, RelationshipPropertiesType>,
  node: NodeType,
  where: unknown,
  problems: ModelProblem[],
  jwtShape?: ReadonlyMap<string, string>,
  viewerNode?: NodeType,
) {
  checkRuleWhere(
    nodes,
    props,
    node,
    node.name,
    undefined,
    where,
    problems,
    jwtShape,
    viewerNode,
  );
}

/**
 * Check a rule's where. Without `node` (a relationship property's rules)
 * only claims may be tested: a @relationshipProperties type can sit under
 * several relationship fields, from either end, so `node` would be
 * ambiguous.
 */
/** The ends a relationship field's rule tests, for its checks. */
interface RuleEnds {
  source: NodeType;
  target: NodeType;
  edge: RelationshipPropertiesType | undefined;
}

function checkRuleWhere(
  nodes: ReadonlyMap<string, NodeType>,
  props: ReadonlyMap<string, RelationshipPropertiesType>,
  node: NodeType | undefined,
  type: string,
  field: string | undefined,
  where: unknown,
  problems: ModelProblem[],
  jwtShape?: ReadonlyMap<string, string>,
  viewerNode?: NodeType,
  ends?: RuleEnds,
  nodeless = "rules on relationship properties test claims (jwt) only; node rules belong on the node types",
) {
  const at = (message: string) =>
    problems.push({
      type,
      ...(field ? { field } : {}),
      message: `@authorization: ${message}`,
    });
  // What `${node.…}`-style placeholders may read here: the rule's own
  // node, or a relationship rule's ends and edge.
  const references: RuleReferences = ends
    ? { source: ends.source, target: ends.target, edge: ends.edge }
    : node
      ? { node }
      : {};
  // A filter over a node type, with the strings and claims it uses.
  const nodePart = (
    t: NodeType,
    value: unknown,
    here: string,
    refs: RuleReferences = references,
  ) => {
    checkNodeWhere(nodes, props, t, value, here, at);
    for (const problem of ruleStringProblems(value)) at(`${here}: ${problem}`);
    for (const problem of ruleReferenceProblems(value, refs, nodes))
      at(`${here}: ${problem}`);
    for (const problem of viewerRefProblems(value, viewerNode))
      at(`${here}: ${problem}`);
    if (jwtShape) {
      for (const ref of claimRefs(value)) {
        if (!jwtShape.has(ref))
          at(`${here}: $jwt.${ref} is not a claim of the @jwt type`);
      }
    }
  };
  const expected = ends
    ? "source, target, edge, viewer, jwt, AND, OR or NOT"
    : "node, viewer, jwt, AND, OR or NOT";
  const visit = (w: unknown, path: string) => {
    if (w === UNEXPANDED) return; // already reported
    if (!isRecord(w)) return at(`${path || "where"} must be an object`);
    for (const [k, value] of Object.entries(w)) {
      const here = path ? `${path}.${k}` : k;
      if (k === "AND" || k === "OR") {
        if (!Array.isArray(value)) at(`${here} must be a list`);
        else value.forEach((x, i) => visit(x, `${here}[${i}]`));
      } else if (k === "NOT") {
        visit(value, here);
      } else if (ends && (k === "source" || k === "target")) {
        nodePart(ends[k], value, here);
      } else if (ends && k === "edge") {
        if (!ends.edge) at(`${here}: the relationship has no properties`);
        else {
          checkEdgeWhere(ends.edge, value, here, at);
          for (const problem of ruleStringProblems(value))
            at(`${here}: ${problem}`);
          for (const problem of ruleReferenceProblems(value, references, nodes))
            at(`${here}: ${problem}`);
        }
      } else if (k === "node" && ends) {
        at(`${here}: a relationship rule tests source, target and edge`);
      } else if (k === "node" && !node) {
        at(`${here}: ${nodeless}`);
      } else if (k === "node" && node) {
        nodePart(node, value, here);
      } else if (k === "viewer") {
        // Desugaring already reported a missing @viewer. The caller's node
        // is found by the claim alone: nothing of the rule's node to read.
        if (viewerNode) nodePart(viewerNode, value, here, {});
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
              for (const [op, operand] of Object.entries(ops)) {
                if (!JWT_OPS.has(op))
                  at(`${here}.${claim}: unknown operator ${op}`);
                if (JSON.stringify(operand ?? null).includes("${")) {
                  at(
                    `${here}.${claim}.${op}: claim tests compare with literals; \${...} placeholders belong in node parts`,
                  );
                }
              }
            }
          }
        }
      } else {
        at(`${here}: expected ${expected}`);
      }
    }
  };
  visit(where, "");
}

function checkNodeWhere(
  nodes: ReadonlyMap<string, NodeType>,
  props: ReadonlyMap<string, RelationshipPropertiesType>,
  node: NodeType,
  where: unknown,
  path: string,
  at: (message: string) => void,
) {
  if (where === UNEXPANDED) return; // already reported
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
          checkNodeWhere(nodes, props, node, x, `${here}[${i}]`, at),
        );
      continue;
    }
    if (k === "NOT") {
      checkNodeWhere(nodes, props, node, value, here, at);
      continue;
    }
    const field = node.fields.get(k);
    const connection = k.endsWith("Connection")
      ? node.fields.get(k.slice(0, -"Connection".length))
      : undefined;
    const exists = k.endsWith("Exists")
      ? node.fields.get(k.slice(0, -"Exists".length))
      : undefined;
    if (!field && exists?.kind === "relationship" && !exists.list) {
      // `<field>Exists: Boolean`: whether the single relationship is set.
      if (typeof value !== "boolean") at(`${here} must be true or false`);
    } else if (
      !field &&
      connection?.kind === "relationship" &&
      connection.list &&
      connection.properties
    ) {
      // `<field>Connection: { some: { node, edge } }`, as in a public where.
      checkConnectionWhere(nodes, props, connection, value, here, at);
    } else if (!field) {
      at(`${here}: ${node.name} has no field ${k}`);
    } else if (field.kind === "cypher" || field.kind === "custom") {
      at(
        `${here}: ${field.kind === "cypher" ? "@cypher" : "@customResolver"} fields cannot be used in rules`,
      );
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
      const abstract = abstractsOf.get(nodes)?.get(field.target);
      const check = (w: unknown, path: string) =>
        target
          ? checkNodeWhere(nodes, props, target, w, path, at)
          : abstract
            ? checkAbstractWhere(nodes, props, abstract, w, path, at)
            : undefined;
      if (!target && !abstract) continue;
      if (!field.list) {
        check(value, here);
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
          check(inner, `${here}.${q}`);
        } else {
          at(`${here}: expected some, all, none, single or count`);
        }
      }
    }
  }
}

/**
 * Interfaces and unions of a model being built, by its node map: the rule
 * checks below receive the node map only.
 */
const abstractsOf = new WeakMap<
  ReadonlyMap<string, NodeType>,
  ReadonlyMap<string, AbstractType>
>();

/**
 * A rule's filter through a relationship to an interface or union: a
 * union's names members, each a node filter of that member; an
 * interface's tests its own fields and `typename`.
 */
function checkAbstractWhere(
  nodes: ReadonlyMap<string, NodeType>,
  props: ReadonlyMap<string, RelationshipPropertiesType>,
  abstract: AbstractType,
  where: unknown,
  path: string,
  at: (message: string) => void,
) {
  if (where === UNEXPANDED) return; // already reported
  if (!isRecord(where) || Object.keys(where).length === 0) {
    return at(`${path} is empty: a rule must test something`);
  }
  for (const [k, value] of Object.entries(where)) {
    const here = `${path}.${k}`;
    if (value === null) {
      at(`${here} is null: a rule must test something`);
      continue;
    }
    if (abstract.kind === "union") {
      const member = abstract.members.includes(k) ? nodes.get(k) : undefined;
      if (!member) at(`${here}: ${k} is not a member of ${abstract.name}`);
      else checkNodeWhere(nodes, props, member, value, here, at);
      continue;
    }
    if (k === "AND" || k === "OR") {
      if (!Array.isArray(value)) at(`${here} must be a list`);
      else
        value.forEach((x, i) =>
          checkAbstractWhere(nodes, props, abstract, x, `${here}[${i}]`, at),
        );
    } else if (k === "NOT") {
      checkAbstractWhere(nodes, props, abstract, value, here, at);
    } else if (k === "typename") {
      if (
        !Array.isArray(value) ||
        value.some((m) => !abstract.members.includes(m as string))
      ) {
        at(`${here}: a list of ${abstract.name}'s implementations`);
      }
    } else if (!abstract.fields.has(k)) {
      at(`${here}: ${abstract.name} has no field ${k}`);
    } else if (!isRecord(value) || Object.keys(value).length === 0) {
      at(`${here} must be a non-empty operator object`);
    } else {
      for (const [op, operand] of Object.entries(value)) {
        if (!SCALAR_WHERE_OPS.has(op)) at(`${here}: unknown operator ${op}`);
        else if (operand === null) at(`${here}.${op} is null`);
      }
    }
  }
}

/** A rule's `<field>Connection` test: quantifiers over { node, edge }. */
function checkConnectionWhere(
  nodes: ReadonlyMap<string, NodeType>,
  props: ReadonlyMap<string, RelationshipPropertiesType>,
  rel: RelationshipField,
  value: unknown,
  path: string,
  at: (message: string) => void,
) {
  if (!isRecord(value) || Object.keys(value).length === 0) {
    return at(`${path} must be a non-empty object`);
  }
  const target = nodes.get(rel.target);
  const edgeType = props.get(rel.properties!);
  for (const [q, inner] of Object.entries(value)) {
    const here = `${path}.${q}`;
    if (!["some", "all", "none", "single"].includes(q)) {
      at(`${path}: expected some, all, none or single`);
      continue;
    }
    if (!isRecord(inner) || Object.keys(inner).length === 0) {
      at(`${here} is empty: a rule must test something`);
      continue;
    }
    for (const [side, test] of Object.entries(inner)) {
      if (side === "node") {
        if (target)
          checkNodeWhere(nodes, props, target, test, `${here}.node`, at);
      } else if (side === "edge") {
        if (edgeType) checkEdgeWhere(edgeType, test, `${here}.edge`, at);
      } else {
        at(`${here}: expected node or edge`);
      }
    }
  }
}

function checkEdgeWhere(
  edgeType: RelationshipPropertiesType,
  where: unknown,
  path: string,
  at: (message: string) => void,
) {
  if (!isRecord(where) || Object.keys(where).length === 0) {
    return at(`${path} is empty: a rule must test something`);
  }
  for (const [k, value] of Object.entries(where)) {
    const here = `${path}.${k}`;
    if (value === null) {
      at(`${here} is null: a rule must test something`);
    } else if (k === "AND" || k === "OR") {
      if (!Array.isArray(value)) at(`${here} must be a list`);
      else
        value.forEach((x, i) =>
          checkEdgeWhere(edgeType, x, `${here}[${i}]`, at),
        );
    } else if (k === "NOT") {
      checkEdgeWhere(edgeType, value, here, at);
    } else if (!edgeType.fields.has(k)) {
      at(`${here}: ${edgeType.name} has no field ${k}`);
    } else if (!isRecord(value) || Object.keys(value).length === 0) {
      at(`${here} must be a non-empty operator object`);
    } else {
      for (const [op, operand] of Object.entries(value)) {
        if (!SCALAR_WHERE_OPS.has(op)) at(`${here}: unknown operator ${op}`);
        else if (operand === null) at(`${here}.${op} is null`);
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
      // A list of strings indexes each of its strings.
      else if (field.type !== "String" && field.type !== "ID") {
        at(`@fulltext: ${f} is not a String or [String] field`);
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

/**
 * Claim names referenced anywhere in a value: as "$jwt.name…" strings and
 * as ${jwt.name…} placeholders.
 */
function claimRefs(value: unknown): string[] {
  if (typeof value === "string") {
    if (value.includes("${")) {
      return [...value.matchAll(PLACEHOLDER)]
        .filter((m) => m[1] === "jwt")
        .map((m) => m[2]!.split(".")[0]!);
    }
    return value.startsWith("$jwt.") ? [value.slice(5).split(".")[0]!] : [];
  }
  if (Array.isArray(value)) return value.flatMap(claimRefs);
  if (value !== null && typeof value === "object") {
    return Object.values(value).flatMap(claimRefs);
  }
  return [];
}

const REFERENCE_PATH = /^[A-Za-z0-9_]+(\.[A-Za-z0-9_]+)*$/;

/**
 * Malformed claim and context references in a rule's strings: a whole
 * string "$jwt.path" names a path and nothing else; any `${` starts a
 * `${jwt.path}` or `${context.path}` placeholder.
 */
/** `${viewer.field}` placeholders need @viewer, a scalar field, a string operand. */
function viewerRefProblems(
  value: unknown,
  viewerNode: NodeType | undefined,
  inList = false,
): string[] {
  if (typeof value === "string") {
    const refs = [...value.matchAll(PLACEHOLDER)].filter(
      (m) => m[1] === "viewer",
    );
    if (refs.length === 0) return [];
    if (!viewerNode) {
      return [
        `"${value}": \${viewer.…} needs a @viewer claim on the @jwt type`,
      ];
    }
    const out: string[] = [];
    if (inList) {
      out.push(
        `"${value}": \${viewer.…} stands for one value, not inside a list`,
      );
    }
    for (const m of refs) {
      if (viewerNode.fields.get(m[2]!)?.kind !== "scalar") {
        out.push(`"${value}": ${viewerNode.name} has no scalar field ${m[2]}`);
      }
    }
    return out;
  }
  if (Array.isArray(value)) {
    return value.flatMap((v) => viewerRefProblems(v, viewerNode, true));
  }
  if (value !== null && typeof value === "object") {
    return Object.values(value).flatMap((v) =>
      viewerRefProblems(v, viewerNode, inList),
    );
  }
  return [];
}

/** What a rule's `${node.…}`, `${source.…}`, `${target.…}`, `${edge.…}` may read. */
interface RuleReferences {
  node?: NodeType;
  source?: NodeType;
  target?: NodeType;
  edge?: RelationshipPropertiesType | undefined;
}

/**
 * `${node.path}`-style placeholders: the source must be available where
 * the string stands (`node` in a type's rules, `source` / `target` /
 * `edge` in a relationship field's), the path must reach a scalar through
 * single relationships to node types, and it stands for one value.
 */
function ruleReferenceProblems(
  value: unknown,
  refs: RuleReferences,
  nodes: ReadonlyMap<string, NodeType>,
  inList = false,
): string[] {
  if (typeof value === "string") {
    const out: string[] = [];
    for (const m of value.matchAll(PLACEHOLDER)) {
      const [ref, source, path] = m as unknown as [string, string, string];
      if (!RULE_REFERENCES.has(source)) continue;
      if (inList) {
        out.push(`"${value}": ${ref} stands for one value, not inside a list`);
        continue;
      }
      if (source === "edge") {
        if (!refs.edge) {
          out.push(
            `"${value}": \${edge.…} reads a relationship rule's properties; there are none here`,
          );
        } else if (!refs.edge.fields.has(path)) {
          out.push(`"${value}": ${refs.edge.name} has no property ${path}`);
        }
        continue;
      }
      const start = refs[source as "node" | "source" | "target"];
      if (!start) {
        out.push(
          source === "node" && refs.source
            ? `"${value}": a relationship rule reads its ends: \${source.…} or \${target.…}`
            : source === "node"
              ? `"${value}": viewer: { … } tests the caller's node alone; read the rule's node in a node part`
              : refs.node
                ? `"${value}": \${${source}.…} reads a relationship rule's ends; here the rule's node is \${node.…}`
                : `"${value}": viewer: { … } tests the caller's node alone; read the rule's node in a node part`,
        );
        continue;
      }
      let node: NodeType = start;
      const steps = path.split(".");
      for (const [i, step] of steps.entries()) {
        const f: Field | undefined = node.fields.get(step);
        if (i === steps.length - 1) {
          if (f?.kind !== "scalar") {
            out.push(
              `"${value}": ${node.name}.${step} is not a scalar field: a reference ends on one`,
            );
          }
          break;
        }
        const target: NodeType | undefined =
          f?.kind === "relationship" ? nodes.get(f.target) : undefined;
        if (f?.kind !== "relationship" || f.list || !target) {
          out.push(
            `"${value}": ${node.name}.${step} is not a single relationship to a node type: a reference steps through those only`,
          );
          break;
        }
        node = target;
      }
    }
    return out;
  }
  if (Array.isArray(value)) {
    return value.flatMap((v) => ruleReferenceProblems(v, refs, nodes, true));
  }
  if (value !== null && typeof value === "object") {
    return Object.values(value).flatMap((v) =>
      ruleReferenceProblems(v, refs, nodes, inList),
    );
  }
  return [];
}

function ruleStringProblems(value: unknown): string[] {
  if (typeof value === "string") {
    if (value.includes("${")) {
      const rest = value.replace(PLACEHOLDER, (m, _source, path: string) =>
        REFERENCE_PATH.test(path) ? "" : m,
      );
      return rest.includes("${")
        ? [
            `"${value}": a placeholder is \${jwt.<claim>}, \${context.<path>}, \${viewer.<field>} or \${node.<path>}`,
          ]
        : [];
    }
    for (const prefix of ["$jwt.", "$context."]) {
      if (
        value.startsWith(prefix) &&
        !REFERENCE_PATH.test(value.slice(prefix.length))
      ) {
        const source = prefix.slice(1, -1);
        const path =
          /^[A-Za-z0-9_.]*[A-Za-z0-9_]/.exec(value.slice(prefix.length))?.[0] ??
          "claim";
        return [
          `"${value}" is not a ${source} reference; to put one inside a string write "\${${source}.${path}}${value.slice(prefix.length + path.length)}"`,
        ];
      }
    }
    // A misspelt placeholder would be compared as a literal and silently
    // stop meaning what it says. A literal "$" is written "\\$".
    if (
      value.startsWith("$") &&
      !value.startsWith("$jwt.") &&
      !value.startsWith("$context.")
    ) {
      return [
        `"${value}" is not a placeholder: write $jwt.<claim> or $context.<path>, or "\\${value}" for the literal string`,
      ];
    }
    return [];
  }
  if (Array.isArray(value)) return value.flatMap(ruleStringProblems);
  if (value !== null && typeof value === "object") {
    return Object.values(value).flatMap(ruleStringProblems);
  }
  return [];
}
