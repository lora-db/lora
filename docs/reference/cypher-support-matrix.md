# Cypher Support Matrix

Current engine state as verified from the grammar, implementation, and feature
tests. Re-run the workspace tests before publishing exact pass/ignore counts.

## Classification key

| Status | Meaning |
|--------|---------|
| **Supported** | Verified by passing tests |
| **Partial** | Some tested support, with noted limitations |
| **Not yet implemented** | No execution path; parser/analyzer may reject, or tests are ignored |

Source of truth for syntax is `crates/lora-parser/src/cypher.pest`. Source of truth for behavior is the tests in `crates/lora-database/tests/`.

---

## 1. Query clauses

| Clause | Status | Notes |
|--------|--------|-------|
| `MATCH` | **Supported** | Node, label, property, relationship, multi-hop, cross-product |
| `OPTIONAL MATCH` | **Supported** | Returns null rows for missing patterns. A pattern anchored on already-bound variables expands from them per row, like `MATCH`, instead of matching the pattern across the whole graph and joining |
| `WHERE` | **Supported** | All comparison, boolean, string, null, list, regex operators |
| `RETURN` | **Supported** | Projection, aliases, star, computed expressions |
| `CREATE` | **Supported** | Nodes, relationships, patterns, batch via UNWIND |
| `SET` | **Supported** | Property add/update/replace/merge, label add. A `null` value removes the property (`SET n.a = null`, `SET n += {a: null}`); `null` values in a `CREATE` or `SET n = {...}` map are not stored |
| `REMOVE` | **Supported** | Property removal, label removal |
| `DELETE` / `DETACH DELETE` | **Supported** | Plain delete requires no incident relationships |
| `MERGE` | **Supported** | Node and relationship merge, ON MATCH / ON CREATE. Endpoints bound by earlier clauses are honoured, and a pattern that does not match is created whole |
| `WITH` | **Supported** | Variable piping, renaming, filtering, aggregation, star |
| `UNWIND` | **Supported** | List unwinding, empty/null handling, `list.range()` |
| `UNION` / `UNION ALL` | **Supported** | Deduplication, multi-branch, ORDER BY / LIMIT on result |
| `ORDER BY` | **Supported** | ASC, DESC, multi-key, null ordering. Keys may name projection aliases, aggregates, or (for non-aggregating, non-DISTINCT projections) original variables. With a RANGE index or uniqueness constraint on the sort property and a range predicate on it (`WHERE n.key > $after ORDER BY n.key LIMIT k`), rows stream from the index and the scan stops after `k` rows |
| `SKIP` / `LIMIT` | **Supported** | Pagination patterns. Applied after aggregation and `DISTINCT` |
| `DISTINCT` | **Supported** | In RETURN and WITH |
| `EXPLAIN` (Cypher syntax) | **Not in grammar — use API** | Provided as `db.explain(query, params?)`; deliberately not exposed as a Cypher keyword. |
| `PROFILE` (Cypher syntax) | **Not in grammar — use API** | Provided as `db.profile(query, params?)`; runs the query and reports per-operator timing. |
| `CALL` / `CALL ... YIELD` | **Partial** | Supported for the index procedures `db.index.vector.queryNodes`, `db.index.vector.queryRelationships`, `db.index.fulltext.queryNodes`, and `db.index.fulltext.queryRelationships`, both standalone and as a clause: `CALL proc(...) YIELD node [AS n], score [WHERE ...]` composes with later `MATCH` / `WITH` / `ORDER BY` / `LIMIT` and runs once per incoming row. Yielded nodes are fully bound (labels, properties). General-purpose procedures still return an unsupported-feature error. |
| `CALL { ... }` subquery | **Partial** | Runs once per outer row; import outer variables with a leading `WITH`. The body may read and write (`CREATE`, `MERGE`, `SET`, `REMOVE`, `DELETE`, nested `CALL { }`). A returning subquery ends in `RETURN`; a unit subquery ends in an updating clause, keeps the outer rows unchanged, and may end the query. Not yet: `UNION` inside the body, the `CALL (x) { ... }` scope clause, `IN TRANSACTIONS` |
| `FOREACH` | **Supported** | Updating clauses only (`CREATE`, `MERGE`, `DELETE`, `SET`, `REMOVE`, nested `FOREACH`) |
| `CREATE INDEX` / `DROP INDEX` / `SHOW INDEXES` | **Supported** | RANGE/TEXT/POINT/LOOKUP/VECTOR indexes for nodes and relationships |
| `CREATE CONSTRAINT` / `DROP CONSTRAINT` / `SHOW CONSTRAINTS` | **Supported** | Property uniqueness (single + composite), property existence (`IS NOT NULL`), node key, relationship key, and property type (`IS :: <TYPE>`) constraints — see §11 |
| `LOAD CSV` | **Not yet implemented** | Not in grammar |
| `USE <graph>` | **Not yet implemented** | Not in grammar |

