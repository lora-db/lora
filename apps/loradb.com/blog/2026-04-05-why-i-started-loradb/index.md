---
slug: why-i-started-loradb
title: "Why I started LoraDB"
description: "The first reason was simple: I needed a graph database that felt fast enough to keep in the hot path."
authors: [joost]
tags: [founder-notes, design, announcement]
---

I did not start LoraDB because the world was missing another database with a
logo and a query language. I started it because I kept reaching for a graph
database in places where the existing choices felt too heavy for the job.

The shape of the problem was clear: I needed a really fast in-memory graph
database. Not a graph feature bolted onto a document store. Not a large server
that needed its own operational plan before I could answer a product question.
Not a database that looked elegant in a demo but became expensive once the
working set, query fan-out, and deployment model got real.

I needed something smaller, sharper, and more efficient.

<!-- truncate -->

## The Workload That Kept Coming Back

The same workload showed up in different clothes:

- entities connected by typed relationships;
- metadata that mattered at query time;
- short multi-hop traversals;
- ranking, filtering, and projection over neighborhoods;
- state that should be close to the application, not across a slow boundary.

Sometimes the domain was product data. Sometimes it was memory for agents.
Sometimes it was internal tooling. The model kept becoming a graph because the
real world kept refusing to be a table.

The annoying part was not the graph model. The graph model was the relief. The
annoying part was the machinery around it.

I wanted to ask questions like:

```cypher
MATCH (u:User)-[:WORKED_ON]->(p:Project)-[:USES]->(t:Technology)
WHERE u.id = $user_id
RETURN t.name, count(*) AS weight
ORDER BY weight DESC
LIMIT 20
```

And I wanted that query to be cheap enough that I did not have to build a cache
around the database on day one.

## Respect For Existing Systems, Frustration With The Fit

Neo4j did a lot for the graph database category. It made Cypher feel normal.
It made graph queries approachable. It gave developers a mental model that is
still useful.

But for the systems I wanted to build, I did not like the efficiency tradeoff.
The operational footprint was bigger than the product surface I needed. The
runtime model was not shaped around embedding. The storage model was not
something I could easily reason about from inside a small application. Other
graph databases had their own strengths, but I kept seeing the same tension:
great ideas wrapped in systems that were too large, too remote, or too costly
for the hot path.

That does not make those systems bad. It means they were solving a broader
problem than mine.

My problem was narrower:

> Give me a graph database I can understand, run locally, keep in memory, query
> with a familiar language, and make efficient enough that I trust it in the
> product loop.

LoraDB started there.

## Why In-Memory First

In-memory is not a shortcut. It is a product decision.

A database that begins in memory can optimize for a different feeling:

- startup should be quick;
- tests should be cheap;
- queries should be predictable;
- the storage layout should be easy to inspect;
- the developer should not need a cluster before they have a prototype.

For the first version of LoraDB, persistence was less important than making the
core query engine honest. If the parser, analyzer, planner, executor, and graph
store are clean in memory, then durability can be added from a strong base. If
the core is slow or unclear, persistence only preserves the wrong shape.

The first principle was speed in the loop. Can you model a graph, load it, run
queries, and understand what happened without ceremony?

That is what I wanted as a developer. That is also what I think creates the
right customer journey: adopt locally, trust the engine, then choose hosted
operations when the product deserves it.

## The Customer Journey I Wanted

Most infrastructure products ask for trust too early.

They ask you to sign up, provision, configure, connect, migrate, monitor, and
only then discover whether the data model even fits your problem. For a graph
database, that is backwards. Developers need to feel the model first.

The journey should be:

1. Run LoraDB locally.
2. Load a small graph.
3. Write a Cypher query that feels like the problem.
4. See that it is fast enough to stay in the application path.
5. Build something real.
6. Move to managed infrastructure when operations become the expensive part.

That is the company model behind LoraDB: developer-first core, hosted platform
later. The database has to earn adoption before the platform can earn revenue.

## What I Refused To Hide

The first versions of LoraDB are intentionally explicit about limitations. It
is in-memory. It does not pretend to be a distributed system. It does not hide
missing pieces behind enterprise language. It is a graph database engine with a
clear query pipeline and a storage model that can be read.

That matters because efficiency is not only a benchmark result. Efficiency is
also whether a developer can answer:

- Where did this allocation come from?
- Why did the planner choose this direction?
- How many rows does this operation materialize?
- What does this value look like in storage?
- Can I debug this without becoming an archaeologist?

I started LoraDB because I wanted those answers to be reachable.

## The Bet

The bet is that there is room for a graph database that is smaller, faster to
adopt, easier to embed, and honest about the path from open development to a
hosted business.

Not every workload needs LoraDB. Some need a mature cluster with years of
operational tooling. Some need disk-first durability from the first write. Some
need distributed graph processing.

But many teams need a fast graph engine close to the application. Many agent
systems need structured memory. Many products need relationship queries before
they need a database department.

That is why I started LoraDB.

The next post is about the first technical constraint that shaped everything:
the database had to be fast enough to live in memory without becoming wasteful.
