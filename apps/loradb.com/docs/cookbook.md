---
title: LoraDB Query Cookbook
sidebar_label: Cookbook
description: Scenario-driven Cypher recipes for LoraDB — social graphs, e-commerce, event streams, and geospatial queries — each with a concrete data model and working query.
---

# LoraDB Query Cookbook

A scenario-driven companion to the clause-by-clause
[**Queries reference**](./queries/). Reach for this page when you
know the question ("who are my mutual follows?", "which orders are
late?") but aren't sure how to shape the Cypher. Where the reference
answers *"what does this clause do"*, the cookbook answers *"how do
I ask this question"*.

Each recipe names a real problem, states its assumed data model,
gives a query, and explains why it works — then lists useful
variations and related concepts. Recipes are grouped by domain:
social, e-commerce, events, geospatial, vector retrieval. Every
query is idiomatic LoraDB — no APOC, no `CALL`, no window functions
— and when a SQL idiom doesn't translate, the recipe shows the
Cypher-native shape.

## On this page

- [Social graph patterns](#social-graph-patterns)
- [E-commerce patterns](#e-commerce-patterns)
- [Event / time-based patterns](#event--time-based-patterns)
- [Geospatial patterns](#geospatial-patterns)
- [Vector-retrieval patterns](#vector-retrieval-patterns)
- [Backup and restore](#backup-and-restore)
- [See also](#see-also)

---

## Social graph patterns

### Recipe: Friends of friends

#### Problem

"For user `$id`, list people they don't follow yet, but who are
followed by somebody they do follow."

#### Assumed data model

- `:User {id, handle}`
- `(u:User)-[:FOLLOWS]->(v:User)` — directed "u follows v"

#### Query

```cypher
MATCH (me:User {id: $id})-[:FOLLOWS]->(friend:User)-[:FOLLOWS]->(candidate:User)
WHERE candidate <> me
  AND NOT EXISTS { (me)-[:FOLLOWS]->(candidate) }
RETURN candidate.handle,
       count(*) AS shared_paths
ORDER BY shared_paths DESC
LIMIT 20
```

#### Explanation

The two-hop pattern enumerates every `(me, friend, candidate)`
triple. `NOT EXISTS` removes candidates `me` already follows;
`candidate <> me` removes self-loops. `count(*)` becomes the number
of mutual friends — a useful ranking signal.

#### Variations

- Also consider follow**ers**: reverse the last segment to
  `(candidate)-[:FOLLOWS]->(friend)`.
- Combine both directions with a [`UNION`](./queries/return-with#union--union-all)
  and dedup with [`collect(DISTINCT …)`](./functions/aggregation#collect).
- Weight by recency of `friend`'s follow using a `:FOLLOWS {since}`
  property — see the e-commerce [co-purchase recipe](#recipe-co-purchase-patterns).

#### Related concepts

- [MATCH — multi-hop patterns](./queries/match#multi-hop-patterns)
- [WHERE — pattern existence](./queries/where#pattern-existence)
- [Aggregation — count](./queries/aggregation#count)

---

### Recipe: Mutual connections

#### Problem

"List pairs of users who follow each other."

#### Assumed data model

- `:User`
- `(a)-[:FOLLOWS]->(b)` — directed

#### Query

```cypher
MATCH (a:User)-[:FOLLOWS]->(b:User)-[:FOLLOWS]->(a)
WHERE id(a) < id(b)
RETURN a.handle, b.handle
```

#### Explanation

The pattern reads "a follows b and b follows a" — a 2-cycle. The
`id(a) < id(b)` predicate keeps one row per unordered pair; without
it you'd get both `(alice, bob)` and `(bob, alice)`.

#### Variations

- Add a property filter: only mutuals where both accounts are
  active — add `WHERE a.active AND b.active`.
- Rank by "oldest mutuals first" by collecting the relationship:
  `MATCH (a)-[r1:FOLLOWS]->(b)-[r2:FOLLOWS]->(a)` and order by
  `min(r1.since, r2.since)` — see
  [ORDER BY](./queries/ordering#ordering-by-computed-expression).

#### Related concepts

- [Nodes → id() usage](./functions/overview#entity-introspection)
- [Concepts → relationships direction conventions](./concepts/relationships#direction-conventions)

---

### Recipe: Recommendations via second-degree connections

#### Problem

"Recommend accounts for `$id` to follow, ranked by how many of their
current follows already follow that account."

#### Assumed data model

- `:User`
- `(u)-[:FOLLOWS]->(v)`

#### Query

```cypher
MATCH (me:User {id: $id})-[:FOLLOWS]->(:User)-[:FOLLOWS]->(rec:User)
WHERE rec <> me
  AND NOT EXISTS { (me)-[:FOLLOWS]->(rec) }
RETURN rec.handle,
       count(*) AS score,
       collect(DISTINCT rec.country)[..3] AS sample_countries
ORDER BY score DESC
LIMIT 10
```

#### Explanation

Similar to [friends-of-friends](#recipe-friends-of-friends) but
projects a richer result — the count becomes a recommendation score,
and we collect a diverse sample of metadata. `[..3]` slices the
collected list to the first three entries — a
[list slice](./functions/list#indexing-and-slicing).

#### Variations

- Add a CASE-based boost for accounts with a `verified` badge:
  `count(CASE WHEN rec.verified THEN 1 END) * 2 + count(*)`.
- Expand to 3-hop with `[:FOLLOWS*2..3]` — bound it to avoid
  run-away — see [Paths → variable-length](./queries/paths#variable-length-relationships).

#### Related concepts

- [CASE expressions](./queries/return-with#case-expressions)
- [Count-if via CASE](./queries/aggregation#conditional-count-count-if)

---

### Recipe: Influence score (weighted walk)

#### Problem

"For each user, estimate an influence score based on how many
followers reach them within two hops."

#### Assumed data model

- `:User`
- `(follower)-[:FOLLOWS]->(followee)` — standard directed follow

#### Query

```cypher
MATCH (u:User)
OPTIONAL MATCH (u)<-[:FOLLOWS*1..2]-(reacher:User)
RETURN u.handle,
       count(DISTINCT reacher) AS reach
ORDER BY reach DESC
LIMIT 20
```

#### Explanation

`OPTIONAL MATCH` preserves users with zero reach. `count(DISTINCT
reacher)` avoids double-counting any user reached through multiple
paths. Bounded at two hops to stay tractable.

#### Variations

- Replace `reacher` with `count(DISTINCT length(p))` paths of
  different lengths to reveal reach-at-each-distance.
- Combine with `:FOLLOWS {since}` weights:
  `sum(exp(-duration.inDays(r.since, datetime()) * 0.02))`.

#### Related concepts

- [OPTIONAL MATCH with aggregation](./queries/match#optional-match-with-aggregation)
- [count(expr)](./functions/aggregation#count)

---

## E-commerce patterns

### Recipe: Top N products by revenue

#### Problem

"The ten products with the highest paid-order revenue this month."

#### Assumed data model

- `:Product {id, name, price}`
- `:Order {id, placed_at, status}`
- `(o)-[:CONTAINS {quantity}]->(p)`

#### Query

```cypher
MATCH (o:Order)-[c:CONTAINS]->(p:Product)
WHERE o.status = 'paid'
  AND o.placed_at >= date.truncate('month', date())
RETURN p.name,
       sum(c.quantity * p.price) AS revenue,
       sum(c.quantity)           AS units,
       count(DISTINCT o)         AS orders
ORDER BY revenue DESC
LIMIT 10
```

#### Explanation

Joins orders and products through the `:CONTAINS` edge, filters to
paid orders in the current month, then aggregates revenue per
product. `count(DISTINCT o)` captures orders (an order can contain
many items), distinct from the total `units` sold.

#### Variations

- Top-N per category: group by `p.category` as well and then
  pick-first within each using
  [`collect(…)[..1]`](./queries/aggregation#top-contributor-per-group-pipeline-trick).
- Year-to-date: replace the date filter with
  `o.placed_at >= date.truncate('year', date())`.

#### Related concepts

- [Aggregation walkthrough](./queries/aggregation#a-five-step-walkthrough)
- [Temporal → truncation](./functions/temporal#truncation)

---

### Recipe: Co-purchase patterns

#### Problem

"Which products are frequently bought together with `$sku`?"

#### Assumed data model

- `:Product {sku}`
- `:Order`
- `(o)-[:CONTAINS]->(p)`

#### Query

```cypher
MATCH (anchor:Product {sku: $sku})<-[:CONTAINS]-(:Order)-[:CONTAINS]->(other:Product)
WHERE other <> anchor
RETURN other.sku,
       count(*) AS co_orders
ORDER BY co_orders DESC
LIMIT 20
```

#### Explanation

Traverses orders that contain the anchor product and jumps to other
products in the same order. `count(*)` is the number of orders in
which the co-occurrence happened.

#### Variations

- Only ship-completed orders: add `WHERE o.status = 'shipped'` after
  binding `(o:Order)`.
- Lift score over per-product baseline:
  `count(*) * 1.0 / ((MATCH (:Order)-[:CONTAINS]->(anchor)…))` — two
  stages combined via `WITH` — see
  [Aggregation → pipeline aggregation](./queries/aggregation#pipeline-aggregation).

#### Related concepts

- [WHERE — pattern filters](./queries/where#pattern-existence)
- [Ordering — Top-N](./queries/ordering#top-n)

---

### Recipe: Repeat buyers

#### Problem

"Users who placed more than one paid order."

#### Assumed data model

- `:User`
- `:Order {status}`
- `(u)-[:PLACED]->(o)`

#### Query

```cypher
MATCH (u:User)-[:PLACED]->(o:Order {status: 'paid'})
WITH u, count(o) AS orders
WHERE orders > 1
RETURN u.email,
       orders
ORDER BY orders DESC
```

#### Explanation

Classic HAVING-style pipeline: aggregate first, then filter the
aggregated column. Cypher has no `HAVING` keyword — pipe through
[`WITH`](./queries/return-with#with) instead.

#### Variations

- Lifetime value: replace `count(o)` with `sum(o.amount)`.
- Last-purchase recency: add `max(o.placed_at) AS last` and
  filter on that.

#### Related concepts

- [HAVING-style filtering](./queries/return-with#having-style-filtering-with)
- [MERGE upsert patterns](./queries/unwind-merge#merge)

---

### Recipe: Cart abandonment

#### Problem

"Users with an open cart containing more than `$n` items, but no
order placed in the last 30 days."

#### Assumed data model

- `:User`
- `:Cart {status}`, `:Item`
- `(u)-[:HAS_CART]->(c)`, `(c)-[:CONTAINS]->(:Item)`, `(u)-[:PLACED]->(:Order)`

#### Query

```cypher
MATCH (u:User)-[:HAS_CART]->(c:Cart {status: 'open'})-[:CONTAINS]->(i:Item)
WITH u, c, count(i) AS items
WHERE items > $n
  AND NOT EXISTS {
    (u)-[:PLACED]->(o:Order)
    WHERE o.placed_at >= datetime() - duration('P30D')
  }
RETURN u.email, c.id, items
ORDER BY items DESC
```

#### Explanation

The `NOT EXISTS { }` sub-pattern is an anti-join — keeps users
without any recent order. The surrounding pipeline filters open
carts above the threshold size.

#### Variations

- Add recency of cart update:
  `AND c.updated_at >= datetime() - duration('P7D')` for "stale but
  not abandoned".
- Compute total cart value: `sum(i.price * i.quantity) AS total`.

#### Related concepts

- [Pattern existence with NOT EXISTS](./queries/where#pattern-existence)
- [Temporal arithmetic](./functions/temporal#arithmetic)

---

## Event / time-based patterns

### Recipe: Attendees per event

#### Problem

"Total and unique attendees for each upcoming event, plus a flag
for whether the event is sold out."

#### Assumed data model

- `:Event {id, starts_at, capacity}`
- `:User`
- `(u)-[:RSVP {status}]->(e)`

#### Query

```cypher
MATCH (e:Event)
WHERE e.starts_at >= datetime()
OPTIONAL MATCH (u:User)-[r:RSVP {status: 'yes'}]->(e)
WITH e, count(u) AS going
RETURN e.id,
       e.starts_at,
       going,
       e.capacity,
       CASE
         WHEN going >= e.capacity THEN 'sold_out'
         WHEN going >= e.capacity * 0.8 THEN 'filling'
         ELSE 'open'
       END AS status
ORDER BY e.starts_at
```

#### Explanation

`OPTIONAL MATCH` ensures events with zero RSVPs still appear.
[`CASE`](./queries/return-with#case-expressions) computes a tiered
status per event.

#### Variations

- Break out by ticket tier:
  `collect(DISTINCT r.tier) AS tiers`.
- Include "maybe" RSVPs separately:
  `count(CASE WHEN r.status = 'maybe' THEN 1 END) AS maybes`.

#### Related concepts

- [CASE expressions](./queries/return-with#case-expressions)
- [OPTIONAL MATCH](./queries/match#optional-match)

---

### Recipe: Events in a rolling window

#### Problem

"All events between now and `$horizon_days` days in the future, with
their host."

#### Assumed data model

- `:Event {starts_at}`
- `:User` (host)
- `(u)-[:HOSTS]->(e)`

#### Query

```cypher
MATCH (host:User)-[:HOSTS]->(e:Event)
WHERE e.starts_at >= datetime()
  AND e.starts_at <  datetime() + duration({days: $horizon_days})
RETURN e.id,
       e.starts_at,
       host.handle
ORDER BY e.starts_at
```

#### Explanation

`duration({days: $horizon_days})` accepts a bound integer —
`duration('P7D')` only works with a literal string. Use the map
form whenever the window size is dynamic.

#### Variations

- Hour-bucket count to drive a chart:
  `datetime.truncate('hour', e.starts_at)` grouped with `count(*)` —
  see [Temporal → bucketing](./functions/temporal#bucketing-rows).
- Timezone-specific: store `starts_at` as `DateTime` (UTC-offset
  aware) — see [Temporal types](./data-types/temporal).

#### Related concepts

- [Temporal truncation](./functions/temporal#truncation)
- [Temporal types](./data-types/temporal)

---

### Recipe: Overlapping attendance

#### Problem

"For each pair of users who attended at least `$n` common events
this year, list the events they shared."

#### Assumed data model

- `:User`, `:Event`
- `(u)-[:ATTENDED]->(e)` with `e.at: DateTime`

#### Query

```cypher
MATCH (a:User)-[:ATTENDED]->(e:Event)<-[:ATTENDED]-(b:User)
WHERE id(a) < id(b)
  AND e.at >= date.truncate('year', date())
WITH a, b, collect(DISTINCT e.id) AS shared
WHERE size(shared) >= $n
RETURN a.handle, b.handle, shared, size(shared) AS n_shared
ORDER BY n_shared DESC
```

#### Explanation

The pattern generates all `(a, e, b)` triples for attendees of the
same event. `id(a) < id(b)` keeps one row per unordered pair.
`collect(DISTINCT e.id)` deduplicates events (an attendee can't
attend the same event twice but the pattern could still
double-count in more complex schemas).

#### Variations

- Use `count(DISTINCT e)` instead of `size(collect(…))` for a
  cheaper pairwise score when you don't need the event ids.
- Weight events by `e.importance`.

#### Related concepts

- [Symmetric-pair dedup](./queries/match#symmetric-pair-deduplication)
- [collect → distinct](./functions/aggregation#collect)

---

### Recipe: Cohort retention

#### Problem

"For each signup-month cohort, what fraction of users logged in
during the last 30 days?"

#### Assumed data model

- `:User {id, created}`
- `:Login {at}`
- `(u)-[:LOGGED_IN]->(l:Login)`

#### Query

```cypher
MATCH (u:User)
WITH date.truncate('month', u.created) AS cohort, u
OPTIONAL MATCH (u)-[:LOGGED_IN]->(l:Login)
WHERE l.at >= datetime() - duration('P30D')
RETURN cohort,
       count(DISTINCT u)                                   AS total,
       count(DISTINCT CASE WHEN l IS NOT NULL THEN u END)  AS active_30d,
       toFloat(count(DISTINCT CASE WHEN l IS NOT NULL THEN u END))
         / count(DISTINCT u)                               AS retention
ORDER BY cohort
```

#### Explanation

`OPTIONAL MATCH` preserves cohort members who never logged in.
Counting `DISTINCT u` gives cohort size; the conditional `CASE`
inside `count(DISTINCT …)` gives the active subset. The division
yields retention between 0 and 1.

#### Variations

- Slice by acquisition channel: add
  `WITH cohort, u.channel AS channel, u` and include `channel` in
  the `RETURN`.
- Wider windows: change `P30D` to `P90D` / `P365D`.

#### Related concepts

- [Temporal truncation](./functions/temporal#truncation)
- [CASE inside count](./queries/aggregation#conditional-count-count-if)

---

## Geospatial patterns

### Recipe: N nearest places

#### Problem

"Ten nearest venues to `$here`, with distance in metres."

#### Assumed data model

- `:Venue {id, name, location: Point}` — WGS-84 2D preferred

#### Query

```cypher
MATCH (v:Venue)
WITH v, distance(v.location, $here) AS metres
WHERE metres IS NOT NULL            -- guard cross-SRID
RETURN v.name, metres
ORDER BY metres
LIMIT 10
```

#### Explanation

[`distance`](./functions/spatial#distance) on same-SRID WGS-84
points returns metres. The null guard catches cases where some
venues were stored with a different SRID — cross-SRID distance
returns `null`.

#### Variations

- Cluster by kilometre ring:
  `toInteger(metres / 1000) AS km` grouped with `count(*)`.
- Include only open venues: `WHERE v.status = 'open'` before the
  distance computation.

#### Related concepts

- [Spatial functions → distance](./functions/spatial#distance)
- [Spatial data types](./data-types/spatial)

---

### Recipe: Region / bounding-box filter

#### Problem

"Every city between latitudes `$s..$n` and longitudes `$w..$e`."

#### Assumed data model

- `:City {name, location}` — WGS-84 2D

#### Query

```cypher
MATCH (c:City)
WHERE c.location.latitude  >= $s AND c.location.latitude  <= $n
  AND c.location.longitude >= $w AND c.location.longitude <= $e
RETURN c.name, c.location.latitude AS lat, c.location.longitude AS lon
ORDER BY lat
```

#### Explanation

LoraDB has no `point.withinBBox()` — compose the four bounds
explicitly with `>=` / `<=`. `BETWEEN` is also unsupported. See
[Limitations](./limitations#operators-and-expressions).

#### Variations

- Combine with category filter:
  `WHERE c.country = $country AND …`.
- Return area-approximated stats with
  `count(*)` grouped by rounded coordinates — see
  [Cluster by rounded coordinates](./data-types/spatial#cluster-by-rounded-coordinates).

#### Related concepts

- [Spatial point components](./functions/spatial#component-access)
- [WHERE — arithmetic and comparison](./queries/where#comparison)

---

### Recipe: Closest-per-category

#### Problem

"Nearest shop of each category to `$here`."

#### Assumed data model

- `:Shop {category, location}` — WGS-84 2D

#### Query

```cypher
MATCH (s:Shop)
WITH s.category AS category, s, distance(s.location, $here) AS metres
ORDER BY metres ASC
WITH category, collect({s: s, metres: metres})[0] AS nearest
RETURN category,
       nearest.s.name AS name,
       nearest.metres AS metres
ORDER BY metres
```

#### Explanation

Order by distance *first*, then `collect` per category. The first
collected element is the closest. `collect(…)[0]` is the
group-level "pick one" idiom — LoraDB has no window functions.

#### Variations

- Pick the *farthest* in each category: sort `DESC`.
- Top-3 per category: `collect(…)[..3]` and then `UNWIND` to one
  row per shop.

#### Related concepts

- [Top contributor per group](./queries/aggregation#top-contributor-per-group-pipeline-trick)
- [collect → slice](./functions/aggregation#collect--slice-for-top-n)

---

### Recipe: Join on proximity

#### Problem

"Find all pairs of sensors within 500 metres of each other."

#### Assumed data model

- `:Sensor {id, location}` — WGS-84 2D

#### Query

```cypher
MATCH (a:Sensor), (b:Sensor)
WHERE id(a) < id(b)
  AND distance(a.location, b.location) < 500
RETURN a.id, b.id, distance(a.location, b.location) AS metres
ORDER BY metres
```

#### Explanation

The Cartesian join between sensors is expensive — use a label scope
to keep it tight. The `id(a) < id(b)` predicate removes symmetric
pair duplicates.

#### Variations

- Constrain by network / region first (`WHERE a.region = b.region`)
  to shrink the cartesian product dramatically.
- Store a Cartesian point instead for small domains (floor plans,
  game maps) — distance is then an `O(1)` square root.

#### Related concepts

- [Cartesian products in MATCH](./queries/match#multiple-patterns-cross-product)
- [Performance notes](./queries/paths#performance)

---

## Vector-retrieval patterns

### Recipe: Top-k by similarity

#### Problem

"Return the ten documents most similar to `$query`, exhaustive scan."

#### Assumed data model

- `:Doc {id, title, embedding}` — `embedding` is a `VECTOR<FLOAT32>`
  of fixed dimension (e.g. 384 for a small sentence-transformer).

#### Query

```cypher
MATCH (d:Doc)
RETURN d.id AS id, d.title AS title
ORDER BY vector.similarity.cosine(d.embedding, $query) DESC
LIMIT 10
```

Pass `$query` either as a tagged vector (`vector([...], 384, FLOAT32)`
on the host) or as a plain numeric list — both are accepted by the
similarity function. See
[Vectors → Passing vectors as parameters](./data-types/vectors#passing-vectors-as-parameters).

#### Explanation

Vector indexes are not implemented yet, so every matched node is
scored linearly. Keep the `MATCH` as narrow as possible (label,
property filter) to shrink the candidate set before similarity runs.

#### Variations

- **Score in a `WITH` stage** if the score is reused downstream:
  `WITH d, vector.similarity.cosine(d.embedding, $query) AS score`.
- **Swap the metric** for Euclidean-bounded similarity
  (`vector.similarity.euclidean`) or a signed distance
  (`vector_distance(d.embedding, $query, EUCLIDEAN)`, then
  `ORDER BY … ASC`).

#### Related concepts

- [Vectors](./data-types/vectors)
- [Limitations → Vectors](./limitations#vectors)

---

### Recipe: Graph-filtered retrieval

#### Problem

"Return the five documents most similar to `$query`, but only those
that mention an entity of a given type — and include the matched
entity names in the result."

#### Assumed data model

- `:Doc {id, title, embedding}` with a `VECTOR` embedding.
- `(:Doc)-[:MENTIONS]->(:Entity {name, type})`.

#### Query

```cypher
MATCH (d:Doc)
WITH d, vector.similarity.cosine(d.embedding, $query) AS score
MATCH (d)-[:MENTIONS]->(e:Entity)
WHERE e.type = $entity_type
RETURN d.id, d.title, score, collect(e.name) AS entities
ORDER BY score DESC
LIMIT 5
```

#### Explanation

Similarity supplies candidates; the graph explains and constrains
them. The score is captured once in the first `WITH`, then the
pipeline walks `MENTIONS` edges for filtering and returns the entity
list alongside the ranking. This is the shape that motivated putting
`VECTOR` next to the graph.

#### Variations

- **Filter before scoring** if the entity constraint is selective —
  move the `MATCH (d)-[:MENTIONS]->(e:Entity {type: $entity_type})`
  above the similarity `WITH` so similarity only scores documents that
  already passed the entity filter.
- **Rerank by recency** as a tie-breaker:
  `ORDER BY score DESC, d.updated_at DESC`.

#### Related concepts

- [Vectors → Exhaustive kNN](./data-types/vectors#exhaustive-knn)
- [Queries → Parameters](./queries/parameters#semantic-retrieval-with-a-vector-parameter)

---

## Backup and restore

LoraDB's snapshot API lets you persist the full graph to a single
file and restore it later. It's a point-in-time dump — atomic on
rename, no WAL, no background persistence. These recipes cover the
two common operational shapes.

### Recipe: Periodic snapshot from host code

#### Problem

"Persist the live graph every N minutes from the same process that
owns the `Database` handle."

#### Assumed setup

- Any binding with a `save_snapshot` / `saveSnapshot` method (Rust,
  Python, Node, WASM, Go, Ruby) — see the
  [language-specific quickstarts](./getting-started/installation) or
  the canonical [Snapshots guide](./snapshot).
- A writable target path on a local disk.

#### Code (Rust)

```rust
use std::time::Duration;
use std::sync::Arc;
use lora_database::Database;

fn snapshot_loop(db: Arc<Database>, path: &'static str) {
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_secs(300));
        match db.save_snapshot_to(path) {
            Ok(meta) => tracing::info!(
                nodes = meta.node_count,
                rels  = meta.relationship_count,
                "snapshot saved"
            ),
            Err(e) => tracing::error!(error = %e, "snapshot failed"),
        }
    });
}
```

#### Explanation

- `save_snapshot_to` writes to `<path>.tmp`, `fsync`s, and renames
  over the target — a crashed save never leaves a half-written file
  at `<path>`.
- The call serialises against every query on the handle. The mutex
  is held for the serialize step (`O(n + r)` in nodes and
  relationships). Pick an interval larger than the measured save
  wall-time so successive saves don't stack.
- You can safely overwrite the same path on every tick. For
  versioned backups, rotate the filename (`graph-2026-04-24.bin`)
  and garbage-collect outside the process.

#### Variations

- **Rotating filenames.** Append a date stamp and prune on a cron.
- **HTTP from a cron job.** If the server runs with
  `--snapshot-path`, call `POST /admin/snapshot/save` from an
  external cron instead of a thread inside the process. See
  [HTTP API → Admin endpoints](./api/http#admin-endpoints-opt-in).
- **Python / Node / WASM / Go / Ruby.** Same shape — see each
  quickstart's _Persisting your graph_ section:
  [Rust](./getting-started/rust#persisting-your-graph),
  [Python](./getting-started/python#persisting-your-graph),
  [Node](./getting-started/node#persisting-your-graph),
  [WASM](./getting-started/wasm#persisting-your-graph),
  [Go](./getting-started/go#persisting-your-graph),
  [Ruby](./getting-started/ruby#persisting-your-graph).

### Recipe: Boot from snapshot, save on shutdown (server setup)

#### Problem

"Self-hosted `lora-server` should load state at start-up and
persist it on graceful shutdown."

#### Assumed setup

- A host running `lora-server` as a long-lived process (systemd, Docker,
  plain `nohup`, …).
- A writable path for the snapshot file.

#### Configuration

```bash
# start-up — restore from, and default-save to, the same file
lora-server \
  --host 127.0.0.1 --port 4747 \
  --snapshot-path /var/lib/lora/db.bin \
  --restore-from  /var/lib/lora/db.bin
```

`--restore-from` at boot: missing file is fine (empty graph, logged);
malformed file is fatal.

#### Save on shutdown (systemd sketch)

```ini
# /etc/systemd/system/lora-server.service
[Service]
ExecStart=/usr/local/bin/lora-server \
  --host 127.0.0.1 --port 4747 \
  --snapshot-path /var/lib/lora/db.bin \
  --restore-from  /var/lib/lora/db.bin
ExecStop=/usr/bin/curl -sX POST http://127.0.0.1:4747/admin/snapshot/save
TimeoutStopSec=30
```

The `ExecStop` call saves the current state before the process is
killed. `TimeoutStopSec` needs to comfortably cover your measured
save duration.

#### Explanation

- The admin endpoints are opt-in — they're mounted **only** because
  `--snapshot-path` is set. On any other host they return `404`.
- The admin surface has **no authentication**, so the bind address
  must be privileged (here: `127.0.0.1` only). See
  [HTTP API → Admin endpoints (opt-in)](./api/http#admin-endpoints-opt-in)
  for the security warning.
- The save runs as the server UID, which must have write access to
  the target path and its parent directory.

#### Variations

- **Scheduled cron-driven saves.** Combine the boot-from-snapshot
  setup above with a systemd `.timer` unit or an ordinary cron that
  curls `POST /admin/snapshot/save` every N minutes. The endpoint
  returns a `SnapshotMeta` you can log for observability.
- **Immutable seed + writable runtime.** Pass `--restore-from
  /var/lib/lora/seed.bin` and `--snapshot-path /var/lib/lora/runtime.bin`
  to boot from a shared seed and save to a host-local file.

See the canonical [Snapshots guide](./snapshot) for the file format,
the full admin-surface security profile, and every binding's save /
load API.

---

## See also

- [**Queries → Examples**](./queries/examples) — copy-paste recipes
  grouped by clause.
- [**Queries → Aggregation**](./queries/aggregation) — clause-level
  grouping, HAVING, percentiles.
- [**HTTP server → Snapshots and restore**](./getting-started/server#snapshots-and-restore)
  — flag reference for the admin endpoints.
- [**HTTP API → Admin endpoints (opt-in)**](./api/http#admin-endpoints-opt-in)
  — full wire reference.
- [**Tutorial**](./getting-started/tutorial) — guided from-zero walkthrough.
- [**Troubleshooting**](./troubleshooting) — what to do when a
  recipe returns fewer rows than you expected.
- [**Concepts → Modelling checklist**](./concepts/graph-model#modelling-checklist)
  — how to decide the shape of a new domain.