## 2. Pattern matching

| Feature | Status | Notes |
|---------|--------|-------|
| Node matching (labeled / unlabeled) | **Supported** | `(n)`, `(n:User)` |
| Multi-label nodes | **Supported** | `(n:User:Admin)` matches subset |
| Inline property filters | **Supported** | `(n:User {name: 'Alice'})` |
| Directed relationships `->` / `<-` | **Supported** | |
| Undirected relationships `-` | **Supported** | |
| Anonymous nodes / relationships | **Supported** | `()-[:T]->()` |
| Relationship properties | **Supported** | `-[:FOLLOWS {since: 2020}]->` |
| Multiple patterns (cross-product) | **Supported** | `MATCH (a), (b)` |
| Variable-length paths | **Supported** | Fixed range, unbounded, zero-hop, direction, cycle handling |
| Path binding | **Supported** | `MATCH p = (a)-[*]->(b)` |
| Path functions | **Supported** | `path.length(p)`, `path.nodes(p)`, `path.edges(p)` |
| Multi-hop explicit patterns | **Supported** | Tested through 6-hop chains |
| Self-loops / parallel edges | **Supported** | |
| `shortestPath()` | **Supported** | Returns one shortest path, empty if none exists |
| `allShortestPaths()` | **Supported** | Returns every path of minimum length |
| Quantified path patterns | **Not yet implemented** | Future openCypher feature |
| Inline WHERE inside variable-length | **Not yet implemented** | Not in grammar |

## 3. Variable-length paths (detail)

| Feature | Status |
|---------|--------|
| Fixed range `*1..3` | **Supported** |
| Exact distance `*3..3` | **Supported** |
| Unbounded `*` | **Supported** |
| Upper-bound-only `*..3` | **Supported** |
| Lower-bound-only `*3..` | **Supported** |
| Zero-hop `*0..1` | **Supported** |
| Forward / backward / undirected | **Supported** |
| Cycle avoidance (visited tracking) | **Supported** |
| Long chains (20+ nodes) | **Supported** |
| Diamond / fan patterns | **Supported** |

## 4. Expressions and operators

| Feature | Status | Notes |
|---------|--------|-------|
| Integer / float / string / bool / null literals | **Supported** | |
| Hex / octal integer literals | **Supported** | `0xFF`, `0o17` |
| List / map literals | **Supported** | Nested, heterogeneous |
| Arithmetic `+ - * / % ^` | **Supported** | `/` and `%` by zero → null |
| Unary `-` / `+` | **Supported** | |
| Equality `=` / `<>` | **Supported** | |
| Comparison `< > <= >=` | **Supported** | Numbers (integers exactly, also above 2^53), strings, booleans (`false < true`), temporals of one kind, durations, and lists: element by element, a prefix before the longer list. A null or an operand of another kind gives `null`, never `false`, so `[a, b] > $cursor` works for keyset pagination and `NOT (1 < 'a')` is `null`. Two temporals of different kinds (a `DATE` against a `DATETIME`) are an error on every plan, index-backed or not, where Cypher gives `null`: see section 12 |
| `AND` / `OR` / `NOT` / `XOR` | **Supported** | Three-valued logic with nulls |
| `IN` list membership | **Supported** | Null propagation per Cypher spec |
| `IS NULL` / `IS NOT NULL` | **Supported** | |
| `STARTS WITH` / `ENDS WITH` / `CONTAINS` | **Supported** | Case-sensitive |
| `CASE` (generic and simple) | **Supported** | |
| Regex matching `=~` | **Supported** | Full Rust `regex` crate |
| List indexing `[i]` | **Supported** | Negative indices supported |
| List slicing `[a..b]` | **Supported** | Open-ended slices |
| List concatenation `+` | **Supported** | |
| String concatenation `+` | **Supported** | |
| List comprehension `[x IN list WHERE p \| e]` | **Supported** | |
| Pattern comprehension `[pattern WHERE p \| e]` | **Supported** | |
| `EXISTS { pattern }` subquery | **Supported** | In WHERE |
| `COUNT { pattern [WHERE ...] }` subquery | **Supported** | Number of matches; `MATCH` keyword optional |
| Label predicate `n:Label`, `n:A:B`, `n:A\|B` | **Supported** | Boolean expression, e.g. `WHERE n:Festival` |
| `REDUCE(acc = init, x IN list \| expr)` | **Supported** | |
| List predicates (`all`, `any`, `none`, `single`) | **Supported** | |
| Operator precedence | **Supported** | Parenthesized expressions |
| Map projection `n {.name, .age, .*}` | **Supported** | |

