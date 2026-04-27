---
title: Troubleshooting LoraDB Errors
sidebar_label: Troubleshooting
description: A symptom-indexed guide to common LoraDB errors — parse failures, semantic errors, unexpected results, server startup issues — with the shortest fix for each.
---

# Troubleshooting LoraDB Errors

When a query fails, returns no rows, or the server refuses to start,
find the symptom in the lookup table below and jump to the fix.
Each section names the cause, shows the failure mode, and gives the
shortest way out.

## Quick lookup

| Symptom | Jump to |
|---|---|
| Parse error, missing paren/direction | [Parse errors](#parse-errors) |
| `Unknown label`, `Unknown variable`, `Unknown function` | [Semantic errors](#semantic-errors) |
| `DeleteNodeWithRelationships` | [Executor errors](#executor-errors) |
| Query returns empty for no reason | [Queries return empty results](#queries-return-empty-results) |
| N × M row explosion | [MATCH returns a cross-product](#match-returns-a-cross-product) |
| `SET` destroyed properties | [SET wiped my properties](#set-wiped-my-properties) |
| `DELETE` complains about edges | [DELETE fails](#delete-fails-with-still-has-relationships) |
| Parameters seem ignored | [Parameters](#parameters) |
| Server won't start | [Server](#server) |
| Admin snapshot endpoint returns 404 | [Snapshots → `/admin/snapshot/*` returns 404](#admin-snapshot-returns-404) |
| A `.tmp` file is left beside the snapshot | [Snapshots → leftover `.tmp` file](#leftover-tmp-file-beside-the-snapshot) |
| Snapshot load fails with "bad magic" or "bad CRC" | [Snapshots → load fails with bad magic / CRC](#snapshot-load-fails-with-bad-magic-or-crc) |
| Snapshot load reports unsupported version | [Snapshots → unsupported format version](#snapshot-load-reports-unsupported-format-version) |
| Result JSON shape is wrong | [Result format](#result-json-looks-nothing-like-what-i-expected) |
| Build fails | [Build](#build) |

## Build

### `error: linker 'cc' not found`

Install a C toolchain. On macOS:

```bash
xcode-select --install
```

### Slow release builds

Release builds use `lto = "fat"` and `codegen-units = 1`. For faster
iteration, use debug builds:

```bash
cargo build            # debug — fast
cargo build --release  # release — slow, optimised
```

## Server

### `Address already in use`

Another process holds the server port (default `4747`):

```bash
lsof -i :4747
kill <PID>
```

Or start on a different port:

```bash
lora-server --port 5000
# or
LORA_SERVER_PORT=5000 lora-server
```

See [HTTP Server → run](./getting-started/server#configure) for all options.

### HTTP 400 on every request

Check the `content-type` header — the server expects `application/json`:

```bash
curl -s http://127.0.0.1:4747/query \
  -H 'content-type: application/json' \
  -d '{"query": "MATCH (n) RETURN count(*)"}'
```

### Result JSON looks nothing like what I expected

Different `format` values return different shapes. The engine default
is `graph`, which returns deduplicated nodes+edges — if you were
expecting rows, pass `"format": "rows"` (or `"rowArrays"`). See
[**Result formats**](./concepts/result-formats) for every shape and
when to pick which.

## Queries

### Parse errors

Common mistakes:

- Missing parentheses: `MATCH n` → `MATCH (n)`.
- Missing direction on `CREATE`: `(a)-[:T]-(b)` is valid in `MATCH`,
  not in [`CREATE`](./queries/create). Use `-[:T]->` or `<-[:T]-`.
- Missing type on `CREATE`: `(a)-[]->(b)` must have a type, e.g.
  `-[:FOLLOWS]->`.
- `BETWEEN` is **not** supported — use `x >= a AND x <= b`. See
  [Limitations](./limitations#operators-and-expressions).
- Unclosed string literal — double the quote to escape:
  `'it''s fine'`. See [Scalars → String](./data-types/scalars#string).

### Semantic errors

| Message | Cause |
|---|---|
| `Unknown label :Foo` | No node with that label exists yet; populate the graph first or use [`CREATE`](./queries/create). |
| `Unknown variable x` | `x` wasn't introduced by an earlier clause, or it was dropped by a [`WITH`](./queries/return-with#with) that didn't project it. |
| `Unsupported feature: CALL` | `CALL` / procedures aren't implemented — see [Limitations](./limitations). |
| `Unknown function 'foo'` | Not in the built-in list. See [Functions](./functions/overview). |
| `WrongArity` | Function exists but was called with the wrong number of arguments. |
| `Aggregate in WHERE` | Aggregates aren't allowed in [`WHERE`](./queries/where). Use [`WITH … WHERE`](./queries/return-with#having-style-filtering-with). |

### Executor errors

| Message | Cause |
|---|---|
| `DeleteNodeWithRelationships` | Use [`DETACH DELETE`](./queries/set-delete#detach-delete) instead of plain `DELETE`. |
| `MissingRelationshipType` | `CREATE (a)-[]->(b)` — a [relationship](./concepts/relationships) must have a type. |
| `ReadOnlyCreate` | Should not occur via normal paths; filed bug if you see this. |

### Queries return empty results

1. **Data was created on a different non-persistent handle.** Plain
   in-memory databases start empty on each process run. Use a
   archive-backed open (`createDatabase("app", { databaseDir: "./data" })`, `Database.create("app", {"database_dir": "./data"})`,
   `lora.New("app", lora.Options{DatabaseDir: "./data"})`, etc.) or load a snapshot if you expect data to
   survive restarts. See [Limitations → Storage](./limitations#storage).
2. **Label case mismatch** — `:user` ≠ `:User`. Labels and types are
   case-sensitive. See [Nodes](./concepts/nodes).
3. **Property type mismatch** — `{id: 1}` matches integer `1`, not the
   string `"1"`. See [Data Types](./data-types/overview).
4. **A parameter is unbound** — missing parameters resolve to `null`,
   which usually filters everything out. See
   [Parameters](./queries/parameters).
5. **`= null`** — never matches. Use
   [`IS NULL` / `IS NOT NULL`](./queries/where#null-checks).
6. **Regex anchoring** — `=~ 'foo'` matches only the full string
   `"foo"`. Use `.*` or `CONTAINS` for substring. See
   [WHERE → regex](./queries/where#regex).

### `MATCH` returns a cross-product

```cypher
MATCH (a:User), (b:User) RETURN a, b    -- N * N rows
```

Use a relationship pattern to connect them:

```cypher
MATCH (a:User)-[:FOLLOWS]->(b:User) RETURN a, b
```

Or scope both sides before the write:

```cypher
MATCH (a:User {id: $from}), (b:User {id: $to})
CREATE (a)-[:FOLLOWS]->(b)
```

### `SET` wiped my properties

[`SET n = {…}`](./queries/set-delete#replace-all-properties-)
**replaces** the property map. To update individual keys:

```cypher
SET n.prop = value         -- single key
SET n += {newProp: value}  -- merge keys
```

### `DELETE` fails with "still has relationships"

```cypher
MATCH (n:User {id: 1}) DETACH DELETE n
```

[`DETACH DELETE`](./queries/set-delete#detach-delete) removes the edges
in one step.

### WITH dropped my variable

A variable must be explicitly projected through `WITH`:

```cypher
MATCH (a)-[r:KNOWS]->(b)
WITH a                     -- r and b are now out of scope
RETURN a, r                -- error: Unknown variable r
```

Either pass them through — `WITH a, r, b` — or don't bind them in the
first place.

### Aggregation gave one row when I expected per-group

Every non-aggregated column in the same `RETURN` becomes part of the
implicit group key. See
[Aggregation → Grouping](./queries/aggregation#grouping).

### Ordering puts nulls in an unexpected place

`null` sorts last ASC / first DESC. Override with
[`coalesce`](./functions/overview#type-conversion-and-checking):

```cypher
MATCH (p:Person)
RETURN p.name, p.rank
ORDER BY coalesce(p.rank, 2147483647) ASC
```

See [Ordering → nulls in ordering](./queries/ordering#nulls-in-ordering).

### Shortest path returns nothing

1. No path exists between the endpoints.
2. The relationship type filter excludes the only path.
3. Direction is too strict — try `[:R*]` or `[:R*]-` (undirected) on
   `MATCH`.

Wrap in [`OPTIONAL MATCH`](./queries/match#optional-match) if you still
want a row:

```cypher
MATCH (a:User {id: $from}), (b:User {id: $to})
OPTIONAL MATCH p = shortestPath((a)-[:FOLLOWS*]->(b))
RETURN a, b, length(p) AS hops
```

## Snapshots

### `/admin/snapshot/*` returns 404 {#admin-snapshot-returns-404}

**Symptom:** `POST /admin/snapshot/save` or `/admin/snapshot/load`
returns `404 Not Found`.

**Likely cause:** The server was not started with
`--snapshot-path <PATH>` (or the `LORA_SERVER_SNAPSHOT_PATH` env
var). The admin routes are **opt-in** — they are not mounted at all
when that flag is unset.

**Fix:** Restart with the flag, or use a per-binding
`save_snapshot` call instead.

```bash
lora-server \
  --host 127.0.0.1 --port 4747 \
  --snapshot-path /var/lib/lora/db.bin
```

See [HTTP server → Snapshots, WAL, and restore](./getting-started/server#snapshots-wal-and-restore)
and the [HTTP API reference](./api/http#admin-endpoints-opt-in).

### Leftover `.tmp` file beside the snapshot

**Symptom:** A file named `<path>.tmp` sits next to the target
snapshot file.

**Likely cause:** A save was interrupted — `SIGKILL`, a power loss,
or `ENOSPC` on the target disk. The save writes to `<path>.tmp`,
`fsync`s, and renames over the target in one atomic step. If the
process died between the write and the rename, the tmp remains.

**Fix:** If the target `<path>` still exists, it is valid and loads
cleanly — the last successful save. Delete the `<path>.tmp` and
investigate whatever killed the process (disk space? OOM? operator
error?). If the target does **not** exist, the tmp is your most
recent attempt but has not been atomically committed — rename it
and try loading; if CRC validation fails, restore from an earlier
backup.

### Snapshot load fails with "bad magic" or "bad CRC" {#snapshot-load-fails-with-bad-magic-or-crc}

**Symptom:** `SnapshotError::BadMagic` or `SnapshotError::BadCrc`
on load.

**Likely cause:**
- **Bad magic** — the file is not a LoraDB snapshot. The first 8
  bytes should be `LORASNAP`.
- **Bad CRC** — the file is corrupt (truncated, bit-flipped, or an
  unrelated file matching the magic by accident).

**Fix:**

```bash
# Confirm it looks like a snapshot at all.
head -c 8 path/to/snapshot.bin
# => LORASNAP
```

If the magic is wrong, check you pointed at the right path. If the
magic is right but CRC fails, restore from a known-good copy — a
corrupt snapshot never loads partially on purpose, to prevent
silently accepting half a graph. See the [Snapshots operator
doc (internal)](https://github.com/lora-db/lora/blob/main/docs/operations/snapshots.md#file-format)
for the on-disk layout.

### Snapshot load reports unsupported format version

**Symptom:** `SnapshotError::UnsupportedVersion` on load.

**Likely cause:**
- The file was written by a **newer** LoraDB than the reader — the
  reader is older than the writer.
- The file was written by an **obsolete** LoraDB whose format has
  since been retired (the reader's `SNAPSHOT_MIN_SUPPORTED_FORMAT_VERSION`
  has been raised above the file's version).

**Fix:**
- If the reader is older: upgrade the reader to a release that
  understands the file's format.
- If the file is from a retired format: use the last LoraDB release
  that accepted that version, export via Cypher (`MATCH (n) RETURN n`
  and `MATCH (a)-[r]->(b) RETURN id(a), id(b), type(r), properties(r)`),
  and re-import on the new release.

Version / compatibility policy (internal):
[Change management → Snapshot format compatibility](https://github.com/lora-db/lora/blob/main/docs/design/change-management.md#snapshot-format-compatibility).

## WAL and checkpoints

### `/admin/wal/*` or `/admin/checkpoint` returns 404

**Symptom:** `POST /admin/wal/status`,
`POST /admin/wal/truncate`, or `POST /admin/checkpoint` returns
`404 Not Found`.

**Likely cause:** The server was not started with `--wal-dir <DIR>`
(or the `LORA_SERVER_WAL_DIR` env var). WAL admin routes are mounted
only when a WAL directory is attached.

**Fix:** Restart with a WAL directory:

```bash
lora-server \
  --host 127.0.0.1 --port 4747 \
  --wal-dir /var/lib/lora/wal
```

See [WAL and checkpoints](./wal) and
[HTTP API → Admin endpoints](./api/http#admin-endpoints-opt-in).

### `/admin/checkpoint` returns 400

**Symptom:** `POST /admin/checkpoint` returns
`400 Bad Request` with a message about no checkpoint path.

**Likely cause:** The server has `--wal-dir` but no
`--snapshot-path`, and the request body did not provide a `path`.
WAL-only deployments can checkpoint, but the target snapshot path must
come from the request body.

**Fix:** Either pass `path` in the request:

```bash
curl -sX POST http://127.0.0.1:4747/admin/checkpoint \
  -H 'content-type: application/json' \
  -d '{"path":"/var/lib/lora/checkpoint.bin"}'
```

or start the server with `--snapshot-path` so body-less checkpoints
have a default target.

### WAL/archive root is already open

**Symptom:** Opening a WAL-backed or archive-backed database fails with
an error that the WAL/archive root is already open by another live handle.

**Likely cause:** Another live process or database handle already owns
that WAL directory or `.loradb` archive. LoraDB takes a lock so two appenders
cannot write to the same log at once.

**Fix:** Use one WAL/archive root per live database, or close the first
handle before reopening the same directory in the same process:
`db.dispose()` in Node, `db.close()` / `await db.close()` in Python,
`db.Close()` in Go, or `db.close` in Ruby.

### WAL is poisoned or `bgFailure` is set

**Symptom:** Queries fail with `WAL poisoned`, `WAL flush failed`, or
`/admin/wal/status` returns a non-null `bgFailure`.

**Likely cause:** A WAL append or fsync failed. In `group` sync mode,
the background flusher latches the first fsync failure so later writes
fail loudly instead of pretending they are durable.

**Fix:** Stop accepting writes, fix the underlying disk or permission
problem, then restart from the last consistent snapshot + WAL. Inspect
`/admin/wal/status` before restart if you need the latched error text
for logs.

### Recovery warns about an older snapshot

**Symptom:** Startup prints a warning that the snapshot LSN is older
than the newest checkpoint marker in the WAL.

**Likely cause:** The WAL contains evidence of a newer checkpoint than
the snapshot passed to `--restore-from` or `Database::recover(...)`.

**Fix:** If you have the newer checkpoint snapshot, restore from that
file instead. If not, the current recovery is still safe: LoraDB
replays every committed WAL record above the snapshot's own `walLsn`,
which may take longer but preserves correctness.

## Parameters

### Why are my queries returning nothing?

Missing [parameters](./queries/parameters) resolve to `null`, which
usually filters everything out. Verify every `$name` in your query has
a corresponding entry in the params map passed to
`execute_with_params`.

### The HTTP API ignored my parameters

`POST /query` does not currently accept a `params` body field — see
[Limitations](./limitations). Bind parameters via the Rust / Node /
Python APIs for now.

### Integer precision lost in JS

JS `number` loses precision above `Number.MAX_SAFE_INTEGER` (2^53).
Use `bigint` parameters or string-encoded ids for large values. See
[Node → gotchas](./getting-started/node#performance--best-practices).

## Performance

### Query is slow on a big graph

- No property indexes — `MATCH ({id: 1})` is `O(n)`. Scope to a label
  (`MATCH (n:L {id: 1})`) to narrow the search.
- Unbounded variable-length traversals explode fast. Cap with a max
  depth: `[:R*1..6]`.
- `ORDER BY` on a huge unbounded result requires a full sort. Pair
  with `LIMIT`.
- See [Limitations → Storage](./limitations#storage) for the full list
  of storage gaps.

### Queries block each other

LoraDB serialises queries on a single mutex. There is no concurrent
read execution. See [Limitations → Concurrency](./limitations#concurrency).

## Debugging query pipelines

A Cypher query is a pipeline — each clause feeds its rows into the
next. When a query returns surprising results the bug is almost
always **between stages**, not within a single clause. The cure is to
step through the pipeline and inspect what each stage emits.

### The golden rule

> If a query doesn't return what you expect, read it clause by
> clause and ask: _what does this stage produce, and which variables
> are in scope after it?_

### Variable scope across WITH

A variable leaves scope the moment it isn't projected through `WITH`.
Future clauses can't see it.

**Symptom:** `Unknown variable r` / `Unknown variable b`.

**Likely cause:** A `WITH` between `MATCH` and `RETURN` dropped the
variable.

**Fix:** Project every variable you need downstream.

**Example:**

```cypher
-- Broken
MATCH (a)-[r:KNOWS]->(b)
WITH a                    -- r and b are now out of scope
RETURN a, r, b            -- error

-- Fixed
MATCH (a)-[r:KNOWS]->(b)
WITH a, r, b
RETURN a, r, b
```

See [WITH — losing variables](./queries/return-with#losing-variables-through-with).

### `WITH *` vs explicit projection

`WITH *` passes every in-scope variable forward — convenient but
easy to misuse. The instant you add a computed column, you need to
enumerate the existing variables too, otherwise they silently drop.

**Symptom:** A variable that existed a moment ago is suddenly
unknown.

**Likely cause:** Someone wrote `WITH x AS renamed` expecting the
other variables to survive.

**Fix:** Either use `WITH *, x AS renamed` or list every needed
variable explicitly.

**Example:**

```cypher
-- Broken — drops u
MATCH (u:User)-[:WROTE]->(p:Post)
WITH count(p) AS posts
RETURN u.name, posts          -- error: u is not in scope

-- Fixed
MATCH (u:User)-[:WROTE]->(p:Post)
WITH u, count(p) AS posts
RETURN u.name, posts
```

The fix is not `WITH *` — aggregates plus `WITH *` together cause
different trouble, because the aggregate needs an explicit grouping
key column.

### Variable loss between stages

**Symptom:** Second `MATCH` in a multi-stage query returns zero rows.

**Likely cause:** A `WITH` stage narrowed the rows to a subset, and
subsequent `MATCH` clauses only run for the surviving rows.

**Fix:** Push the second `MATCH` earlier, or remove the narrowing.

**Example:**

```cypher
-- "Only see friends of the top-3 oldest users" — works
MATCH (u:User)
WITH u ORDER BY u.age DESC LIMIT 3
MATCH (u)-[:FOLLOWS]->(other)
RETURN u.name, other.name

-- Likely a bug — the LIMIT 3 applies too soon
MATCH (u:User)
WITH u LIMIT 3
MATCH (u)-[:FOLLOWS]->(other:User)
WHERE other.active
RETURN u, other
```

The second `MATCH` sees at most three users; if none of their
follows are `active`, the whole query is empty. Push the filter up
into the first `WITH`, or don't `LIMIT` yet.

### Debugging inside the pipeline

Print-debug a pipeline by swapping the final `RETURN` for one that
exposes intermediate state:

```cypher
-- Inspect what WITH is emitting
MATCH (u:User)
WITH u.country AS country, count(*) AS n
RETURN country, n
ORDER BY n DESC
```

Then paste the rows into a spreadsheet — spotting duplicate keys, a
missing `country`, or an unexpected cardinality often takes five
seconds once you can see the rows.

## CASE expression pitfalls

[`CASE`](./queries/return-with#case-expressions) is powerful but its
rules interact with three-valued logic and type-mixing in ways that
surprise new users.

### Missing ELSE returns null

**Symptom:** A derived column has `null` values you didn't expect.

**Likely cause:** The `CASE` has no `ELSE` branch and one of the
input rows doesn't satisfy any `WHEN`.

**Fix:** Add an explicit `ELSE`, or accept `null` as the implicit
default.

**Example:**

```cypher
-- Broken — users with score < 50 become null
MATCH (u:User)
RETURN u.name,
       CASE WHEN u.score >= 50 THEN 'ok' END AS tier

-- Fixed
MATCH (u:User)
RETURN u.name,
       CASE WHEN u.score >= 50 THEN 'ok' ELSE 'low' END AS tier
```

### Null in the predicate

`CASE WHEN expr` treats a `null` `expr` as *not matching* — because
three-valued logic propagates.

**Symptom:** Rows with `null` properties land in the `ELSE` branch,
even though the condition wasn't explicitly false.

**Likely cause:** `u.score >= 50` is `null` (not `false`) when
`u.score` is `null`; the branch doesn't fire.

**Fix:** Guard with [`coalesce`](./functions/overview#type-conversion-and-checking)
or an explicit `IS NULL` branch placed **before** the numeric
comparison.

```cypher
MATCH (u:User)
RETURN u.name,
       CASE
         WHEN u.score IS NULL  THEN 'unknown'
         WHEN u.score >= 50    THEN 'ok'
         ELSE                       'low'
       END AS tier
```

### Inconsistent branch types

**Symptom:** A downstream `ORDER BY` or comparison misbehaves on
the `CASE` column.

**Likely cause:** Different branches return different types — e.g.
an `Int` in one branch and a `String` in another.

**Fix:** Make every branch return the same type. If you genuinely
need heterogeneous output, convert with `toString`.

**Example:**

```cypher
-- Mixed types — results downstream-unpredictable
CASE WHEN n.score >= 50 THEN n.score ELSE 'unknown' END

-- Fixed
CASE WHEN n.score >= 50 THEN toString(n.score) ELSE 'unknown' END
```

### Simple vs generic form confusion

Simple form (`CASE x WHEN v THEN …`) compares `x` against values
using equality. It can't express ranges or boolean predicates per
branch — that's the generic form (`CASE WHEN pred THEN …`).

```cypher
-- Doesn't work — comparison is hidden inside the simple form
CASE p.age WHEN >= 18 THEN 'adult' ELSE 'minor' END

-- Use the generic form
CASE WHEN p.age >= 18 THEN 'adult' ELSE 'minor' END
```

## WITH clause pitfalls

### Aggregate without an explicit group key

**Symptom:** The aggregate returns a single row when you expected
one row per group.

**Likely cause:** There's no non-aggregated column in the `WITH` or
`RETURN`, so all rows collapse to a single group.

**Fix:** Add the grouping column.

**Example:**

```cypher
-- "Orders per region" — one row total
MATCH (o:Order)
RETURN count(*)

-- Fixed — one row per region
MATCH (o:Order)
RETURN o.region, count(*)
ORDER BY count(*) DESC
```

### Implicit group key by accident

The opposite surprise: an aggregate query that returns many rows
because a non-aggregated column you didn't realise was there formed
part of the key.

**Symptom:** A `count(*)` query returns many rows instead of one.

**Likely cause:** You projected something extra (e.g. the `Node`
itself) alongside the aggregate, and each node became its own group.

**Fix:** Drop the extra column.

```cypher
-- Broken: returns one row per user
MATCH (u:User)
RETURN u, count(*)

-- Fixed: single total
MATCH (u:User)
RETURN count(*)
```

### Aggregates in WHERE

**Symptom:** `Aggregate in WHERE` analysis error.

**Likely cause:** Aggregates aren't allowed in `WHERE`. Cypher has
no `HAVING` keyword.

**Fix:** Pipe through `WITH` and filter after.

```cypher
-- Broken
MATCH (u:User)-[:WROTE]->(p:Post)
WHERE count(p) > 5
RETURN u

-- Fixed (HAVING-style)
MATCH (u:User)-[:WROTE]->(p:Post)
WITH u, count(p) AS posts
WHERE posts > 5
RETURN u, posts
```

See [WITH — HAVING-style filtering](./queries/return-with#having-style-filtering-with).

### Ordering a `WITH` drops the order downstream?

**Symptom:** You sort in a `WITH` stage and the final result comes
back unsorted.

**Likely cause:** `ORDER BY` attached to `WITH` only guarantees
ordering for that stage's output and for any `ORDER BY`-sensitive
aggregate that immediately consumes it (such as `collect`). A
subsequent `MATCH` then re-emits rows in no particular order.

**Fix:** Either `collect` in the sorted stage, or re-apply `ORDER
BY` on the final `RETURN`.

```cypher
-- Broken — final order is unspecified
MATCH (u:User)
WITH u ORDER BY u.created DESC
MATCH (u)-[:WROTE]->(p)
RETURN u.name, count(p)

-- Fixed
MATCH (u:User)
WITH u ORDER BY u.created DESC
MATCH (u)-[:WROTE]->(p)
RETURN u.name, count(p)
ORDER BY u.created DESC
```

## Aggregation pitfalls

### `count(*)` vs `count(expr)`

**Symptom:** `OPTIONAL MATCH` aggregation yields `1` for entities
that should be `0`.

**Likely cause:** `count(*)` counts rows. An `OPTIONAL MATCH` that
missed still produces a row with `null` bindings — `count(*)`
counts it.

**Fix:** Use `count(expr)` on a variable from the optional side.
`count(expr)` skips `null`.

**Example:**

```cypher
-- Broken — users with no posts get 1
MATCH (u:User)
OPTIONAL MATCH (u)-[:WROTE]->(p:Post)
RETURN u.name, count(*) AS posts

-- Fixed
MATCH (u:User)
OPTIONAL MATCH (u)-[:WROTE]->(p:Post)
RETURN u.name, count(p) AS posts
```

### Missing DISTINCT in collect

**Symptom:** `collect` returns duplicate values.

**Likely cause:** A many-to-many join upstream produced the same
child multiple times, once per ancestor.

**Fix:** `collect(DISTINCT …)`.

**Example:**

```cypher
-- Broken — same city listed many times if the person visited it often
MATCH (p:Person)-[:VISITED]->(c:City)
RETURN p.name, collect(c.name) AS cities

-- Fixed
MATCH (p:Person)-[:VISITED]->(c:City)
RETURN p.name, collect(DISTINCT c.name) AS cities
```

### Aggregating after filtering vs after projection

**Symptom:** Aggregate seems to include rows the `WHERE` should
have excluded.

**Likely cause:** The `WHERE` runs against post-aggregate output,
not the rows that fed the aggregate — because the query put it in
the wrong stage.

**Fix:** If you want the filter *before* the aggregate, place it in
a pre-aggregate `WHERE`. If you want it *after* (HAVING-style),
pipe through `WITH`.

```cypher
-- Pre-aggregate filter (input rows only)
MATCH (o:Order)
WHERE o.status = 'paid'
RETURN o.region, sum(o.amount) AS revenue

-- Post-aggregate filter (computed totals only)
MATCH (o:Order)
WITH o.region AS region, sum(o.amount) AS revenue
WHERE revenue > 1000
RETURN region, revenue
```

### `stdev`/`percentile*` don't support DISTINCT

**Symptom:** `stdev(DISTINCT …)` / `percentileCont(DISTINCT …)` fails
with an analysis error.

**Likely cause:** These aggregates don't support `DISTINCT`
directly (see [Limitations](./limitations#aggregates)).

**Fix:** `collect(DISTINCT …)`, `UNWIND`, then aggregate.

```cypher
-- Broken
MATCH (r:Review) RETURN stdev(DISTINCT r.stars)

-- Fixed
MATCH (r:Review)
WITH collect(DISTINCT r.stars) AS xs
UNWIND xs AS x
RETURN stdev(x)
```

## Empty results and filtering issues

### Silent filter from an unbound parameter

**Symptom:** Query returns zero rows in production but works in the
local REPL.

**Likely cause:** A `$param` isn't bound. Unbound parameters resolve
to `null`, which silently filters out every row.

**Fix:** Audit parameter bindings on the host side before executing.

```cypher
MATCH (u:User) WHERE u.id = $id RETURN u
-- If $id is not bound, this returns zero rows without raising
```

### `= null` never matches

**Symptom:** A predicate intended to match missing properties
returns zero rows.

**Likely cause:** `prop = null` is always `null`. Use
`IS NULL` / `IS NOT NULL`.

**Fix:**

```cypher
-- Broken
MATCH (n) WHERE n.optional = null RETURN n

-- Fixed
MATCH (n) WHERE n.optional IS NULL RETURN n
```

### Regex anchored by default

`=~ 'foo'` matches only the *full* string `foo`, not any string
containing `foo`. Use `=~ '.*foo.*'` or `CONTAINS 'foo'` for
substring matching.

### Case-sensitive when you meant insensitive

All string operators (`=`, `STARTS WITH`, `ENDS WITH`, `CONTAINS`)
are case-sensitive. Normalise both sides with `toLower`.

```cypher
MATCH (u:User)
WHERE toLower(u.email) = toLower($candidate)
RETURN u
```

## Duplicate results

### Pattern reached twice

**Symptom:** The same node appears in results multiple times.

**Likely cause:** Two different paths reach the same node; the
pattern matches each path independently.

**Fix:** Use [`DISTINCT`](./queries/return-with#distinct) on the
`RETURN`, or restructure with `EXISTS { }` when you only need
existence.

```cypher
-- May duplicate `c` if a is connected to many b
MATCH (a:Person)-[:FOLLOWS]->(b)-[:FOLLOWS]->(c)
RETURN a, c

-- One row per distinct (a, c) pair
MATCH (a:Person)-[:FOLLOWS]->(b)-[:FOLLOWS]->(c)
RETURN DISTINCT a, c
```

### Undirected match doubles symmetric pairs

**Symptom:** Every pair appears twice in the results.

**Likely cause:** `(a)-[:R]-(b)` matches both directions. A pattern
that's symmetric in `a` / `b` matches each pair twice.

**Fix:** Filter with `id(a) < id(b)` (or `<>`).

```cypher
MATCH (a:Person)-[:KNOWS]-(b:Person)
WHERE id(a) < id(b)
RETURN a.name, b.name
```

## Debugging workflow (step-by-step)

When a query misbehaves, follow this loop. Every step is cheap.

### 1. Simplify the MATCH

Remove everything except the patterns. Check you get any rows at
all.

```cypher
MATCH (u:User) RETURN count(*)
```

Zero? Your label is wrong or the graph is empty. See
[Queries return empty results](#queries-return-empty-results).

### 2. Remove WHERE, inspect rows

Drop the `WHERE` and look at what the pattern actually binds.

```cypher
MATCH (u:User)-[:FOLLOWS]->(f)
RETURN u.handle, f.handle
LIMIT 20
```

Spot duplicates, unexpected relationships, or nulls here.

### 3. Reintroduce predicates one at a time

Add each `WHERE` clause back one at a time. Count rows at each step
— the step that drops too many rows is the bug.

```cypher
MATCH (u:User)-[:FOLLOWS]->(f)
WHERE u.active                  -- step 1
RETURN count(*)

MATCH (u:User)-[:FOLLOWS]->(f)
WHERE u.active
  AND f.country = u.country     -- step 2
RETURN count(*)
```

### 4. Inspect intermediate WITH stages

If your query has multiple stages, replace the final `RETURN` with
one that exposes the `WITH` stage output. Do this per stage.

```cypher
-- Instead of the full query:
MATCH (u:User)-[:WROTE]->(p:Post)
WITH u, count(p) AS posts
WHERE posts > 5
RETURN u.handle, posts

-- Inspect stage 1:
MATCH (u:User)-[:WROTE]->(p:Post)
RETURN u.handle, count(p) AS posts
ORDER BY posts DESC
LIMIT 20
```

Does stage 1 emit what you think? If not, the bug is before the
`WITH`.

### 5. Check parameter bindings

Confirm every `$param` the query uses is in the call. Unbound
parameters become `null` and silently filter.

### 6. Re-read the problem

If you've been through the loop twice and the result is still
wrong, the bug may be in the **data model**, not the query. See the
[Modelling checklist](./concepts/graph-model#modelling-checklist).

## See also

- [**Limitations**](./limitations) — what's intentionally not supported.
- [**Queries**](./queries/) — clause reference.
- [**Cheat sheet**](./queries/cheat-sheet) — one-page quick reference.
- [**Parameters**](./queries/parameters) — typed parameter binding.
- [**Result formats**](./concepts/result-formats) — each response shape in detail.
- [**Schema-free**](./concepts/schema-free) — strict reads, permissive writes.
- [**WHERE**](./queries/where) — predicate reference.
- [**RETURN / WITH**](./queries/return-with) — projection and HAVING.
- [**Aggregation (queries)**](./queries/aggregation) — clause-level grouping.
- [**Aggregation (functions)**](./functions/aggregation) — per-function details.
- [**Functions → Overview**](./functions/overview) — built-ins.
- [**Tutorial**](./getting-started/tutorial) — guided walkthrough from scratch.
