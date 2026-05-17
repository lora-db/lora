---
title: Built-in Functions in LoraDB
sidebar_label: Overview
description: How LoraDB functions work — namespaces, compatibility aliases, casts, null and error behavior, aggregate behavior, and links to each per-category reference.
---

# Built-in Functions in LoraDB

Functions are expressions. You can use them anywhere an expression is
accepted: in `RETURN`, `WITH`, `WHERE`, `ORDER BY`, `SET`, map
projections, list comprehensions, and nested inside other function
calls.

<QueryCodeBlock code={String.raw`MATCH (d:Doc)
WITH d,
     vector.similarity(d.embedding, $query) AS score,
     temporal.truncate('month', d.published_at) AS month
WHERE score >= 0.75
RETURN d.title, month, score
ORDER BY score DESC
LIMIT 10`} />

Function names are **case-insensitive**. Canonical LoraDB functions are
mostly namespaced, such as `string.lower`, `math.sqrt`, `list.range`,
`temporal.now`, `geo.distance`, `vector.similarity`, `type.of`, and
`cast.try`. A small set of historical and convenience aliases remains
for common Cypher-style helpers and zero-argument value helpers, such as
`toInteger`, `toString`, `size`, `head`, `length`, `coalesce`, and
`now`.

The analyzer resolves every function call before execution. That means
unknown names, wrong argument counts, unknown type literals, and invalid
enum literals such as vector metric names are rejected before rows are
scanned. The executor then evaluates only validated calls.

Most functions **propagate `null`** — any `null` argument makes the
result `null`. The exceptions are aggregates, `coalesce`, current-time
helpers, and constants/random helpers.

## Function expression basics

Functions do not introduce a new query clause. They are part of the
expression language, so they evaluate once for each row flowing through
the clause that contains them.

<QueryCodeBlock code={String.raw`MATCH (u:User)
WITH u, string.lower(u.email) AS email
WHERE email ENDS WITH '@loradb.com'
RETURN u.name, email
ORDER BY email`} />

In that query:

- `string.lower(u.email)` runs for each matched `User` row.
- `WITH` gives the result a name so later clauses can reuse it.
- `WHERE`, `RETURN`, and `ORDER BY` all see the shaped row from `WITH`.

Functions are side-effect free. They cannot create nodes, update
properties, query indexes by themselves, or change the graph. Graph
writes still happen through clauses such as `CREATE`, `MERGE`, `SET`,
and `DELETE`; functions only compute values for those clauses to use.

## Reading signatures and examples

Function pages use compact signatures:

| Notation | Meaning |
|---|---|
| `fn(x)` | one required argument |
| `fn(x[, y])` | `y` is optional |
| `fn(a, b, ...)` | accepts a variable number of arguments |
| `LIST<T>` | a list whose elements are usually the same type |
| `null → null` | a null input returns null instead of an error |

Examples are written as complete Cypher fragments whenever possible.
Comments show representative results, not a promise about exact display
format in every host binding.

<QueryCodeBlock code={String.raw`RETURN string.slice('loradb', 0, 4);     // 'lora'
RETURN '2024-01-15'::DATE;               // DATE value
RETURN TRY_CAST('not a date' AS DATE)   // null`} />

## How functions are organised

LoraDB keeps value construction and value operations separate:

| Use case | Preferred syntax | Why |
|---|---|---|
| Build a typed value in query text | <CypherCode code="'2024-01-15'::DATE" />, <CypherCode code="{x: 1, y: 2}::POINT" />, <CypherCode code="[1,2,3]::VECTOR<INTEGER>(3)" /> | Casts make the target type explicit. |
| Convert with CAST syntax | <CypherCode code="CAST(value AS TYPE)" /> | First-class Cypher grammar for strict conversion. |
| Convert without throwing | <CypherCode code="TRY_CAST(value AS TYPE)" /> | Nullable cast; returns `null` when conversion fails. |
| Ask about a value's type | <CypherCode code="type.of(value)" /> | Returns tags such as `"DATE"` or `"VECTOR<FLOAT32>(384)"`. |
| Work with temporal values | <CypherCode code="temporal.now()" />, <CypherCode code="temporal.truncate('month', d)" /> | Current-time helpers and temporal operations. |
| Work with points | <CypherCode code="geo.distance(a, b)" />, <CypherCode code="geo.within_bbox(p, ll, ur)" /> | Spatial operations over existing `POINT` values. |
| Work with vectors | <CypherCode code="vector.similarity(a, b)" />, <CypherCode code="vector.distance(a, b, EUCLIDEAN)" /> | Vector math over existing `VECTOR` values. |