## 5. Aggregation functions

| Function | Status | Notes |
|----------|--------|-------|
| `count(expr)` / `count(*)` | **Supported** | Including `count(DISTINCT ...)` |
| `sum(expr)` | **Supported** | Int or float based on input; skips nulls |
| `avg(expr)` | **Supported** | Returns float; skips nulls; null for empty set |
| `min(expr)` / `max(expr)` | **Supported** | Numeric, string, and temporal ordering |
| `collect(expr)` | **Supported** | Including `collect(DISTINCT ...)` |
| `stdev(expr)` | **Supported** | Sample standard deviation (n-1) |
| `stdevp(expr)` | **Supported** | Population standard deviation (n) |
| `percentileCont(expr, p)` | **Supported** | Continuous, linear interpolation |
| `percentileDisc(expr, p)` | **Supported** | Discrete, nearest-rank |
| Grouped aggregation | **Supported** | Non-aggregated columns act as GROUP BY |
| Multi-aggregate queries | **Supported** | Multiple aggregates in one RETURN |
| HAVING-style filtering | **Supported** | Via `WITH ... WHERE` |

## 6. Scalar / introspection functions

| Function | Status |
|----------|--------|
| `id(node \| rel)` | **Supported** |
| `labels(node)` | **Supported** |
| `type(rel)` | **Supported** |
| `keys(node \| rel \| map)` | **Supported** |
| `properties(node \| rel \| map)` | **Supported** |
| `coalesce(expr, ...)` | **Supported** |
| `temporal.timestamp()` / `timestamp()` | **Supported** |
| `temporal.timezone()` / `timezone()` | **Supported** |
| `uuid.new()` / `new()` | **Supported** |
| `type.of(expr)` | **Supported** |

`type.of` returns one of the scalar/graph type names, including `"NULL"`,
`"BOOLEAN"`, `"INTEGER"`, `"FLOAT"`, `"STRING"`, `"BINARY"`, `"LIST<T>"`,
`"MAP"`, `"NODE"`, `"RELATIONSHIP"`, `"PATH"`, temporal names, `"POINT"`, and
`"VECTOR<COORD>(N)"`.

## 7. String functions

String helpers operate on UTF-8 strings. Case conversion uses Unicode
case mapping, `string.length` counts Unicode code points, and
normalization supports NFC/NFD/NFKC/NFKD.

| Function | Status |
|----------|--------|
| `string.lower`, `string.upper` (`toLower`, `toUpper` aliases) | **Supported** |
| `string.trim`, `string.trim_left`, `string.trim_right` (`trim`, `ltrim`, `rtrim` aliases) | **Supported** |
| `string.replace(str, find, repl)` (`replace` alias) | **Supported** |
| `string.slice(str, start[, len])` | **Supported** |
| `string.prefix(str, n)`, `string.suffix(str, n)` | **Supported** |
| `string.find`, `string.count`, `string.before`, `string.after` | **Supported** |
| `string.split(str, delim)` (`split` alias), `string.join(list, delim)`, `string.words(str)` | **Supported** | An empty `delim` splits into characters: `split('p12', '')` is `['p', '1', '2']` |
| `string.slugify`, `string.escape`, `string.url_encode`, `string.url_decode` | **Supported** |
| `string.reverse(str)` / `value.reverse(str)` | **Supported** |
| `string.length(str)` (`char_length`, `character_length` aliases), `value.size(str)` / `size(str)` | **Supported** |
| `string.pad_left(str, len, pad)`, `string.pad_right(str, len, pad)` | **Supported** |
| `toString`, `toInteger`, `toFloat`, `toBoolean` | **Supported** |
| `string.normalize(str[, form])` (`normalize` alias) | **Supported** (NFC/NFD/NFKC/NFKD) |

## 8. Math functions

