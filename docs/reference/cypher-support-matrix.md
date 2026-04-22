# Cypher Support Matrix

Current engine state as verified by the test suite. **1698 passing tests, 0 failing, 58 ignored.**

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
| `OPTIONAL MATCH` | **Supported** | Returns null rows for missing patterns |
| `WHERE` | **Supported** | All comparison, boolean, string, null, list, regex operators |
| `RETURN` | **Supported** | Projection, aliases, star, computed expressions |
| `CREATE` | **Supported** | Nodes, relationships, patterns, batch via UNWIND |
| `SET` | **Supported** | Property add/update/replace/merge, label add |
| `REMOVE` | **Supported** | Property removal, label removal |
| `DELETE` / `DETACH DELETE` | **Supported** | Plain delete requires no incident relationships |
| `MERGE` | **Supported** | Node and relationship merge, ON MATCH / ON CREATE |
| `WITH` | **Supported** | Variable piping, renaming, filtering, aggregation, star |
| `UNWIND` | **Supported** | List unwinding, empty/null handling, `range()` |
| `UNION` / `UNION ALL` | **Supported** | Deduplication, multi-branch, ORDER BY / LIMIT on result |
| `ORDER BY` | **Supported** | ASC, DESC, multi-key, null ordering |
| `SKIP` / `LIMIT` | **Supported** | Pagination patterns |
| `DISTINCT` | **Supported** | In RETURN and WITH |
| `EXPLAIN` | **Partial** | Parses; executor still runs the underlying query |
| `CALL` (standalone) | **Not yet implemented** | Parsed; analyzer returns `SemanticError::UnsupportedFeature` |
| `CALL ... YIELD` (in-query) | **Not yet implemented** | Parsed; analyzer returns `SemanticError::UnsupportedFeature` |
| `FOREACH` | **Not yet implemented** | Not in grammar |
| `CREATE INDEX` / `CREATE CONSTRAINT` | **Not yet implemented** | Not in grammar |
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
| Path functions | **Supported** | `length(p)`, `nodes(p)`, `relationships(p)` |
| Multi-hop explicit patterns | **Supported** | Tested through 6-hop chains |
| Self-loops / parallel edges | **Supported** | |
| `shortestPath()` | **Supported** | Returns one shortest path, empty if none exists |
| `allShortestPaths()` | **Supported** | Returns every path of minimum length |
| Quantified path patterns | **Not yet implemented** | Future openCypher feature |
| Inline WHERE inside variable-length | **Not yet implemented** | 1 ignored test |

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
| Comparison `< > <= >=` | **Supported** | Numeric, string, and temporal |
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
| `timestamp()` | **Supported** |
| `valueType(expr)` | **Supported** |

`valueType` returns one of: `"NULL"`, `"BOOLEAN"`, `"INTEGER"`, `"FLOAT"`, `"STRING"`, `"LIST<T>"`, `"MAP"`, `"NODE"`, `"RELATIONSHIP"`, `"PATH"`, `"DATE"`, `"DATE_TIME"`, `"LOCAL_DATE_TIME"`, `"TIME"`, `"LOCAL_TIME"`, `"DURATION"`, `"POINT"`.

## 7. String functions

All ASCII-based. Unicode normalization is a no-op placeholder.

| Function | Status |
|----------|--------|
| `toLower`, `toUpper` | **Supported** (ASCII only) |
| `trim`, `lTrim`, `rTrim` | **Supported** |
| `replace(str, find, repl)` | **Supported** |
| `substring(str, start[, len])` | **Supported** |
| `left(str, n)`, `right(str, n)` | **Supported** |
| `split(str, delim)` | **Supported** |
| `reverse(str)` | **Supported** (also on lists) |
| `size(str)`, `length(str)`, `charLength(str)` | **Supported** |
| `lpad(str, len, pad)`, `rpad(str, len, pad)` | **Supported** |
| `toString`, `toInteger`, `toFloat`, `toBoolean` | **Supported** |
| `normalize(str)` | **Partial** (ASCII passthrough, no Unicode NFC) |

## 8. Math functions

| Function | Status |
|----------|--------|
| `abs`, `ceil`, `floor`, `round`, `sign` | **Supported** |
| `sqrt` | **Supported** (negative input → null) |
| `log` / `ln`, `log10`, `exp` | **Supported** |
| `sin`, `cos`, `tan`, `asin`, `acos`, `atan`, `atan2` | **Supported** |
| `degrees`, `radians` | **Supported** |
| `pi()`, `e()` | **Supported** |
| `rand()` | **Supported** (`[0, 1)` based on system-time nanos) |