The important distinction: `temporal.*`, `geo.*`, and `vector.*` are
operation namespaces. They are not the default way to construct
`DATE`, `POINT`, or `VECTOR` values in query text.

<QueryCodeBlock code={String.raw`RETURN '2024-01-15'::DATE,
       CAST('2024-01-15' AS DATE),
       TRY_CAST($maybe_date AS DATE)`} />

Use host-language helpers separately when binding parameters from
Node, Python, Go, Ruby, WASM, or Rust. For example, a Node query may
bind `{ at: datetime("2026-05-01T09:00:00Z") }`, but the equivalent
query-text literal is `'2026-05-01T09:00:00Z'::DATETIME`.

### Choosing a construction form

Use the form that best matches where the value comes from:

| Source value | Recommended query syntax | Notes |
|---|---|---|
| Literal ISO date/time text | <CypherCode code="'2026-05-01'::DATE" /> | Most readable for handwritten queries. |
| Literal map point | <CypherCode code="{longitude: 4.89, latitude: 52.37}::POINT" /> | Lets the `POINT` cast validate CRS/SRID rules. |
| Literal numeric vector | <CypherCode code="[0.1, 0.2, 0.3]::VECTOR<FLOAT32>(3)" /> | Dimension and coordinate type are visible at the use site. |
| Parameter that may be invalid | <CypherCode code="TRY_CAST($value AS DATE)" /> | Returns `null`, so pair it with `IS NOT NULL` or `coalesce`. |
| Generated query text | <CypherCode code="CAST($value AS DATE)" /> | Strict conversion using the Cypher grammar. |

Strict casts are useful when bad input should stop the query. `TRY_CAST`
is better for ingestion and optional filters, where a failed conversion
should become `null` and be handled by normal Cypher predicates.

The older constructor-shaped temporal, spatial, and vector forms are not
the preferred public query syntax. Use casts in query text, and use
client-library binding helpers only when creating parameters in a host
language.

## Calling functions

A function call has three parts: the name, the arguments, and sometimes
a literal type or option argument.

<QueryCodeBlock code={String.raw`RETURN string.lower('LORA') AS name,
       math.round(3.14159, 2) AS rounded,
       vector.distance($a, $b, EUCLIDEAN) AS distance,
       TRY_CAST($raw AS DATE) AS maybe_date`} />

Namespaced functions are ordinary expression functions. They do not
require `CALL`, do not stream rows by themselves, and can be nested
inside other expressions:

<QueryCodeBlock code={String.raw`MATCH (d:Doc)
WHERE type.of(d.published_at) = 'DATE'
RETURN string.upper(coalesce(d.title, 'untitled')) AS title,
       temporal.truncate('year', d.published_at) AS year`} />

Some arguments are not values from the graph; they are compile-time
symbols:

| Symbol kind | Used by | Example |
|---|---|---|
| Type literal | casts and type checks | <CypherCode code="TRY_CAST($x AS INTEGER)" />, <CypherCode code="type.is($x, DATE)" /> |
| Vector metric | vector distance and norm | <CypherCode code="vector.distance(a, b, EUCLIDEAN)" />, <CypherCode code="vector.norm(v, MANHATTAN)" /> |

Vector metrics can also be supplied as strings when that is more
convenient for generated queries. If the target type itself must be
dynamic, use the lower-level `cast.to` / `cast.try` helpers with a type
string:

