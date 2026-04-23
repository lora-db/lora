---
slug: loradb-v0-2-vectors
title: "LoraDB v0.2: vector values for connected AI context"
description: "LoraDB v0.2 adds first-class VECTOR values, vector functions, binding support, and documentation for graph-shaped AI retrieval on top of the v0.1 core."
authors: [loradb]
tags: [release-notes, announcement, ai, cypher]
---

LoraDB v0.2 adds first-class `VECTOR` values.

You can now construct vectors in Cypher, store them as node or
relationship properties, pass them in as parameters through every
binding, and run exhaustive similarity search against them. The value
type, the wire format, the function surface, and the binding helpers
all landed together so vectors behave like every other typed value in
the engine.

What this release is not is a vector-index product. There is no
approximate nearest-neighbour search, no built-in embedding
generation, and no plugin compatibility layer. Those are deliberately
out of scope for v0.2. The goal here is to make embeddings comfortable
inside the graph model — to ship the foundation that an index-backed
retrieval path will eventually sit on.

<!-- truncate -->

## What Changed

The short list:

- `VECTOR` is a first-class value type, alongside scalars, lists,
  maps, temporal values, and spatial points.
- A new `vector(value, dimension, coordinateType)` constructor.
- Six supported coordinate types:
  - `FLOAT` / `FLOAT64`
  - `FLOAT32`
  - `INTEGER` / `INT` / `INT64` / `INTEGER64` / `SIGNED INTEGER`
  - `INTEGER32` / `INT32`
  - `INTEGER16` / `INT16`
  - `INTEGER8` / `INT8`
- Storage as node and relationship properties.
- `toIntegerList(v)` and `toFloatList(v)` for converting coordinates
  back to lists.
- `vector_dimension_count(v)` and `size(v)` for introspection.
- `vector.similarity.cosine(a, b)` — bounded to `[0, 1]`.
- `vector.similarity.euclidean(a, b)` — bounded to `[0, 1]`.
- `vector_distance(a, b, metric)` — signed distance under one of six
  metrics.
- `vector_norm(v, metric)` — Euclidean or Manhattan norm.
- Parameter support through every binding.
- A canonical tagged wire shape:

  ```json
  {
    "kind": "vector",
    "dimension": 3,
    "coordinateType": "FLOAT32",
    "values": [0.1, 0.2, 0.3]
  }
  ```

Language support is included in this release for:

- the Rust core;
- Node.js / TypeScript (`crates/lora-node`);
- WebAssembly (`crates/lora-wasm`);
- Python (`crates/lora-python`);
- Go (`crates/lora-go`);
- Ruby (`crates/lora-ruby`).

Each binding ships a `vector(...)` helper and an `isVector` /
`is_vector` / `IsVector` / `vector?` guard, so callers do not need to
build the tagged object by hand.

## Why Vectors In A Graph Database

A graph database and a vector store usually get treated as two
products. The graph stores relationships; the vector store retrieves
similar items; the application glues them together.

For most AI workloads that is the wrong shape. You want to retrieve
candidates by similarity *and* use the graph to rank, filter, or
explain them. When the embedding lives on a separate service, every
query needs a round trip and every piece of context needs a join by
hand.

Putting `VECTOR` into LoraDB as a value type collapses that
separation. The embedding is a property on the same node that carries
the label, the text, and the relationships. You score with
`vector.similarity.cosine(...)` and walk with `MATCH` in the same
Cypher.

That is the whole argument. Similarity finds candidates. The graph
explains them.

## Using VECTOR Values

### Creating A Vector Property

```cypher
CREATE (d:Doc {
  id:        1,
  title:     'Onboarding checklist',
  embedding: vector([0.1, 0.2, 0.3], 3, FLOAT32)
})
```

The third argument can be a bare identifier (`INTEGER`, `FLOAT32`,
`INT8`) or a string literal (`'INTEGER'`, `'SIGNED INTEGER'`). Bare
identifiers are rewritten to string literals by the analyzer only in
this specific argument position, so normal variable resolution is
unaffected elsewhere in the query.

### Passing A Vector As A Parameter

