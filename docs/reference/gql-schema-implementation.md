# GQL Schema Implementation Showcase

Created: 2026-05-03

This document focuses on the schema and catalog side of GQL as described by
[`gql.yml`](./gql.yml). It is intentionally separate from
[`gql-examples.md`](./gql-examples.md) because schema support deserves its own
implementation track: it affects parser shape, AST design, database catalog
state, validation, and eventually storage layout.

Important status note: the current LoraDB GQL parser work accepts a small
query/update subset. The schema/catalog syntax below is standard-oriented
implementation guidance and test material, not current executable behavior.

For an application-schema extension that adds custom scalars, enums,
interfaces, abstract nodes, policies, hooks, fulltext indexes, and deprecations,
see [`gql-extended-schema-language.md`](./gql-extended-schema-language.md) and
the full [`social-platform.gqlx`](./social-platform.gqlx) example.

## Relevant Grammar Surface

The schema-related rules in `gql.yml` center around these BNF definitions:

| Area | Representative rules |
| --- | --- |
| Session schema | `session set schema clause`, `schema reference`, `at schema clause` |
| Catalog schema DDL | `create schema statement`, `drop schema statement` |
| Graph DDL | `create graph statement`, `drop graph statement`, `open graph type`, `of graph type`, `graph source` |
| Graph type DDL | `create graph type statement`, `drop graph type statement`, `graph type source` |
| Graph type body | `nested graph type specification`, `graph type specification body`, `element type list` |
| Element types | `node type specification`, `edge type specification` |
| Labels and properties | `label set phrase`, `label set specification`, `property types specification`, `property type list` |
| Value types | `predefined type`, `constructed value type`, `dynamic union type`, `not null` |

In `gql.yml`, `create graph statement` is shaped as:

```text
CREATE [PROPERTY] GRAPH <open-or-of-graph-type> <graph-name> [graph-source]
```

The graph examples below follow that ordering.

## Catalog Model

GQL separates a catalog schema from a property graph:

- A **catalog schema** is a namespace.
- A **property graph** is a named graph object inside a schema.
- A **graph type** is a reusable schema contract for graph structure.
- A **node type** describes allowed labels and property types.
- An **edge type** describes labels, direction, endpoints, and property types.

A LoraDB implementation can model this as:

```rust
pub struct Catalog {
    pub schemas: BTreeMap<SchemaName, Schema>,
    pub current_schema: Option<SchemaName>,
}

pub struct Schema {
    pub graphs: BTreeMap<GraphName, GraphDefinition>,
    pub graph_types: BTreeMap<GraphTypeName, GraphTypeDefinition>,
}

pub enum GraphDefinition {
    Open,
    Typed(GraphTypeRef),
    Inline(GraphTypeDefinition),
    CopyOf(GraphRef),
    Like(GraphRef),
}

pub struct GraphTypeDefinition {
    pub elements: Vec<ElementTypeDefinition>,
}

pub enum ElementTypeDefinition {
    Node(NodeTypeDefinition),
    Edge(EdgeTypeDefinition),
}

pub struct NodeTypeDefinition {
    pub name: Option<String>,
    pub labels: Vec<String>,
    pub properties: BTreeMap<String, PropertyType>,
}

pub struct EdgeTypeDefinition {
    pub name: Option<String>,
    pub direction: EdgeDirection,
    pub labels: Vec<String>,
    pub source: NodeEndpoint,
    pub destination: NodeEndpoint,
    pub properties: BTreeMap<String, PropertyType>,
}

pub struct GraphIndexPolicy {
    pub label_indexes: bool,
    pub relationship_type_indexes: bool,
    pub adjacency_indexes: bool,
    pub property_indexes: Vec<PropertyIndexDefinition>,
}

pub enum PropertyIndexDefinition {
    Node {
        labels: Vec<String>,
        property: String,
        unique: bool,
    },
    Relationship {
        types: Vec<String>,
        property: String,
        unique: bool,
    },
    CompositeNode {
        labels: Vec<String>,
        properties: Vec<String>,
        unique: bool,
    },
}
```