<QueryCodeBlock code={String.raw`RETURN cast.try($raw, $type_name) AS maybe_date,
       vector.distance($a, $b, 'euclidean') AS distance`} />

## Namespaces and aliases

Canonical names are deliberately explicit:

| Namespace | Contains | Examples |
|---|---|---|
| `string.*` | string case, search, tokenizing, slicing, regex, encoding | `string.lower(x)`, `string.words(x)`, `string.count(x, needle)` |
| `math.*` | arithmetic helpers, trig, constants, random | `math.abs(x)`, `math.hypot(a, b)`, `math.random()` |
| `number.*` / `bits.*` | numeric formatting, base conversion, numeric predicates, bit operations | `number.format(n, 2)`, `number.to_base(n, 16)`, `bits.and(a, b)` |
| `list.*` | list construction, indexing, and transforms | `list.range(1, 10)`, `list.at(xs, -1)`, `list.take_last(xs, 3)` |
| `map.*` | map lookup, patching, projection, deep merge, nested paths, entries, flattening | `map.get(m, 'k')`, `map.get_path(m, 'a.b')`, `map.deep_merge(a, b)` |
| `temporal.*` | current time, truncation, differences, fields | `temporal.now()`, `temporal.today()`, `temporal.between(a, b)` |
| `geo.*` | spatial predicates and distances | `geo.distance(a, b)`, `geo.within_bbox(p, ll, ur)` |
| `vector.*` | similarity, distance, norms, vector shape | `vector.similarity(a, b)`, `vector.dimension(v)` |
| `text.*` | string distance, similarity, and phonetic helpers | `text.distance(a, b, 'levenshtein')`, `text.phonetic(s, 'soundex')` |
| `bytes.*` / `crypto.*` | byte conversion, compression, and hashes | `bytes.base64_encode(x)`, `crypto.blake3(x)` |
| `uuid.*` / `json.*` | UUID generation/validation and JSON encoding/path access | `uuid.new()`, `json.path(x, '$.a[0]')` |
| `path.*` | path decomposition and path length | `path.nodes(p)`, `path.edges(p)`, `path.length(p)` |
| `node.*` / `edge.*` | node and relationship introspection | `node.labels(n)`, `edge.type(r)` |
| `type.*` | runtime type inspection and checks | `type.of(x)`, `type.is(x, DATE)` |
| `cast.*` | explicit conversion helpers | `cast.to(x, DATE)`, `cast.try(x, INTEGER)` |
| `value.*` | polymorphic value helpers | `value.size(x)`, `value.keys(x)`, `value.coalesce(a, b)` |

Compatibility aliases exist for common Cypher spellings:

| Alias | Canonical behavior |
|---|---|
| `toString(x)` | `x::STRING` / `CAST(x AS STRING)` |
| `toInteger(x)` | `x::INTEGER` / `CAST(x AS INTEGER)` |
| `toIntegerOrNull(x)` | `TRY_CAST(x AS INTEGER)` |
| `size(x)` | `value.size(x)` |
| `head(xs)` / `last(xs)` | list first / last helpers |
| `id(x)`, `labels(n)`, `type(r)` | entity introspection helpers |
| `now()` | `temporal.now()` |
| `timestamp()` | `temporal.timestamp()` |
| `timezone()` | `temporal.timezone()` |
| `new()` | `uuid.new()` |
| `random()` | `math.random()` |

Prefer canonical names in new documentation when the namespace clarifies
intent. Use aliases where they are the familiar Cypher form and already
documented on the category page.

## Category guide

The function library is intentionally small. Each category covers one
kind of value work:

| Category | Use it for | Good first function |
|---|---|---|
| Aggregation | Collapsing many rows into grouped summaries | `count(*)` |
| String | Cleaning, slicing, parsing, and comparing text | `string.trim`, `string.lower` |
| Math | Numeric formulas and random/constants | `math.round`, `math.sqrt` |
| Number | Formatting, radix conversion, integer predicates, and bit helpers | `number.format`, `number.to_base` |
| List | Selecting, reshaping, and reducing list values | `value.size`, `list.range` |
| Temporal | Current time, calendar math, truncation, date differences | `temporal.now()` |
| Spatial | Point distance, bounding boxes, CRS-aware component access | `geo.distance` |
| Vector | Embedding similarity, distance metrics, vector introspection | `vector.similarity` |
| Entity/value | Inspecting graph values, maps, and runtime types | `value.properties`, `type.of` |