| Function | Status |
|----------|--------|
| `math.abs`, `math.ceil`, `math.floor`, `math.round`, `math.sign` | **Supported** |
| `math.sqrt` | **Supported** (negative input → null) |
| `math.log` / `math.ln`, `math.log10`, `math.exp` | **Supported** |
| `math.sin`, `math.cos`, `math.tan`, `math.asin`, `math.acos`, `math.atan`, `math.atan2` | **Supported** |
| `math.degrees`, `math.radians` | **Supported** |
| `math.pi()`, `math.e()` | **Supported** |
| `math.random()` / `random()` | **Supported** (`[0, 1)` based on system-time nanos) |

The standard Cypher names `abs`, `ceil`, `floor`, `round`, `sign`, `sqrt`,
`log` (natural), `log10`, `exp`, `sin`, `cos`, `tan`, `cot`, `asin`, `acos`,
`atan`, `atan2`, `degrees`, `radians`, `pi`, and `e` resolve to the `math.*`
functions. `round` rounds half away from zero and returns an integer for
integral results, where Neo4j returns a float.

## 9. List functions

| Function | Status |
|----------|--------|
| `value.size(list)` / `size(list)` | **Supported** |
| `list.first` / `head` / `first`, `list.rest` / `tail`, `list.last` / `last` | **Supported** |
| `value.reverse(list)` / `reverse(list)` | **Supported** |
| `list.range(start, end[, step])` | **Supported** |
| `reduce(acc = init, x IN list \| expr)` | **Supported** |
| List comprehension `[x IN list WHERE p \| e]` | **Supported** |

## 10. List predicates

| Predicate | Status |
|-----------|--------|
| `all(x IN list WHERE pred)` | **Supported** |
| `any(x IN list WHERE pred)` | **Supported** |
| `none(x IN list WHERE pred)` | **Supported** |
| `single(x IN list WHERE pred)` | **Supported** |

## 11. Path functions

| Function | Status |
|----------|--------|
| `path.length(path)` | **Supported** |
| `path.nodes(path)` | **Supported** |
| `path.edges(path)` | **Supported** |

## 12. Temporal types and functions

All six temporal types have first-class `LoraValue` and `PropertyValue` variants. They can be stored as node / relationship properties, used in expressions, compared, and piped through clauses.

| Type | Status | Representation |
|------|--------|----------------|
| `Date` | **Supported** | year (i32), month, day |
| `Time` | **Supported** | hour, minute, second, nanosecond + UTC offset |
| `LocalTime` | **Supported** | timezone-naive clock time |
| `DateTime` | **Supported** | local fields + UTC offset |
| `LocalDateTime` | **Supported** | timezone-naive datetime |
| `Duration` | **Supported** | months, days, seconds, nanoseconds |

| Function | Status | Notes |
|----------|--------|-------|
| `date()`, `datetime()`, `localdatetime()`, `time()`, `localtime()` | **Supported** | Cypher constructors. With no argument, the current value of that type (`date()` is today's `DATE`). With one, the value cast to that type: `date(x)` is `x::DATE`. A string is parsed, a map gives the components (`{year, month, day, hour, minute, second, millisecond, microsecond, nanosecond, timezone}`), and another temporal keeps the components the target has (`date(datetime())` is today's date, `time(dt)` keeps the offset). An argument that does not convert is an error, `null` gives `null` |
| `duration(x)` | **Supported** | `x::DURATION`: ISO 8601 string or component map |
| `temporal.today()` / `'...'::DATE` / `{year, month, day}::DATE` | **Supported** | ISO string, map, or current day |
| `temporal.now()` / `now()` / `temporal.now(kind)` / `'...'::DATETIME` / `{...}::DATETIME` | **Supported** | Current `DATETIME` by default; `kind` accepts `"date"`, `"time"`, `"local_time"`, `"local_datetime"` |
| `'...'::TIME` | **Supported** | ISO string |
| `'...'::LOCAL_TIME` | **Supported** | ISO string |
| `'...'::LOCAL_DATETIME` / `{...}::LOCAL_DATETIME` | **Supported** | |
| `'...'::DURATION` / `{...}::DURATION` | **Supported** | ISO 8601 or `{years, months, days, hours, minutes, seconds}` |
| `CAST(value AS TYPE)` / `TRY_CAST(value AS TYPE)` | **Supported** | Cast syntax in the Cypher grammar; `TRY_CAST` returns `null` on failed conversion |
| `temporal.truncate(unit, date)` | **Partial** | Supported units: `"year"`, `"month"`, `"day"` |
| `temporal.truncate(unit, datetime)` | **Partial** | Supported units: `"year"`, `"month"`, `"day"`, `"hour"` |
| `temporal.between(a, b)` | **Supported** | Between dates or datetimes |
| `temporal.in_days(a, b)` | **Supported** | `DATE` values |

