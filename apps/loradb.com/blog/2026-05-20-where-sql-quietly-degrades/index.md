---
slug: where-sql-quietly-degrades
title: "Where SQL quietly degrades"
description: "Why relational performance falls off in almost every modern product workload, from connected data and interaction-time queries to agent memory and retrieval, and what the cost actually is."
authors: [joost]
tags: [founder-notes, design, performance, deep-dive, cypher]
---

I do not want to write a post about how SQL is bad. SQL is not bad.
Postgres is one of the most well-engineered pieces of software on the
planet, and the relational model has held up for fifty years for very
good reasons.

But the workloads I see developers actually building today are not the
workloads SQL was tuned for. And in almost every one of those
workloads, relational performance degrades. Not because of one
catastrophic failure, but because of a stack of small frictions that
compound into something you eventually start working around with
caches, read replicas, materialized views, and a Slack channel called
`#schema-discussion`.

This post is about where that degradation happens, and why it is
structural rather than fixable with another index.

<!-- truncate -->

## "Fast" Is A Schema Decision

The first place SQL gets slow is before any query runs.

When you write `CREATE TABLE`, you are choosing a small set of
questions the database will answer cheaply. Everything else becomes a
join. Joins are not free, and the cost of a join compounds with the
number of tables involved, the selectivity of the filters, and the
shape of the indexes.

That is fine when the questions are known up front. A reporting
warehouse, a billing system, a ledger: these have stable shapes and
stable questions. SQL is excellent there.

Product workloads are not like that. The questions change weekly:

- yesterday's "how many orders did this customer place" becomes
- today's "show me customers whose last three orders include products
  that other customers, with similar order histories, returned within
  thirty days."

The relational schema does not change at that pace. So the second
query becomes a six-table join, two subqueries, and a CTE that the
planner sometimes inlines and sometimes does not. The performance
profile of the database is now determined by a decision someone made
two years ago about how to normalize the `orders` table.

This is the part I find most interesting: the database is not slow
because the engine is slow. It is slow because the model in the
database does not match the model in the question.

## The Join Cost That Compounds

Joins are the relational model's mechanism for re-assembling things
that should never have been separated in the first place.

A single join is cheap. Two joins are usually fine. Three start
requiring thought. Four is where you begin tuning. By the time you
hit a six-way join across tables of meaningful size, you are no longer
querying the database. You are negotiating with the optimizer.

The reason multi-hop relational queries get slow is not mysterious.
The cost of a hash join grows with the size of the build side. The
cost of a nested loop grows with the product of the inputs. The cost
of a merge join requires both sides to be sorted on the join key. None
of those scale gracefully when the question is shaped like a
*traversal*:

> Find me everything within three relationship-hops of this node that
> matches some predicate, and rank the results by how it is connected.

In Cypher, that is one line:

```cypher
MATCH (start:Entity {id: $id})-[*1..3]-(neighbor)
WHERE neighbor.score > 0.5
RETURN neighbor, length(shortestPath((start)-[*]-(neighbor))) AS hops
ORDER BY hops, neighbor.score DESC
LIMIT 20
```

In SQL, that is a recursive CTE that allocates intermediate result
sets at each level, joins them back to the edge table, and either
fits in `work_mem` or spills to disk. The query plan is several
screens long. The latency is measured in seconds.

The relational engine is doing exactly what it was designed to do.
The problem is that it was not designed to do *this*.

## Indexes Don't Save You As Much As You Think

The standard response to "this query is slow" in a relational system
is to add an index. Sometimes that works. Often it does not, for
reasons that are worth naming.

A B-tree index helps with equality and range lookups on a single
column or a known prefix. It does very little for queries whose shape
the optimizer cannot predict. The covering index that makes one query
fast often makes three others slower because the write amplification
hits every index on every insert.

The honest accounting for indexes in a busy relational schema looks
like this:

- every index is a write multiplier;
- every index is memory pressure on the buffer cache;
- every index is a vacuum / maintenance cost;
- every index is a thing the planner has to *choose* to use, and
  sometimes it chooses wrong;
- every index covers a narrow slice of queries, and product queries
  rarely sit still in that slice.

A team I worked with had a 38-index table that took longer to write
than to read. They added the indexes one at a time, each one
justified by a real product query. None of them were wrong. The
aggregate was wrong.

This is the second compounding cost: the schema decisions made for
performance keep making other things slower, and the team's mental
overhead grows with every quarter the database is in production.

## The Hot Path Was Never Set Algebra

The deepest reason SQL degrades for modern workloads is that the
relational engine was built to answer questions about *sets*. How
many. What is the average. Which rows match this predicate. Sets are
a powerful abstraction, and SQL is the right tool whenever the
question is shaped that way.

The questions on a product's hot path are not shaped that way:

- "Which related entities should appear while someone is typing?"
- "Which memories should the agent read before choosing a tool?"
- "Which product is connected enough, through purchases, browsing,
  shared sessions, to matter *now*?"
- "Why are these two things related?"

Those are questions about paths, neighborhoods, and structure. The
engine that answers them well needs pointer-cheap traversal, locality
between a node and its edges, and a planner that thinks in terms of
patterns rather than relational algebra.