That structure keeps the query AST language-neutral: schema commands can become
catalog statements, while query statements continue to lower into the canonical
query AST.

## Minimal Parser Split

The current parser facade already has a dialect slot. Schema support should add
catalog statements beside query statements rather than mixing catalog DDL into
query clauses.

```rust
pub enum Statement {
    Query(Query),
    Catalog(CatalogStatement),
    Session(SessionStatement),
    Transaction(TransactionStatement),
}

pub enum CatalogStatement {
    CreateSchema(CreateSchema),
    DropSchema(DropSchema),
    CreateGraph(CreateGraph),
    DropGraph(DropGraph),
    CreateGraphType(CreateGraphType),
    DropGraphType(DropGraphType),
}
```

Recommended parser phases:

1. Parse `CREATE SCHEMA` / `DROP SCHEMA`.
2. Parse `CREATE GRAPH ... ANY` and `DROP GRAPH`.
3. Parse `CREATE GRAPH TYPE ... AS { ... }`.
4. Parse `CREATE GRAPH ... TYPED graph_type`.
5. Add `SESSION SET SCHEMA` and `USE graph`.
6. Enforce graph type constraints during `INSERT`, `SET`, and `REMOVE`.

## Schema Namespace Examples

The YAML grammar describes schema names with an absolute directory path plus a
schema name. Practical examples:

```gql
CREATE SCHEMA IF NOT EXISTS /app;
```

```gql
CREATE SCHEMA IF NOT EXISTS /tenant/app;
```

```gql
DROP SCHEMA IF EXISTS /tenant/app;
```

```gql
SESSION SET SCHEMA /app;
```

```gql
SESSION RESET SCHEMA;
```

```gql
AT /app {
  MATCH (p:Person)
  RETURN p.name AS name
}
```

Implementation notes:

- Store schema names as normalized catalog paths.
- Keep a `current_schema` in session state.
- Resolve unqualified graph and graph type names through `current_schema`.
- Keep absolute paths stable for durable snapshots and WAL replay.

## Open Graph Examples

Open graph type syntax maps nicely to LoraDB's current schema-free graph model.

```gql
CREATE PROPERTY GRAPH ANY /app/social;
```

```gql
CREATE GRAPH ANY /app/scratch;
```

```gql
CREATE OR REPLACE GRAPH ANY /app/scratch;
```

```gql
DROP PROPERTY GRAPH IF EXISTS /app/scratch;
```

Suggested LoraDB meaning:

- `ANY` means no graph type constraints.
- Labels and properties remain dynamic.
- Existing Cypher-compatible behavior maps to this mode.

## Graph Type By Reference

A graph can be attached to a named graph type.

```gql
CREATE PROPERTY GRAPH TYPE /app/social_type AS {
  (:Person {id STRING NOT NULL, name STRING, age INTEGER}),
  (:Company {id STRING NOT NULL, name STRING}),
  (:Person)-[:WORKS_AT {since DATE, title STRING}]->(:Company)
};
```

```gql
CREATE PROPERTY GRAPH TYPED /app/social_type /app/social;
```

```gql
CREATE PROPERTY GRAPH /app/social_type /app/social;
```

Suggested LoraDB meaning:

- `/app/social_type` is stored once in the catalog.
- `/app/social` references the graph type.
- Inserts and updates are validated against referenced node and edge types.
- Dropping a referenced graph type should fail unless a force/drop-cascade mode
  is later added.

## Inline Graph Type Examples

The grammar permits graph type specifications inline.

```gql
CREATE PROPERTY GRAPH TYPED GRAPH {
  (:Person {id STRING NOT NULL, name STRING, age INTEGER}),
  (:Company {id STRING NOT NULL, name STRING}),
  (:Person)-[:WORKS_AT {since DATE}]->(:Company)
} /app/social;
```