Comparison operators (`<`, `>`, `<=`, `>=`, `=`) work between values of the same temporal type. Ordering two values of different temporal types (`date('2026-10-01') >= datetime()`) is an error, not Cypher's `null`: a `WHERE` on it would otherwise drop every row without a word. It fails the same way when a RANGE index answers the predicate and the indexed property holds another temporal type than the bound. `=` between different temporal types is `false`, and `ORDER BY`, `min` and `max` still order mixed values. Convert one side first: `date(x)`, `datetime(x)`. Ordering comparisons and `ORDER BY` use the instant a value denotes, at nanosecond precision (zoned values are compared in UTC); `=` also compares the offset. `Date + Duration` and `DateTime - DateTime` arithmetic are supported for the subset of tests in `tests/temporal.rs`.

## 13. Spatial types and functions

| Type | Status | SRID |
|------|--------|------|
| `Point` (Cartesian 2D) | **Supported** | 7203 |
| `Point` (WGS-84 geographic 2D) | **Supported** | 4326 |

| Function | Status | Notes |
|----------|--------|-------|
| `{x, y}::POINT` | **Supported** | Cartesian 2D |
| `{latitude, longitude}::POINT` | **Supported** | WGS-84 geographic 2D |
| `geo.distance(a, b)` | **Supported** | Euclidean for Cartesian, Haversine for geographic (Earth radius 6,371 km) |
| Component access: `p.x`, `p.y`, `p.latitude`, `p.longitude`, `p.srid` | **Supported** | Via property access on Point |
| 3D points (Cartesian SRID 9157, WGS-84 SRID 4979) | **Supported** | `z` / `height` exposed via property access; `geo.distance()` on WGS-84-3D ignores height and falls back to great-circle |
| `geo.within_bbox(p, ll, ur)` | **Supported** | Same-SRID closed bounding box; mixed 2D/3D inputs return `null` |

## 13a. Index DDL and optimizer rewrites

| Feature | Status | Notes |
|---------|--------|-------|
| `CREATE INDEX name FOR (n:Label) ON (n.prop)` | **Supported** | RANGE index; name may be omitted or supplied by string parameter. Indexes numbers, strings, booleans, lists, maps and the temporal types `DATE`, `DATETIME`, `LOCAL_DATETIME`, `TIME`, `LOCAL_TIME` (ordered by the instant they denote). `DURATION`, `POINT` and `VECTOR` values are not held; a range bound of one of those types is answered by a scan |
| `CREATE INDEX name FOR ()-[r:TYPE]-() ON (r.prop)` | **Supported** | Relationship RANGE index |
| `CREATE TEXT INDEX` | **Supported** | Node and relationship scopes; accelerates `STARTS WITH`, `CONTAINS`, `ENDS WITH` |
| `CREATE POINT INDEX` | **Supported** | Node and relationship scopes; accelerates `geo.within_bbox` and `geo.distance(...) <= radius` candidates |
| `CREATE LOOKUP INDEX ... ON EACH node.labels(n)` | **Supported** | Catalog-visible token index for labels |
| `CREATE LOOKUP INDEX ... ON EACH edge.type(r)` | **Supported** | Catalog-visible token index for relationship types |
| `IF NOT EXISTS` | **Supported** | Duplicate name or equivalent schema becomes a no-op |
| `DROP INDEX name [IF EXISTS]` | **Supported** | Missing index without `IF EXISTS` returns `42N51` |
| `SHOW INDEXES` / `SHOW INDEX` | **Supported** | Returns name, type, entityType, labelsOrTypes, properties, state, populationPercent. Accepts type filter (`SHOW {ALL\|RANGE\|TEXT\|POINT\|LOOKUP\|FULLTEXT\|VECTOR} INDEXES`). Accepts a YIELD-anchored tail: `YIELD {*\|items} [ORDER BY ...] [SKIP n] [LIMIT n] [WHERE expr] [RETURN items [ORDER BY ...] [SKIP n] [LIMIT n]]`. Same tail also accepted on `SHOW CONSTRAINTS`. |
| Duplicate index name | **Supported error** | Returns GQLSTATUS-shaped `22N71` |
| Equivalent index under another name | **Supported error** | Returns GQLSTATUS-shaped `22N70` |
| Equality and `IN` seeks | **Supported** | `n.key = v` and `n.key IN list` in a WHERE are pushed down to the scan of `n` wherever it sits in the pattern and run as index seeks (one per distinct `IN` element); a chain starts from whichever end can seek |
| Composite RANGE index catalog entries | **Partial** | Accepted and shown; current optimizer rewrites are single-property |
| Property uniqueness constraints | **Supported** | Single + composite; backed by a RANGE index of the same name; mutation-time enforcement returns `22N79` |
| Property existence constraints (`IS NOT NULL`) | **Supported** | Single property only; rejects CREATE missing the prop, REMOVE of the prop, and SET-label that would activate it on an incomplete node; returns `22N77`. An entity created by a statement that also sets properties (`CREATE ... SET`, `MERGE ... ON CREATE SET`) is checked when the statement finishes |
| Node / relationship key constraints | **Supported** | Composition of existence + uniqueness; single + composite; node-key uses `IS NODE KEY`, rel-key uses `IS RELATIONSHIP KEY` |
| Property type constraints (`IS :: T`) | **Supported** | Scalar (`BOOLEAN`/`STRING`/`INTEGER`/`FLOAT`/`DATE`/`LOCAL TIME`/`ZONED TIME`/`LOCAL DATETIME`/`ZONED DATETIME`/`DURATION`/`POINT`), `LIST<T NOT NULL>`, `VECTOR<COORD>(DIM)`, and closed dynamic unions (`T1 \| T2`); `MAP`/`ANY` rejected with `22N90` |
| Vector index / ANN index | **Partial** | `CREATE VECTOR INDEX FOR (n:L) ON (n.p) OPTIONS {indexConfig: {vector.dimensions, vector.similarity_function}}` (node + rel). Procedures `db.index.vector.queryNodes` / `queryRelationships` execute a flat scan over label-matching entities; ANN structure (HNSW) is a follow-up. |
| Full-text indexing | **Supported (standard/simple analyzer)** | `CREATE FULLTEXT INDEX FOR (n:A\|B) ON EACH [n.p, n.q]` — multi-label, multi-property, relationship scope. `OPTIONS {fulltext.analyzer}` accepts `'standard'` (default) and `'simple'`; others rejected. Procedures `db.index.fulltext.queryNodes` / `queryRelationships` tokenise with lowercase + ASCII folding (`Sónar` matches `Sonar`, `Øya` matches `Oya`) + non-alphanumeric split, intersect posting lists (AND semantics); a term ending in `*` matches every indexed term with that prefix. Scores are summed TF; rows `(node\|relationship, score)` come back sorted descending with full node / relationship values. |