If a task needs graph traversal, start with a pattern in `MATCH`. If it
needs row grouping, start with aggregation. If it needs to reshape a
single value already in the row, reach for a scalar function.

<QueryCodeBlock code={String.raw`// Traversal first, then scalar functions, then aggregation.
MATCH (u:User)-[:POSTED]->(p:Post)
WITH u, temporal.truncate('month', p.created_at) AS month
RETURN u.id, month, count(*) AS posts
ORDER BY month`} />

## Errors and nulls

For a single expression, LoraDB distinguishes validation failures,
runtime conversion failures, ordinary null propagation, and aggregate
semantics.

### Validation errors

These are rejected while the query is analyzed:

- unknown function names
- wrong argument counts
- invalid type names in `CAST`, `TRY_CAST`, `cast.to`, or `cast.try`
- invalid vector metrics in `vector.distance` or `vector.norm`

For example, `vector.distance(a, b)` is invalid because distance needs a
metric, and `vector.distance(a, b, BOGUS)` is invalid because `BOGUS` is
not a supported metric.

### Conversion errors

| Situation | Result |
|---|---|
| Unknown function or wrong argument count | analysis error before execution |
| Invalid strict conversion, such as <CypherCode code="'bad'::DATE" /> | runtime error |
| Nullable conversion, such as <CypherCode code="TRY_CAST('bad' AS DATE)" /> | `null` |

Strict casts are for inputs that must be valid. Nullable casts are for
ingestion, optional filters, and user-supplied values where invalid data
should simply disappear from the result:

<QueryCodeBlock code={String.raw`UNWIND $rows AS row
WITH row, TRY_CAST(row.signup_date AS DATE) AS signup_date
WHERE signup_date IS NOT NULL
CREATE (:Signup {email: row.email, signup_date: signup_date})`} />

### Null propagation

Most non-aggregate functions follow normal null propagation:

<QueryCodeBlock code={String.raw`RETURN geo.distance(null, {x: 1, y: 2}::POINT),       // null
       vector.similarity(null, [1, 2, 3]::VECTOR<INTEGER>(3)), // null
       temporal.truncate('month', null),              // null
       coalesce(null, 'fallback')                     // 'fallback'`} />

That behavior is intentional: missing input usually means missing output,
and the query can decide how to handle it with `IS NULL`, `IS NOT NULL`,
`coalesce`, or `CASE`.

### Aggregate behavior

Aggregates are different because they operate over groups of rows:
`count(*)` counts rows, `count(x)` counts non-null `x`, and functions
such as `sum`, `avg`, `min`, and `max` skip null inputs.

<QueryCodeBlock code={String.raw`MATCH (p:Person)
RETURN count(*) AS people,
       count(p.email) AS people_with_email,
       avg(p.age) AS average_known_age`} />

### Filtering after nullable functions

`null` is not equal to anything, including itself. When you use
`TRY_CAST` or a function that can return `null`, filter with `IS NULL`
or `IS NOT NULL`.

<QueryCodeBlock code={String.raw`UNWIND $rows AS row
WITH row, TRY_CAST(row.started_at AS DATETIME) AS started_at
WHERE started_at IS NOT NULL
CREATE (:Event {id: row.id, started_at: started_at})`} />

For default values, use `coalesce`:

<QueryCodeBlock code={String.raw`RETURN coalesce(TRY_CAST($limit AS INTEGER), 100) AS limit`} />

## Categories