```gql
CREATE PROPERTY GRAPH TYPED GRAPH {
  (:Station {code STRING NOT NULL, name STRING}),
  (:Station)-[:ROUTE {distance FLOAT64, duration DURATION(DAY TO SECOND)}]->(:Station)
} /app/routes;
```

Suggested LoraDB meaning:

- Inline graph types can be materialized as anonymous catalog-owned types.
- A stable internal id is better than storing only text, because the type will
  be needed by analyzer and storage validation.

## Graph Copy And Like Examples

GQL distinguishes copying a graph from typing a graph like another graph.

```gql
CREATE PROPERTY GRAPH ANY /app/social_archive
AS COPY OF /app/social;
```

```gql
CREATE PROPERTY GRAPH LIKE /app/social /app/social_shadow;
```

```gql
CREATE PROPERTY GRAPH TYPE /app/social_type_v2 LIKE /app/social;
```

```gql
CREATE GRAPH TYPE /app/social_type_copy COPY OF /app/social_type;
```

Suggested LoraDB meaning:

- `AS COPY OF graph` should duplicate graph data and metadata.
- `LIKE graph` should derive graph type shape from graph metadata, not copy data.
- `COPY OF graph type` should duplicate only the type definition.

## Pattern-Style Graph Type Body

Pattern-style graph type bodies are the most natural fit for LoraDB because
they resemble query and insert patterns.

```gql
CREATE GRAPH TYPE /app/people_type AS {
  (:Person {id STRING NOT NULL, name STRING, age INTEGER}),
  (:City {name STRING NOT NULL}),
  (:Person)-[:LIVES_IN]->(:City)
};
```

```gql
CREATE GRAPH TYPE /app/employment_type AS {
  (:Person {id STRING NOT NULL, name STRING}),
  (:Company {id STRING NOT NULL, name STRING}),
  (:Person)-[:WORKS_AT {since DATE, title STRING}]->(:Company)
};
```

```gql
CREATE GRAPH TYPE /app/package_type AS {
  (:Package {name STRING NOT NULL, version STRING NOT NULL}),
  (:Package)-[:DEPENDS_ON {scope STRING}]->(:Package)
};
```

```gql
CREATE GRAPH TYPE /app/content_type AS {
  (:Account {id STRING NOT NULL, handle STRING NOT NULL}),
  (:Post {id STRING NOT NULL, body STRING, tags LIST<STRING>}),
  (:Account)-[:WROTE {createdAt ZONED DATETIME}]->(:Post),
  (:Account)-[:LIKED {createdAt ZONED DATETIME}]->(:Post)
};
```

Implementation notes:

- Treat each node pattern as a `NodeTypeDefinition`.
- Treat each edge pattern as an `EdgeTypeDefinition`.
- Store endpoint constraints by matching node labels or by explicit type alias
  if present.
- Support pattern-style graph types before phrase-style syntax, because it
  reuses more parser machinery.

## Phrase-Style Graph Type Body

The grammar also contains phrase-style node and edge type definitions:
`NODE`, `VERTEX`, `EDGE`, `RELATIONSHIP`, `DIRECTED`, `UNDIRECTED`, and
`CONNECTING`.

Examples for an eventual parser:

```gql
CREATE GRAPH TYPE /app/phrase_people_type AS {
  NODE Person TYPE :Person {id STRING NOT NULL, name STRING},
  NODE City TYPE :City {name STRING NOT NULL},
  DIRECTED EDGE LIVES_IN :LIVES_IN CONNECTING (Person -> City)
};
```

```gql
CREATE GRAPH TYPE /app/phrase_employment_type AS {
  VERTEX Employee TYPE :Person {id STRING NOT NULL, name STRING},
  VERTEX Employer TYPE :Company {id STRING NOT NULL, name STRING},
  DIRECTED RELATIONSHIP WorksAt :WORKS_AT {since DATE, title STRING}
    CONNECTING (Employee -> Employer)
};
```