## 13b. Vector types and functions

### Coordinate types

| Type | Status | Storage | Aliases accepted on input |
|------|--------|---------|---------------------------|
| `VECTOR<FLOAT64>`   | **Supported** | `Vec<f64>` | `FLOAT`, `FLOAT64` |
| `VECTOR<FLOAT32>`   | **Supported** | `Vec<f32>` | `FLOAT32` |
| `VECTOR<INTEGER>`   | **Supported** | `Vec<i64>` | `INTEGER`, `INT`, `INT64`, `INTEGER64` |
| `VECTOR<INTEGER32>` | **Supported** | `Vec<i32>` | `INTEGER32`, `INT32` |
| `VECTOR<INTEGER16>` | **Supported** | `Vec<i16>` | `INTEGER16`, `INT16` |
| `VECTOR<INTEGER8>`  | **Supported** | `Vec<i8>`  | `INTEGER8`, `INT8` |

Alias matching is case-insensitive.
`DOUBLE` is **rejected** explicitly so typos surface as a clear
"unknown coordinate type" error rather than silently mapping to
`FLOAT64`. Dimension is capped at `1..=4096`.

### Functions

| Function | Arity | Status | Notes |
|----------|-------|--------|-------|
| `value::VECTOR<COORD>(DIM)` | — | **Supported** | Cast-based construction from a numeric list or string like `"[1.0, 2.0]"`. `CAST(value AS VECTOR<COORD>(DIM))` and `TRY_CAST(value AS VECTOR<COORD>(DIM))` are also supported. |
| `vector.similarity(a, b)` | 2 | **Supported** | Accepts `VECTOR` or `LIST<NUMBER>`; list coerced to `FLOAT32` vector. Bounded to `[0, 1]` as `(1 + raw_cosine)/2`. Zero-norm vector → `null`. `f32` arithmetic. |
| `vector.similarity(a, b, 'euclidean')` | 3 | **Supported** | Same input acceptance; returns `1 / (1 + d²)`. |
| `vector.distance(a, b, metric)` | 3 | **Supported** | Both operands must be `VECTOR` (plain list rejected). Metrics: `EUCLIDEAN`, `EUCLIDEAN_SQUARED`, `MANHATTAN`, `COSINE` (= `1 - raw_cosine`), `DOT` (= `-(a·b)`), `HAMMING` (f32 comparison). Case-insensitive; identifier or string. |
| `vector.norm(v, metric)` | 2 | **Supported** | `EUCLIDEAN` or `MANHATTAN`. Case-insensitive. |
| `vector.dimension(v)` | 1 | **Supported** | Returns `dimension`. |
| `value.size(v)` on a `VECTOR` | 1 | **Supported** | Returns `dimension` — identical to `vector.dimension`. |
| `type.of(v)` | 1 | **Supported** | Returns `"VECTOR<COORD>(N)"`. |
| `vector.coordinates(v, INTEGER)` | 2 | **Supported** | Rejects non-vector; float coordinates truncate toward zero. |
| `vector.coordinates(v, FLOAT)` | 2 | **Supported** | Rejects non-vector. |
| Vector index procedures / approximate kNN | — | **Partial** | `db.index.vector.queryNodes` / `queryRelationships` query the cataloged index scope but still execute a flat scan. Dedicated ANN execution is not yet implemented. Exhaustive kNN also works via `ORDER BY vector.similarity(...) LIMIT k`. |
| Built-in embedding / plugin integration | — | **Not yet implemented** | LoraDB has no plugin surface — produce embeddings host-side. |

