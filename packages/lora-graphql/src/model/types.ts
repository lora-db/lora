// The validated graph model: what the annotated SDL says about labels,
// properties, relationships and the API surface. Everything downstream
// (schema generation, compilation, index inference) reads only this.

export type ScalarType =
  | "String"
  | "ID"
  | "Int"
  | "Float"
  | "Boolean"
  | "BigInt"
  | "Date"
  | "Time"
  | "LocalTime"
  | "DateTime"
  | "LocalDateTime"
  | "Duration"
  | "Point"
  | "CartesianPoint"
  /** A user-declared enum, stored as its value name. */
  | "Enum";

export type FilterOperator =
  | "EQ"
  | "IN"
  | "LT"
  | "LTE"
  | "GT"
  | "GTE"
  | "CONTAINS"
  | "STARTS_WITH"
  | "ENDS_WITH"
  | "WITHIN_BBOX"
  | "DISTANCE"
  | "INCLUDES"
  | "IS_NULL"
  | "CASE_INSENSITIVE";

export type IndexKind = "RANGE" | "TEXT" | "POINT";

export type MutationOperation = "CREATE" | "UPDATE" | "DELETE";
export type AuthOperation =
  | "READ"
  | MutationOperation
  | "CREATE_RELATIONSHIP"
  | "DELETE_RELATIONSHIP"
  | "SUBSCRIBE"
  | RelationshipOperation;

/** Operations of rules on a relationship field, over `source`, `target`, `edge`. */
export type RelationshipOperation =
  | "CONNECT"
  | "DISCONNECT"
  | "UPDATE_EDGE"
  | "READ_EDGE";

export const RELATIONSHIP_OPERATIONS: ReadonlySet<AuthOperation> = new Set([
  "CONNECT",
  "DISCONNECT",
  "UPDATE_EDGE",
  "READ_EDGE",
]);

/** Field-level options shared by every field kind. */
interface FieldBase {
  /** Operations on which reading this field needs an authenticated request. */
  authentication: ReadonlySet<AuthOperation> | undefined;
  /** Claims `@authentication(jwt:)` requires, when set. */
  authenticationJwt?: AuthorizationWhere | undefined;
  /** Field-level `@authorization` (validate rules only). */
  authorization?: Authorization | undefined;
}

export interface ScalarField extends FieldBase {
  kind: "scalar";
  /** API field name. */
  name: string;
  /** Stored property name (differs from `name` under `@alias`). */
  property: string;
  type: ScalarType;
  /** Enum type name when `type` is `Enum`. */
  enumName: string | undefined;
  /** The custom scalar clients see, when the field is one (`@storedAs`). */
  customScalar?: string | undefined;
  list: boolean;
  required: boolean;
  key: boolean;
  unique: boolean;
  private: boolean;
  relayId: boolean;
  filters: ReadonlySet<FilterOperator>;
  sortable: boolean;
  /** `@groupBy`: a grouping key of `<plural>Grouped`. */
  groupBy: boolean;
  /** Indexes requested explicitly with `@index`. */
  indexes: readonly IndexKind[];
  /** `@key(generate: true)`: creates fill a UUID when the input omits it. */
  generate: boolean;
  /**
   * `@key(scope: VIEWER)`: a created key starts with the caller's `@viewer`
   * claim and `separator`.
   */
  keyScope?: { separator: string } | undefined;
  /** `@default(value:)`, stored on create when the input omits the field. */
  defaultValue: { value: unknown } | undefined;
  /** `@timestamp`: set to the current time by these operations. */
  timestamp: ReadonlySet<"CREATE" | "UPDATE"> | undefined;
  /** Never client-settable (`@readonly`, `@timestamp`, `@private`). */
  readonly: boolean;
  /** `@settable`: which mutations may set it (both false when readonly). */
  settableOn: { create: boolean; update: boolean };
  /** `@selectable`: readable (false: write-only), and aggregatable. */
  selectableOn: { read: boolean; aggregate: boolean };
  /** `@populatedBy`: a named callback computes it on these operations. */
  populatedBy:
    | { callback: string; operations: ReadonlySet<"CREATE" | "UPDATE"> }
    | undefined;
  /**
   * Set on the stand-in for a `@filterable` / `@sortable` `@cypher` field:
   * its value is the statement's, computed per row, not a property.
   */
  computedBy?: CypherField;
  /** `@vector`: stored as a VECTOR, searchable by similarity. */
  vector:
    | { dimensions: number; similarity: "COSINE" | "EUCLIDEAN" }
    | undefined;
  description: string | undefined;
}

export type NestedOperation =
  | "CREATE"
  | "CONNECT"
  | "DISCONNECT"
  | "UPDATE"
  | "DELETE";