| Category | Examples | Reference |
|---|---|---|
| **Aggregation** | <CypherCode code="count" />, <CypherCode code="sum" />, <CypherCode code="collect" />, <CypherCode code="percentileCont" /> | [Aggregation](./aggregation) |
| **String** | <CypherCode code="string.lower" />, <CypherCode code="string.words" />, <CypherCode code="string.count" />, <CypherCode code="string.normalize" /> | [String](./string) |
| **Math** | <CypherCode code="math.abs" />, <CypherCode code="math.hypot" />, <CypherCode code="math.log_base" />, <CypherCode code="math.random" /> | [Math](./math) |
| **Number** | <CypherCode code="number.format" />, <CypherCode code="number.to_base" />, <CypherCode code="number.from_base" />, <CypherCode code="bits.and" /> | [Number](./number) |
| **List** | <CypherCode code="value.size" />, <CypherCode code="list.at" />, <CypherCode code="list.take_last" />, <CypherCode code="reduce" /> | [List](./list) |
| **Map** | <CypherCode code="map.get" />, <CypherCode code="map.get_path" />, <CypherCode code="map.pick" />, <CypherCode code="map.deep_merge" />, <CypherCode code="map.entries" /> | [Map](./map) |
| **Temporal** | <CypherCode code="temporal.now" />, <CypherCode code="temporal.today" />, <CypherCode code="temporal.between" />, <CypherCode code="'2024-01-01'::DATE" /> | [Temporal](./temporal) |
| **Spatial** | <CypherCode code="{x: 1, y: 2}::POINT" />, <CypherCode code="geo.distance" /> | [Spatial](./spatial) |
| **Vector** | <CypherCode code="[1,2,3]::VECTOR<INTEGER>(3)" />, <CypherCode code="vector.similarity" />, <CypherCode code="vector.distance" />, <CypherCode code="vector.norm" />, <CypherCode code="vector.dimension" />, <CypherCode code="vector.coordinates" /> | [Vector](./vectors) |
| **Utility** | <CypherCode code="type.of" />, <CypherCode code="json.path" />, <CypherCode code="text.distance" />, <CypherCode code="bytes.base64_encode" />, <CypherCode code="uuid.new" /> | [Utility](./utility) |
| **Path** | <CypherCode code="path.length" />, <CypherCode code="path.nodes" />, <CypherCode code="path.edges" /> | [Paths](../queries/paths) |

The category pages are the detailed references. This page explains the
shared rules that apply across categories.

## Entity introspection

| Common alias | Canonical helper | Takes | Returns |
|---|---|---|---|
| <CypherCode code="id(x)" /> | <CypherCode code="value.id(x)" /> | node \| relationship | `Int` — internal id |
| <CypherCode code="labels(n)" /> | <CypherCode code="node.labels(n)" /> | node | `List<String>` |
| <CypherCode code="type(r)" /> | <CypherCode code="edge.type(r)" /> | relationship | `String` — rel type |
| <CypherCode code="keys(x)" /> | <CypherCode code="value.keys(x)" /> | node \| rel \| map | `List<String>` |
| <CypherCode code="properties(x)" /> | <CypherCode code="value.properties(x)" /> | node \| rel \| map | `Map` |

<QueryCodeBlock code={String.raw`MATCH (u:User)-[r:FOLLOWS]->(v:User)
RETURN id(u), labels(u), type(r), keys(u), properties(u)`} />

### Common uses

<QueryCodeBlock code={String.raw`// Dump every property on a node as a map
MATCH (u:User {id: $id}) RETURN properties(u)

;// Discover which labels a node carries
MATCH (n) WHERE id(n) = $raw_id RETURN labels(n)

;// Inspect the type of a matched edge
MATCH (a)-[r]->(b) RETURN type(r), count(*) ORDER BY count(*) DESC

;// Avoid duplicate pair rows
MATCH (a)-[:KNOWS]-(b) WHERE id(a) < id(b) RETURN a, b`} />

