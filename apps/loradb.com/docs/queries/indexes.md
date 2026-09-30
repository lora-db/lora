---
title: Indexes
sidebar_label: Indexes
description: How to create, inspect, query, and drop LoraDB indexes for range, text, point, lookup, vector, and full-text workloads.
---

# Indexes

LoraDB is still schema-free by default: labels, relationship types,
and property keys appear when you write them. Indexes are optional
catalog entries that tell the in-memory store which secondary
structures to maintain for frequently used predicates, vector search,
and full-text search.

## Create an index

<QueryCodeBlock code={String.raw`CREATE INDEX user_email FOR (u:User) ON (u.email);
CREATE INDEX user_age IF NOT EXISTS FOR (u:User) ON (u.age);
CREATE TEXT INDEX user_name FOR (u:User) ON (u.name);
CREATE POINT INDEX venue_location FOR (v:Venue) ON (v.location);
CREATE VECTOR INDEX doc_embedding FOR (d:Doc) ON (d.embedding)
OPTIONS {indexConfig: {\`vector.dimensions\`: 1536, \`vector.similarity_function\`: 'cosine'}};
CREATE FULLTEXT INDEX article_search FOR (a:Article) ON EACH [a.title, a.body];`} />

Relationship indexes use the relationship pattern form:

<QueryCodeBlock code={String.raw`CREATE INDEX rel_since FOR ()-[r:FOLLOWS]-() ON (r.since);
CREATE TEXT INDEX rel_note FOR ()-[r:TAGGED]-() ON (r.note);
CREATE POINT INDEX rel_location FOR ()-[r:DELIVERED]-() ON (r.location);
CREATE VECTOR INDEX rel_embedding FOR ()-[r:CONTAINS]-() ON (r.embedding)
OPTIONS {indexConfig: {\`vector.dimensions\`: 384, \`vector.similarity_function\`: 'euclidean'}};
CREATE FULLTEXT INDEX rel_summary FOR ()-[r:WROTE]-() ON EACH [r.summary];`} />

If you omit the name, LoraDB creates a deterministic `index_...` name:

<QueryCodeBlock code={String.raw`CREATE INDEX FOR (p:Product) ON (p.sku);`} />

Index names may also come from a string parameter:

<QueryCodeBlock code={String.raw`CREATE INDEX $name FOR (u:User) ON (u.email);`} />

## Index kinds

| Kind | Syntax | Useful predicates |
|---|---|---|
| RANGE | `CREATE INDEX ...` or `CREATE RANGE INDEX ...` | `=`, `<`, `<=`, `>`, `>=`, bounded ranges |
| TEXT | `CREATE TEXT INDEX ...` | `STARTS WITH`, `CONTAINS`, `ENDS WITH` |
| POINT | `CREATE POINT INDEX ...` | `geo.within_bbox(...)`, `geo.distance(...) <= radius` |
| LOOKUP | `CREATE LOOKUP INDEX ...` | Catalog-visible label/type token indexes |
| VECTOR | `CREATE VECTOR INDEX ... OPTIONS {indexConfig: {...}}` | `db.index.vector.queryNodes`, `db.index.vector.queryRelationships` |
| FULLTEXT | `CREATE FULLTEXT INDEX ... ON EACH [...]` | `db.index.fulltext.queryNodes`, `db.index.fulltext.queryRelationships` |

Lookup indexes are catalog entries over labels or relationship types:

<CypherSnippet code={String.raw`CREATE LOOKUP INDEX node_labels FOR (n) ON EACH node.labels(n);
CREATE LOOKUP INDEX rel_types FOR ()-[r]-() ON EACH edge.type(r);`} />

Composite RANGE indexes are accepted and shown in the catalog:

<QueryCodeBlock code={String.raw`CREATE INDEX person_age_country FOR (p:Person) ON (p.age, p.country);`} />

Current optimizer rewrites use single-property scopes. Keep composite
indexes for catalog policy and future planner work rather than expecting
multi-column seek behavior today.

## Vector indexes

Vector indexes are single-property node or relationship indexes. They
require an `indexConfig` map with:

- `vector.dimensions` - integer dimension in `1..=4096`;
- `vector.similarity_function` - `'cosine'`, `'euclidean'`, `'dot'`
  (alias `'dot_product'`), or `'manhattan'`.

<QueryCodeBlock code={String.raw`CREATE VECTOR INDEX movie_embedding
FOR (m:Movie)
ON (m.embedding)
OPTIONS {indexConfig: {
  \`vector.dimensions\`: 3,
  \`vector.similarity_function\`: 'cosine'
}};

CREATE (:Movie {title: 'A', embedding: [1.0, 0.0, 0.0]::VECTOR<FLOAT32>(3)});
CREATE (:Movie {title: 'B', embedding: [0.9, 0.1, 0.0]::VECTOR<FLOAT32>(3)});

CALL db.index.vector.queryNodes('movie_embedding', 2, [1.0, 0.0, 0.0])
YIELD node, score;`} />

The relationship procedure has the same shape but yields
`relationship`:

<QueryCodeBlock code={String.raw`CALL db.index.vector.queryRelationships('rel_embedding', 10, $query)
YIELD relationship, score;`} />

`k` must be positive. The query argument can be a `VECTOR`, a
`[...]::VECTOR<COORD>(DIM)` cast, a numeric list, or a parameter containing a vector.
Numeric lists are coerced to `FLOAT32` vectors. The query dimension
must match the index dimension. Results are sorted by descending score,
ties broken by ascending entity id.

### Restricting results

An optional fourth argument takes an options map. Its only key is
`restrictTo`, a list of entity ids (or node / relationship values) that
the results may come from:

<QueryCodeBlock code={String.raw`CALL db.index.vector.queryNodes('movie_embedding', 5, $query, {restrictTo: $ids})
YIELD node, score;`} />

Unknown option keys are rejected. When the `CALL` stands alone (nothing
after `YIELD`), the options must be an inline map literal — values
inside it may be parameters, as above, but `$opts` as the whole map is
rejected. A `CALL` followed by further clauses (`RETURN`, `WITH`, …)
accepts a map parameter too.

### Providers: flat (exact) and HNSW (approximate)

Each vector index has a backend, chosen with the optional
`vector.indexProvider` key:

| Provider | Behaviour |
|---|---|
| `'flat'` (default) | Exact. Scores every entity in the index scope and returns the true top `k`. |
| `'hnsw'` | Approximate nearest-neighbour search over an HNSW graph. Much faster on large scopes; may miss some true neighbours. |

<QueryCodeBlock code={String.raw`CREATE VECTOR INDEX doc_embedding_ann
FOR (d:Doc)
ON (d.embedding)
OPTIONS {indexConfig: {
  \`vector.dimensions\`: 384,
  \`vector.similarity_function\`: 'cosine',
  \`vector.indexProvider\`: 'hnsw',
  \`vector.hnsw.m\`: 16,
  \`vector.hnsw.ef_construction\`: 200,
  \`vector.hnsw.ef_search\`: 100
}};

CALL db.index.vector.queryNodes('doc_embedding_ann', 10, $query)
YIELD node, score;`} />

Queries are unchanged — the same `db.index.vector.*` procedures run
against either provider. HNSW tuning keys, all optional:

| Key | Default | Range | Effect |
|---|---|---|---|
| `vector.hnsw.m` | `16` | `4..=128` | Neighbours per node per layer. Higher improves recall and costs memory and insert time. |
| `vector.hnsw.ef_construction` | `200` | `16..=2000` | Candidate list size while building the graph. Higher gives a better graph and slower inserts. |
| `vector.hnsw.ef_search` | `100` | `16..=2000` | Candidate list size per query (never below `k`). Higher improves recall and slows queries. |
| `vector.hnsw.quantization` | `'none'` | `'none'`, `'int8'` | `'int8'` stores coordinates as 8-bit integers (about 4× less memory, slightly less precise scores). Requires `'cosine'` similarity and coordinates in `[-1, 1]` (unit-normalised embeddings); larger values are clipped. |