export interface RelationshipField extends FieldBase {
  kind: "relationship";
  name: string;
  /** Node type that declares the field. */
  owner: string;
  /** Relationship type, e.g. `IN_GENRE`. */
  type: string;
  direction: "IN" | "OUT";
  /** Target node type name. */
  target: string;
  list: boolean;
  required: boolean;
  /** `@relationshipProperties` type name. */
  properties: string | undefined;
  /**
   * The concrete @node types the field reaches: the target itself, or an
   * interface's implementations / a union's members.
   */
  members: readonly string[];
  /** For a member copy of a polymorphic field: the field it came from. */
  via?: RelationshipField | undefined;
  filterable: boolean;
  /** UNDIRECTED: reads follow the relationship both ways. */
  queryDirection: "DIRECTED" | "UNDIRECTED";
  /** What deleting the owner does to nodes reached through the field. */
  onDelete: "DETACH" | "CASCADE" | "RESTRICT";
  /** Nested writes mutation inputs offer for the field. */
  nestedOperations: ReadonlySet<NestedOperation>;
  /**
   * `@settable` / `@readonly`: whether create and update inputs offer the
   * field at all (an upsert of an existing node keeps its relationship).
   */
  settableOn: { create: boolean; update: boolean };
  /** Whether the field's connection and filters offer aggregates. */
  aggregate: boolean;
  cardinality: number | undefined;
  limit: PageLimit | undefined;
  description: string | undefined;
}

/** How a `@cypher` field's value, or one of its arguments, is typed. */
export interface TypeShape {
  /** Named type: a scalar, enum or (for results) node type name. */
  named: string;
  list: boolean;
  required: boolean;
  itemRequired: boolean;
}

export interface CypherArgument {
  name: string;
  type: TypeShape;
  defaultValue: unknown;
  description: string | undefined;
}

export interface CypherField extends FieldBase {
  kind: "cypher";
  name: string;
  /** Node type name, or `Query` / `Mutation` for root fields. */
  owner: string;
  statement: string;
  /** The result column holding the field's value. */
  columnName: string;
  /** The field's type; `node` names a @node type when it returns nodes. */
  type: TypeShape;
  node: string | undefined;
  /** An interface or union over @node types, when it returns those. */
  abstract: string | undefined;
  /** A plain object type (no @node), when it returns maps. */
  object: string | undefined;
  args: readonly CypherArgument[];
  /** `$parameters` the statement uses, in order of first use. */
  params: readonly string[];
  /**
   * `@filterable` / `@sortable`: a scalar stand-in for filters and sorts,
   * whose value the statement computes per row (never through an index).
   */
  computed: ScalarField | undefined;
  description: string | undefined;
}

/**
 * A field computed in JavaScript (`@customResolver`): never stored or
 * filtered; `requires` is fetched with the node so the resolver can use it.
 */
export interface CustomField extends FieldBase {
  kind: "custom";
  name: string;
  owner: string;
  /** Selection on the owner type, e.g. `name genre { name }`. */
  requires: string | undefined;
  type: TypeShape;
  description: string | undefined;
}

export type Field = ScalarField | RelationshipField | CypherField | CustomField;

/** A `${jwt.path}` / `${context.path}` placeholder inside a rule string. */
export const PLACEHOLDER = /\$\{(jwt|context)\.([A-Za-z0-9_.]+)\}/g;

/** `{ node, jwt, AND, OR, NOT }`, as written in `@authorization`. */
export type AuthorizationWhere = Record<string, unknown>;

export interface AuthorizationFilterRule {
  operations: ReadonlySet<AuthOperation>;
  requireAuthentication: boolean;
  /** `requireAuthentication` was left to its default (true). */
  requireAuthenticationDefaulted?: boolean;
  where: AuthorizationWhere;
}

export interface AuthorizationValidateRule {
  operations: ReadonlySet<AuthOperation>;
  when: ReadonlySet<"BEFORE" | "AFTER">;
  requireAuthentication: boolean;
  /** `requireAuthentication` was left to its default (true). */
  requireAuthenticationDefaulted?: boolean;
  where: AuthorizationWhere;
}

export interface Authorization {
  filter: readonly AuthorizationFilterRule[];
  validate: readonly AuthorizationValidateRule[];
  /** `bypass: false`: the schema's bypass does not skip these rules. */
  bypass?: boolean;
  /** `public:` operations deliberately open to every caller. */
  public?: ReadonlySet<AuthOperation>;
  /**
   * `mask:` on a scalar field: a row failing `unless` reads the field as
   * `value`, in projections and filters alike.
   */
  mask?: ReadonlyArray<{ unless: AuthorizationWhere; value: unknown }>;
}

export interface PageLimit {
  default: number;
  max: number;
}