```gql
CREATE GRAPH TYPE /app/phrase_friend_type AS {
  NODE Person TYPE :Person {id STRING NOT NULL, name STRING},
  UNDIRECTED EDGE Knows :KNOWS {since DATE}
    CONNECTING (Person ~ Person)
};
```

Implementation notes:

- Phrase style needs explicit alias resolution inside the graph type body.
- `CONNECTING (A -> B)` should bind source and destination endpoint types.
- `CONNECTING (A ~ B)` should create an undirected endpoint constraint.
- Direction should be stored on the edge type, even if the runtime graph stores
  directed relationships internally.

## Property Type Examples

Graph type properties use GQL value types. The grammar includes scalar,
temporal, list, record, reference, and union-like dynamic types.

```gql
CREATE GRAPH TYPE /app/property_type_showcase AS {
  (:Thing {
    id STRING NOT NULL,
    slug VARCHAR(120),
    active BOOLEAN,
    retryCount INTEGER,
    score FLOAT64,
    created DATE,
    updated ZONED DATETIME,
    ttl DURATION(DAY TO SECOND),
    tags LIST<STRING>,
    metadata RECORD {source STRING, confidence FLOAT64}
  })
};
```

```gql
CREATE GRAPH TYPE /app/numeric_type_showcase AS {
  (:Metric {
    int8Value INT8,
    int64Value INT64,
    exactValue DECIMAL(18, 4),
    approximateValue FLOAT64,
    ratio DOUBLE PRECISION
  })
};
```

```gql
CREATE GRAPH TYPE /app/required_type_showcase AS {
  (:User {
    id STRING NOT NULL,
    email STRING NOT NULL,
    displayName STRING,
    verified BOOLEAN NOT NULL
  })
};
```

Implementation notes:

- First pass can support a smaller `PropertyType` enum:
  `String`, `Bool`, `Integer`, `Float`, `Date`, `DateTime`, `Duration`,
  `List(Box<PropertyType>)`, `Record`.
- Preserve unknown or unsupported types as parsed catalog metadata before
  enforcing them.
- `NOT NULL` should be enforced on `INSERT` and on property replacement.

## Label Constraints

The grammar models label sets with `LABEL`, `LABELS`, `:Label`, and
`IS Label` forms. For implementation, normalize all of them to a set of label
names.

```gql
CREATE GRAPH TYPE /app/label_type_showcase AS {
  (:Person {id STRING NOT NULL}),
  (IS Company {id STRING NOT NULL}),
  (LABEL City {name STRING NOT NULL}),
  (LABELS Person&Employee {employeeId STRING NOT NULL})
};
```

Suggested normalized form:

```text
NodeType(labels = ["Person"], properties = { id: String! })
NodeType(labels = ["Company"], properties = { id: String! })
NodeType(labels = ["City"], properties = { name: String! })
NodeType(labels = ["Person", "Employee"], properties = { employeeId: String! })
```

## Index Examples

I checked `gql.yml` for `INDEX`, `CREATE INDEX`, and `DROP INDEX` grammar. The
file does not define explicit index DDL. It only contains index-adjacent pieces:

- `sort key`, which is part of `ORDER BY`.
- `node type key label set` and `edge type key label set`, which use
  `IMPLIES` / `=>` in graph type definitions.
- `CONSTRAINT` and `UNIQUE` as pre-reserved words, not implemented statement
  rules in this grammar dump.

So for ISO-shaped GQL schema work, indexes should initially be an implementation
detail derived from graph data and graph type metadata, not a parsed GQL
catalog statement.

Current LoraDB already maintains useful physical indexes:

- Label index for `MATCH (n:Label)`.
- Relationship type index for typed relationship scans and expansions.
- Incoming/outgoing adjacency indexes for traversal.
- Hash property indexes for equality lookup on indexable node and relationship
  properties.