You can answer those questions in SQL. The result is slower because
the storage layout, the join algorithm, and the optimizer were all
tuned for a different shape of question. The cost is not measured in
big-O at small N. It is measured in the gap between "fast enough to
sit in the request path" and "needs a precomputed cache."

## ORM Latency Is Real Latency

Most relational workloads are not accessed through hand-written SQL.
They are accessed through an ORM, which means the slowest version of
every query is the default version.

The patterns are well known. Lazy collection access fans out into
N+1 queries. Eager loading materializes huge joined result sets that
the application then throws away half of. Transaction boundaries get
extended because someone touched a relationship outside a session.
Identity maps cache stale state. Migrations rename a column and
break a query that was generated three layers deep in the framework.

I do not think the ORM is the villain. The ORM exists because the
object model and the relational model are far enough apart that we
need a translator. Translators leak. Every team using an ORM
eventually spends meaningful engineering time figuring out where it
is leaking and how much that costs in production.

The latency budget for the user-facing request includes the ORM. The
database can be fast, and the experience can still be slow, because
the round-trip between the application's model and the database's
model is paid on every request.

## The Workloads Where SQL Was Always Going To Lose

Some specific shapes degrade faster than others. Naming them helps
because they cover most of what people build now:

- **Connected data with cheap traversal**. Knowledge graphs, social
  graphs, dependency graphs, recommendation graphs. The interesting
  queries walk. Walks in SQL are recursive CTEs, and recursive CTEs
  are expensive.
- **Similarity plus structure**. AI retrieval that needs to find
  candidates by embedding similarity, then filter or rank by
  relationships. Two stores, two query paths, glue code in between,
  and consistency that is no longer the database's problem.
- **Agent memory**. Documents, entities, observations, decisions,
  and the edges that explain why one led to another. Forcing that
  into normalized tables makes every memory read into a join plan.
- **Real-time personalization**. The path from "user did this" to
  "show that" runs through several hops of related entities. Doing
  it in SQL means either denormalizing into a wide table that lies
  about its updates, or paying the join cost at request time.
- **Anything you would naturally draw on a whiteboard with arrows**.
  If your first sketch of the problem has nodes and edges, the
  database that matches that sketch is the one that stays fast as
  the questions evolve.

For each of these, you can make SQL work. You can also make a
spreadsheet work as a database. "Possible" is not the same as
"performant".

## What "Faster" Actually Means

When I say a graph engine is faster for these workloads, I do not
mean it beats Postgres on every benchmark. It does not. For a
single-table aggregate, Postgres is going to win, and it should.

What I mean is more specific:

- the storage layout puts a node next to its edges, so traversal is
  a sequence of cheap pointer reads instead of a join plan;
- the query language has a path primitive, so a four-hop pattern is
  four characters of syntax rather than four self-joins;
- the planner's job is to choose traversal directions and expansion
  order, which is the actual cost surface of the query;
- there is no ORM, because the model in the database matches the
  model in the question;
- the engine can run in the same process as the application, so the
  request budget does not include a network hop.

Those properties are what let a graph query feel like ordinary
application code instead of an analytics job. That is the bar I
care about.

## Where SQL Is Still Right

I want to be clear about this, because it is easy to overstate.

SQL is still the right tool for:

- transactional integrity over a stable schema with a known shape;
- financial systems, ledgers, billing, anything where the rules
  Codd wrote down are exactly the rules you want;
- analytical workloads where the dominant operation is aggregating
  large flat tables;
- reporting and BI, where the questions are slow on purpose because
  they cover the whole dataset;
- the parts of an application that genuinely are rows-with-columns,
  and there are plenty of them.

The argument is not "stop using SQL". The argument is "stop using
SQL for the part of the system that is shaped like a graph,
because the performance cost is structural and it gets worse the
more successful the product becomes."

## The Compounding Bet

The thing that makes relational degradation hard to fix in place is
that the costs compound in the same direction the product grows.

More users means more data, which means more joins per query,
which means more index pressure, which means more write
amplification, which means more replication lag, which means more
read replicas, which means more cache layers, which means more
consistency work, which means more on-call.

Each step is a reasonable response to the previous one. The
aggregate is a team spending most of its database time on
infrastructure that exists to compensate for a model mismatch.

A graph engine does not eliminate that curve. It changes its
slope. When the questions are shaped like the storage, the
infrastructure you have to add later is smaller, and the things
you can do at request time are bigger.

## Closing

SQL is not the villain of this story. The relational model is one
of the most successful ideas in computer science. The mistake is
treating it as the default for every workload, including the ones
where the engine has to spend most of its time pretending the data
is shaped like rows when it is actually shaped like a network.

Almost every product I see being built now has a graph somewhere
in the middle of it. Sometimes it is the whole product. Sometimes
it is the part that decides what to show next. Sometimes it is the
agent memory underneath a feature that nobody calls "graph"
out loud.

For those parts, the relational performance curve is the wrong
curve to be on. Not catastrophically wrong on day one. Slowly
wrong as the product grows, in a way that gets harder to reverse
the longer you wait.

LoraDB exists because I wanted the part of the system that is a
graph to be served by a database that knows it is a graph: close
to the application, fast on the hot path, and honest about what
"fast" means in the workloads developers actually have.

The next post is about the inverse claim: not where SQL degrades,
but where a graph engine has to earn its keep when the question
*is* set algebra. Because the rule cuts both ways.