Out-of-range or mistyped values fail `CREATE VECTOR INDEX`, and any
provider other than `'flat'` or `'hnsw'` is rejected. The `vector.hnsw.*`
keys are validated for every vector index but only take effect with
`'hnsw'`. `SHOW INDEXES` lists them under `options`. A label or
relationship type and property can carry only one vector index, so
switching provider means `DROP INDEX` and creating it again.

With `restrictTo`, HNSW widens its search internally, but a very
selective filter can still return fewer than `k` hits; raise
`vector.hnsw.ef_search` or use a flat index for tightly filtered
queries. Snapshots carry the HNSW graph, so a restore returns the same
results as before; after a WAL recovery the graph is rebuilt
from the stored vectors.

### Lazy population

`vector.populate.async: true` skips the backfill at create time. The
index shows as `POPULATING` in `SHOW INDEXES` until the first query
against it builds it and flips it to `ONLINE`.

## Full-text indexes

Full-text indexes use `ON EACH [...]` and can cover multiple properties.
Node full-text indexes may cover multiple labels; relationship
full-text indexes may cover multiple relationship types:

<QueryCodeBlock code={String.raw`CREATE FULLTEXT INDEX article_search
FOR (a:Article|Note)
ON EACH [a.title, a.body]
OPTIONS {\`fulltext.analyzer\`: 'standard'};

CALL db.index.fulltext.queryNodes('article_search', 'graph search')
YIELD node, score;`} />

Relationship full-text search yields `relationship`:

<QueryCodeBlock code={String.raw`CREATE FULLTEXT INDEX wrote_search
FOR ()-[r:WROTE]-()
ON EACH [r.summary];

CALL db.index.fulltext.queryRelationships('wrote_search', 'graph')
YIELD relationship, score;`} />

Procedure calls return the yielded columns directly. The current
analyzer tokenizes by lowercasing and splitting on
non-alphanumeric characters. Multiple query terms use AND semantics:
all terms must be present. Scores are based on summed term frequency
and results are sorted by descending score.

A list property is indexed element by element: each string in the list
is tokenized and indexed, whether the list was written before or after
the index was created. Non-string list elements and non-string
properties are skipped silently, without an error:

<QueryCodeBlock code={String.raw`CREATE FULLTEXT INDEX article_tags FOR (a:Article) ON EACH [a.tags];

CREATE (:Article {title: 'Graphs', tags: ['graph databases', 'cypher', 42]});

CALL db.index.fulltext.queryNodes('article_tags', 'cypher')
YIELD node, score
RETURN node.title, score;`} />

`fulltext.analyzer` accepts `'standard'` and `'simple'`; unsupported
names are rejected. `fulltext.eventually_consistent` accepts a boolean
option, but index maintenance is currently synchronous.

## Inspect indexes

<QueryCodeBlock code={String.raw`SHOW INDEXES;`} />

Rows contain:

| Column | Meaning |
|---|---|
| `name` | Index name |
| `type` | `RANGE`, `TEXT`, `POINT`, or `LOOKUP` |
| `entityType` | `NODE` or `RELATIONSHIP` |
| `labelsOrTypes` | Label or relationship type scope, empty for lookup indexes |
| `properties` | Indexed property keys |
| `state` | Currently `ONLINE` for created indexes |
| `populationPercent` | `100.0` for online indexes |

`type` can be `RANGE`, `TEXT`, `POINT`, `LOOKUP`, `VECTOR`, or
`FULLTEXT`.

Use a type filter when you only want one kind:

<QueryCodeBlock code={String.raw`SHOW RANGE INDEXES;
SHOW TEXT INDEXES;
SHOW POINT INDEXES;
SHOW LOOKUP INDEXES;
SHOW VECTOR INDEXES;
SHOW FULLTEXT INDEXES;
SHOW ALL INDEXES;`} />

The singular spelling also works:

<QueryCodeBlock code={String.raw`SHOW RANGE INDEX;`} />

Catalog output can be shaped with a `YIELD`-anchored pipeline:

