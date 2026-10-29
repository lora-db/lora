// The directive vocabulary and built-in scalars, as SDL. Prepended to the
// user's typeDefs so graphql-js validates every directive usage (unknown
// directives, argument names and types) before the model is built.
// Exported so editors and codegen can load it next to `schema.graphql`.

export const directiveTypeDefs = /* GraphQL */ `
  "A node label set. Defaults to the type name."
  directive @node(labels: [String!], plural: String) on OBJECT

  "Natural key: required, unique and immutable; the tie-breaker of every sort and the anchor of cursors, global ids, updates and deletes. With generate: true, creates fill it with a UUID when the input leaves it out."
  directive @key(
    generate: Boolean = false
    "VIEWER: a created key must start with the caller's @viewer claim and the separator (lou:tomorrowland)."
    scope: KeyScope
    separator: String = ":"
  ) on FIELD_DEFINITION

  enum KeyScope {
    VIEWER
  }

  "On an interface field: every implementation declares this relationship (with @relationship, possibly of different types or directions), so clients can select it on the interface."
  directive @declareRelationship on FIELD_DEFINITION

  "Offer this field as a grouping key of <plural>Grouped (needs @query(aggregate: true))."
  directive @groupBy on FIELD_DEFINITION

  "A field computed in JavaScript by the resolver passed in the resolvers option. requires is a selection on this type (for example name capacity) fetched in the same statement, so the resolver reads it from its source."
  directive @customResolver(requires: String) on FIELD_DEFINITION

  "On a custom scalar: how its values are stored. Filters, sorts and indexes follow the storage type; the scalar is passed through unless the scalars option supplies an implementation."
  directive @storedAs(type: StorageType!) on SCALAR

  enum StorageType {
    STRING
    INT
    FLOAT
    BOOLEAN
    DATETIME
    DATE
  }

  "A uniqueness constraint."
  directive @unique on FIELD_DEFINITION

  "No two nodes of this type (among those matching where) share the values of fields: scalar fields, single relationships (by the target's @key) and at most one list relationship (by the set of target keys). Enforced by every generated mutation, for every caller."
  directive @uniqueTogether(
    fields: [String!]!
    where: NodeWhere
  ) repeatable on OBJECT

  "An explicit index. Usually unnecessary: inferred from @filterable and @sortable."
  directive @index(kind: IndexKind!) on FIELD_DEFINITION

  "An edge. The target type is the field's type."
  directive @relationship(
    type: String!
    direction: RelationshipDirection!
    properties: String
    "UNDIRECTED reads follow the relationship both ways; writes use direction."
    queryDirection: QueryDirection = DIRECTED
    "What deleting this node does to nodes reached through the field."
    onDelete: OnDelete = DETACH
    "Which nested writes mutation inputs offer for the field."
    nestedOperations: [NestedOperation!] = [
      CREATE
      CONNECT
      DISCONNECT
      UPDATE
      DELETE
    ]
    "false: no aggregate on the field's connection and no aggregate filter."
    aggregate: Boolean = true
  ) on FIELD_DEFINITION

  "UPDATE changes a connected node and its relationship properties in place; UPDATE_EDGE only the properties."
  enum NestedOperation {
    CREATE
    CONNECT
    DISCONNECT
    UPDATE
    UPDATE_EDGE
    DELETE
  }

  enum QueryDirection {
    DIRECTED
    UNDIRECTED
  }

  "DETACH removes the relationships; CASCADE deletes the related nodes too; RESTRICT refuses while any exist."
  enum OnDelete {
    DETACH
    CASCADE
    RESTRICT
  }

  "Which mutations may set the field. @readonly is onCreate: false, onUpdate: false."
  directive @settable(
    onCreate: Boolean = true
    onUpdate: Boolean = true
  ) on FIELD_DEFINITION

  "Whether the field can be read (onRead: false makes it write-only) or aggregated."
  directive @selectable(
    onRead: Boolean = true
    onAggregate: Boolean = true
  ) on FIELD_DEFINITION

  "Set by a callback passed to LoraGraphQL({ callbacks }) on these operations. Client-settable only with an explicit @settable(onCreate: true) or (onUpdate: true) and a field-level @authorization(validate:) rule for that operation."
  directive @populatedBy(
    callback: String!
    operations: [TimestampOperation!]! = [CREATE, UPDATE]
  ) on FIELD_DEFINITION

  "The shape of the request's claims. Rules may only test declared claims."
  directive @jwt on OBJECT

  "Where a declared claim lives in the token, e.g. app_metadata.roles."
  directive @jwtClaim(path: String!) on FIELD_DEFINITION

  "The claim that identifies the caller: the node of \`type\` whose \`field\` (a @key or @unique field) equals it. Enables isViewer and viewer in rules."
  directive @viewer(type: String!, field: String!) on FIELD_DEFINITION

  "Properties carried by a relationship type."
  directive @relationshipProperties on OBJECT

  "API name differs from the stored property."
  directive @alias(property: String!) on FIELD_DEFINITION

  "Stored, never exposed."
  directive @private on FIELD_DEFINITION

  "Declared upper bound on a relationship's fan-out, used by cost estimates."
  directive @cardinality(max: Int!) on FIELD_DEFINITION

  "How many items a list holds, with unique refusing the same item twice. On a list field: checked on every write. On a list argument of a @cypher field: checked before the statement runs (without max, maxListArgument applies). On a list @relationship field, max alone: the most items each nested connect, create, disconnect and update list of one input takes, checked before anything is written."
  directive @size(
    min: Int
    max: Int
    unique: Boolean
    "Replaces the default message of this rule."
    message: String
  ) on ARGUMENT_DEFINITION | FIELD_DEFINITION

  "Bounds of a number: min and max are inclusive, exclusiveMin and exclusiveMax are not. On an Int, Float or BigInt field (each item, for a list): checked on every write. On an argument of a @cypher field: checked before the statement runs."
  directive @range(
    min: Float
    max: Float
    exclusiveMin: Float
    exclusiveMax: Float
    "Replaces the default message of this rule."
    message: String
  ) on ARGUMENT_DEFINITION | FIELD_DEFINITION | SCALAR

  "How long a String or ID is, in Unicode code points (each item, for a list). On a custom scalar: for every field of that type."
  directive @length(
    min: Int
    max: Int
    "Replaces the default message of this rule."
    message: String
  ) on FIELD_DEFINITION | SCALAR | ARGUMENT_DEFINITION

  "An expression the whole value of a String or ID must match (each item, for a list). Needs @length(max:) on the same field."
  directive @pattern(
    regex: String!
    "Replaces the default message of this rule."
    message: String
  ) on FIELD_DEFINITION | SCALAR | ARGUMENT_DEFINITION

  "A named shape a String or ID must have: email, email-strict, url, uuid, hostname, ipv4, ipv6, slug, hex-color, ulid or e164."
  directive @format(
    as: String!
    "Replaces the default message of this rule."
    message: String
  ) on FIELD_DEFINITION | SCALAR | ARGUMENT_DEFINITION

  "Changes a String or ID before it is validated and stored (each item, for a list): trim removes surrounding whitespace, unicode normalises, case maps, in that order; emptyToNull then stores no value for an empty string. Opt in: nothing is changed without it."
  directive @normalize(
    trim: Boolean
    case: TextCase
    unicode: UnicodeForm
    emptyToNull: Boolean
  ) on FIELD_DEFINITION | SCALAR | ARGUMENT_DEFINITION

  enum TextCase {
    LOWER
    UPPER
  }

  enum UnicodeForm {
    NFC
    NFD
    NFKC
    NFKD
  }

  "An Int or Float must be a whole multiple of value (each item, for a list)."
  directive @multipleOf(
    value: Float!
    "Replaces the default message of this rule."
    message: String
  ) on FIELD_DEFINITION | SCALAR | ARGUMENT_DEFINITION

  "Generated read operations for a node type. Reads are on by default."
  directive @query(
    read: Boolean = true
    aggregate: Boolean = false
  ) on OBJECT | INTERFACE | UNION

  "The plural of an interface or union, for its root field."
  directive @plural(value: String!) on INTERFACE | UNION

  "Filter operators for a field. Without arguments: EQ and IN. On a relationship field: enables relationship filters."
  directive @filterable(byValue: [FilterOperator!]) on FIELD_DEFINITION

  """
  Sort and keyset-paginate on this field. \`nulls\` puts a nullable field's
  nulls first or last whichever the direction asked for (without it, Cypher's
  order: last ascending, first descending).
  """
  directive @sortable(nulls: SortNulls) on FIELD_DEFINITION

  "Where a @sortable field's nulls go."
  enum SortNulls {
    FIRST
    LAST
  }

  "Page size bounds for lists of this type."
  directive @limit(
    default: Int
    max: Int
  ) on OBJECT | INTERFACE | UNION | FIELD_DEFINITION

  "Expose an opaque global id derived from this @key field."
  directive @relayId on FIELD_DEFINITION

  """
  How long, in seconds, a result over this type or field may be served from a
  cache. An operation's policy (\`cacheControl\` on \`execute()\`'s result) is the
  smallest maxAge of the fields it selects, each field's own hint or else its
  type's; it has none when it selects a type or a scalar root field without one.
  """
  directive @cacheControl(
    maxAge: Int!
    scope: CacheScope = PUBLIC
  ) on OBJECT | INTERFACE | UNION | FIELD_DEFINITION

  "Who may share a cached result."
  enum CacheScope {
    "Any reader with the same variables."
    PUBLIC
    "Only the reader it was computed for."
    PRIVATE
  }

  "Full-text search over String fields: a FULLTEXT index and a search root field per entry."
  directive @fulltext(indexes: [FulltextIndex!]!) on OBJECT

  "A vector embedding ([Float!]): a VECTOR index and a similarity root field."
  directive @vector(
    dimensions: Int!
    similarity: VectorSimilarity = COSINE
    queryName: String
  ) on FIELD_DEFINITION

  input FulltextIndex {
    "Index name. Default: <label>_search for the first, required after."
    name: String
    fields: [String!]!
    analyzer: FulltextAnalyzer = STANDARD
    "Root field name. Default: search<Plural>, or search<Plural>By<Name>."
    queryName: String
    """
    A query word shorter than this matches as a whole word even when it ends
    in \`*\`: \`lo*\` becomes \`lo\`, so a two-letter prefix can't expand to
    most of the index. Default: every \`*\` word is a prefix.
    """
    minPrefixLength: Int
    """
    The most matches one query ranks: the index keeps its best \`maxMatches\`
    (by score) and \`where\`, \`sort\`, pages and \`totalCount\` apply within
    them. Bounds a broad query's time and memory, and its cost. Default: every
    match.
    """
    maxMatches: Int
  }

  enum FulltextAnalyzer {
    STANDARD
    SIMPLE
  }

  enum VectorSimilarity {
    COSINE
    EUCLIDEAN
  }

  "Generated subscriptions to changes made through the library. None without this directive."
  directive @subscription(
    operations: [MutationOperation!]! = [CREATE, UPDATE, DELETE]
    "Also send CONNECT and DISCONNECT events for the type's relationships."
    relationships: Boolean = false
    "Send the stored values before an update or delete (one extra read per write)."
    previousState: Boolean = false
  ) on OBJECT

  "A Cypher statement run inside the transaction of every generated mutation that creates, updates or deletes nodes of this type, before the commit. this is each written node; $jwt, $viewer and $operation are bound. Nodes it returns as created or updated join the mutation's write-set. Raw Cypher and @cypher mutations fire no trigger. With on: [CONNECT] or [DISCONNECT] and field: it runs once per relationship a mutation created or removed through that relationship (from either side), with this the node of this type and target the node at the other end."
  directive @trigger(
    on: [TriggerOperation!]!
    statement: String!
    "Fire an UPDATE trigger only for nodes whose update set one of these fields."
    fields: [String!]
    "The relationship field a CONNECT or DISCONNECT trigger fires for."
    field: String
  ) repeatable on OBJECT

  enum TriggerOperation {
    CREATE
    UPDATE
    DELETE
    "A relationship of field: was created."
    CONNECT
    "A relationship of field: was removed, both ends still there."
    DISCONNECT
  }

  "Nodes of this type are work for lora.worker(): each one created by a generated mutation or returned as created by a @trigger is handed to the worker's handler after the commit, at least once, with retries. The delivery state is stored on the node, outside the API."
  directive @outbox on OBJECT

  "Generated mutations for a node type. None without this directive."
  directive @mutation(
    operations: [MutationOperation!]! = [CREATE, UPDATE, DELETE]
  ) on OBJECT

  "Value stored on create when the input leaves the field out."
  directive @default(value: DefaultValue!) on FIELD_DEFINITION

  "Set to the current time by the listed operations. Client-settable only with an explicit @settable(onCreate: true) or (onUpdate: true) and a field-level @authorization(validate:) rule for that operation."
  directive @timestamp(
    operations: [TimestampOperation!]! = [CREATE, UPDATE]
  ) on FIELD_DEFINITION

  "Readable but never client-settable."
  directive @readonly on FIELD_DEFINITION

  "A field backed by a Cypher statement. \`this\` is the parent node; arguments are $parameters, $jwt holds the request's claims, and $viewer the caller's @viewer node key."
  directive @cypher(
    statement: String!
    columnName: String
    "Rows the statement examines each time it runs, charged against maxCost: once for a root field, once per parent node for a field of a type. Without it only the rows the field returns are charged."
    cost: Int
  ) on FIELD_DEFINITION

  "Require an authenticated request (a jwt in the context) for these operations."
  directive @authentication(
    operations: [AuthOperation!]! = [
      READ
      CREATE
      UPDATE
      DELETE
      CREATE_RELATIONSHIP
      DELETE_RELATIONSHIP
      SUBSCRIBE
    ]
    "Claims the request must also satisfy, such as a role in roles."
    jwt: AuthorizationWhere
  ) on OBJECT | FIELD_DEFINITION

  "Row-level rules over the node and the request's claims, compiled into the statements."
  directive @authorization(
    filter: [AuthorizationFilterRule!]
    validate: [AuthorizationValidateRule!]
    "On a type: false keeps the schema's bypass (see @authorizationDefaults) from skipping this type's rules; true lets it skip them, as without the argument, and tells check() that was meant."
    bypass: Boolean
    "On a @mutation type: operations deliberately open to every caller, so check() does not report them as unguarded."
    public: [AuthOperation!]
    "On a scalar field: a row failing unless reads the field as value (null when left out) instead of failing the request."
    mask: [AuthorizationMask!]
  ) on OBJECT | FIELD_DEFINITION

  "Named rules over claims, usable in any rule as { rule: name }, on \`extend schema\`."
  directive @authorizationRules(
    rules: [AuthorizationRuleDefinition!]!
  ) on SCHEMA

  input AuthorizationRuleDefinition {
    name: String!
    where: AuthorizationWhere!
  }

  "A named rule of this type, usable in its rules as { rule: name } and through a relationship to it as { rel: { rule: name } }."
  directive @authorizationRule(
    name: String!
    where: AuthorizationWhere!
  ) repeatable on OBJECT

  "Schema-wide authorization settings, on \`extend schema\`."
  directive @authorizationDefaults(
    "A claims-only test (jwt, AND, OR, NOT): a request passing it skips every filter and validate rule (not @authentication)."
    bypass: AuthorizationWhere
    "The write rule of every @mutation type that declares no CREATE, UPDATE or DELETE rule of its own."
    mutations: AuthorizationWhere
    "Default true: without a token every rule denies. false: a rule with a branch that reads no claims decides that branch for anonymous requests too; a rule that needs claims still asks for a token."
    requireAuthentication: Boolean
  ) on SCHEMA

  input AuthorizationMask {
    unless: AuthorizationWhere!
    value: DefaultValue
  }

  input AuthorizationFilterRule {
    operations: [AuthOperation!]! = [READ, UPDATE, DELETE]
    where: AuthorizationWhere!
  }

  input AuthorizationValidateRule {
    operations: [AuthOperation!]! = [READ, CREATE, UPDATE, DELETE]
    when: [AuthorizationWhen!]! = [BEFORE, AFTER]
    where: AuthorizationWhere!
  }

  enum MutationOperation {
    CREATE
    UPDATE
    DELETE
  }

  enum TimestampOperation {
    CREATE
    UPDATE
  }

  enum AuthOperation {
    READ
    CREATE
    UPDATE
    DELETE
    CREATE_RELATIONSHIP
    DELETE_RELATIONSHIP
    SUBSCRIBE
    "On a relationship field: creating one of its relationships (connect, nested create)."
    CONNECT
    "On a relationship field: removing one of its relationships (disconnect)."
    DISCONNECT
    "On a relationship field: setting properties on an existing relationship (edge update, re-connect)."
    UPDATE_EDGE
    "On a relationship field: reading, filtering, sorting or aggregating its properties."
    READ_EDGE
  }

  enum AuthorizationWhen {
    BEFORE
    AFTER
  }

  "Any literal."
  scalar DefaultValue

  "{ node: <Type>Where-shaped filter, jwt: claim filter, AND, OR, NOT }. String values starting with $jwt. are replaced by claims."
  scalar AuthorizationWhere

  "A <Type>Where-shaped filter over the type the directive is on."
  scalar NodeWhere

  enum IndexKind {
    RANGE
    TEXT
    POINT
  }

  enum RelationshipDirection {
    IN
    OUT
  }

  enum FilterOperator {
    EQ
    IN
    LT
    LTE
    GT
    GTE
    CONTAINS
    STARTS_WITH
    ENDS_WITH
    WITHIN_BBOX
    DISTANCE
    INCLUDES
    IS_NULL
    CASE_INSENSITIVE
  }

  scalar BigInt
  scalar Date
  scalar Time
  scalar LocalTime
  scalar DateTime
  scalar LocalDateTime
  scalar Duration

  type Point {
    longitude: Float!
    latitude: Float!
    height: Float
    srid: Int!
    crs: String!
  }

  type CartesianPoint {
    x: Float!
    y: Float!
    z: Float
    srid: Int!
    crs: String!
  }
`;

/** Names defined by the prelude; never treated as user types. */
export const PRELUDE_TYPES = new Set([
  "KeyScope",
  "TextCase",
  "UnicodeForm",
  "AuthorizationMask",
  "AuthorizationRuleDefinition",
  "QueryDirection",
  "OnDelete",
  "FulltextIndex",
  "FulltextAnalyzer",
  "CacheScope",
  "VectorSimilarity",
  "MutationOperation",
  "TimestampOperation",
  "AuthOperation",
  "AuthorizationWhen",
  "AuthorizationFilterRule",
  "AuthorizationValidateRule",
  "DefaultValue",
  "AuthorizationWhere",
  "NodeWhere",
  "IndexKind",
  "RelationshipDirection",
  "FilterOperator",
  "BigInt",
  "Date",
  "Time",
  "LocalTime",
  "DateTime",
  "LocalDateTime",
  "Duration",
  "Point",
  "CartesianPoint",
]);
