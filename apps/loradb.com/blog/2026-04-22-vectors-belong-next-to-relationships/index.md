---
slug: vectors-belong-next-to-relationships
title: "Vectors belong next to relationships"
description: "Why LoraDB adds VECTOR values: similarity is useful, but connected context is what makes retrieval explainable."
authors: [joost]
tags: [founder-notes, ai, design, cypher]
---

The conventional advice for AI retrieval is to pick a side.

You pick a vector database if you want similarity. You pick a graph
database if you want structure. You bolt them together with glue code
when the product inevitably needs both.

That framing has never matched the workloads I actually care about. The
interesting systems — agent memory, recommendations, internal search
over connected product data, knowledge graphs that feed chat features —
do not want a vector store *or* a graph store. They want to retrieve
candidates by similarity, then explain and filter those candidates by
relationships. Splitting that into two products splits the query path,
the data model, and eventually the team.

LoraDB v0.2 adds `VECTOR` as a first-class value type. Vectors live
directly on nodes and relationships, next to labels, properties, and
edges. The argument is not that a graph database should replace a
vector database. The argument is that similarity belongs next to the
relationships that give it meaning.

<!-- truncate -->

## Similarity Is Useful, But It Is Not Memory

Embeddings are good at one thing: finding items that are close to a
query in some learned semantic space. That is genuinely useful. A lot
of retrieval systems live or die on whether the top ten candidates
contain the right answer.

But similarity alone does not preserve the information a product
usually needs *around* that answer:

- Where did this chunk come from?
- Which entities does it mention?
- Which session produced it?
- What depends on it?
- What contradicts it?
- Is it more recent than the other candidates?
- Is it from a trusted source?

A flat list of similar chunks cannot answer those questions. The
relationships have to be recorded somewhere, and the system that
retrieves embeddings has to reach that somewhere cheaply. When those
two things live in different databases, "cheaply" stops being a local
property of the query engine.

That is why vectors belong next to the graph, not behind a sidecar.

## The Agent Memory Shape

Think about what an agent actually stores across a session.

It stores documents or chunks, with embeddings so they can be
retrieved by similarity. It stores entities extracted from those
documents. It stores tool calls and their arguments. It stores
decisions, observations, and the context that led to each one. It
stores edges between those things: this observation led to that
decision; this tool was selected because of that memory; this document
mentions those entities.

That is a graph. And it is a graph with embeddings on some of the
nodes.

A small version looks like this:

```cypher
CREATE (d:Doc {
  id:        'doc-17',
  title:     'Onboarding checklist',
  embedding: vector($embedding, 384, FLOAT32)
})
CREATE (e:Entity {name: 'Alice'})
CREATE (d)-[:MENTIONS]->(e)
CREATE (o:Observation {session: 's1', ts: datetime()})
CREATE (o)-[:OBSERVED_IN]->(d)
```

Later, when the agent needs to retrieve memory, the query is not "find
the ten closest chunks." The useful query is "find the ten closest
chunks, show which entities they mention, and give me enough structure
to rank or filter":

```cypher
MATCH (d:Doc)
WITH d, vector.similarity.cosine(d.embedding, $query) AS score
MATCH (d)-[:MENTIONS]->(e:Entity)
RETURN d.id, d.title, score, collect(e.name) AS entities
ORDER BY score DESC
LIMIT 5
```

Similarity finds the candidates. The graph explains them.

That shape is easier to build — and easier to debug — when the
embedding is a property on the same node that carries the
relationships. There is no sync process, no second database, no
second query pipeline. There is one model.

## Why VECTOR Is A Value Type

A vector in LoraDB is not a special table or a separate index
namespace. It is a value type, like a string or a point or a duration.
That design choice has consequences.

A `LoraVector` has:

- a fixed dimension (`1..=4096`);
- a typed coordinate choice (`FLOAT64`, `FLOAT32`, `INTEGER`,
  `INTEGER32`, `INTEGER16`, `INTEGER8`);
- its coordinates stored in a single typed array.

It can appear anywhere a value can appear:

- as a node property: `CREATE (:Doc {embedding: vector([...], 384, FLOAT32)})`
- as a relationship property: `CREATE (a)-[:SIM {score: vector([...], 3, FLOAT32)}]->(b)`
- as a Cypher parameter: `vector(value, dimension, coordinateType)`
- inside a `RETURN`, `WITH`, `ORDER BY`, or `WHERE` clause.

Every binding speaks the same canonical tagged shape:

```json
{
  "kind": "vector",
  "dimension": 3,
  "coordinateType": "FLOAT32",
  "values": [0.1, 0.2, 0.3]
}
```

This is not only a wire format. It is the thing the Node.js, WASM,
Python, Go, and Ruby helpers build. A developer in any of those
languages can construct a vector in application code, pass it in as a
parameter, store it on a node, read it back, and get the same tagged
object on the other side. No bridge code. No JSON schemas to keep in
sync.

That value-type framing also forces a property-storage rule worth
stating out loud: a vector is fine as a property, but a **list of
vectors** is not. If you want many vectors, hang them off separate
nodes with separate embeddings. That restriction is enforced at write
time — the engine rejects the property instead of silently storing a
shape that future indexing could not support.

## Why Exhaustive First

v0.2 ships vectors, but it does not ship vector indexes. That is a
deliberate line.

What works today is exhaustive similarity search. You write a `MATCH`
that produces a candidate set, you score every candidate with
`vector.similarity.cosine(...)` or `vector.similarity.euclidean(...)`,
you `ORDER BY score DESC LIMIT k`. The engine scans every matched
node.

For a graph of a few hundred thousand embeddings, this is completely
fine on a laptop. For millions, you want a proper index. The
difference is not in the query language — the `ORDER BY … LIMIT k`
shape is the same either way — it is in what the engine does under
the hood.

Shipping exhaustive first lets v0.2 land the parts that are hardest to
change later:

- the `VECTOR` value type and its coordinate rules;
- the tagged wire shape that every binding speaks;
- the property-storage semantics;
- the function surface (`vector.similarity.*`, `vector_distance`,
  `vector_norm`, `toIntegerList`, `toFloatList`,
  `vector_dimension_count`, `size(vector)`);
- the Cypher ergonomics, including the bare-identifier rewrite that
  lets you write `INTEGER8` and `EUCLIDEAN` without declaring them as
  variables.

Those are the decisions that bindings, tests, and user code depend
on. If we got any of them wrong, a vector index would inherit the
mistake. The order is: value model first, index-backed retrieval
later.

It also matches LoraDB's customer journey. The first question is not
"can this serve ten million embeddings?" The first question is "does
the model fit my problem?" An exhaustive scan over a thousand docs is
enough to answer that.

What is explicitly *not* included in v0.2 is worth stating plainly:

- no vector indexes;
- no approximate nearest-neighbour search;
- no built-in embedding generation;
- no hardened public-internet database hosting.

Generate your embeddings in application code — whatever you already
use, whether that is a hosted API, a local model, or a batch job —
and pass them in as parameters. The database's job is to store them
next to the graph and let you query both in one language.

## The Customer Journey For Vectors

The flow I want for a developer adopting vectors in LoraDB mirrors the
one I wanted for adopting LoraDB at all.

1. They have a retrieval problem that is not purely similarity —
   there is structure around the items they want to find.
2. They generate embeddings in application code and store them on
   graph nodes with a single `CREATE`.
3. They run `vector.similarity.cosine(...)` against a small local
   dataset. The query shape is ordinary Cypher.
4. They add `MATCH` patterns that filter or explain the results using
   relationships.
5. They build a prototype, a tool, or a product feature on that
   combined query.
6. When the dataset grows past what an exhaustive scan can serve,
   they move to an index-backed variant — without rewriting the
   application, because the Cypher stays the same.
7. Eventually, managed operations follow.

That is the same staircase as before. Local trust first, persistence
and platform later. Vectors fit because they slot into the same model
as everything else LoraDB stores.

## Closing

Similarity helps you find candidates. Graphs help you explain them.

Most of the AI systems I care about — agent memory, internal search
over connected data, recommendations with guardrails, knowledge graphs
that feed chat — need both. The mistake is treating that as two
products. LoraDB v0.2 is the argument that it should be one.

The v0.2 release article has the full list of what landed, the
functions, the binding support, and the Cypher examples. The short
version is that `vector` is now a value you can put on a node, pass as
a parameter, and query with a few small honest functions. The longer
version is everything we are *not* trying to be — a vector-index
product, a hosted ANN service, a plugin marketplace — because the
first job is to make the graph model comfortable with embeddings
sitting on it.

If you try it, the feedback I want is concrete:

- what graph did you load, and what embedding workflow did you pair
  with it;
- what did the query look like once similarity and structure were in
  the same Cypher;
- at what size did the exhaustive scan stop being good enough;
- what would a vector index need to support for your workload — a
  specific metric, a specific filter shape, a specific freshness
  guarantee?

That is what will shape the next release.