Generate the embedding in your application, pass it in with the
language helper, and use it like any other parameter:

```ts
import { vector } from "@loradb/lora-node";

const query = vector(embedding, 384, "FLOAT32");

await db.execute(
  `MATCH (d:Doc)
   RETURN d.id AS id
   ORDER BY vector.similarity.cosine(d.embedding, $q) DESC
   LIMIT 10`,
  { q: query },
);
```

The same pattern works in Python (`vector(values, dimension,
coordinate_type)`), Go (`lora.Vector(values, dimension, coordinateType)`),
Ruby (`LoraRuby.vector(values, dimension, coordinate_type)`), and the
raw FFI / JSON path used by the C ABI.

### Bulk Insert

The canonical way to load many embeddings is a single `UNWIND` over a
parameter list of rows. Each row is a map with scalar fields and a
tagged vector. The engine fans the list into per-row `CREATE`s without
round-tripping each one:

```ts
import { vector } from "@loradb/lora-node";

const batch = docs.map((doc) => ({
  id:        doc.id,
  title:     doc.title,
  embedding: vector(doc.embedding, 384, "FLOAT32"),
}));

await db.execute(
  `UNWIND $batch AS row
   CREATE (:Doc {id: row.id, title: row.title, embedding: row.embedding})`,
  { batch },
);
```

The same shape in Python:

```python
from lora_python import Database, vector

db = Database.create()

batch = [
    {
        "id":        doc["id"],
        "title":     doc["title"],
        "embedding": vector(doc["embedding"], 384, "FLOAT32"),
    }
    for doc in docs
]

db.execute(
    """
    UNWIND $batch AS row
    CREATE (:Doc {id: row.id, title: row.title, embedding: row.embedding})
    """,
    {"batch": batch},
)
```

Two things are worth knowing about this pattern. First, each vector is
stored as its own property on its own node — the batch is a list of
*maps*, not a list of vectors, so the property rule (no lists of
vectors) is satisfied by construction. Second, `UNWIND` runs the whole
batch in one query, so the per-row overhead is a map extraction and a
`CREATE`, not a full parse + plan + execute cycle per document.

### Exhaustive kNN

```cypher
MATCH (d:Doc)
RETURN d.id AS id
ORDER BY vector.similarity.cosine(d.embedding, $query) DESC
LIMIT 10
```

Every `MATCH` candidate is scored. Cost is `O(n)` in the number of
matched nodes. That is fine for a local dataset, a test, a demo, or a
small internal tool. It is not how you would serve a corpus of
millions — that is what vector indexes are for, and they are not
implemented yet.

### Graph-Filtered Retrieval

The version that actually motivated adding vectors to LoraDB looks
like this:

```cypher
MATCH (d:Doc)
WITH d, vector.similarity.cosine(d.embedding, $query) AS score
MATCH (d)-[:MENTIONS]->(e:Entity)
WHERE e.type = $entity_type
RETURN d.id, d.title, score, collect(e.name) AS entities
ORDER BY score DESC
LIMIT 5
```

The similarity function supplies the candidates. The graph structure
(`MENTIONS`, the entity type filter) explains and constrains them.
Both live in one query and one engine.

### Storing And Reading Back

Vectors round-trip through storage unchanged:

```cypher
CREATE (:Doc {id: 1, embedding: vector([1, 2, 3], 3, INTEGER)})
MATCH (d:Doc {id: 1}) SET d.embedding = vector([0.1, 0.2], 2, FLOAT32)
MATCH (d:Doc {id: 1}) RETURN d.embedding AS e
```

A vector is also a legal map value, so a property map containing a
vector is stored intact. The one restriction is that **a list
containing vectors cannot be stored as a property** — if you need
many embeddings, hang them off separate nodes. The engine rejects the
write at property-conversion time instead of silently storing a shape
a future vector index could not support.

## AI Use Cases

Vectors in LoraDB are aimed at the workloads that already sit
awkwardly between "vector store" and "graph database":

- **Agent memory.** Embed documents, chunks, observations, or tool
  calls. Connect them with edges to the sessions, entities, and
  decisions that reference them. Retrieve by similarity, filter by
  recency, explain by provenance.
