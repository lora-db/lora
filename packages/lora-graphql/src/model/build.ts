import {
  buildASTSchema,
  concatAST,
  getNamedType,
  isEnumType,
  isInterfaceType,
  isObjectType,
  isScalarType,
  isUnionType,
  parse,
  type DocumentNode,
  type GraphQLField,
  type GraphQLInterfaceType,
  type GraphQLUnionType,
  type GraphQLObjectType,
  type GraphQLSchema,
} from "graphql";
import { ModelError, type ModelProblem } from "../errors.js";
import { directiveTypeDefs, PRELUDE_TYPES } from "./directives.js";
import { checkDirectivePositions } from "./positions.js";
import {
  checkViewer,
  desugarRule,
  testsRelationshipEnds,
  type DesugarContext,
  type NamedRules,
  type ViewerMapping,
} from "./desugar.js";
import { uniqueTogetherTouched } from "./unique-together.js";
import type {
  AbstractType,
  AuthOperation,
  Authorization,
  AuthorizationWhere,
  CypherField,
  EnumType,
  Field,
  ModelWarning,
  MutationOperation,
  TypeShape,
  FilterOperator,
  GraphModel,
  DeclaredRelationship,
  NodeType,
  PlainObjectType,
  RelationshipField,
  RelationshipPropertiesType,
  ScalarField,
  ScalarType,
  UniqueTogether,
} from "./types.js";
import { RELATIONSHIP_OPERATIONS } from "./types.js";
import { isUpdatable } from "./inputs.js";
import {
  resolveGlobalLimit,
  resolveLimit,
  type ModelOptions,
} from "./build/limits.js";
import {
  BUILTIN_SCALARS,
  unwrap,
  isScalarLike,
  storageOf,
  lowerFirst,
  defaultPlural,
  shapeOf,
} from "./build/shapes.js";
import { directive, directiveNodes } from "./build/directive-args.js";
import { buildScalarField, defaultMatches } from "./build/scalar-field.js";
import {
  declareRelationship,
  buildRelationshipField,
} from "./build/relationship-field.js";
import { buildCypherField } from "./build/cypher-field.js";
import {
  authOps,
  readAuthorization,
  PROPERTY_OPS,
  passableWithoutClaims,
  claimsOnlyViolations,
  relationshipRuleOperations,
  isRelationshipRule,
} from "./build/authorization.js";
import {
  checkAuthorizationWhere,
  checkRuleWhere,
  abstractsOf,
  type RuleEnds,
} from "./build/rule-checks.js";
import { readSearch } from "./build/search.js";
import { readUniqueTogether } from "./build/unique-together.js";

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
    requireAuthentication?: boolean;
  } = {};
  const defaultArgs = directive(d("authorizationDefaults"), schema, (message) =>
    problems.push({ type: "schema", message }),
  );
  if (defaultArgs?.["bypass"] != null)
    defaults.bypass = defaultArgs["bypass"] as AuthorizationWhere;
  if (defaultArgs?.["mutations"] != null)
    defaults.mutations = defaultArgs["mutations"] as AuthorizationWhere;
  if (defaultArgs?.["requireAuthentication"] != null)
    defaults.requireAuthentication = defaultArgs[
      "requireAuthentication"
    ] as boolean;
  // `extend schema @authorizationRules(rules: [...])`: claims-only rules.
  const schemaRules = new Map<string, AuthorizationWhere>();
  for (const { node: dir } of directiveNodes("authorizationRules", schema)) {
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
    for (const { node: dir } of directiveNodes("authorizationRule", t)) {
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
      // Per operation: the default guards each write the type's own rules
      // leave uncovered. A filter rule never covers CREATE (there is no
      // node to filter before it exists).
      const uncovered = WRITES.filter(
        (op) =>
          !(node.authorization?.validate ?? []).some((r) =>
            r.operations.has(op),
          ) &&
          (op === "CREATE" ||
            !(node.authorization?.filter ?? []).some((r) =>
              r.operations.has(op),
            )),
      );
      if (uncovered.length === 0) continue;
      (node as { authorization: Authorization }).authorization = {
        ...(node.authorization ?? { filter: [] }),
        validate: [
          ...(node.authorization?.validate ?? []),
          {
            operations: new Set<AuthOperation>(uncovered),
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
      } else if (masks.length > 0 && f.kind === "scalar" && f.vector) {
        // The vector index ranks by the stored vector, mask or not.
        at(
          "@authorization(mask:) does not fit a @vector field: similarity search ranks by the stored vector. Guard it with READ validate rules",
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
      if ("requireAuthentication" in rule) {
        // A rule no anonymous request can pass asks for a token either
        // way: the caller is told to sign in, not that they are refused.
        const r = rule as {
          requireAuthentication: boolean;
          requireAuthenticationDefaulted?: boolean;
        };
        r.requireAuthentication =
          (defaults.requireAuthentication ?? true) ||
          !passableWithoutClaims(rule.where);
        r.requireAuthenticationDefaulted =
          defaults.requireAuthentication === undefined;
      }
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

  // A relationship's rules hold from either side, so one side declares
  // them: the same operation ruled on both would silently need both.
  for (const node of nodes.values()) {
    for (const f of node.fields.values()) {
      if (f.kind !== "relationship") continue;
      const own = relationshipRuleOperations(f);
      if (own.size === 0) continue;
      for (const g of nodes.get(f.target)?.fields.values() ?? []) {
        if (
          g.kind !== "relationship" ||
          g === f ||
          g.type !== f.type ||
          g.target !== f.owner ||
          g.direction === f.direction ||
          // Each pair once: reported on the field that sorts first.
          `${g.owner}.${g.name}` < `${f.owner}.${f.name}`
        ) {
          continue;
        }
        const both = [...relationshipRuleOperations(g)].filter((op) =>
          own.has(op),
        );
        if (both.length > 0) {
          problems.push({
            type: node.name,
            field: f.name,
            message: `@authorization: ${both.join(", ")} ${both.length === 1 ? "rules are" : "rules are each"} declared on both ${f.owner}.${f.name} and ${g.owner}.${g.name}, the two sides of ${f.type}; a relationship's rules hold from either side, so declare each operation on one (combine with AND where both must pass)`,
          });
        }
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
  // A @cypher mutation's write-set is unknown: @uniqueTogether types it
  // may write are checked by comparing every node of the type.
  for (const field of model.mutations) {
    const touched = uniqueTogetherTouched(model, field.statement);
    if (touched.length === 0) continue;
    warnings.push({
      type: "Mutation",
      field: field.name,
      message: `the statement may write ${touched.map((n) => n.name).join(", ")} (@uniqueTogether): each call compares every node of ${touched.length === 1 ? "that type" : "those types"} after the statement (a scan); a generated mutation checks only the nodes it writes`,
    });
  }
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