## 9. List functions

| Function | Status |
|----------|--------|
| `size(list)` | **Supported** |
| `head`, `tail`, `last` | **Supported** |
| `reverse(list)` | **Supported** |
| `range(start, end[, step])` | **Supported** |
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
| `length(path)` | **Supported** |
| `nodes(path)` | **Supported** |
| `relationships(path)` | **Supported** |

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
| `date()` / `date(string)` / `date({year, month, day})` | **Supported** | ISO string, map, or current day |
| `datetime()` / `datetime(string \| map)` | **Supported** | ISO string or map with optional timezone |
| `time(string)` | **Supported** | ISO string |
| `localtime(string)` | **Supported** | ISO string |
| `localdatetime(string \| map)` | **Supported** | |
| `duration(string \| map)` | **Supported** | ISO 8601 or `{years, months, days, hours, minutes, seconds}` |
| `date.truncate(unit, date)` | **Partial** | Supported units: `"year"`, `"month"` |
| `datetime.truncate(unit, datetime)` | **Partial** | Supported units: `"day"`, `"hour"`, `"month"` |
| `duration.between(a, b)` | **Supported** | Between dates or datetimes |
| `duration.inDays(a, b)` | **Supported** | |

Comparison operators (`<`, `>`, `<=`, `>=`, `=`) work between values of the same temporal type. `Date + Duration` and `DateTime - DateTime` arithmetic are supported for the subset of tests in `tests/temporal.rs`.

## 13. Spatial types and functions

| Type | Status | SRID |
|------|--------|------|
| `Point` (Cartesian 2D) | **Supported** | 7203 |
| `Point` (WGS-84 geographic 2D) | **Supported** | 4326 |

| Function | Status | Notes |
|----------|--------|-------|
| `point({x, y})` | **Supported** | Cartesian 2D |
| `point({latitude, longitude})` | **Supported** | WGS-84 geographic 2D |
| `point.distance(a, b)` / `distance(a, b)` | **Supported** | Euclidean for Cartesian, Haversine for geographic (Earth radius 6,371 km) |
| Component access: `p.x`, `p.y`, `p.latitude`, `p.longitude`, `p.srid` | **Supported** | Via property access on Point |
| 3D points | **Not yet implemented** | No `z` dimension |

## 14. Data types

| Type | Status | Notes |
|------|--------|-------|
| Integer (`i64`) | **Supported** | |
| Float (`f64`) | **Supported** | IEEE 754 |
| String | **Supported** | UTF-8, escape sequences |
| Boolean | **Supported** | |
| Null | **Supported** | Three-valued logic |
| List | **Supported** | Heterogeneous, nested, indexing, slicing |
| Map | **Supported** | Nested maps |
| Node | **Supported** | Hydrated to `{id, labels, properties}` |
| Relationship | **Supported** | Hydrated to `{kind, id, startId, endId, type, properties}` |
| Path | **Supported** | Alternating nodes and relationships |
| Date / Time / LocalTime / DateTime / LocalDateTime / Duration | **Supported** | See §12 |
| Point (Cartesian, WGS-84) | **Supported** | See §13 |

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
| Parameter support over HTTP | **Not yet implemented** | Rust API only (`execute_with_params`) |

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
| Unknown label / type / property in MATCH | **Supported** | Only on non-empty graph |
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
| `CALL` (standalone and YIELD) | Clause | Analyzer rejects with `UnsupportedFeature` |
| `FOREACH` | Clause | Not in grammar |
| `CREATE INDEX` / `CREATE CONSTRAINT` | DDL | Not in grammar |
| `LOAD CSV` | DDL | Not in grammar |
| `USE <graph>` (multi-database) | Clause | Not in grammar |
| Quantified path patterns | Pattern | Future openCypher syntax |
| Inline WHERE inside variable-length | Pattern | 1 ignored test |
| 3D points | Type | No `z` dimension |
| Type mismatch detection in comparison | Validation | 1 ignored test |
| Parameter as label | Parameters | Non-standard |
| Parameter type checking at parse time | Parameters | |
| Parameters over HTTP | Transport | Rust API only |
| APOC-style utilities | Functions | No compatibility layer |
| Persistence (WAL / snapshots) | Storage | In-memory only |
| Authentication / TLS | Server | See [`../operations/security.md`](../operations/security.md) |

---

*Last verified against `cargo test --workspace`: 1698 passing, 0 failing, 58 ignored.*