Schema support can make these indexes more deliberate by deriving an
`GraphIndexPolicy` from graph type definitions.

### Label Index Examples

Given this schema:

```gql
CREATE GRAPH TYPE /app/social_type AS {
  (:Person {id STRING NOT NULL, name STRING, age INTEGER}),
  (:Company {id STRING NOT NULL, name STRING}),
  (:City {id STRING NOT NULL, name STRING})
};
```

These queries should use the label index:

```gql
USE /app/social
MATCH (p:Person)
RETURN p
```

```gql
USE /app/social
MATCH (c:Company)
RETURN c.name AS company
```

```gql
USE /app/social
MATCH (city:City)
WHERE city.name = 'Amsterdam'
RETURN city
```

Implementation note: labels in graph type bodies should eagerly register label
index namespaces even before data exists. The actual index entries still come
from node creation and label mutation.

### Relationship Type And Adjacency Index Examples

Given this schema:

```gql
CREATE GRAPH TYPE /app/employment_type AS {
  (:Person {id STRING NOT NULL, name STRING}),
  (:Company {id STRING NOT NULL, name STRING}),
  (:Person)-[:WORKS_AT {since DATE, title STRING}]->(:Company),
  (:Person)-[:KNOWS {since DATE}]->(:Person)
};
```

These queries should use relationship type and adjacency indexes:

```gql
USE /app/employment
MATCH (p:Person)-[:WORKS_AT]->(c:Company)
RETURN p.name AS person, c.name AS company
```

```gql
USE /app/employment
MATCH (p:Person)<-[:KNOWS]-(friend:Person)
RETURN p, friend
```

```gql
USE /app/employment
MATCH (p:Person)-[:KNOWS]-(friend:Person)
RETURN p, friend
```

Implementation note: edge type definitions tell the planner which relationship
types and endpoint labels are legal. The storage layer still maintains the
physical adjacency sets automatically.

### Property Equality Index Examples

Graph type properties are good index candidates when they are frequently used
in equality predicates:

```gql
CREATE GRAPH TYPE /app/user_type AS {
  (:User {
    id STRING NOT NULL,
    email STRING NOT NULL,
    tenantId STRING NOT NULL,
    active BOOLEAN
  })
};
```

These predicates should use node property indexes:

```gql
USE /app/users
MATCH (u:User {id: 'u-100'})
RETURN u
```

```gql
USE /app/users
MATCH (u:User)
WHERE u.email = 'ada@example.com'
RETURN u
```

```gql
USE /app/users
MATCH (u:User)
WHERE u.tenantId = 'dream' AND u.active = true
RETURN u
```

Relationship property equality can use the relationship property index:

```gql
CREATE GRAPH TYPE /app/audit_type AS {
  (:User {id STRING NOT NULL}),
  (:Document {id STRING NOT NULL}),
  (:User)-[:VIEWED {requestId STRING, at ZONED DATETIME}]->(:Document)
};
```

```gql
USE /app/audit
MATCH (u:User)-[v:VIEWED]->(d:Document)
WHERE v.requestId = 'req-123'
RETURN u, d, v
```

Implementation note: LoraDB's current hash property indexes cover stable
equality values such as nulls, booleans, integers, non-NaN floats, strings, and
nested list/map values made from those types. Temporal, spatial, vector, and
NaN values should keep a scan fallback until the store has canonical hash keys
for them.

### Composite And Unique Index Candidates

`gql.yml` does not define explicit uniqueness or index statements, but graph
types can still guide future physical design.

```gql
CREATE GRAPH TYPE /app/multitenant_user_type AS {
  (:User {
    tenantId STRING NOT NULL,
    email STRING NOT NULL,
    displayName STRING,
    active BOOLEAN
  })
};
```

Useful implementation-level index candidates:

```text
NodePropertyIndex(labels = ["User"], property = "tenantId")
NodePropertyIndex(labels = ["User"], property = "email")
CompositeNodePropertyIndex(labels = ["User"], properties = ["tenantId", "email"], unique = true)
```