### Storage semantics

| Behaviour | Status |
|---|---|
| `VECTOR` as node property | **Supported** |
| `VECTOR` as relationship property | **Supported** |
| `VECTOR` as a value inside a `Map` property | **Supported** |
| `VECTOR` inside a `List` stored as property (at any depth, including via nested `Map`) | **Rejected at write time** (`PropertyConversionError::NestedVectorInList`) |
| List / `collect(...)` of vectors inside a query (RETURN / WITH / UNWIND) | **Supported** — only the write path enforces the no-list-of-vectors rule |
| Equality across coord types with equal values | `false` — coord type is part of identity |
| `DISTINCT` key | Coord type + dimension + stringified values |
| `ORDER BY` on a `VECTOR` column | Deterministic but unspecified ordering — use a scalar score for intent |

## 14. Data types

| Type | Status | Notes |
|------|--------|-------|
| Integer (`i64`) | **Supported** | |
| Float (`f64`) | **Supported** | IEEE 754 |
| String | **Supported** | UTF-8, escape sequences |
| Binary | **Supported** | Byte-string property value exposed through binding wire formats |
| Boolean | **Supported** | |
| Null | **Supported** | Three-valued logic |
| List | **Supported** | Heterogeneous, nested, indexing, slicing |
| Map | **Supported** | Nested maps |
| Node | **Supported** | Hydrated to `{id, labels, properties}` |
| Relationship | **Supported** | Hydrated to `{kind, id, startId, endId, type, properties}` |
| Path | **Supported** | Alternating nodes and relationships |
| Date / Time / LocalTime / DateTime / LocalDateTime / Duration | **Supported** | See §12 |
| Point (Cartesian, WGS-84) | **Supported** | See §13 |
| Vector (typed coordinates, dim <= 4096) | **Supported** | See §13b |

## 15. Parameter binding

| Feature | Status |
|---------|--------|
| Named parameters `$name` | **Supported** |
| Numeric parameters `$1`, `$2` | **Supported** |
| String / integer / float / boolean / null parameters | **Supported** |
| List parameter (including `x IN $list`) | **Supported** |
| Map parameter (e.g. property map in CREATE) | **Supported** |
| Missing parameter resolves to `null` | **Supported** |
| Parameter in WHERE, CREATE, RETURN | **Supported** |
| Temporal / spatial parameters | **Supported** |
| Parameter as label | **Not yet implemented** | Not standard Cypher |
| Parameter type checking at parse time | **Not yet implemented** | |
| 64-bit integers from JavaScript | **Supported** | Node binding: pass `bigint`; results outside `Number.MIN_SAFE_INTEGER..MAX_SAFE_INTEGER` come back as `bigint`. An integer-valued `number` beyond 2^53 is rejected (`LORA_INVALID_PARAMS`) rather than stored rounded |
| Point parameters as `{latitude, longitude[, height]}` | **Supported** | Same shape reads return; implies WGS-84 when no `srid` is given. `{x, y[, z]}` still works |
| Parameter support over HTTP | **Supported** | `/query`, `/explain`, and `/profile` accept a JSON `params` object; typed values need query casts where JSON has no native shape |