- **Semantic document retrieval with context.** Find the k closest
  docs, then expand to the entities, authors, or topics they connect
  to before returning anything to the application.
- **Tool and context selection.** Score candidate prompts, tools, or
  examples by similarity to the current state, then filter by graph
  constraints (same tenant, same permission scope, same task type).
- **Knowledge graph enrichment.** Attach embeddings to entities so
  fuzzy lookups can locate the right node before structural queries
  run.
- **Recommendations with graph guardrails.** Cosine similarity
  produces a long list of candidates; graph relationships
  (`BLOCKED`, `ALREADY_SEEN`, `OWNED_BY`) filter that list before
  ranking.
- **Internal search over connected product, customer, or support
  data.** Small corpora, high structure, and queries that want both
  "similar to this ticket" and "in the same account and escalation
  path."

Every one of those workloads has the same shape: similarity for
candidates, structure for the rest.

## What Is Not Included Yet

This release is deliberately narrow. It does not include:

- **Vector indexes.** All vector functions are exhaustive today.
- **Approximate nearest-neighbour search.** There is no ANN path.
- **Built-in embedding generation.** LoraDB does not produce
  embeddings; bring them in from your application code.
- **Hardened public-internet database hosting.** The HTTP server is
  useful for local development and controlled environments. It is
  not a managed service.

Those are not hidden. They are the roadmap.

Exhaustive similarity is fine for small datasets, tests, demos, local
prototypes, and internal workflows. It is not yet a substitute for an
index-backed vector search service at scale.

## Why This Fits The LoraDB Journey

LoraDB is developer-first.

That sequencing applies to vectors too. The first version needs the
value model, the wire shape, the binding helpers, and the Cypher
ergonomics to be right. Those are the parts that user code and tests
depend on. A vector index that sits on a broken value model would
inherit every problem. Shipping the foundation first, and being
honest about the absence of an index, keeps the upgrade path clean.

It also matches how developers actually pick up a new capability.
Clone the repo, generate a few embeddings, store them on nodes, write
a similarity query, see whether the model fits the problem. Only
after that does scale, persistence, and managed operations become
worth talking about.

v0.2 makes that first loop work.

## Try It

Get the repo and run the server:

```bash
cargo run --bin lora-server
```

Then try a vector query from `curl` or any binding:

```bash
curl -X POST http://127.0.0.1:4747/query \
  -H 'content-type: application/json' \
  -d '{"query":"RETURN vector([1,2,3], 3, INTEGER) AS v"}'
```

The docs site has a dedicated page for the value type, the coordinate
rules, the functions, the storage restrictions, and the exhaustive
kNN pattern:

- [Data types → Vectors](/docs/data-types/vectors)
- [Functions → Overview](/docs/functions/overview)
- [Queries → Parameters](/docs/queries/parameters)

Internal notes on the value model and the Cypher support matrix have
been updated to match.

## What Comes Next

Three directions stand out after v0.2:

1. **A vector index.** The Cypher shape stays the same
   (`ORDER BY vector.similarity.* LIMIT k`); the executor starts
   routing scored candidates through an index instead of a linear
   scan. The design depends on the workloads people actually bring.
2. **More metrics and norms as real usage demands them.** The
   current set (`EUCLIDEAN`, `EUCLIDEAN_SQUARED`, `MANHATTAN`,
   `COSINE`, `DOT`, `HAMMING` for distance; `EUCLIDEAN` and
   `MANHATTAN` for norm) covers the common cases. Extending is
   mechanical once a concrete need shows up.
3. **The hosted path.** Vectors on stored nodes eventually need
   managed operations to match. That is a separate release, but the
   value-type work here is what makes it possible without a second
   data model.

If you try v0.2 with vectors, the most useful feedback is concrete:

- what graph did you load, and what embedding workflow did you pair
  with it;
- where did exhaustive scan stop being enough;
- what would a vector index need to support for your workload — a
  specific metric, a specific filter shape, a specific freshness
  guarantee;
- which binding did you use, and did the tagged shape round-trip
  cleanly through your application code.

That is the feedback that will shape v0.3.