export interface NodeType {
  name: string;
  /** First label is the primary label every MATCH uses. */
  labels: readonly string[];
  /** Plural used for root fields, e.g. `festivals`. */
  plural: string;
  fields: ReadonlyMap<string, Field>;
  key: ScalarField;
  read: boolean;
  aggregate: boolean;
  /** Generated mutations; empty unless `@mutation`. */
  mutations: ReadonlySet<MutationOperation>;
  /** Change events to subscribe to; empty unless `@subscription`. */
  subscriptions: ReadonlySet<MutationOperation>;
  /** `@subscription(relationships:, previousState:)`. */
  subscriptionOptions: { relationships: boolean; previousState: boolean };
  limit: PageLimit;
  /** Operations that need an authenticated request. */
  authentication: ReadonlySet<AuthOperation> | undefined;
  /** Claims `@authentication(jwt:)` requires, when set. */
  authenticationJwt: AuthorizationWhere | undefined;
  authorization: Authorization | undefined;
  /** Full-text and vector indexes with their search root fields. */
  search: readonly SearchIndex[];
  /** Interfaces the type implements. */
  interfaces: readonly string[];
  description: string | undefined;
}

export type SearchIndex =
  | {
      kind: "fulltext";
      name: string;
      fields: readonly ScalarField[];
      analyzer: "STANDARD" | "SIMPLE";
      /** Root field, e.g. `searchFestivals`. */
      queryName: string;
    }
  | {
      kind: "vector";
      name: string;
      field: ScalarField;
      dimensions: number;
      similarity: "COSINE" | "EUCLIDEAN";
      queryName: string;
    };

/** An interface over @node types, or a union of them. */
export interface AbstractType {
  kind: "interface" | "union";
  name: string;
  /** Concrete @node types: implementations or members. */
  members: readonly string[];
  /** An interface's scalar fields, as declared on the interface. */
  fields: ReadonlyMap<string, ScalarField>;
  /**
   * An interface's `@declareRelationship` fields: every implementation has
   * a relationship field of that name, target and shape.
   */
  relationships: ReadonlyMap<string, DeclaredRelationship>;
  plural: string;
  read: boolean;
  limit: PageLimit;
  description: string | undefined;
}

export interface DeclaredRelationship {
  name: string;
  target: string;
  list: boolean;
  description: string | undefined;
}

/**
 * An object type without @node, returned by @cypher fields as maps: its
 * fields are read from the map's keys.
 */
export interface PlainObjectType {
  name: string;
  fields: ReadonlyMap<string, { name: string; type: TypeShape }>;
  description: string | undefined;
}

export interface RelationshipPropertiesType {
  name: string;
  fields: ReadonlyMap<string, ScalarField>;
  description: string | undefined;
}

export interface EnumType {
  name: string;
  values: ReadonlyArray<{ name: string; description: string | undefined }>;
  description: string | undefined;
}

export interface GraphModel {
  nodes: ReadonlyMap<string, NodeType>;
  /** Interfaces and unions over @node types. */
  abstracts: ReadonlyMap<string, AbstractType>;
  enums: ReadonlyMap<string, EnumType>;
  relationshipProperties: ReadonlyMap<string, RelationshipPropertiesType>;
  /** `@cypher` fields declared on `Query` and `Mutation`. */
  queries: readonly CypherField[];
  mutations: readonly CypherField[];
  /** Problems that do not stop the model, e.g. an unused @cypher argument. */
  warnings: readonly ModelWarning[];
  /** Custom scalars (`@storedAs`), by name: their storage type. */
  scalars: ReadonlyMap<string, ScalarType>;
  /** Custom scalars' SDL descriptions, by name, when they have one. */
  scalarDescriptions: ReadonlyMap<string, string>;
  /** Object types without @node that @cypher fields return. */
  objects: ReadonlyMap<string, PlainObjectType>;
  /** The `@jwt` claims shape, when declared: claim name → token path. */
  jwt: ReadonlyMap<string, string> | undefined;
  /**
   * `@authorizationDefaults(bypass:)`: a claims-only test that, when a
   * request passes it, skips every filter and validate rule.
   */
  bypass: AuthorizationWhere | undefined;
  /** The `@viewer` claim: the caller's node type and identifying field. */
  viewer: { claim: string; type: string; field: string } | undefined;
  /** Secret cursors are signed with, when configured. */
  cursorSecret: string | undefined;
}

export interface ModelWarning {
  type: string;
  field?: string;
  message: string;
}

export function scalarFields(t: {
  fields: ReadonlyMap<string, Field>;
}): ScalarField[] {
  return [...t.fields.values()].filter(
    (f): f is ScalarField => f.kind === "scalar",
  );
}

export function cypherFields(t: NodeType): CypherField[] {
  return [...t.fields.values()].filter(
    (f): f is CypherField => f.kind === "cypher",
  );
}

export function relationshipFields(t: NodeType): RelationshipField[] {
  return [...t.fields.values()].filter(
    (f): f is RelationshipField => f.kind === "relationship",
  );
}