## 16. Write operations

| Operation | Status |
|-----------|--------|
| Create node | **Supported** |
| Create relationship | **Supported** |
| Create pattern (node + rel in one clause) | **Supported** |
| `SET n.prop = value` | **Supported** |
| `SET n.prop = null` (effective remove) | **Supported** |
| `SET n = {map}` (replace all) | **Supported** |
| `SET n += {map}` (merge) | **Supported** |
| `SET n:Label` / `SET n:A:B` | **Supported** |
| `REMOVE n.prop` / `REMOVE n:Label` | **Supported** |
| `DELETE n` (no incident rels) | **Supported** |
| `DETACH DELETE n` | **Supported** |
| `MERGE` (node / relationship) | **Supported** |
| `ON MATCH SET` / `ON CREATE SET` | **Supported** |
| Batch create via `UNWIND` | **Supported** |
| Write statement without `RETURN` | **Supported** | Returns no rows (the writes apply) |
| Schema DDL inside a transaction | **Supported** | `CREATE` / `DROP` index or constraint commit or roll back together with the data statements around them |

## 17. Result formats

The HTTP server chooses a format from the request body's `"format"` field. The Rust API accepts a `ResultFormat` on `ExecuteOptions`.

| Format | Shape |
|--------|-------|
| `rows` | Array of maps (variable → value) |
| `rowArrays` | `{columns, rows}` with positional arrays |
| `graph` | Extracted node and relationship projections — **default** |
| `combined` | Combined columns + row arrays + graph projection |

## 18. Error handling and validation

| Feature | Status |
|---------|--------|
| Parse error with span | **Supported** |
| Unknown label / type / property | **Not an error** | Standard Cypher: a pattern naming a label or relationship type no entity carries matches nothing, and reading a missing key yields `null`. The answer never depends on whether matching data happens to exist (empty graph, last node deleted, `REMOVE`d, rolled back) |
| Unknown variable in RETURN | **Supported** |
| Duplicate variable binding / projection alias | **Supported** |
| Duplicate map key | **Supported** |
| Unknown function name | **Supported** | Analysis-time error |
| Wrong function arity | **Supported** |
| DELETE node with relationships | **Supported** | Requires DETACH |
| Invalid relationship range (min > max) | **Supported** |
| Aggregation in WHERE rejected | **Supported** |
| UNION column-count / name mismatch | **Supported** |
| Labels / types allowed in CREATE / MERGE | **Supported** | Any name accepted in write contexts |
| Type mismatch detection in comparison | **Not yet implemented** | 1 ignored test |

## 19. Null semantics

| Behavior | Status |
|----------|--------|
| `null = null` → `null` | **Supported** |
| `null <> null` → `null` | **Supported** |
| `null + value` → `null` | **Supported** |
| `null AND false` → `false` | **Supported** |
| `null AND true` → `null` | **Supported** |
| `null OR true` → `true` | **Supported** |
| `null OR false` → `null` | **Supported** |
| `null IN list` → `null` | **Supported** |
| `IS NULL` / `IS NOT NULL` | **Supported** |
| Aggregates skip nulls (except `count(*)`) | **Supported** |

## 20. Not yet implemented (summary)

| Feature | Category | Reason |
|---------|----------|--------|
| `CALL` for procedures other than the index queries | Clause | Analyzer rejects with `UnsupportedFeature` |
| `EXPLAIN` / `PROFILE` (as Cypher keywords) | Clause | Not in grammar — exposed instead as the `db.explain()` / `db.profile()` API methods so callers must explicitly request plan-only or instrumented execution. |
| `LOAD CSV` | DDL | Not in grammar |
| `USE <graph>` (multi-database) | Clause | Not in grammar |
| Quantified path patterns | Pattern | Future openCypher syntax |
| Inline WHERE inside variable-length | Pattern | Not in grammar |
| Type mismatch detection in comparison | Validation | 1 ignored test |
| Parameter as label | Parameters | Non-standard |
| Parameter type checking at parse time | Parameters | |
| Typed helper constructors over HTTP | Transport | JSON params are supported, but helper constructors live in host bindings; use query casts for `DATE`, `POINT`, `VECTOR`, etc. |
| Compatibility utility functions | Functions | No compatibility layer |
| Authentication / TLS | Server | See [`../operations/security.md`](../operations/security.md) |

---

*Last updated from code audit: the parser grammar lives in
`crates/lora-parser/src/cypher.pest`, behavior tests live under
`crates/lora-database/tests/`.*