Queries that benefit from those candidates:

```gql
USE /app/users
MATCH (u:User)
WHERE u.tenantId = 'dream' AND u.email = 'ada@example.com'
RETURN u
```

```gql
USE /app/users
MATCH (u:User)
WHERE u.tenantId = 'dream'
RETURN u.email AS email
ORDER BY email
```

### Non-Standard Index DDL Sketch

The examples in this subsection are **not from `gql.yml`**. They are only a
possible LoraDB extension if explicit index management becomes necessary.

```gql
CREATE INDEX user_email_index
FOR (u:User)
ON (u.email);
```

```gql
CREATE UNIQUE INDEX user_tenant_email_index
FOR (u:User)
ON (u.tenantId, u.email);
```

```gql
CREATE INDEX viewed_request_index
FOR ()-[v:VIEWED]->()
ON (v.requestId);
```

```gql
DROP INDEX user_email_index;
```

If this extension is added, keep it separate from the ISO GQL grammar path and
gate it behind `QueryDialect::CypherCompat` or a future explicit Lora extension
dialect.

## Insert Validation Examples

Given:

```gql
CREATE GRAPH TYPE /app/social_type AS {
  (:Person {id STRING NOT NULL, name STRING, age INTEGER}),
  (:Company {id STRING NOT NULL, name STRING}),
  (:Person)-[:WORKS_AT {since DATE}]->(:Company)
};

CREATE PROPERTY GRAPH TYPED /app/social_type /app/social;
```

This should pass:

```gql
USE /app/social
INSERT (:Person {id: 'p1', name: 'Ada', age: 37})
FINISH
```

This should fail because `id` is required:

```gql
USE /app/social
INSERT (:Person {name: 'Ada'})
FINISH
```

This should fail because `age` expects an integer:

```gql
USE /app/social
INSERT (:Person {id: 'p1', name: 'Ada', age: 'thirty-seven'})
FINISH
```

This should pass:

```gql
USE /app/social
INSERT
  (:Person {id: 'p1', name: 'Ada'})
  -[:WORKS_AT {since: DATE '2020-01-01'}]->
  (:Company {id: 'c1', name: 'Dream'})
FINISH
```

This should fail because the edge endpoint does not match the graph type:

```gql
USE /app/social
INSERT
  (:Company {id: 'c1', name: 'Dream'})
  -[:WORKS_AT {since: DATE '2020-01-01'}]->
  (:Person {id: 'p1', name: 'Ada'})
FINISH
```

## Set And Remove Validation Examples

Given a typed graph:

```gql
USE /app/social
MATCH (p:Person {id: 'p1'})
SET p.age = 38
RETURN p
```

Should pass if `age` is an `INTEGER`.

```gql
USE /app/social
MATCH (p:Person {id: 'p1'})
SET p.age = 'old'
RETURN p
```

Should fail if `age` is an `INTEGER`.

```gql
USE /app/social
MATCH (p:Person {id: 'p1'})
REMOVE p.id
RETURN p
```

Should fail if `id` is `NOT NULL`.

```gql
USE /app/social
MATCH (p:Person {id: 'p1'})
SET p:Employee
RETURN p
```

Should pass only if the graph type allows the `Employee` label or LoraDB
chooses open-label semantics for typed graphs.

## Proposed Analyzer Responsibilities

Schema support should be split between catalog analysis and query analysis.

Catalog analyzer:

- Resolve schema, graph, and graph type names.
- Reject duplicate schemas, graphs, graph types, and element type names.
- Resolve graph type references.
- Validate edge endpoint references inside graph type bodies.
- Normalize labels, property names, value types, and direction.
- Derive index candidates from labels, relationship types, required keys, and
  frequently-addressed property declarations.

Query analyzer:

- Resolve current graph from `USE`, `SESSION SET GRAPH`, or database default.
- Attach graph type metadata to the query analysis context.
- Validate `INSERT` node labels and edge labels.
- Validate required properties.
- Validate property value types.
- Validate `SET`, `REMOVE`, and label mutation against typed graph rules.
- Prefer label/type/property index-backed scans when predicates are equality
  constraints over indexable values.

## Proposed Storage Responsibilities

The store does not need to understand all GQL grammar. It should receive already
normalized catalog metadata:

```rust
pub trait CatalogStore {
    fn create_schema(&mut self, schema: SchemaName) -> Result<()>;
    fn drop_schema(&mut self, schema: &SchemaName) -> Result<()>;
    fn create_graph_type(&mut self, name: GraphTypeName, ty: GraphTypeDefinition) -> Result<()>;
    fn create_graph(&mut self, name: GraphName, graph: GraphDefinition) -> Result<()>;
    fn graph_type_for_graph(&self, graph: &GraphName) -> Option<&GraphTypeDefinition>;
    fn index_policy_for_graph(&self, graph: &GraphName) -> Option<&GraphIndexPolicy>;
}
```

Keep graph data operations separate from catalog operations. Catalog changes
should be WAL-recorded and snapshotted alongside graph data so typed graph
validation remains deterministic after recovery.

Index metadata should also be deterministic after recovery. Physical index
contents can be rebuilt from graph records, but the index policy itself belongs
in catalog metadata.

## Parser Test Seed Cases

Start with syntax that is narrow, valuable, and maps directly to catalog state:

```gql
CREATE SCHEMA IF NOT EXISTS /app;
```

```gql
DROP SCHEMA IF EXISTS /app;
```

```gql
CREATE PROPERTY GRAPH ANY /app/social;
```

```gql
DROP PROPERTY GRAPH IF EXISTS /app/social;
```

```gql
CREATE GRAPH TYPE /app/social_type AS {
  (:Person {id STRING NOT NULL, name STRING}),
  (:Company {id STRING NOT NULL, name STRING}),
  (:Person)-[:WORKS_AT {since DATE}]->(:Company)
};
```

```gql
CREATE PROPERTY GRAPH TYPED /app/social_type /app/social;
```

```gql
CREATE PROPERTY GRAPH TYPED GRAPH {
  (:Person {id STRING NOT NULL, name STRING}),
  (:Person)-[:KNOWS]->(:Person)
} /app/social;
```

```gql
SESSION SET SCHEMA /app;
```

```gql
USE /app/social
MATCH (p:Person)
RETURN p
```

## Implementation Milestones

1. Add catalog/session statement variants to `lora-ast`.
2. Add GQL pest rules for catalog statements and graph type bodies.
3. Build a catalog analyzer that produces normalized catalog commands.
4. Add in-memory catalog state to `Database`.
5. WAL-record catalog mutations.
6. Add graph type metadata to snapshots.
7. Derive label/type/property index policy from graph type metadata.
8. Enforce `NOT NULL`, scalar property types, labels, and edge endpoints.
9. Expand value type enforcement to lists, records, temporal values, and unions.
10. Consider non-standard explicit index DDL only after automatic schema-backed
    indexes are working.

## Recommended First Cut

The smallest useful schema implementation for LoraDB is:

- `CREATE SCHEMA IF NOT EXISTS /name`
- `DROP SCHEMA IF EXISTS /name`
- `CREATE PROPERTY GRAPH ANY /schema/name`
- `DROP PROPERTY GRAPH IF EXISTS /schema/name`
- `CREATE GRAPH TYPE /schema/type AS { pattern-style body }`
- `CREATE PROPERTY GRAPH TYPED /schema/type /schema/name`
- `SESSION SET SCHEMA /schema`
- `USE /schema/graph`
- Automatic label, relationship type, adjacency, and property equality indexes
  derived from the graph type.

That gives LoraDB a real GQL catalog shape without forcing the full ISO grammar
into the first pass. It also preserves the current schema-free behavior by
mapping existing databases to one default open graph.