<QueryCodeBlock code={String.raw`SHOW INDEXES
YIELD name, type, entityType
WHERE type = 'VECTOR'
RETURN name
ORDER BY name
LIMIT 10;`} />

## Drop an index

<QueryCodeBlock code={String.raw`DROP INDEX user_email;
DROP INDEX maybe_missing IF EXISTS;`} />

Dropping a missing index without `IF EXISTS` returns a stable
GQLSTATUS-shaped error (`42N51`). Creating an index with a duplicate
name returns `22N71`; creating an equivalent index under a different
name returns `22N70`. `IF NOT EXISTS` turns either conflict into a
no-op.

Indexes owned by constraints cannot be dropped directly. Use
[`DROP CONSTRAINT`](./constraints#drop-constraints) for those.

## What the optimizer uses

Declared indexes can replace scan-and-filter plans with specialized
operators:

<QueryCodeBlock code={String.raw`CREATE INDEX person_age FOR (p:Person) ON (p.age);
CREATE TEXT INDEX person_name FOR (p:Person) ON (p.name);
CREATE POINT INDEX place_location FOR (p:Place) ON (p.location);`} />

Inspect the plan with your binding's `explain` method or HTTP
`POST /explain`. These queries should show the specialized scan names
in the returned plan tree:

<QueryCodeBlock code={String.raw`MATCH (p:Person) WHERE p.age >= 30 AND p.age < 50 RETURN p
;// NodeByPropertyRangeScan

MATCH (p:Person) WHERE p.name STARTS WITH 'Al' RETURN p
;// NodeByTextScan

MATCH (p:Place)
WHERE geo.within_bbox(
  p.location,
  {x: 0, y: 0}::POINT,
  {x: 100, y: 100}::POINT
)
RETURN p
// NodeByPointScan`} />

A WGS-84 bounding box whose lower-left longitude is greater than its
upper-right longitude crosses the antimeridian. The POINT index answers
it with two seeks, one for `[lowerLeft.longitude, 180]` and one for
`[-180, upperRight.longitude]`, for node and relationship indexes alike:

<QueryCodeBlock code={String.raw`MATCH (v:Venue)
WHERE geo.within_bbox(
  v.location,
  {longitude: 170, latitude: -50}::POINT,
  {longitude: -170, latitude: -30}::POINT
)
RETURN v`} />

See [`geo.within_bbox`](../functions/spatial#geowithin_bbox) for the full
semantics.

The same rewrite family exists for relationship scans when the pattern
can be satisfied from the relationship index:

<QueryCodeBlock code={String.raw`CREATE INDEX knows_since FOR ()-[r:KNOWS]-() ON (r.since);

MATCH ()-[r:KNOWS]->()
WHERE r.since > 2020
RETURN r;`} />

The original predicate still runs after the index candidate set is
produced. That keeps semantics correct for compound predicates and for
conservative TEXT/POINT candidate indexes.

## Durability

Index catalog changes are part of the normal write path. WAL-backed
databases replay `CREATE INDEX` and `DROP INDEX` events during recovery,
and snapshots include the index catalog trailer in the current body
format. Older snapshots without a catalog still load with an empty
index list.

## Limitations

- Vector indexes default to the exact `flat` provider; HNSW is opt-in
  via `vector.indexProvider: 'hnsw'` and returns approximate results.
- Full-text query strings use term intersection and term-frequency
  scoring, not a Lucene-style query language.
- Composite RANGE indexes are cataloged, but current planner rewrites
  are single-property.
- FULLTEXT indexes require `ON EACH [...]`; non-full-text indexes use
  `ON (...)`.

## See also

- [WHERE](./where) - predicates that can benefit from indexes.
- [Constraints](./constraints) - uniqueness, existence, keys, and type checks.
- [Spatial functions](../functions/spatial) - point predicates.
- [Vector values](../data-types/vectors) - storing and querying embeddings.
- [HTTP `POST /explain`](../api/http#post-explain) - inspect the
  physical plan from HTTP, or use the equivalent binding methods.
- [Limitations](../limitations) - remaining schema and storage gaps.