See [**Graph model → Identity**](../concepts/graph-model#identity) for
why `id()` is opaque.

Use aliases when writing familiar Cypher-style queries. Use canonical
helpers when you want the type of value being inspected to be obvious in
mixed examples or generated query text.

## Type conversion and checking

| Function | Behaviour |
|---|---|
| <CypherCode code="toString(x)" /> | any → `String`; `null` → `null` |
| <CypherCode code="toInteger(x)" /> / <CypherCode code="toIntegerOrNull(x)" /> | `Int`/`Float`/`String`/`Bool` → `Int`; `OrNull` form suppresses conversion errors |
| <CypherCode code="toFloat(x)" /> / <CypherCode code="toFloatOrNull(x)" /> | `Int`/`Float`/`String` → `Float`; `OrNull` form suppresses conversion errors |
| <CypherCode code="toBoolean(x)" /> / <CypherCode code="toBooleanOrNull(x)" /> | `Bool`/`String`/`Int` → `Bool`; `OrNull` form suppresses conversion errors |
| <CypherCode code="x::TYPE" /> | preferred explicit cast syntax |
| <CypherCode code="CAST(x AS TYPE)" /> | strict cast syntax |
| <CypherCode code="TRY_CAST(x AS TYPE)" /> | nullable cast; returns `null` on invalid input |
| <CypherCode code="type.of(x)" /> | name of the value's type, e.g. `"INTEGER"`, `"LIST<T>"` |
| <CypherCode code="coalesce(a, b, …)" /> | first non-null argument |
| <CypherCode code="temporal.timestamp()" /> / <CypherCode code="timestamp()" /> | current Unix time in milliseconds |

<QueryCodeBlock code={String.raw`RETURN toInteger('42'),                         // 42
       toIntegerOrNull('abc'),                  // null
       '2024-01-15'::DATE,                      // DATE
       TRY_CAST('bad date' AS DATE),             // null
       toFloat(42),                             // 42.0
       coalesce(null, null, 'fallback'),        // 'fallback'
       type.of(1),                            // 'INTEGER'
       type.of([1, 2, 3]),                    // 'LIST<INTEGER>'
       type.of('2024-01-15'::DATE)            // 'DATE'`} />

### coalesce recipes

<QueryCodeBlock code={String.raw`// Default a missing property
MATCH (p:Person) RETURN p.name, coalesce(p.nickname, p.name) AS display

;// Cascade through several optional fields
RETURN coalesce($phone, $email, 'unknown') AS contact

;// Replace null in ordering
MATCH (p:Person)
RETURN p.name, coalesce(p.rank, 999999) AS rank_for_sort
ORDER BY rank_for_sort`} />

For multi-branch logic with arbitrary predicates per branch (not just
"first non-null"), use [`CASE`](../queries/return-with#case-expressions).

### type.of recipes

<QueryCodeBlock code={String.raw`// Filter a heterogeneous list to numbers only
MATCH (n)
WHERE all(x IN n.values WHERE type.of(x) = 'INTEGER')
RETURN n

;// Group by runtime type
UNWIND [1, 'two', 3.0, true, null] AS x
RETURN type.of(x) AS t, count(*) AS n
ORDER BY t`} />

### timestamp

Wall-clock milliseconds since the Unix epoch.

<QueryCodeBlock code={String.raw`MERGE (c:Counter {name: 'events'})
  ON CREATE SET c.first_seen = temporal.timestamp()
  SET c.last_seen = temporal.timestamp()`} />

See [Data Types](../data-types/overview) for every `type.of` return
value and for how each type maps between LoraDB and host languages.

### Conversion forms compared

All three forms below target the same type system, but they are meant
for different reading and error-handling styles:

<QueryCodeBlock code={String.raw`RETURN '2024-01-15'::DATE              AS preferred,
       CAST('2024-01-15' AS DATE)      AS casted,
       TRY_CAST('not a date' AS DATE)  AS nullable`} />

- `value::TYPE` is the preferred documentation style for handwritten
  query text.
- `CAST(value AS TYPE)` is Cypher cast syntax and useful for query
  generators or users who prefer that form.
- `TRY_CAST(value AS TYPE)` is the nullable form. It is the right tool
  for imports, user-provided filters, and optional parameters.

Legacy helpers such as `toInteger` remain intentionally documented for
Cypher compatibility and simple scalar parsing. For typed construction
of `DATE`, `TIME`, `DATETIME`, `LOCAL_DATETIME`, `DURATION`, `POINT`,
and `VECTOR`, prefer casts.

## Null propagation — the common thread

Most functions return `null` when any argument is `null`. A small
handful don't, so they're worth memorising:

- [Aggregates](./aggregation) (`count`, `sum`, …) skip null inputs
  (except `count(*)`, which counts rows).
- `coalesce(a, b, …)` — returns the first non-null argument.
- `temporal.timestamp()` / `timestamp()`, `math.pi()`, `math.e()`,
  `math.random()` / `random()` —
  take no arguments.

Everywhere else, expect `null` in → `null` out. This is what makes
[`IS NULL` / `IS NOT NULL`](../queries/where#null-checks) essential over
`= null`.

## Quick lookup

Finding the right function for a task:

| I want to… | Reach for |
|---|---|
| Pick the first non-null value | [<CypherCode code="coalesce(a, b, …)" />](#type-conversion-and-checking) |
| Branch on arbitrary conditions | [<CypherCode code="CASE WHEN … THEN … END" />](../queries/return-with#case-expressions) |
| Count rows matching a condition | [<CypherCode code="count(CASE WHEN … THEN 1 END)" />](./aggregation#count) |
| Concatenate a list into a string | [<CypherCode code="reduce" />](./list#reduce) over <CypherCode code="string.split" /> / <CypherCode code="collect" /> |
| Current time (ms) | [<CypherCode code="temporal.timestamp()" />](#timestamp) / <CypherCode code="timestamp()" /> |
| Current calendar day | [<CypherCode code="temporal.today()" />](./temporal#construction-and-current-time) |
| Name of a value's type | [<CypherCode code="type.of(x)" />](#type-conversion-and-checking) |
| Internal id of a node / rel | [<CypherCode code="id(x)" />](#entity-introspection) |
| Total order over temporal values | <CypherCode code="<" />, <CypherCode code="<=" />, <CypherCode code=">" />, <CypherCode code=">=" /> — see [Ordering](../queries/ordering) |
| Cartesian or geodesic distance | [<CypherCode code="geo.distance(a, b)" />](./spatial#geodistance) |
| Score a VECTOR against a query vector | [<CypherCode code="vector.similarity(v, $q)" />](../data-types/vectors#bounded-similarity-in-0-1) |
| Signed distance under a metric | [<CypherCode code="vector.distance(a, b, EUCLIDEAN)" />](../data-types/vectors#signed-distance-metrics) |
| Magnitude of a VECTOR | [<CypherCode code="vector.norm(v, EUCLIDEAN)" />](../data-types/vectors#vector-norms) |
| Dimension of a VECTOR | [<CypherCode code="vector.dimension(v)" />](../data-types/vectors#introspection) or <CypherCode code="value.size(v)" /> |
| Convert VECTOR coordinates back to a LIST | [<CypherCode code="vector.coordinates(v, FLOAT)" /> / <CypherCode code="vector.coordinates(v, INTEGER)" />](../data-types/vectors#introspection) |

## Not supported

- **Compatibility utility namespaces** — no external utility compatibility layer.
- **General-purpose procedures** (`CALL db.labels()` etc.) — rejected at
  analysis time. The supported `CALL` surface today is limited to the
  vector and full-text index query procedures documented in
  [Indexes](../queries/indexes).
- **User-defined functions** — no registration surface.

Full list in [Limitations](../limitations).

## See also

- [**Aggregation Functions**](./aggregation) — `count`, `collect`, percentiles.
- [**String**](./string), [**Math**](./math), [**List**](./list) — everyday helpers.
- [**Temporal**](./temporal), [**Spatial**](./spatial) — typed domains.
- [**Data Types Overview**](../data-types/overview) — value shapes.
- [**Queries → Parameters**](../queries/parameters) — binding typed values from the host.
